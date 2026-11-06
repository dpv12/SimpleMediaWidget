//! Windows backend: System Media Transport Controls (SMTC) through WinRT.
//!
//! SMTC is the same service that drives the volume-flyout media widget, so any
//! app that shows up there (Spotify, browsers, VLC, ...) works. Windows decides
//! which session is "current"; we follow that choice.

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};

use super::{MediaBackend, MediaCommand, MediaState};

pub struct SmtcBackend {
    manager: Option<Manager>,
}

impl SmtcBackend {
    pub fn new() -> Self {
        Self { manager: None }
    }

    fn current_session(&mut self) -> Option<Session> {
        // Requested lazily and cached; retried on the next poll if it fails.
        if self.manager.is_none() {
            self.manager = Manager::RequestAsync().ok()?.get().ok();
        }
        self.manager.as_ref()?.GetCurrentSession().ok()
    }
}

impl MediaBackend for SmtcBackend {
    fn poll(&mut self) -> MediaState {
        let Some(session) = self.current_session() else {
            return MediaState::default();
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

        let playing = session
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
            .map(|status| status == PlaybackStatus::Playing)
            .unwrap_or(false);

        MediaState {
            has_session: true,
            playing,
            title,
            artist,
        }
    }

    fn send(&mut self, cmd: MediaCommand) {
        let Some(session) = self.current_session() else {
            return;
        };
        let _ = match cmd {
            MediaCommand::Previous => session.TrySkipPreviousAsync().and_then(|op| op.get()),
            MediaCommand::PlayPause => session.TryTogglePlayPauseAsync().and_then(|op| op.get()),
            MediaCommand::Next => session.TrySkipNextAsync().and_then(|op| op.get()),
        };
    }
}
