//! What the engine remembers between runs, in the app's config folder:
//!
//! - **Peer memory** (`peers.json`): the users who sent us good files, how fast, and the artists.
//!   A new run first looks in their shares (a free peer message, not a search).
//! - **Search cache** (`search-cache.json`): the answers to each search for some hours, so the same
//!   song in another playlist, or the same link again, needs no new search.
//!
//! Soulseek counts searches (34 every 220 s), never peer messages, so both save searches.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::slsk::{SearchResponse, SharedFile};

/// How long cached answers are used.
pub const CACHE_HOURS: f64 = 6.0;
const CACHE_MAX_QUERIES: usize = 400;
const CACHE_MAX_FILES: usize = 150;
const PEERS_MAX: usize = 300;
const ARTISTS_MAX: usize = 40;

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Peer {
    /// Downloads that finished.
    pub hits: u32,
    /// Downloads that did not start or broke.
    pub fails: u32,
    pub bytes: u64,
    /// Seconds those downloads took.
    pub secs: f64,
    /// Unix seconds of the last good download.
    pub last: f64,
    /// Artists (normalized) it sent, newest last.
    pub artists: Vec<String>,
}

impl Peer {
    /// Bytes per second it sent us.
    pub fn speed(&self) -> u32 {
        if self.secs > 0.5 { (self.bytes as f64 / self.secs) as u32 } else { 0 }
    }

    /// Higher is better: many good downloads, few failures, fast, recent.
    pub fn score(&self, now: f64) -> f64 {
        let days = ((now - self.last) / 86_400.0).max(0.0);
        let reliability = (self.hits as f64 + 1.0) / (self.hits as f64 + self.fails as f64 + 2.0);
        (self.hits as f64).ln_1p() * 10.0 * reliability + (self.speed() as f64 / 250_000.0).min(8.0) - days.min(60.0) * 0.2
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CachedFile {
    f: String,
    s: u64,
    e: String,
    br: u32,
    d: u32,
    sr: u32,
    bd: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CachedResponse {
    user: String,
    free: bool,
    speed: u32,
    queue: u32,
    files: Vec<CachedFile>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Cached {
    at: f64,
    responses: Vec<CachedResponse>,
}

fn opt(v: u32) -> Option<u32> {
    (v > 0).then_some(v)
}

#[derive(Default)]
pub struct Store {
    dir: Option<PathBuf>,
    pub peers: HashMap<String, Peer>,
    cache: HashMap<String, Cached>,
}

fn read<T: for<'de> Deserialize<'de> + Default>(p: &Path) -> T {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_atomic(p: &Path, bytes: &[u8]) {
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
}

impl Store {
    /// Reads the store in `dir` (nothing is kept without a folder).
    pub fn load(dir: Option<PathBuf>) -> Store {
        let Some(d) = dir else { return Store::default() };
        let peers = read(&d.join("peers.json"));
        let cache = read(&d.join("search-cache.json"));
        Store { dir: Some(d), peers, cache }
    }

    pub fn save(&mut self) {
        let Some(d) = self.dir.clone() else { return };
        let now = now();
        self.cache.retain(|_, c| now - c.at < CACHE_HOURS * 3600.0);
        if self.cache.len() > CACHE_MAX_QUERIES {
            let mut ages: Vec<(String, f64)> = self.cache.iter().map(|(k, c)| (k.clone(), c.at)).collect();
            ages.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (k, _) in ages.into_iter().skip(CACHE_MAX_QUERIES) {
                self.cache.remove(&k);
            }
        }
        if self.peers.len() > PEERS_MAX {
            let mut s: Vec<(String, f64)> = self.peers.iter().map(|(k, p)| (k.clone(), p.score(now))).collect();
            s.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (k, _) in s.into_iter().skip(PEERS_MAX) {
                self.peers.remove(&k);
            }
        }
        if let Ok(b) = serde_json::to_vec(&self.peers) {
            write_atomic(&d.join("peers.json"), &b);
        }
        if let Ok(b) = serde_json::to_vec(&self.cache) {
            write_atomic(&d.join("search-cache.json"), &b);
        }
    }

    /// A finished download from `user`.
    pub fn hit(&mut self, user: &str, bytes: u64, secs: f64, artist: &str) {
        let p = self.peers.entry(user.to_string()).or_default();
        p.hits += 1;
        p.bytes += bytes;
        p.secs += secs.max(0.5);
        p.last = now();
        if !artist.is_empty() {
            p.artists.retain(|a| a != artist);
            p.artists.push(artist.to_string());
            if p.artists.len() > ARTISTS_MAX {
                p.artists.remove(0);
            }
        }
    }

    /// A download from `user` that did not start or broke (only for peers we know).
    pub fn fail(&mut self, user: &str) {
        if let Some(p) = self.peers.get_mut(user) {
            p.fails += 1;
        }
    }

    /// The best `n` peers we know, best first. Peers that shared one of `artists` come first.
    pub fn best_peers(&self, n: usize, artists: &[String]) -> Vec<(String, Peer)> {
        let now = now();
        let mut v: Vec<(String, Peer, f64)> = self
            .peers
            .iter()
            .filter(|(_, p)| p.hits > 0)
            .map(|(u, p)| {
                let shared = p.artists.iter().filter(|a| artists.contains(a)).count() as f64;
                (u.clone(), p.clone(), p.score(now) + shared * 15.0)
            })
            .collect();
        v.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        v.into_iter().take(n).map(|(u, p, _)| (u, p)).collect()
    }

    /// The cached answers to `query`, when they are recent.
    pub fn cached(&self, query: &str) -> Option<Vec<SearchResponse>> {
        let c = self.cache.get(query)?;
        if now() - c.at > CACHE_HOURS * 3600.0 {
            return None;
        }
        Some(
            c.responses
                .iter()
                .map(|r| SearchResponse {
                    user: r.user.clone(),
                    free_slot: r.free,
                    avg_speed: r.speed,
                    queue_len: r.queue,
                    files: r
                        .files
                        .iter()
                        .map(|f| SharedFile { filename: f.f.clone(), size: f.s, ext: f.e.clone(), bitrate: opt(f.br), duration: opt(f.d), vbr: None, sample_rate: opt(f.sr), bit_depth: opt(f.bd) })
                        .collect(),
                })
                .collect(),
        )
    }

    /// Keeps the answers to `query` (the audio files only, at most a few per peer).
    pub fn cache(&mut self, query: &str, responses: &[SearchResponse]) {
        let mut total = 0;
        let mut out = Vec::new();
        for r in responses {
            if total >= CACHE_MAX_FILES {
                break;
            }
            let files: Vec<CachedFile> = r
                .files
                .iter()
                .filter(|f| crate::rank::is_audio(&f.ext))
                .take(6)
                .map(|f| CachedFile {
                    f: f.filename.clone(),
                    s: f.size,
                    e: f.ext.clone(),
                    br: f.bitrate.unwrap_or(0),
                    d: f.duration.unwrap_or(0),
                    sr: f.sample_rate.unwrap_or(0),
                    bd: f.bit_depth.unwrap_or(0),
                })
                .collect();
            if files.is_empty() {
                continue;
            }
            total += files.len();
            out.push(CachedResponse { user: r.user.clone(), free: r.free_slot, speed: r.avg_speed, queue: r.queue_len, files });
        }
        if !out.is_empty() {
            self.cache.insert(query.to_string(), Cached { at: now(), responses: out });
        }
    }
}

/// The folder of a peer's file ("a\b\c.mp3" → "a\b").
pub fn folder_of(path: &str) -> &str {
    match path.rfind(['\\', '/']) {
        Some(i) => &path[..i],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_peers_and_answers_between_runs() {
        let dir = std::env::temp_dir().join(format!("spotseek-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::load(Some(dir.clone()));
        s.hit("zwem", 9_000_000, 6.0, "dillom");
        s.hit("zwem", 9_000_000, 6.0, "duki");
        s.hit("slow", 9_000_000, 90.0, "abba");
        s.fail("slow");
        s.fail("stranger");
        let f = SharedFile { filename: r"@@m\Dillom\Coyote.mp3".into(), size: 8_000_000, ext: "mp3".into(), bitrate: Some(320), duration: Some(200), ..Default::default() };
        let jpg = SharedFile { filename: r"@@m\Dillom\cover.jpg".into(), size: 90_000, ext: "jpg".into(), ..Default::default() };
        s.cache("dillom coyote", &[SearchResponse { user: "zwem".into(), free_slot: true, avg_speed: 1_500_000, queue_len: 0, files: vec![f.clone(), jpg] }]);
        s.save();

        let s = Store::load(Some(dir.clone()));
        assert!(!s.peers.contains_key("stranger"), "only peers that sent something are kept");
        let best = s.best_peers(5, &["abba".into()]);
        assert_eq!(best.len(), 2);
        assert_eq!(best[0].0, "slow", "a peer with the run's artists comes first");
        let best = s.best_peers(5, &[]);
        assert_eq!(best[0].0, "zwem", "else the fast, reliable one");
        assert_eq!(best[0].1.speed(), 1_500_000);
        let c = s.cached("dillom coyote").unwrap();
        assert_eq!(c[0].files, vec![SharedFile { vbr: None, ..f }], "audio files only");
        assert!(s.cached("other").is_none());
        assert_eq!(folder_of(r"@@m\Dillom\Coyote.mp3"), r"@@m\Dillom");
        let _ = std::fs::remove_dir_all(dir);
    }
}
