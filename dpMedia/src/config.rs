//! Tiny JSON config: just the window position.
//!
//! Linux:   ~/.config/dpmedia/config.json
//! Windows: %APPDATA%\dpmedia\config.json

use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Outer window position in logical points.
    pub x: f32,
    pub y: f32,
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("dpmedia").join("config.json"))
}

impl Config {
    pub fn load() -> Option<Self> {
        let text = fs::read_to_string(path()?).ok()?;
        let cfg: Self = serde_json::from_str(&text).ok()?;
        (cfg.x.is_finite() && cfg.y.is_finite()).then_some(cfg)
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
}
