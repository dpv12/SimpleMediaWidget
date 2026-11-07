//! Identifies which app a media session belongs to and decides whether the
//! overlay should show it.
//!
//! Ids are normalized (lower-case, no ".exe", no MPRIS prefix / instance
//! suffix) so the same app yields the same id on every run. That id is what the
//! "ignore this source" list in the config stores.

/// A player (or browser) the OS currently reports a media session for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    /// Normalized, stable id (see [`normalize_id`]).
    pub id: String,
    /// Human-readable name for the settings list.
    pub name: String,
}

/// Which sources the overlay is allowed to follow.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFilter {
    /// Ignore every browser (YouTube, Netflix, ... all arrive as "the browser").
    pub hide_browsers: bool,
    /// Normalized ids the user switched off one by one.
    pub hidden: Vec<String>,
}

impl SourceFilter {
    pub fn blocks(&self, id: &str) -> bool {
        (self.hide_browsers && is_browser(id)) || self.hidden.iter().any(|h| h == id)
    }
}

/// Token prefixes that identify a browser (matched per alphanumeric token, so
/// "bravesoftware.brave-browser" and "msedge" both hit, "spotify" does not).
const BROWSER_PREFIXES: &[&str] = &[
    "chrome",
    "chromium",
    "msedge",
    "microsoftedge",
    "edge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "librewolf",
    "waterfox",
    "thorium",
    "iexplore",
];

/// Short names that must match a whole token to avoid false positives.
const BROWSER_EXACT: &[&str] = &["zen", "arc"];

pub fn is_browser(id: &str) -> bool {
    id.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .any(|token| {
            BROWSER_EXACT.contains(&token) || BROWSER_PREFIXES.iter().any(|p| token.starts_with(p))
        })
}

/// Lower-case id without file extension, MPRIS bus prefix or per-run instance
/// suffix (`...chromium.instance1234` -> `chromium`).
pub fn normalize_id(raw: &str) -> String {
    let mut id = raw.trim().to_lowercase();
    if let Some(rest) = id.strip_prefix("org.mpris.mediaplayer2.") {
        id = rest.to_owned();
    }
    if let Some(pos) = id.rfind(".instance") {
        id.truncate(pos);
    }
    if let Some(stripped) = id.strip_suffix(".exe") {
        id = stripped.to_owned();
    }
    id
}

/// Friendly name for an id (known apps get their proper spelling).
pub fn display_name(id: &str) -> String {
    // Store apps look like "publisher.app_hash!entry"; keep the entry part.
    let base = id.rsplit('!').next().unwrap_or(id);
    let known = match base {
        "spotify" => "Spotify",
        "chrome" => "Google Chrome",
        "chromium" => "Chromium",
        "msedge" => "Microsoft Edge",
        "firefox" => "Firefox",
        "brave" => "Brave",
        "opera" => "Opera",
        "vivaldi" => "Vivaldi",
        "vlc" => "VLC",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    let mut chars = base.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ids() {
        assert_eq!(normalize_id("Spotify.exe"), "spotify");
        assert_eq!(normalize_id("org.mpris.MediaPlayer2.chromium.instance1234"), "chromium");
        assert_eq!(normalize_id("org.mpris.MediaPlayer2.firefox.instance_1_42"), "firefox");
        assert_eq!(normalize_id("  MSEdge  "), "msedge");
    }

    #[test]
    fn detects_browsers() {
        for id in [
            "chrome",
            "msedge",
            "firefox",
            "brave",
            "bravesoftware.brave-browser",
            "microsoft.microsoftedge.stable_8wekyb3d8bbwe!app",
            "opera",
            "vivaldi",
            "chromium",
            "zen",
        ] {
            assert!(is_browser(id), "{id} should be a browser");
        }
        for id in [
            "spotify",
            "vlc",
            "mpv",
            "foobar2000",
            "spotifyab.spotifymusic_zpdnekdrzrea0!spotify",
        ] {
            assert!(!is_browser(id), "{id} should not be a browser");
        }
    }

    #[test]
    fn filter_blocks_browsers_and_listed_ids() {
        let mut filter = SourceFilter::default();
        assert!(!filter.blocks("chrome"));
        filter.hide_browsers = true;
        assert!(filter.blocks("chrome"));
        assert!(!filter.blocks("spotify"));
        filter.hidden.push("spotify".to_owned());
        assert!(filter.blocks("spotify"));
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name("msedge"), "Microsoft Edge");
        assert_eq!(display_name("spotifyab.spotifymusic_abc!spotify"), "Spotify");
        assert_eq!(display_name("foobar2000"), "Foobar2000");
    }
}
