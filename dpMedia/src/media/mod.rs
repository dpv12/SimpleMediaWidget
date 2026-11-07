//! Platform-neutral media types plus a background worker thread.
//!
//! The worker owns the OS backend (SMTC on Windows, MPRIS on Linux), polls it
//! at a low rate, and wakes the UI (`Context::request_repaint`) *only* when
//! the playback state, the artwork or the source list actually changed.
//! Button presses and seeks travel the other way over a channel and are
//! executed immediately.
//!
//! The playback *position* changes every second, so it is ignored for change
//! detection unless the UI says it is showing the timeline
//! (`set_timeline_wanted`). Otherwise an idle overlay would wake once a second
//! for nothing.
//!
//! Artwork is fetched and decoded here, once per track change, and handed to
//! the UI as a ready-to-upload `ColorImage`.
//!
//! Backend objects (WinRT / D-Bus connections) are created inside the worker
//! thread and never cross threads, so no `Send` bounds are needed on them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, ColorImage};

mod art;
mod source;

pub use source::{display_name, is_browser, SourceFilter, SourceInfo};

#[cfg(windows)]
mod smtc;
#[cfg(windows)]
use smtc::SmtcBackend as PlatformBackend;

#[cfg(target_os = "linux")]
mod mpris_backend;
#[cfg(target_os = "linux")]
use mpris_backend::MprisBackend as PlatformBackend;

#[cfg(not(any(windows, target_os = "linux")))]
compile_error!("dpMedia supports only Windows and Linux");

/// How often the worker asks the OS for the current state.
const POLL_INTERVAL: Duration = Duration::from_millis(1000);
/// After a command, re-poll quickly so the play/pause icon and title update fast.
const POST_COMMAND_DELAY: Duration = Duration::from_millis(250);
/// Players often publish the cover a moment after the title; try a few polls.
const ART_ATTEMPTS: u8 = 4;

/// Where we are in the current track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    pub position_ms: u64,
    /// 0 = unknown (live stream, player does not report a length).
    pub duration_ms: u64,
    pub can_seek: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaState {
    /// False when no (allowed) player is registered with the OS.
    pub has_session: bool,
    pub playing: bool,
    pub title: String,
    pub artist: String,
    /// Normalized id of the source being shown (empty = none).
    pub source: String,
    pub timeline: Timeline,
    /// Every player the OS currently reports, including filtered-out ones, so
    /// the settings can list them.
    pub sources: Vec<SourceInfo>,
}

#[derive(Clone, Copy, Debug)]
pub enum MediaCommand {
    Previous,
    PlayPause,
    Next,
    /// Jump to this position (milliseconds from the start).
    Seek(u64),
}

/// What happened to the artwork since the previous update.
pub enum ArtUpdate {
    Unchanged,
    /// New track, no cover (yet): show the placeholder.
    Clear,
    Set(ColorImage),
}

pub struct MediaUpdate {
    pub state: MediaState,
    pub art: ArtUpdate,
}

/// One implementation per OS.
pub trait MediaBackend {
    /// Which sources the backend may pick from (applies from the next call on).
    fn set_filter(&mut self, filter: SourceFilter);
    /// Read the current state of the active, allowed player.
    fn poll(&mut self) -> MediaState;
    /// Forward a command to the active player.
    fn send(&mut self, cmd: MediaCommand);
    /// Raw encoded cover bytes (PNG / JPEG / WebP) of the current track.
    /// May block briefly (network); only called on track change.
    fn artwork(&mut self) -> Option<Vec<u8>>;
}

enum Msg {
    Command(MediaCommand),
    /// Poll right now and publish the result even if nothing looks different.
    Refresh,
}

/// UI-side handle to the worker thread.
pub struct MediaHandle {
    msgs: Sender<Msg>,
    updates: Receiver<MediaUpdate>,
    filter: Arc<Mutex<SourceFilter>>,
    want_timeline: Arc<AtomicBool>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl MediaHandle {
    pub fn send(&self, cmd: MediaCommand) {
        let _ = self.msgs.send(Msg::Command(cmd));
    }

    /// Change which sources are followed; takes effect immediately.
    pub fn set_filter(&self, filter: SourceFilter) {
        *lock(&self.filter) = filter;
        let _ = self.msgs.send(Msg::Refresh);
    }

    /// Tell the worker whether the UI currently shows the timeline. Only then
    /// are position changes published (about once per second).
    pub fn set_timeline_wanted(&self, wanted: bool) {
        let before = self.want_timeline.swap(wanted, Ordering::Relaxed);
        if wanted && !before {
            let _ = self.msgs.send(Msg::Refresh); // fresh position right away
        }
    }

    /// Everything published since the last call, merged: the newest state and
    /// the most recent artwork change (so a `Set` is never lost to a later
    /// state-only update).
    pub fn latest(&self) -> Option<MediaUpdate> {
        let mut merged: Option<MediaUpdate> = None;
        while let Ok(update) = self.updates.try_recv() {
            merged = Some(match merged {
                None => update,
                Some(prev) => MediaUpdate {
                    state: update.state,
                    art: match update.art {
                        ArtUpdate::Unchanged => prev.art,
                        newer => newer,
                    },
                },
            });
        }
        merged
    }
}

pub fn spawn(ctx: egui::Context, filter: SourceFilter) -> MediaHandle {
    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
    let (update_tx, update_rx) = mpsc::channel::<MediaUpdate>();
    let shared_filter = Arc::new(Mutex::new(filter));
    let want_timeline = Arc::new(AtomicBool::new(false));

    let worker_filter = Arc::clone(&shared_filter);
    let worker_want = Arc::clone(&want_timeline);
    thread::Builder::new()
        .name("dpmedia-media".into())
        .spawn(move || worker(ctx, msg_rx, update_tx, worker_filter, worker_want))
        .expect("failed to spawn media thread");

    MediaHandle {
        msgs: msg_tx,
        updates: update_rx,
        filter: shared_filter,
        want_timeline,
    }
}

type TrackKey = (bool, String, String, String);

fn worker(
    ctx: egui::Context,
    msgs: Receiver<Msg>,
    updates: Sender<MediaUpdate>,
    shared_filter: Arc<Mutex<SourceFilter>>,
    want_timeline: Arc<AtomicBool>,
) {
    let mut backend = PlatformBackend::new();
    let mut applied_filter = lock(&shared_filter).clone();
    backend.set_filter(applied_filter.clone());

    let mut last: Option<MediaState> = None;
    let mut last_key: Option<TrackKey> = None;
    let mut art_pending = false;
    let mut art_tries = 0u8;
    let mut next_poll = Instant::now();

    loop {
        // Sleep until the next poll, but wake instantly for a message.
        let wait = next_poll.saturating_duration_since(Instant::now());
        let mut force = false;
        match msgs.recv_timeout(wait) {
            Ok(Msg::Command(cmd)) => {
                backend.send(cmd);
                next_poll = Instant::now() + POST_COMMAND_DELAY;
                continue;
            }
            Ok(Msg::Refresh) => force = true,
            Err(RecvTimeoutError::Timeout) => {}
            // UI is gone: shut down.
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let wanted_filter = lock(&shared_filter).clone();
        if wanted_filter != applied_filter {
            backend.set_filter(wanted_filter.clone());
            applied_filter = wanted_filter;
            force = true;
        }

        let state = backend.poll();
        next_poll = Instant::now() + POLL_INTERVAL;

        let mut art = ArtUpdate::Unchanged;

        // A different track, player or source invalidates the cover.
        let key: TrackKey = (
            state.has_session,
            state.title.clone(),
            state.artist.clone(),
            state.source.clone(),
        );
        if last_key.as_ref() != Some(&key) {
            last_key = Some(key);
            art_pending = state.has_session;
            art_tries = 0;
            art = ArtUpdate::Clear;
        }

        if art_pending {
            match backend.artwork().and_then(|bytes| art::decode(&bytes)) {
                Some(image) => {
                    art = ArtUpdate::Set(image);
                    art_pending = false;
                }
                None => {
                    art_tries += 1;
                    if art_tries >= ART_ATTEMPTS {
                        art_pending = false; // give up until the next track
                    }
                }
            }
        }

        // Without a visible timeline the ticking position is not a change.
        let mut comparable = state.clone();
        if !want_timeline.load(Ordering::Relaxed) {
            comparable.timeline = Timeline::default();
        }

        let changed = force
            || last.as_ref() != Some(&comparable)
            || !matches!(art, ArtUpdate::Unchanged);
        if changed {
            last = Some(comparable);
            if updates.send(MediaUpdate { state, art }).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}
