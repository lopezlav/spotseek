//! The JSON events the engine writes, one per line, for the app window. The fields match
//! `ui/src/backend.rs`. Empty fields are left out.

use serde::Serialize;

fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}
fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Event {
    pub ev: String,
    /// phase: spotify, soulseek, youtube, convert, done
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phase: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub artist: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<TrackInfo>,
    /// The track index in the collection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub i: Option<usize>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub state: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub peer: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub bytes: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub total: u64,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub bitrate: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// file: the final mp3. track (downloading): the file that grows, for the preview player.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// track (downloading): the format of the file that grows ("mp3", "flac").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub format: String,
    /// track: the id of the candidate it downloads (see `candidates`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cand: Option<u32>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cover: String,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub done: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub count: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub files: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub missing: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub dir: String,
    #[serde(skip_serializing_if = "is_false")]
    pub stopped: bool,
    /// collection: the run waits for a pick for each track (`--manual`).
    #[serde(skip_serializing_if = "is_false")]
    pub manual: bool,
    /// candidates: the Soulseek files for track `i`, best first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cands: Option<Vec<CandInfo>>,
    /// candidates: the search is over (no more will come).
    #[serde(skip_serializing_if = "is_false")]
    pub settled: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub line: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

impl Event {
    pub fn new(ev: &str) -> Event {
        Event { ev: ev.into(), ..Default::default() }
    }
    pub fn phase(p: &str) -> Event {
        Event { phase: p.into(), ..Event::new("phase") }
    }
    pub fn log(line: impl Into<String>) -> Event {
        Event { line: line.into(), ..Event::new("log") }
    }
    pub fn track(i: usize, state: &str) -> Event {
        Event { i: Some(i), state: state.into(), ..Event::new("track") }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TrackInfo {
    pub artist: String,
    pub title: String,
    pub duration: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub album: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
}

/// One Soulseek file for a track, with what tells if it is the right song and a fast peer.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct CandInfo {
    /// Stable for the run: the pick message names it.
    pub id: u32,
    pub user: String,
    /// The file name without its folders.
    pub file: String,
    /// The folder it is in (the last one or two parts).
    pub folder: String,
    pub ext: String,
    pub size: u64,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub bitrate: u32,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub duration: u32,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub sample_rate: u32,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub bit_depth: u32,
    pub free: bool,
    /// Bytes per second.
    pub speed: u32,
    pub queue: u32,
    /// How well the name and length fit the track, 0 to 100.
    pub fit: u32,
    /// The rank score (higher is better).
    pub score: f32,
    /// Seconds the download should take at the peer's speed (0: not known).
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub eta: u32,
    /// Tried and failed in this run.
    #[serde(skip_serializing_if = "is_false")]
    pub failed: bool,
}

/// Writes events to stdout, one JSON line each, from any thread.
#[derive(Clone)]
pub struct Emitter(std::sync::Arc<dyn Fn(&Event) + Send + Sync>);

impl Emitter {
    pub fn new(f: impl Fn(&Event) + Send + Sync + 'static) -> Emitter {
        Emitter(std::sync::Arc::new(f))
    }
    pub fn stdout() -> Emitter {
        let lock = std::sync::Mutex::new(());
        Emitter::new(move |e| {
            use std::io::Write;
            let _g = lock.lock().unwrap();
            let mut out = std::io::stdout().lock();
            let _ = serde_json::to_writer(&mut out, e);
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        })
    }
    pub fn emit(&self, e: Event) {
        (self.0)(&e)
    }
}
