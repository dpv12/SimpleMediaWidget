//! Windows backend: System Media Transport Controls (SMTC) through WinRT.
//!
//! SMTC is the same service that drives the volume-flyout media widget, so any
//! app that shows up there (Spotify, browsers, VLC, ...) works. We look at all
//! sessions, drop the ones the user switched off, and follow the best of the
//! rest: a playing session first, then the one Windows calls "current".

use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::Interface;
use windows::Foundation::TimeSpan;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStream};

use super::art::MAX_BYTES;
use super::source::{display_name, normalize_id, SourceFilter, SourceInfo};
use super::{MediaBackend, MediaCommand, MediaState, Timeline};

/// SMTC time values are 100-nanosecond ticks.
const TICKS_PER_MS: i64 = 10_000;
/// Ticks between 1601-01-01 (Windows epoch) and 1970-01-01.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

pub struct SmtcBackend {
    manager: Option<Manager>,
    filter: SourceFilter,
}

/// One look at the OS: every source, and the session we follow.
#[derive(Default)]
struct Scan {
    sources: Vec<SourceInfo>,
    chosen: Option<(Session, String)>,
}

impl SmtcBackend {
    pub fn new() -> Self {
        Self {
            manager: None,
            filter: SourceFilter::default(),
        }
    }

    fn manager(&mut self) -> Option<&Manager> {
        // Requested lazily and cached; retried on the next poll if it fails.
        if self.manager.is_none() {
            self.manager = Manager::RequestAsync().ok()?.get().ok();
        }
        self.manager.as_ref()
    }

    fn scan(&mut self) -> Scan {
        let filter = self.filter.clone();
        let mut scan = Scan::default();
        let Some(manager) = self.manager() else {
            return scan;
        };

        let current_id = manager
            .GetCurrentSession()
            .ok()
            .and_then(|s| s.SourceAppUserModelId().ok())
            .map(|h| normalize_id(&h.to_string()));

        let Ok(sessions) = manager.GetSessions() else {
            return scan;
        };

        // rank: 0 = playing, 1 = Windows' current session, 2 = anything else
        let mut best: Option<(u8, Session, String)> = None;
        for i in 0..sessions.Size().unwrap_or(0) {
            let Ok(session) = sessions.GetAt(i) else {
                continue;
            };
            let Ok(raw) = session.SourceAppUserModelId() else {
                continue;
            };
            let id = normalize_id(&raw.to_string());
            if !scan.sources.iter().any(|s| s.id == id) {
                scan.sources.push(SourceInfo {
                    name: display_name(&id),
                    id: id.clone(),
                });
            }
            if filter.blocks(&id) {
                continue;
            }

            let playing = session
                .GetPlaybackInfo()
                .and_then(|info| info.PlaybackStatus())
                .map(|status| status == PlaybackStatus::Playing)
                .unwrap_or(false);
            let rank = if playing {
                0
            } else if Some(&id) == current_id.as_ref() {
                1
            } else {
                2
            };
            if best.as_ref().map_or(true, |(r, _, _)| rank < *r) {
                best = Some((rank, session, id));
            }
        }

        scan.chosen = best.map(|(_, session, id)| (session, id));
        scan
    }
}

impl MediaBackend for SmtcBackend {
    fn set_filter(&mut self, filter: SourceFilter) {
        self.filter = filter;
    }

    fn poll(&mut self) -> MediaState {
        let scan = self.scan();
        let Some((session, id)) = scan.chosen else {
            return MediaState {
                sources: scan.sources,
                ..MediaState::default()
            };
        };

        let (title, artist) = session
            .TryGetMediaPropertiesAsync()
            .and_then(|op| op.get())
            .map(|props| {
                (
                    props.Title().map(|s| s.to_string()).unwrap_or_default(),
                    props.Artist().map(|s| s.to_string()).unwrap_or_default(),
                )
            })
            .unwrap_or_default();

        let info = session.GetPlaybackInfo().ok();
        let playing = info
            .as_ref()
            .and_then(|info| info.PlaybackStatus().ok())
            .map(|status| status == PlaybackStatus::Playing)
            .unwrap_or(false);
        let can_seek = info
            .as_ref()
            .and_then(|info| info.Controls().ok())
            .and_then(|controls| controls.IsPlaybackPositionEnabled().ok())
            .unwrap_or(false);

        MediaState {
            has_session: true,
            playing,
            title,
            artist,
            source: id,
            timeline: read_timeline(&session, playing, can_seek),
            sources: scan.sources,
        }
    }

    fn send(&mut self, cmd: MediaCommand) {
        let Some((session, _)) = self.scan().chosen else {
            return;
        };
        let _ = match cmd {
            MediaCommand::Previous => session.TrySkipPreviousAsync().and_then(|op| op.get()),
            MediaCommand::PlayPause => session.TryTogglePlayPauseAsync().and_then(|op| op.get()),
            MediaCommand::Next => session.TrySkipNextAsync().and_then(|op| op.get()),
            MediaCommand::Seek(ms) => session
                .TryChangePlaybackPositionAsync((ms as i64).saturating_mul(TICKS_PER_MS))
                .and_then(|op| op.get()),
        };
    }

    /// Reads the `Thumbnail` stream of the session's media properties into raw
    /// (still encoded) image bytes.
    fn artwork(&mut self) -> Option<Vec<u8>> {
        let (session, _) = self.scan().chosen?;
        let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
        let thumbnail = props.Thumbnail().ok()?;
        // Cast first so `Size` / `GetInputStreamAt` are called on the plain
        // IRandomAccessStream interface.
        let stream: IRandomAccessStream = thumbnail.OpenReadAsync().ok()?.get().ok()?.cast().ok()?;

        let size = stream.Size().ok()? as usize;
        if size == 0 || size > MAX_BYTES {
            return None;
        }

        let input = stream.GetInputStreamAt(0).ok()?;
        let reader = DataReader::CreateDataReader(&input).ok()?;
        reader.LoadAsync(size as u32).ok()?.get().ok()?;

        let mut bytes = vec![0u8; size];
        reader.ReadBytes(&mut bytes).ok()?;
        Some(bytes)
    }
}

/// Position / length of the current track.
///
/// SMTC reports the position *as of `LastUpdatedTime`*; many players (Spotify
/// among them) only refresh it on play / pause / seek, so while playing we add
/// the time that has passed since.
fn read_timeline(session: &Session, playing: bool, can_seek: bool) -> Timeline {
    let Ok(props) = session.GetTimelineProperties() else {
        return Timeline::default();
    };
    let ticks = |value: windows::core::Result<TimeSpan>| value.map(|t| t.Duration).unwrap_or(0);

    let start = ticks(props.StartTime());
    let end = ticks(props.EndTime());
    let duration = (end - start).max(0);
    let mut position = ticks(props.Position());

    if playing {
        if let Ok(updated) = props.LastUpdatedTime() {
            if updated.UniversalTime > 0 {
                let age = now_ticks() - updated.UniversalTime;
                if age > 0 {
                    position += age;
                }
            }
        }
    }

    let elapsed = (position - start).clamp(0, duration);
    Timeline {
        position_ms: (elapsed / TICKS_PER_MS) as u64,
        duration_ms: (duration / TICKS_PER_MS) as u64,
        can_seek: can_seek && duration > 0,
    }
}

fn now_ticks() -> i64 {
    let since_unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (since_unix.as_nanos() / 100) as i64 + UNIX_EPOCH_TICKS
}
