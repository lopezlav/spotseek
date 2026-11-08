//! Settings kept between launches, as JSON in the user's config folder. The Soulseek password is
//! never written to disk.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dir: String,
    pub bitrate: String,
    pub fallback: bool,
    /// The Soulseek user name (the password is in the system's secure store).
    pub user: String,
    /// "auto" (the system language), "en" or "es".
    pub lang: String,
    /// The user picks the Soulseek file for each track (else the best one is taken).
    pub manual: bool,
    /// A song plays while the mouse is over it (else on a click).
    pub hover_play: bool,
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dir: home().join("Downloads").join("Spotseek").to_string_lossy().into_owned(),
            bitrate: "320".into(),
            fallback: true,
            user: String::new(),
            lang: "auto".into(),
            manual: false,
            hover_play: false,
        }
    }
}

/// Spotseek's folder in the user's config folder: settings, history, logs and covers.
pub fn config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("SPOTSEEK_CONFIG_DIR") {
        return PathBuf::from(p);
    }
    let base = if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
    };
    base.join("Spotseek")
}

fn path() -> PathBuf {
    config_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Settings {
        std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            write_atomic(&p, &b);
        }
    }

    /// The folder with the home folder shown as `~`.
    pub fn short_dir(&self) -> String {
        short_path(&self.dir)
    }
}

/// Writes through a temporary file, so a crash never leaves half a file.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// `path` with the home folder shown as `~`.
pub fn short_path(path: &str) -> String {
    let h = home().to_string_lossy().into_owned();
    match path.strip_prefix(&h) {
        Some(rest) if !h.is_empty() => format!("~{rest}"),
        _ => path.to_string(),
    }
}
