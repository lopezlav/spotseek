//! App state and what the backend's events do to it. No drawing here.

use rune::{AppHandle, TextEdit};

use crate::backend::{self, Event, Run};
use crate::gfx;
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TState {
    Pending,
    Searching,
    Downloading,
    Done,
    /// Not on Soulseek; the YouTube fallback may still find it.
    Failed,
    YtSearch,
    YtDownload,
    Missing,
}

impl TState {
    fn from(s: &str) -> Option<TState> {
        Some(match s {
            "searching" => TState::Searching,
            "downloading" => TState::Downloading,
            "done" => TState::Done,
            "failed" => TState::Failed,
            "yt_search" => TState::YtSearch,
            "yt_download" => TState::YtDownload,
            "missing" => TState::Missing,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub artist: String,
    pub title: String,
    pub state: TState,
    pub bytes: u64,
    pub total: u64,
    pub peer: String,
    pub youtube: bool,
}

impl Track {
    pub fn new(artist: &str, title: &str) -> Track {
        Track { artist: artist.into(), title: title.into(), state: TState::Pending, bytes: 0, total: 0, peer: String::new(), youtube: false }
    }

    /// How far this track is, 0 to 1.
    pub fn progress(&self) -> f32 {
        match self.state {
            TState::Pending => 0.0,
            TState::Searching => 0.04,
            TState::Downloading => 0.06 + 0.94 * if self.total > 0 { self.bytes as f32 / self.total as f32 } else { 0.0 },
            TState::Failed | TState::YtSearch => 0.2,
            TState::YtDownload => 0.55,
            TState::Done | TState::Missing => 1.0,
        }
    }
}

pub struct State {
    pub link: TextEdit,
    pub phase: Phase,
    /// The backend's phase: spotify, soulseek, youtube, convert, done.
    pub stage: String,
    pub name: String,
    pub kind: String,
    pub tracks: Vec<Track>,
    pub convert: (usize, usize),
    pub files: usize,
    pub out_dir: String,
    pub error: String,
    pub notice: String,
    pub last_log: String,
    /// Download speed in bytes per second, smoothed.
    pub speed: f32,
    speed_bytes: u64,
    speed_t: f64,
    pub settings: Settings,
    pub settings_open: bool,
    pub dir_edit: TextEdit,
    pub user_edit: TextEdit,
    pub pass_edit: TextEdit,
    pub run: Option<Run>,
    pub run_id: u64,
    pub running_link: String,
    pub handle: Option<AppHandle<State>>,
    /// `SPOTSEEK_DEMO=1`: runs play a scripted download on the app's clock, with no backend.
    pub demo: bool,
    demo_t0: Option<f64>,
    pub now: f64,
}

impl State {
    pub fn new(handle: Option<AppHandle<State>>) -> State {
        let settings = Settings::load();
        State {
            link: TextEdit::new(""),
            phase: Phase::Idle,
            stage: String::new(),
            name: String::new(),
            kind: String::new(),
            tracks: Vec::new(),
            convert: (0, 0),
            files: 0,
            out_dir: String::new(),
            error: String::new(),
            notice: String::new(),
            last_log: String::new(),
            speed: 0.0,
            speed_bytes: 0,
            speed_t: 0.0,
            dir_edit: TextEdit::new(&settings.dir),
            user_edit: TextEdit::new(&settings.user),
            pass_edit: TextEdit::new("").secure(),
            settings,
            settings_open: false,
            run: None,
            run_id: 0,
            running_link: String::new(),
            handle,
            demo: std::env::var_os("SPOTSEEK_DEMO").is_some_and(|v| v != "0"),
            demo_t0: None,
            now: 0.0,
        }
    }

    /// Cmd-V with no field focused: the text becomes the link.
    pub fn paste(&mut self, text: &str) {
        if self.phase == Phase::Idle && !self.settings_open {
            self.link.replace_text(text.trim());
            self.notice.clear();
        }
    }

    pub fn close_settings(&mut self) {
        let dir = self.dir_edit.text.trim();
        if !dir.is_empty() {
            self.settings.dir = dir.to_string();
        }
        self.settings.user = self.user_edit.text.trim().to_string();
        self.settings.save();
        self.settings_open = false;
    }

    /// The pasted text as a Spotify link, or None.
    pub fn link_url(&self) -> Option<String> {
        spotify_link(&self.link.text)
    }

    pub fn start(&mut self) {
        if self.phase == Phase::Running {
            return;
        }
        let Some(url) = self.link_url() else {
            self.notice = "THAT IS NOT A SPOTIFY LINK".into();
            return;
        };
        self.reset_run();
        self.phase = Phase::Running;
        self.stage = "spotify".into();
        self.running_link = url.clone();
        self.run_id += 1;
        if self.demo {
            self.demo_t0 = Some(self.now);
            return;
        }
        let Some(handle) = self.handle.clone() else { return };
        let id = self.run_id;
        let req = backend::Request {
            url,
            dir: self.settings.dir.clone(),
            bitrate: self.settings.bitrate.clone(),
            fallback: self.settings.fallback,
            user: self.settings.user.clone(),
            pass: self.pass_edit.text.clone(),
        };
        let events = handle.clone();
        let exits = handle.clone();
        let started = Run::start(
            &req,
            move |e| events.update(move |s, _| if s.run_id == id { s.apply(e) }),
            move |msg| exits.update(move |s, _| if s.run_id == id { s.exited(msg) }),
        );
        match started {
            Ok(run) => self.run = Some(run),
            Err(e) => {
                self.phase = Phase::Failed;
                self.error = e;
            }
        }
    }

    pub fn stop(&mut self) {
        self.demo_t0 = None;
        if let Some(mut r) = self.run.take() {
            r.stop();
        }
        // Events of the stopped run are ignored from here on.
        self.run_id += 1;
        self.phase = Phase::Idle;
        self.notice = "STOPPED. FILES SO FAR ARE IN THE WORK FOLDER.".into();
    }

    /// Back to the paste screen for a new link.
    pub fn new_link(&mut self) {
        self.demo_t0 = None;
        if let Some(mut r) = self.run.take() {
            r.stop();
        }
        self.run_id += 1;
        self.reset_run();
        self.phase = Phase::Idle;
        self.link.replace_text("");
        self.link.focused = true;
    }

    fn reset_run(&mut self) {
        self.stage.clear();
        self.name.clear();
        self.kind.clear();
        self.tracks.clear();
        self.convert = (0, 0);
        self.files = 0;
        self.error.clear();
        self.notice.clear();
        self.last_log.clear();
        self.speed = 0.0;
        self.speed_bytes = 0;
    }

    pub fn apply(&mut self, e: Event) {
        match e.ev.as_str() {
            "phase" => self.stage = e.phase,
            "collection" => {
                self.name = e.name;
                self.kind = e.kind;
                self.tracks = e.tracks.iter().map(|t| Track::new(&t.artist, &t.title)).collect();
            }
            "track" => {
                let Some(t) = e.i.and_then(|i| self.tracks.get_mut(i)) else { return };
                let Some(st) = TState::from(&e.state) else { return };
                if st == TState::Downloading && t.state != TState::Downloading {
                    t.bytes = 0;
                }
                t.state = st;
                if !e.peer.is_empty() {
                    t.peer = e.peer;
                }
                if e.total > 0 {
                    t.total = e.total;
                }
                if e.source == "youtube" {
                    t.youtube = true;
                }
                if st == TState::Done && t.total > 0 && !t.youtube {
                    t.bytes = t.total;
                }
            }
            "bytes" => {
                let Some(t) = e.i.and_then(|i| self.tracks.get_mut(i)) else { return };
                if e.bytes > t.bytes {
                    self.speed_bytes += e.bytes - t.bytes;
                }
                t.bytes = e.bytes;
                if e.total > 0 {
                    t.total = e.total;
                }
            }
            "convert" => self.convert = (e.done, e.count),
            "log" => self.last_log = e.line,
            "result" => {
                self.phase = Phase::Done;
                self.stage = "done".into();
                self.files = e.files;
                self.out_dir = e.dir;
                self.run = None;
                // Anything the backend never finished is missing.
                for t in &mut self.tracks {
                    if t.state != TState::Done {
                        t.state = TState::Missing;
                    }
                }
            }
            "error" => {
                self.run = None;
                if e.message == "stopped" {
                    self.phase = Phase::Idle;
                } else {
                    self.phase = Phase::Failed;
                    self.error = e.message;
                }
            }
            _ => {}
        }
    }

    fn exited(&mut self, msg: Option<String>) {
        self.run = None;
        if self.phase == Phase::Running {
            self.phase = Phase::Failed;
            self.error = msg.unwrap_or_else(|| "the backend stopped without a result".into());
        }
    }

    /// Samples the download speed (and moves a demo run). True when something changed.
    pub fn tick(&mut self, now: f64) -> bool {
        self.now = now;
        if let Some(t0) = self.demo_t0 {
            if self.phase == Phase::Running {
                self.demo_step(now - t0);
                return true;
            }
            self.demo_t0 = None;
        }
        let dt = now - self.speed_t;
        if dt < 0.5 {
            return false;
        }
        self.speed_t = now;
        let inst = self.speed_bytes as f32 / dt as f32;
        self.speed_bytes = 0;
        let old = self.speed;
        self.speed = if inst > 0.0 { self.speed * 0.4 + inst * 0.6 } else { self.speed * 0.7 };
        if self.speed < 1000.0 {
            self.speed = 0.0;
        }
        (old - self.speed).abs() > 1.0
    }

    /// The scripted demo run at `e` seconds after its start.
    fn demo_step(&mut self, e: f64) {
        let hash = |i: usize| ((i as f64 * 12.9898).sin() * 43758.5453).fract().abs();
        if e < 0.9 {
            self.stage = "spotify".into();
            self.last_log = "Reading Spotify metadata...".into();
            return;
        }
        if self.tracks.is_empty() {
            self.name = "Fiesta 2005".into();
            self.kind = "playlist".into();
            self.tracks = crate::DEMO.iter().map(|(a, t)| Track::new(a, t)).collect();
            self.stage = "soulseek".into();
        }
        let n = self.tracks.len();
        let mut rate = 0.0;
        let mut last_end = 0.0f64;
        for i in 0..n {
            let search = 1.2 + i as f64 * 0.06;
            let start = 3.4 + i as f64 * 0.3;
            let dur = 1.0 + hash(i) * 1.4;
            let total = 6_000_000 + (hash(i + 7) * 4_000_000.0) as u64;
            let failed = i == 23 || i == 31;
            let yt = i == 9 || i == 23;
            let t = &mut self.tracks[i];
            t.total = total;
            let end = start + dur;
            last_end = last_end.max(end + if failed { 2.5 } else { 0.0 });
            t.state = if e < search {
                TState::Pending
            } else if e < start {
                TState::Searching
            } else if failed {
                if e < start + 0.8 { TState::Failed } else if i == 31 && e > start + 1.6 { TState::Missing } else if e < start + 1.6 { TState::YtSearch } else if e < start + 2.5 { TState::YtDownload } else { TState::Done }
            } else if e < end {
                TState::Downloading
            } else {
                TState::Done
            };
            t.youtube = yt || failed;
            t.peer = ["zwem", "xXdj_nico", "rodiganhomelab", "hfegetude", "djcotter"][i % 5].into();
            if t.state == TState::Downloading {
                let p = ((e - start) / dur).clamp(0.0, 1.0);
                t.bytes = (total as f64 * p) as u64;
                rate += total as f64 / dur;
            } else if t.state == TState::Done {
                t.bytes = total;
            }
            if t.state == TState::Downloading || t.state == TState::Searching {
                self.last_log = format!("[{:03}] SongJob: {}: {} - {}", i + 3, if t.state == TState::Searching { "searching" } else { "downloading" }, t.artist, t.title);
            }
        }
        // Shown at a believable Soulseek speed.
        self.speed = rate as f32 / 22.0;
        if e > last_end {
            self.stage = "convert".into();
            let k = ((e - last_end) / 1.6 * n as f64) as usize;
            self.convert = (k.min(n), n);
            self.last_log = format!("Normalizing to mp3 {}kbps...", self.settings.bitrate);
            if e > last_end + 1.8 {
                self.phase = Phase::Done;
                self.stage = "done".into();
                self.files = self.count(TState::Done);
                self.out_dir = self.settings.dir.clone();
                self.demo_t0 = None;
            }
        }
    }

    pub fn count(&self, st: TState) -> usize {
        self.tracks.iter().filter(|t| t.state == st).count()
    }

    pub fn finished(&self) -> usize {
        self.tracks.iter().filter(|t| matches!(t.state, TState::Done | TState::Missing)).count()
    }

    pub fn downloading(&self) -> bool {
        self.tracks.iter().any(|t| matches!(t.state, TState::Downloading | TState::YtDownload))
    }

    /// Overall progress, 0 to 1: the tracks, then the conversion.
    pub fn progress(&self) -> f32 {
        if self.phase == Phase::Done {
            return 1.0;
        }
        if self.tracks.is_empty() {
            return 0.0;
        }
        let tracks = self.tracks.iter().map(Track::progress).sum::<f32>() / self.tracks.len() as f32;
        let conv = if self.convert.1 > 0 { self.convert.0 as f32 / self.convert.1 as f32 } else { 0.0 };
        if self.stage == "convert" { 0.9 + 0.1 * conv } else { tracks * 0.9 }
    }

    /// The peer that sends the newest download.
    pub fn peer(&self) -> Option<&str> {
        self.tracks.iter().rev().find(|t| matches!(t.state, TState::Downloading | TState::YtDownload)).map(|t| t.peer.as_str())
    }

    /// Headline effect data: `[word, wobble, glitch, 0]`.
    pub fn headline(&self) -> [f32; 4] {
        match self.phase {
            Phase::Idle => {
                let has_text = !self.link.text.trim().is_empty();
                let ok = self.link_url().is_some();
                let glitch = if has_text && !ok { 0.35 } else { 0.0 };
                [gfx::W_PASTE, if ok { 1.0 } else { 0.0 }, glitch, 0.0]
            }
            Phase::Running => match self.stage.as_str() {
                "convert" => [gfx::W_CONVERTING, 0.4, 0.25, 0.0],
                _ if self.downloading() || self.count(TState::Done) > 0 => [gfx::W_DOWNLOADING, 0.45, 0.12, 0.0],
                _ => [gfx::W_SEARCHING, 0.6, 0.45, 0.0],
            },
            Phase::Done => [gfx::W_DONE, 0.0, 0.0, 0.0],
            Phase::Failed => [gfx::W_FAILED, 0.2, 0.6, 0.0],
        }
    }

    /// Progress bar effect data: `[fill, speed, active, tracks done]`.
    pub fn bar(&self) -> [f32; 4] {
        let speed = (self.speed / 2_500_000.0).clamp(0.0, 1.0);
        let active = if self.phase == Phase::Running && self.downloading() { 1.0 } else { 0.0 };
        [self.progress(), speed, active, self.count(TState::Done) as f32]
    }
}

/// Accepts `open.spotify.com/{playlist,album,track}/…` (with or without the scheme or an intl
/// prefix) and `spotify:{playlist,album,track}:…`. Returns the trimmed link.
pub fn spotify_link(text: &str) -> Option<String> {
    let t = text.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    if let Some(rest) = t.strip_prefix("spotify:") {
        let mut it = rest.split(':');
        let (kind, id) = (it.next()?, it.next()?);
        // The backend reads web links, so give it one.
        return (matches!(kind, "playlist" | "album" | "track") && !id.is_empty()).then(|| format!("https://open.spotify.com/{kind}/{id}"));
    }
    let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(t);
    let rest = rest.strip_prefix("open.spotify.com/")?;
    let mut parts = rest.split('/').filter(|p| !p.is_empty());
    let mut kind = parts.next()?;
    if kind.starts_with("intl-") {
        kind = parts.next()?;
    }
    let id = parts.next()?.split('?').next()?;
    if matches!(kind, "playlist" | "album" | "track") && id.len() >= 10 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(if t.starts_with("http") { t.to_string() } else { format!("https://{t}") })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_links() {
        assert!(spotify_link("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M").is_some());
        assert!(spotify_link("  https://open.spotify.com/intl-es/album/4aawyAB9vmqN3uQ7FjRGTy?si=x ").is_some());
        assert_eq!(spotify_link("open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy").unwrap(), "https://open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy");
        assert_eq!(spotify_link("spotify:track:4aawyAB9vmqN3uQ7FjRGTy").unwrap(), "https://open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy");
        assert!(spotify_link("https://open.spotify.com/artist/4aawyAB9vmqN3uQ7FjRGTy").is_none());
        assert!(spotify_link("https://example.com/playlist/4aawyAB9vmqN3uQ7FjRGTy").is_none());
        assert!(spotify_link("hello").is_none());
    }

    #[test]
    fn a_real_run_ends_done_with_every_track_placed() {
        let mut s = State::new(None);
        s.phase = Phase::Running;
        for line in include_str!("../tests/fixtures/real-run.jsonl").lines() {
            s.apply(backend::parse(line).unwrap());
        }
        assert_eq!(s.phase, Phase::Done);
        assert_eq!(s.tracks.len(), 3);
        assert_eq!(s.count(TState::Done), 3);
        assert!(s.tracks[2].youtube, "the third track came from YouTube");
        assert_eq!(s.files, 3);
        assert_eq!(s.progress(), 1.0);
    }

    #[test]
    fn progress_grows_with_bytes() {
        let mut s = State::new(None);
        s.phase = Phase::Running;
        let lines: Vec<&str> = include_str!("../tests/fixtures/real-run.jsonl").lines().collect();
        let mut last = 0.0;
        for line in &lines[..lines.len() - 10] {
            s.apply(backend::parse(line).unwrap());
            let p = s.progress();
            assert!(p + 1e-6 >= last || s.stage == "convert", "progress went back: {last} -> {p}");
            last = p;
        }
        assert!(last > 0.5);
    }
}
