//! What was downloaded, kept between launches as `history.json` in the config folder: one
//! session per link, newest first. Also finds the tracks a new link would download again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::{config_dir, write_atomic};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SavedTrack {
    pub artist: String,
    pub title: String,
    pub album: String,
    /// Spotify track id, when known.
    pub id: String,
    /// Seconds, from Spotify.
    pub duration: u32,
    /// "done" or "missing".
    pub state: String,
    /// "soulseek" or "youtube".
    pub source: String,
    pub peer: String,
    /// The YouTube page of a fallback download.
    pub url: String,
    pub bitrate: u32,
    /// Bytes downloaded.
    pub size: u64,
    /// The mp3 in the output folder.
    pub path: String,
    /// Unix seconds: when the download started and ended.
    pub started: f64,
    pub finished: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Session {
    pub id: String,
    pub link: String,
    pub name: String,
    pub kind: String,
    /// A local copy of the cover image (albums and tracks), or empty.
    pub cover: String,
    pub dir: String,
    /// Unix seconds.
    pub started: f64,
    pub finished: f64,
    pub tracks: Vec<SavedTrack>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct History {
    pub sessions: Vec<Session>,
}

pub fn path() -> PathBuf {
    config_dir().join("history.json")
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl History {
    pub fn load() -> History {
        std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(self) {
            write_atomic(&path(), &b);
        }
    }

    /// Puts `s` first, replacing a session with the same id.
    pub fn put(&mut self, s: Session) {
        self.sessions.retain(|o| o.id != s.id);
        self.sessions.insert(0, s);
    }
}

// ------------------------------------------------------------------ duplicates

/// Lower-case letters and digits only.
pub fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The title without "(feat. X)", "[Remastered]" or " - Remix" parts, as file names often have it.
pub fn base_title(s: &str) -> &str {
    let mut cut = s.len();
    for sep in [" (", " [", " - "] {
        if let Some(i) = s.find(sep) {
            if i > 0 && i < cut {
                cut = i;
            }
        }
    }
    let b = s[..cut].trim();
    if b.is_empty() { s } else { b }
}

/// The first artist of "A, B", "A & B", "A feat. B".
fn first_artist(s: &str) -> &str {
    let lower = s.to_lowercase();
    let mut cut = s.len();
    for sep in [",", ";", " & ", " feat", " ft.", " x ", "/"] {
        if let Some(i) = lower.find(sep) {
            if i > 0 && i < cut {
                cut = i;
            }
        }
    }
    s[..cut].trim()
}

pub fn key(artist: &str, title: &str) -> String {
    format!("{}|{}", norm(first_artist(artist)), norm(base_title(title)))
}

/// The tracks the user already has: from the history (files that still exist) and from the
/// mp3 tags in the output folder, which also covers files from before the history.
#[derive(Default)]
pub struct Library {
    by_key: HashMap<String, String>,
    by_id: HashMap<String, String>,
}

impl Library {
    pub fn scan(history: &History, dir: &str) -> Library {
        let mut lib = Library::default();
        for s in &history.sessions {
            for t in &s.tracks {
                if t.state == "done" && !t.path.is_empty() && Path::new(&t.path).is_file() {
                    lib.by_key.entry(key(&t.artist, &t.title)).or_insert_with(|| t.path.clone());
                    if !t.id.is_empty() {
                        lib.by_id.entry(t.id.clone()).or_insert_with(|| t.path.clone());
                    }
                }
            }
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return lib };
        for e in rd.flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mp3")) {
                continue;
            }
            let ps = p.to_string_lossy().into_owned();
            if let Ok(tag) = id3::Tag::read_from_path(&p) {
                use id3::TagLike;
                if let (Some(a), Some(t)) = (tag.artist(), tag.title()) {
                    lib.by_key.entry(key(a, t)).or_insert_with(|| ps.clone());
                    continue;
                }
            }
            // No tags: "Artist - Title.mp3".
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some((a, t)) = stem.split_once(" - ") {
                lib.by_key.entry(key(a, t)).or_insert_with(|| ps.clone());
            }
        }
        lib
    }

    /// The file of a track the user already has.
    pub fn find(&self, id: &str, artist: &str, title: &str) -> Option<&str> {
        if !id.is_empty() {
            if let Some(p) = self.by_id.get(id) {
                return Some(p);
            }
        }
        self.by_key.get(&key(artist, title)).map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_ignore_features_and_case() {
        assert_eq!(key("Michael Jackson", "Beat It - Single Version"), key("michael jackson", "Beat It"));
        assert_eq!(key("Daddy Yankee, Snow", "Con Calma (feat. Snow)"), key("Daddy Yankee", "Con Calma"));
        assert_ne!(key("Enya", "Only Time"), key("Enya", "Wild Child"));
    }

    #[test]
    fn library_finds_history_files_that_still_exist() {
        let dir = std::env::temp_dir().join(format!("spotseek-lib-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Enya - Only Time.mp3");
        std::fs::write(&file, b"not really an mp3").unwrap();
        let mut h = History::default();
        h.put(Session {
            id: "1".into(),
            tracks: vec![
                SavedTrack { artist: "Enya".into(), title: "Only Time".into(), id: "abc".into(), state: "done".into(), path: file.to_string_lossy().into(), ..Default::default() },
                SavedTrack { artist: "Enya".into(), title: "Gone".into(), state: "done".into(), path: dir.join("gone.mp3").to_string_lossy().into(), ..Default::default() },
            ],
            ..Default::default()
        });
        let lib = Library::scan(&h, dir.to_str().unwrap());
        assert!(lib.find("abc", "", "").is_some());
        assert!(lib.find("", "Enya", "Only Time").is_some());
        assert!(lib.find("", "Enya", "Gone").is_none(), "a deleted file is not a duplicate");
        let _ = std::fs::remove_dir_all(dir);
    }
}
