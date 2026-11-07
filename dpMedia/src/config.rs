//! Tiny JSON config: window position and media-source settings.
//!
//! Linux:   ~/.config/dpmedia/config.json
//! Windows: %APPDATA%\dpmedia\config.json
//!
//! Older files still load: missing fields fall back to their defaults and
//! fields that no longer exist (the removed `pins`) are ignored; they
//! disappear from the file on the next save.

use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::media::SourceFilter;

pub const DEFAULT_X: f32 = 40.0;
pub const DEFAULT_Y: f32 = 40.0;
/// Upper bound for the hand-picked "ignore this source" list.
pub const MAX_HIDDEN: usize = 64;

fn default_x() -> f32 {
    DEFAULT_X
}

fn default_y() -> f32 {
    DEFAULT_Y
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    /// Outer window position in logical points.
    #[serde(default = "default_x")]
    pub x: f32,
    #[serde(default = "default_y")]
    pub y: f32,
    /// The same position in physical screen pixels. Exact across monitors with
    /// different scaling; `x` / `y` stay for older versions of this file.
    #[serde(default)]
    pub phys_x: Option<f32>,
    #[serde(default)]
    pub phys_y: Option<f32>,
    /// Ignore all browsers (YouTube and friends).
    #[serde(default)]
    pub hide_browsers: bool,
    /// Normalized ids of sources switched off one by one (e.g. "spotify").
    #[serde(default)]
    pub hidden_sources: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            x: DEFAULT_X,
            y: DEFAULT_Y,
            phys_x: None,
            phys_y: None,
            hide_browsers: false,
            hidden_sources: Vec::new(),
        }
    }
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("dpmedia").join("config.json"))
}

impl Config {
    pub fn load() -> Option<Self> {
        let text = fs::read_to_string(path()?).ok()?;
        let mut cfg: Self = serde_json::from_str(&text).ok()?;
        if !(cfg.x.is_finite() && cfg.y.is_finite()) {
            cfg.x = DEFAULT_X;
            cfg.y = DEFAULT_Y;
        }
        if !matches!((cfg.phys_x, cfg.phys_y), (Some(x), Some(y)) if x.is_finite() && y.is_finite()) {
            cfg.phys_x = None;
            cfg.phys_y = None;
        }
        // The file can be edited by hand: keep the list tidy and bounded.
        let mut cleaned: Vec<String> = Vec::new();
        for id in cfg.hidden_sources.drain(..) {
            let id = id.trim().to_lowercase();
            if !id.is_empty() && id.len() <= 200 && !cleaned.contains(&id) {
                cleaned.push(id);
            }
        }
        cleaned.truncate(MAX_HIDDEN);
        cfg.hidden_sources = cleaned;
        Some(cfg)
    }

    pub fn save(&self) -> io::Result<()> {
        let path = path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config dir"))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Write + rename so a crash can never leave a half-written file.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(tmp, path)
    }

    /// The media-source filter these settings describe.
    pub fn filter(&self) -> SourceFilter {
        SourceFilter {
            hide_browsers: self.hide_browsers,
            hidden: self.hidden_sources.clone(),
        }
    }
}
