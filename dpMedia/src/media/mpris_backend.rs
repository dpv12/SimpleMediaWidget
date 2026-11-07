//! Linux backend: MPRIS 2 over the D-Bus session bus.
//!
//! Spotify (native, Flatpak and Snap), VLC, mpv-mpris, browsers etc. all
//! expose `org.mpris.MediaPlayer2.*`. We look at every player, drop the ones
//! the user switched off, and follow the best of the rest: a playing player
//! first, then a paused one, then any.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use mpris::{FindingError, PlaybackStatus, Player, PlayerFinder};

use super::art::MAX_BYTES;
use super::source::{display_name, normalize_id, SourceFilter, SourceInfo};
use super::{MediaBackend, MediaCommand, MediaState, Timeline};

/// Network timeout for downloading a cover; the worker thread waits this long
/// at most (button presses are delayed by it, nothing else).
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

pub struct MprisBackend {
    finder: Option<PlayerFinder>,
    filter: SourceFilter,
}

/// One look at the bus: every source, and the player we follow.
#[derive(Default)]
struct Scan {
    sources: Vec<SourceInfo>,
    chosen: Option<(Player, String)>,
}

impl MprisBackend {
    pub fn new() -> Self {
        Self {
            finder: None,
            filter: SourceFilter::default(),
        }
    }

    fn scan(&mut self) -> Scan {
        if self.finder.is_none() {
            self.finder = PlayerFinder::new().ok();
        }
        let filter = self.filter.clone();
        let mut scan = Scan::default();
        let Some(finder) = self.finder.as_ref() else {
            return scan;
        };

        let players = match finder.find_all() {
            Ok(players) => players,
            Err(FindingError::NoPlayerFound) => return scan,
            Err(_) => {
                // Bus connection probably died; reconnect on the next poll.
                self.finder = None;
                return scan;
            }
        };

        // rank: 0 = playing, 1 = paused, 2 = anything else
        let mut best: Option<(u8, Player, String)> = None;
        for player in players {
            let id = normalize_id(player.bus_name());
            if !scan.sources.iter().any(|s| s.id == id) {
                let identity = player.identity();
                let name = if identity.is_empty() {
                    display_name(&id)
                } else {
                    identity.to_owned()
                };
                scan.sources.push(SourceInfo {
                    id: id.clone(),
                    name,
                });
            }
            if filter.blocks(&id) {
                continue;
            }

            let rank = match player.get_playback_status() {
                Ok(PlaybackStatus::Playing) => 0u8,
                Ok(PlaybackStatus::Paused) => 1,
                _ => 2,
            };
            if best.as_ref().map_or(true, |(r, _, _)| rank < *r) {
                best = Some((rank, player, id));
            }
        }

        scan.chosen = best.map(|(_, player, id)| (player, id));
        scan
    }
}

impl MediaBackend for MprisBackend {
    fn set_filter(&mut self, filter: SourceFilter) {
        self.filter = filter;
    }

    fn poll(&mut self) -> MediaState {
        let scan = self.scan();
        let Some((player, id)) = scan.chosen else {
            return MediaState {
                sources: scan.sources,
                ..MediaState::default()
            };
        };

        let playing = matches!(player.get_playback_status(), Ok(PlaybackStatus::Playing));

        let meta = player.get_metadata().ok();
        let (title, artist) = match &meta {
            Some(meta) => (
                meta.title().unwrap_or_default().to_owned(),
                meta.artists().map(|a| a.join(", ")).unwrap_or_default(),
            ),
            None => Default::default(),
        };

        let duration_ms = meta
            .as_ref()
            .and_then(|meta| meta.length())
            .map(|length| length.as_millis() as u64)
            .unwrap_or(0);
        let position_ms = player
            .get_position()
            .map(|position| position.as_millis() as u64)
            .unwrap_or(0)
            .min(duration_ms);
        let can_seek = player.can_seek().unwrap_or(false) && duration_ms > 0;

        MediaState {
            has_session: true,
            playing,
            title,
            artist,
            source: id,
            timeline: Timeline {
                position_ms,
                duration_ms,
                can_seek,
            },
            sources: scan.sources,
        }
    }

    fn send(&mut self, cmd: MediaCommand) {
        let Some((player, _)) = self.scan().chosen else {
            return;
        };
        let _ = match cmd {
            MediaCommand::Previous => player.previous(),
            MediaCommand::PlayPause => player.play_pause(),
            MediaCommand::Next => player.next(),
            MediaCommand::Seek(ms) => {
                // Relative `Seek` needs no track id, unlike `SetPosition`.
                match player.get_position_in_microseconds() {
                    Ok(current) => player.seek(ms as i64 * 1000 - current as i64),
                    Err(_) => Ok(()),
                }
            }
        };
    }

    /// Resolves `mpris:artUrl` (`file://`, `https://` or `http://`) to bytes.
    fn artwork(&mut self) -> Option<Vec<u8>> {
        let (player, _) = self.scan().chosen?;
        let meta = player.get_metadata().ok()?;
        fetch(meta.art_url()?)
    }
}

fn fetch(url: &str) -> Option<Vec<u8>> {
    if let Some(rest) = url.strip_prefix("file://") {
        let path = percent_decode(rest);
        let path = Path::new(&path);
        if fs::metadata(path).ok()?.len() > MAX_BYTES as u64 {
            return None;
        }
        return fs::read(path).ok();
    }

    if url.starts_with("https://") || url.starts_with("http://") {
        let agent = ureq::AgentBuilder::new().timeout(FETCH_TIMEOUT).build();
        let response = agent.get(url).call().ok()?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        return (bytes.len() <= MAX_BYTES).then_some(bytes);
    }

    None
}

/// Minimal `%XX` decoder for `file://` URLs (spaces, umlauts, ...).
fn percent_decode(input: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        (b as char).to_digit(16).map(|d| d as u8)
    }

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Work on bytes, not `&input[..]`: slicing a str in the middle of a
        // multi-byte character would panic.
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
