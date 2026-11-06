//! Platform-neutral media types plus a background worker thread.
//!
//! The worker owns the OS backend (SMTC on Windows, MPRIS on Linux), polls it
//! at a low rate, and wakes the UI (`Context::request_repaint`) *only* when
//! the playback state actually changed. Button presses travel the other way
//! over a channel and are executed immediately.
//!
//! Backend objects (WinRT / D-Bus connections) are created inside the worker
//! thread and never cross threads, so no `Send` bounds are needed on them.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MediaState {
    /// False when no player is registered with the OS.
    pub has_session: bool,
    pub playing: bool,
    pub title: String,
    pub artist: String,
}

#[derive(Clone, Copy, Debug)]
pub enum MediaCommand {
    Previous,
    PlayPause,
    Next,
}

/// One implementation per OS.
pub trait MediaBackend {
    /// Read the current state of the active player.
    fn poll(&mut self) -> MediaState;
    /// Forward a transport command to the active player.
    fn send(&mut self, cmd: MediaCommand);
}

/// UI-side handle to the worker thread.
pub struct MediaHandle {
    commands: Sender<MediaCommand>,
    updates: Receiver<MediaState>,
}

impl MediaHandle {
    pub fn send(&self, cmd: MediaCommand) {
        let _ = self.commands.send(cmd);
    }

    /// The newest state published since the last call, if the state changed.
    pub fn latest(&self) -> Option<MediaState> {
        let mut newest = None;
        while let Ok(state) = self.updates.try_recv() {
            newest = Some(state);
        }
        newest
    }
}

pub fn spawn(ctx: egui::Context) -> MediaHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<MediaCommand>();
    let (state_tx, state_rx) = mpsc::channel::<MediaState>();

    thread::Builder::new()
        .name("dpmedia-media".into())
        .spawn(move || worker(ctx, cmd_rx, state_tx))
        .expect("failed to spawn media thread");

    MediaHandle {
        commands: cmd_tx,
        updates: state_rx,
    }
}

fn worker(ctx: egui::Context, commands: Receiver<MediaCommand>, updates: Sender<MediaState>) {
    let mut backend = PlatformBackend::new();
    let mut last: Option<MediaState> = None;
    let mut next_poll = Instant::now();

    loop {
        // Sleep until the next poll, but wake instantly for a command.
        let wait = next_poll.saturating_duration_since(Instant::now());
        match commands.recv_timeout(wait) {
            Ok(cmd) => {
                backend.send(cmd);
                next_poll = Instant::now() + POST_COMMAND_DELAY;
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            // UI is gone: shut down.
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let state = backend.poll();
        next_poll = Instant::now() + POLL_INTERVAL;

        if last.as_ref() != Some(&state) {
            last = Some(state.clone());
            if updates.send(state).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    }
}
