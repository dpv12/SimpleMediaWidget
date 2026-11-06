//! Linux backend: MPRIS 2 over the D-Bus session bus.
//!
//! Spotify (native, Flatpak and Snap), VLC, mpv-mpris, browsers etc. all
//! expose `org.mpris.MediaPlayer2.*`. `find_active` prefers a player that is
//! currently playing, then a paused one, then any.

use mpris::{FindingError, PlaybackStatus, Player, PlayerFinder};

use super::{MediaBackend, MediaCommand, MediaState};

pub struct MprisBackend {
    finder: Option<PlayerFinder>,
}

impl MprisBackend {
    pub fn new() -> Self {
        Self { finder: None }
    }

    fn active_player(&mut self) -> Option<Player> {
        if self.finder.is_none() {
            self.finder = PlayerFinder::new().ok();
        }
        let finder = self.finder.as_ref()?;
        match finder.find_active() {
            Ok(player) => Some(player),
            Err(FindingError::NoPlayerFound) => None,
            Err(_) => {
                // Bus connection probably died; reconnect on the next poll.
                self.finder = None;
                None
            }
        }
    }
}

impl MediaBackend for MprisBackend {
    fn poll(&mut self) -> MediaState {
        let Some(player) = self.active_player() else {
            return MediaState::default();
        };

        let playing = matches!(player.get_playback_status(), Ok(PlaybackStatus::Playing));

        let (title, artist) = match player.get_metadata() {
            Ok(meta) => (
                meta.title().unwrap_or_default().to_owned(),
                meta.artists().map(|a| a.join(", ")).unwrap_or_default(),
            ),
            Err(_) => Default::default(),
        };

        MediaState {
            has_session: true,
            playing,
            title,
            artist,
        }
    }

    fn send(&mut self, cmd: MediaCommand) {
        let Some(player) = self.active_player() else {
            return;
        };
        let _ = match cmd {
            MediaCommand::Previous => player.previous(),
            MediaCommand::PlayPause => player.play_pause(),
            MediaCommand::Next => player.next(),
        };
    }
}
