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
    pub user: String,
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
        }
    }
}

fn path() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
    };
    base.join("Spotseek").join("settings.json")
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
            let _ = std::fs::write(p, b);
        }
    }

    /// The folder with the home folder shown as `~`.
    pub fn short_dir(&self) -> String {
        let h = home().to_string_lossy().into_owned();
        match self.dir.strip_prefix(&h) {
            Some(rest) if !h.is_empty() => format!("~{rest}"),
            _ => self.dir.clone(),
        }
    }
}
