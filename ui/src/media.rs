//! Playback, waveforms and covers for the track details. The audio device lives on its own
//! thread; decoding runs on worker threads. Each finished job wakes the view.
//!
//! A file that is still downloading can play too (a preview): the player only plays the part
//! that is on disk. It never plays past it; when the play head comes near the end of what is
//! there, it waits, and goes on when more has come.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rune::Pixels;

/// Bars in a waveform.
pub const BARS: usize = 120;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// The file that is loaded, or empty.
    pub path: String,
    pub playing: bool,
    /// Seconds.
    pub pos: f64,
    pub dur: f64,
    pub error: String,
    /// Paused at the end of what has downloaded; it goes on by itself.
    pub buffering: bool,
}

/// A file that is still downloading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Grow {
    /// Bytes on disk (from the backend's events).
    pub bytes: u64,
    /// The final size.
    pub total: u64,
    /// The length of the song in seconds.
    pub secs: f64,
}

/// Bytes kept back from the end of what is on disk: the decoder reads a little ahead.
const SAFE_BYTES: u64 = 96 * 1024;
/// Seconds kept back from the end of what is on disk.
const SAFE_SECS: f64 = 1.5;

impl Grow {
    pub fn complete(&self) -> bool {
        self.total > 0 && self.bytes >= self.total
    }

    /// Seconds from the start that can play now.
    pub fn playable(&self) -> f64 {
        if self.complete() {
            return self.secs;
        }
        if self.total == 0 || self.secs <= 0.0 {
            return 0.0;
        }
        (self.secs * self.bytes.saturating_sub(SAFE_BYTES) as f64 / self.total as f64 - SAFE_SECS).max(0.0)
    }

    /// The part on disk, 0 to 1.
    pub fn frac(&self) -> f32 {
        if self.total == 0 { 0.0 } else { (self.bytes as f64 / self.total as f64).min(1.0) as f32 }
    }
}

type Growing = Arc<Mutex<HashMap<String, Grow>>>;

#[derive(Clone)]
pub enum Wave {
    Loading,
    /// Peaks 0..1, one per bar, and the length in seconds.
    Ready(Arc<Vec<f32>>, f64),
    Failed,
}

#[derive(Clone)]
pub enum Cover {
    Loading,
    Ready(Pixels),
    None,
}

enum Cmd {
    Play(String, f64),
    Toggle,
    Pause,
    Seek(f64),
    Stop,
    /// The downloaded file became the final mp3: go on with it at the same place.
    Swap(String, String),
}

type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct Media {
    cmds: Mutex<Option<mpsc::Sender<Cmd>>>,
    status: Arc<Mutex<Status>>,
    waves: Arc<Mutex<HashMap<String, Wave>>>,
    /// For waves of growing files: the bytes they were made from.
    wave_bytes: Arc<Mutex<HashMap<String, u64>>>,
    covers: Arc<Mutex<HashMap<String, Cover>>>,
    growing: Growing,
    /// The loudness of what plays, newest last, about 60 values a second (for the live line).
    levels: Levels,
    /// Where each file was left (seconds), so it goes on from there.
    positions: Positions,
    /// Tests: no sound output; commands are only written down.
    silent: Option<Mutex<Vec<String>>>,
    wake: Wake,
}

type Levels = Arc<Mutex<std::collections::VecDeque<f32>>>;
type Positions = Arc<Mutex<HashMap<String, f64>>>;
/// Loudness values kept for the live line.
pub const LEVELS: usize = 96;

impl Media {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Media {
        Media {
            cmds: Mutex::new(None),
            status: Arc::default(),
            waves: Arc::default(),
            wave_bytes: Arc::default(),
            covers: Arc::default(),
            growing: Arc::default(),
            levels: Arc::default(),
            positions: Arc::default(),
            silent: None,
            wake: Arc::new(wake),
        }
    }

    /// For tests and headless scenes: never opens a sound output.
    pub fn silent() -> Media {
        Media { silent: Some(Mutex::default()), ..Media::new(|| {}) }
    }

    /// Tests: the commands a silent media got.
    pub fn sent(&self) -> Vec<String> {
        self.silent.as_ref().map(|s| s.lock().unwrap().clone()).unwrap_or_default()
    }

    /// The loudness of the last moments of what plays, 0 to 1, oldest first.
    pub fn levels(&self) -> Vec<f32> {
        self.levels.lock().unwrap().iter().copied().collect()
    }

    /// Where `path` was left, in seconds.
    pub fn position(&self, path: &str) -> Option<f64> {
        let st = self.status();
        if st.path == path && (st.playing || st.pos > 0.0) {
            return Some(st.pos);
        }
        self.positions.lock().unwrap().get(path).copied()
    }

    /// For headless scenes: a made-up loudness line.
    pub fn fake_levels(&self, seed: u32) {
        let v = (0..LEVELS).map(|i| (0.25 + 0.6 * ((i as f32 * 0.37 + seed as f32).sin() * (i as f32 * 0.11).cos()).abs()).min(1.0)).collect();
        *self.levels.lock().unwrap() = v;
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn send(&self, c: Cmd) {
        if let Some(log) = &self.silent {
            let line = match &c {
                Cmd::Play(p, at) => format!("play {p} {at:.1}"),
                Cmd::Toggle => "toggle".into(),
                Cmd::Pause => "pause".into(),
                Cmd::Seek(s) => format!("seek {s:.1}"),
                Cmd::Stop => "stop".into(),
                Cmd::Swap(a, b) => format!("swap {a} {b}"),
            };
            log.lock().unwrap().push(line);
            return;
        }
        let mut cmds = self.cmds.lock().unwrap();
        if cmds.is_none() {
            let (tx, rx) = mpsc::channel();
            let (status, wake, growing, levels, positions) = (self.status.clone(), self.wake.clone(), self.growing.clone(), self.levels.clone(), self.positions.clone());
            std::thread::spawn(move || audio(rx, status, wake, growing, levels, positions));
            *cmds = Some(tx);
        }
        let _ = cmds.as_ref().unwrap().send(c);
    }

    /// Plays `path` from `at` seconds, or pauses / resumes it when it is already loaded.
    pub fn toggle(&self, path: &str) {
        let st = self.status();
        if st.path == path {
            self.send(Cmd::Toggle);
        } else {
            // From where it was left.
            let at = self.positions.lock().unwrap().get(path).copied().unwrap_or(0.0);
            self.send(Cmd::Play(path.to_string(), at));
        }
    }

    /// Hover play: plays `path` from where it was left (`true`), or pauses it (`false`).
    pub fn hover(&self, path: &str, on: bool) {
        let st = self.status();
        if on {
            if st.path == path {
                if !st.playing {
                    self.send(Cmd::Toggle);
                }
            } else {
                let at = self.positions.lock().unwrap().get(path).copied().unwrap_or(0.0);
                self.send(Cmd::Play(path.to_string(), at));
            }
        } else if st.path == path && (st.playing || st.buffering) {
            self.send(Cmd::Pause);
        }
    }

    /// Pauses what plays (nothing when nothing plays).
    pub fn pause(&self) {
        let st = self.status();
        if st.playing || st.buffering {
            self.send(Cmd::Pause);
        }
    }

    /// Jumps to `secs` in `path`, and plays it.
    pub fn seek(&self, path: &str, secs: f64) {
        if self.status().path == path {
            self.send(Cmd::Seek(secs));
        } else {
            self.send(Cmd::Play(path.to_string(), secs));
        }
    }

    /// `path` is downloading: `bytes` of `total` are on disk, and the song is `secs` long.
    pub fn set_growing(&self, path: &str, bytes: u64, total: u64, secs: f64) {
        if path.is_empty() {
            return;
        }
        self.growing.lock().unwrap().insert(path.to_string(), Grow { bytes, total, secs });
    }

    /// The download state of `path`, or None when it is a whole file.
    pub fn growing(&self, path: &str) -> Option<Grow> {
        self.growing.lock().unwrap().get(path).copied()
    }

    /// The download of `old` became the file `new`: a preview that plays goes on with it.
    pub fn swap(&self, old: &str, new: &str) {
        if !old.is_empty() && self.status().path == old {
            self.send(Cmd::Swap(old.to_string(), new.to_string()));
        }
    }

    pub fn stop(&self) {
        if self.cmds.lock().unwrap().is_some() {
            self.send(Cmd::Stop);
        }
    }

    /// The waveform of `path`; the first ask starts decoding it.
    pub fn wave(&self, path: &str) -> Wave {
        let grow = self.growing(path);
        let mut waves = self.waves.lock().unwrap();
        if let Some(w) = waves.get(path) {
            // A growing file gets a new wave when a tenth more has come.
            let stale = grow.is_some_and(|g| {
                let had = self.wave_bytes.lock().unwrap().get(path).copied().unwrap_or(0);
                had != u64::MAX && (g.complete() || g.bytes >= had + g.total / 10)
            });
            if !stale || matches!(w, Wave::Loading) {
                return w.clone();
            }
        }
        let old = waves.get(path).cloned();
        waves.insert(path.to_string(), old.clone().unwrap_or(Wave::Loading));
        // u64::MAX: being made now.
        self.wave_bytes.lock().unwrap().insert(path.to_string(), u64::MAX);
        let now = old.clone();
        let (waves, wave_bytes, wake, p, growing) = (self.waves.clone(), self.wave_bytes.clone(), self.wake.clone(), path.to_string(), self.growing.clone());
        std::thread::spawn(move || {
            let g = growing.lock().unwrap().get(&p).copied();
            let w = match peaks(&p, g, &growing) {
                Some((v, d)) => Wave::Ready(Arc::new(v), d),
                None if g.is_some() => old.unwrap_or(Wave::Loading),
                None => Wave::Failed,
            };
            waves.lock().unwrap().insert(p.clone(), w);
            wave_bytes.lock().unwrap().insert(p, g.map_or(0, |g| g.bytes));
            wake();
        });
        now.unwrap_or(Wave::Loading)
    }

    /// For headless scenes: a waveform made up from `seed`.
    pub fn fake_wave(&self, path: &str, secs: f64, seed: u32) {
        let v = (0..BARS)
            .map(|i| {
                let x = i as f32 / BARS as f32;
                let h = ((i as f32 * 12.9898 + seed as f32 * 78.233).sin() * 43758.547).fract().abs();
                (0.25 + 0.55 * h) * (0.55 + 0.45 * (x * 9.0 + seed as f32).sin().abs()) * (x * 30.0).min(1.0)
            })
            .collect();
        self.waves.lock().unwrap().insert(path.to_string(), Wave::Ready(Arc::new(v), secs));
    }

    /// For headless scenes: show a playing position.
    pub fn fake_status(&self, st: Status) {
        *self.status.lock().unwrap() = st;
    }

    /// The cover of `path` (its embedded picture), else the image file `fallback`.
    pub fn cover(&self, path: &str, fallback: &str) -> Cover {
        let k = format!("{path}\u{0}{fallback}");
        let mut covers = self.covers.lock().unwrap();
        if let Some(c) = covers.get(&k) {
            return c.clone();
        }
        covers.insert(k.clone(), Cover::Loading);
        let (covers, wake, p, f) = (self.covers.clone(), self.wake.clone(), path.to_string(), fallback.to_string());
        std::thread::spawn(move || {
            let c = match cover_rgba(&p, &f) {
                Some((w, h, rgba)) => {
                    let px = Pixels::new();
                    px.set(w, h, rgba);
                    Cover::Ready(px)
                }
                None => Cover::None,
            };
            covers.lock().unwrap().insert(k, c);
            wake();
        });
        Cover::Loading
    }
}

/// Opens `path` for the decoder. A growing file is read only up to what is on disk.
fn open(path: &str, growing: &Growing, wait: bool) -> Result<rodio::Decoder<GrowingFile>, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let grow = growing.lock().unwrap().get(path).copied();
    let total = match grow {
        Some(g) if g.total > 0 => g.total,
        _ => f.metadata().map(|m| m.len()).unwrap_or(0),
    };
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("mp3").to_lowercase();
    let src = GrowingFile { file: f, pos: 0, total, growing: grow.is_some(), wait };
    rodio::Decoder::builder().with_data(src).with_byte_len(total).with_hint(&ext).with_seekable(true).build().map_err(|e| e.to_string())
}

/// A file that may still be growing. Reads stop at what is on disk; with `wait`, a read there
/// waits a moment for more first (the player pauses before it gets there, so this is rare).
pub struct GrowingFile {
    file: std::fs::File,
    pos: u64,
    total: u64,
    growing: bool,
    wait: bool,
}

impl std::io::Read for GrowingFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::Seek;
        if self.growing {
            let mut tries = 0;
            loop {
                let on_disk = self.file.metadata().map(|m| m.len()).unwrap_or(0);
                if self.pos < on_disk || on_disk >= self.total {
                    break;
                }
                if !self.wait || tries >= 40 {
                    return Ok(0);
                }
                tries += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        self.file.seek(std::io::SeekFrom::Start(self.pos))?;
        let n = self.file.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for GrowingFile {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        self.pos = match to {
            std::io::SeekFrom::Start(p) => p,
            std::io::SeekFrom::End(d) => (self.total as i64 + d).max(0) as u64,
            std::io::SeekFrom::Current(d) => (self.pos as i64 + d).max(0) as u64,
        };
        Ok(self.pos)
    }
}

/// Passes the sound through and writes its loudness (the peak of each 1/60 s) for the live line.
struct Meter<S> {
    inner: S,
    levels: Levels,
    peak: f32,
    n: usize,
    block: usize,
}

impl<S: rodio::Source> Meter<S> {
    fn new(inner: S, levels: Levels) -> Meter<S> {
        let block = (inner.sample_rate().get() as usize * inner.channels().get() as usize / 60).max(1);
        Meter { inner, levels, peak: 0.0, n: 0, block }
    }
}

impl<S: rodio::Source> Iterator for Meter<S> {
    type Item = rodio::Sample;
    fn next(&mut self) -> Option<rodio::Sample> {
        let s = self.inner.next()?;
        self.peak = self.peak.max((s as f32).abs());
        self.n += 1;
        if self.n >= self.block {
            let mut l = self.levels.lock().unwrap();
            l.push_back(self.peak.min(1.0));
            while l.len() > LEVELS {
                l.pop_front();
            }
            self.n = 0;
            self.peak = 0.0;
        }
        Some(s)
    }
}

impl<S: rodio::Source> rodio::Source for Meter<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)
    }
}

fn audio(rx: mpsc::Receiver<Cmd>, status: Arc<Mutex<Status>>, wake: Wake, growing: Growing, levels: Levels, positions: Positions) {
    let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
        Ok(mut s) => {
            s.log_on_drop(false);
            s
        }
        Err(e) => {
            status.lock().unwrap().error = format!("no audio output: {e}");
            wake();
            // Keep answering, so the view does not wait on a dead thread.
            while rx.recv().is_ok() {}
            return;
        }
    };
    let mut player: Option<rodio::Player> = None;
    // How far a growing file can play now, or None for a whole file.
    let limit = |path: &str| growing.lock().unwrap().get(path).filter(|g| !g.complete()).map(|g| g.playable());
    // Loads `path` and plays it from `at` seconds.
    let load = |path: String, at: f64, player: &mut Option<rodio::Player>| {
        if let Some(p) = player.take() {
            p.stop();
        }
        let grow = growing.lock().unwrap().get(&path).copied();
        let at = match grow.filter(|g| !g.complete()) {
            Some(g) => {
                if g.playable() < 1.0 {
                    *status.lock().unwrap() = Status { path, error: "a moment: it is still downloading".into(), ..Default::default() };
                    return;
                }
                at.min(g.playable() - 1.0).max(0.0)
            }
            None => at,
        };
        let opened = open(&path, &growing, true);
        let mut st = status.lock().unwrap();
        match opened {
            Ok(dec) => {
                use rodio::Source;
                let dur = dec.total_duration().map_or(0.0, |d| d.as_secs_f64());
                let dur = if dur > 0.0 { dur } else { grow.map_or(0.0, |g| g.secs) };
                let p = rodio::Player::connect_new(sink.mixer());
                levels.lock().unwrap().clear();
                p.append(Meter::new(dec, levels.clone()));
                if at > 0.0 {
                    let _ = p.try_seek(Duration::from_secs_f64(at));
                }
                p.play();
                *st = Status { path, playing: true, pos: at, dur, ..Default::default() };
                *player = Some(p);
            }
            Err(e) => *st = Status { path, error: format!("cannot play: {e}"), ..Default::default() },
        }
    };
    loop {
        let cmd = rx.recv_timeout(Duration::from_millis(80));
        let mut changed = cmd.is_ok();
        match cmd {
            Ok(Cmd::Play(path, at)) => {
                // The one that played is left where it was.
                let st = status.lock().unwrap().clone();
                if !st.path.is_empty() && st.path != path {
                    positions.lock().unwrap().insert(st.path, st.pos);
                }
                load(path, at, &mut player)
            }
            Ok(Cmd::Pause) => {
                if let Some(p) = &player {
                    p.pause();
                }
                let mut st = status.lock().unwrap();
                st.playing = false;
                st.buffering = false;
                positions.lock().unwrap().insert(st.path.clone(), st.pos);
            }
            Ok(Cmd::Toggle) => {
                let buffering = status.lock().unwrap().buffering;
                if buffering {
                    // Waiting for the download: pressing pause stops the wait.
                    let mut st = status.lock().unwrap();
                    st.buffering = false;
                    st.playing = false;
                    if let Some(p) = &player {
                        p.pause();
                    }
                } else if let Some(p) = &player {
                    if p.is_paused() {
                        p.play();
                    } else {
                        p.pause();
                    }
                    status.lock().unwrap().playing = !p.is_paused();
                } else {
                    // Played to the end: start again.
                    let path = status.lock().unwrap().path.clone();
                    if !path.is_empty() {
                        load(path, 0.0, &mut player);
                    }
                }
            }
            Ok(Cmd::Seek(secs)) => {
                if player.is_none() {
                    let path = status.lock().unwrap().path.clone();
                    if !path.is_empty() {
                        load(path, secs, &mut player);
                    }
                } else if let Some(p) = &player {
                    let path = status.lock().unwrap().path.clone();
                    let secs = match limit(&path) {
                        Some(l) => secs.min(l - 1.0).max(0.0),
                        None => secs,
                    };
                    let _ = p.try_seek(Duration::from_secs_f64(secs.max(0.0)));
                    p.play();
                    let mut st = status.lock().unwrap();
                    st.pos = secs;
                    st.playing = true;
                    st.buffering = false;
                }
            }
            Ok(Cmd::Swap(old, new)) => {
                let st = status.lock().unwrap().clone();
                if st.path == old {
                    let was_playing = st.playing || st.buffering;
                    load(new, st.pos, &mut player);
                    if !was_playing {
                        if let Some(p) = &player {
                            p.pause();
                        }
                        status.lock().unwrap().playing = false;
                    }
                }
            }
            Ok(Cmd::Stop) => {
                if let Some(p) = player.take() {
                    p.stop();
                }
                let st = status.lock().unwrap().clone();
                if !st.path.is_empty() {
                    positions.lock().unwrap().insert(st.path, st.pos);
                }
                *status.lock().unwrap() = Status::default();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let path = status.lock().unwrap().path.clone();
        let lim = limit(&path);
        if let Some(p) = &player {
            let mut st = status.lock().unwrap();
            if p.empty() {
                if lim.is_some() {
                    // The decoder ran out of downloaded data: wait for more.
                    st.playing = false;
                    st.buffering = true;
                } else {
                    // The end of the song.
                    st.playing = false;
                    st.buffering = false;
                    st.pos = 0.0;
                    positions.lock().unwrap().insert(st.path.clone(), 0.0);
                }
                player = None;
                changed = true;
            } else if !p.is_paused() {
                st.pos = p.get_pos().as_secs_f64();
                // Never past what is on disk: wait there.
                if let Some(l) = lim {
                    if st.pos >= l {
                        p.pause();
                        st.playing = false;
                        st.buffering = true;
                        changed = true;
                    }
                }
            } else if st.buffering && lim.is_none_or(|l| l >= st.pos + 3.0) {
                // Enough has come: go on.
                p.play();
                st.playing = true;
                st.buffering = false;
                changed = true;
            }
        } else {
            let st = status.lock().unwrap().clone();
            if st.buffering && lim.is_none_or(|l| l >= st.pos + 3.0) {
                load(st.path, st.pos, &mut player);
                status.lock().unwrap().buffering = player.is_some();
                if let Some(p) = &player {
                    p.play();
                    let mut s2 = status.lock().unwrap();
                    s2.buffering = false;
                    s2.playing = true;
                }
                changed = true;
            }
        }
        if changed {
            wake();
        }
    }
}

/// The loudest sample of each bar, scaled so the loudest bar is 1, and the length in seconds.
/// For a growing file (`grow`), only the part on disk; the bars are placed on the whole length.
fn peaks(path: &str, grow: Option<Grow>, growing: &Growing) -> Option<(Vec<f32>, f64)> {
    use rodio::Source;
    let dec = open(path, growing, false).ok()?;
    let ch = dec.channels().get() as usize;
    let rate = dec.sample_rate().get() as f64;
    // One value per 1024 frames first; the length is not known before the end.
    let block = 1024 * ch;
    let mut blocks = Vec::new();
    let (mut n, mut peak, mut total) = (0usize, 0f32, 0usize);
    for s in dec {
        peak = peak.max((s as f32).abs());
        n += 1;
        total += 1;
        if n == block {
            blocks.push(peak);
            n = 0;
            peak = 0.0;
        }
    }
    if n > 0 {
        blocks.push(peak);
    }
    if blocks.is_empty() {
        return None;
    }
    let mut bars = vec![0f32; BARS];
    let secs = total as f64 / ch as f64 / rate;
    // A growing file: place the blocks on the song's whole length.
    let span = match grow {
        Some(g) if !g.complete() && g.secs > secs => (blocks.len() as f64 * g.secs / secs.max(0.1)) as usize,
        _ => blocks.len(),
    };
    for (i, b) in blocks.iter().enumerate() {
        let k = (i * BARS / span.max(1)).min(BARS - 1);
        bars[k] = bars[k].max(*b);
    }
    let max = bars.iter().cloned().fold(0.0f32, f32::max).max(1e-4);
    // A little compression, so quiet parts still show.
    let bars = bars.iter().map(|b| (b / max).powf(0.8)).collect();
    let len = match grow {
        Some(g) if !g.complete() && g.secs > 0.0 => g.secs,
        _ => secs,
    };
    Some((bars, len))
}

fn cover_rgba(path: &str, fallback: &str) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = (!path.is_empty())
        .then(|| id3::Tag::read_from_path(path).ok())
        .flatten()
        .and_then(|tag| {
            let pics: Vec<_> = tag.pictures().collect();
            pics.iter().find(|p| p.picture_type == id3::frame::PictureType::CoverFront).or(pics.first()).map(|p| p.data.clone())
        })
        .or_else(|| (!fallback.is_empty()).then(|| std::fs::read(fallback).ok()).flatten())?;
    let img = image::load_from_memory(&bytes).ok()?;
    let img = img.thumbnail(320, 320).to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

/// "3:07" (or "1:02:03").
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_growing_file_plays_only_what_is_on_disk() {
        let g = Grow { bytes: 4_000_000, total: 8_000_000, secs: 200.0 };
        assert!(g.playable() < 100.0 && g.playable() > 95.0);
        assert_eq!(Grow { bytes: 50_000, ..g }.playable(), 0.0, "too little: nothing");
        assert_eq!(Grow { bytes: 8_000_000, ..g }.playable(), 200.0);
        assert!(Grow { bytes: 8_000_000, ..g }.complete());
    }

    #[test]
    fn reads_stop_at_the_end_of_what_is_on_disk() {
        use std::io::{Read, Seek, SeekFrom};
        let p = std::env::temp_dir().join(format!("spotseek-grow-{}", std::process::id()));
        std::fs::write(&p, vec![7u8; 1000]).unwrap();
        let mut f = GrowingFile { file: std::fs::File::open(&p).unwrap(), pos: 0, total: 5000, growing: true, wait: false };
        let mut buf = vec![0u8; 4096];
        let mut got = 0;
        loop {
            let n = f.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            got += n;
        }
        assert_eq!(got, 1000, "never past the bytes on disk");
        assert_eq!(f.seek(SeekFrom::End(0)).unwrap(), 5000, "the size is the final one");
        std::fs::OpenOptions::new().append(true).open(&p).unwrap().write_all_at_end(&[1u8; 500]);
        f.seek(SeekFrom::Start(1000)).unwrap();
        assert_eq!(f.read(&mut buf).unwrap(), 500, "more came: it reads on");
        let _ = std::fs::remove_file(p);
    }

    trait WriteAll {
        fn write_all_at_end(&mut self, b: &[u8]);
    }
    impl WriteAll for std::fs::File {
        fn write_all_at_end(&mut self, b: &[u8]) {
            use std::io::Write;
            self.write_all(b).unwrap();
        }
    }

    /// Needs ffmpeg: a half-downloaded mp3 decodes up to its end, and gives its peaks on the
    /// whole length.
    #[test]
    fn a_half_mp3_decodes() {
        let dir = std::env::temp_dir().join(format!("spotseek-half-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let full = dir.join("full.mp3");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=20", "-c:a", "libmp3lame", "-b:a", "320k"])
            .arg(&full)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("no ffmpeg: skipped");
            return;
        }
        let bytes = std::fs::read(&full).unwrap();
        let half = dir.join("half.mp3");
        std::fs::write(&half, &bytes[..bytes.len() / 2]).unwrap();
        let path = half.to_string_lossy().into_owned();
        let growing: Growing = Arc::default();
        growing.lock().unwrap().insert(path.clone(), Grow { bytes: bytes.len() as u64 / 2, total: bytes.len() as u64, secs: 20.0 });
        let dec = open(&path, &growing, false).expect("opens a half file");
        use rodio::Source;
        let rate = dec.sample_rate().get() as f64 * dec.channels().get() as f64;
        let n = dec.count() as f64;
        let secs = n / rate;
        assert!(secs > 8.0 && secs < 11.0, "decodes the half on disk: {secs:.1} s");
        let g = growing.lock().unwrap().get(&path).copied();
        let (bars, len) = peaks(&path, g, &growing).unwrap();
        assert_eq!(len, 20.0);
        assert!(bars[..50].iter().all(|b| *b > 0.1), "the first half has sound");
        assert!(bars[70..].iter().all(|b| *b == 0.0), "the rest is not there yet");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn clock_like_spotify() {
        assert_eq!(super::clock(0.0), "0:00");
        assert_eq!(super::clock(187.4), "3:07");
        assert_eq!(super::clock(3723.0), "1:02:03");
    }
}

#[cfg(test)]
mod real_file {
    /// `SPOTSEEK_TEST_MP3=/path/song.mp3 cargo test --release real_mp3 -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_mp3() {
        let path = std::env::var("SPOTSEEK_TEST_MP3").expect("set SPOTSEEK_TEST_MP3");
        let t = std::time::Instant::now();
        let (bars, secs) = super::peaks(&path, None, &Default::default()).expect("waveform");
        println!("waveform: {} bars, {secs:.1} s, in {:?}", bars.len(), t.elapsed());
        assert_eq!(bars.len(), super::BARS);
        assert!(secs > 30.0);
        let cover = super::cover_rgba(&path, "");
        println!("cover: {:?}", cover.as_ref().map(|(w, h, _)| (*w, *h)));
        // Plays, seeks and reports the position.
        let m = super::Media::new(|| {});
        m.toggle(&path);
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let a = m.status();
        m.seek(&path, 60.0);
        std::thread::sleep(std::time::Duration::from_millis(600));
        let b = m.status();
        m.toggle(&path);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let c = m.status();
        m.stop();
        println!("playing: {a:?}\nafter seek: {b:?}\npaused: {c:?}");
        if a.error.is_empty() {
            assert!(a.playing && a.pos > 0.5, "plays");
            assert!(b.pos >= 60.0, "seeks");
            assert!(!c.playing, "pauses");
        }
    }
}

#[cfg(test)]
mod preview {
    /// Plays a file while it is written slower than it plays (needs ffmpeg and a sound output):
    /// `cargo test --release preview_never_plays_past_the_download -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn preview_never_plays_past_the_download() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("spotseek-prev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let full = dir.join("full.mp3");
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=330:duration=30", "-c:a", "libmp3lame", "-b:a", "320k"])
            .arg(&full)
            .status()
            .unwrap()
            .success());
        let bytes = std::fs::read(&full).unwrap();
        let total = bytes.len() as u64;
        let part = dir.join("part.mp3");
        let path = part.to_string_lossy().into_owned();
        let mut f = std::fs::File::create(&part).unwrap();
        let m = super::Media::new(|| {});
        // 40% first, then 0.5% every 300 ms: half the speed of playback.
        let mut written = (total * 40 / 100) as usize;
        f.write_all(&bytes[..written]).unwrap();
        f.flush().unwrap();
        m.set_growing(&path, written as u64, total, 30.0);
        m.toggle(&path);
        let (mut max_over, mut waited, mut resumed) = (f64::MIN, false, false);
        let t0 = std::time::Instant::now();
        let mut n = 0;
        while t0.elapsed().as_secs() < 25 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            n += 1;
            if n % 3 == 0 && written < bytes.len() {
                let next = (written + bytes.len() / 200).min(bytes.len());
                f.write_all(&bytes[written..next]).unwrap();
                f.flush().unwrap();
                written = next;
                m.set_growing(&path, written as u64, total, 30.0);
            }
            let st = m.status();
            assert!(st.error.is_empty(), "{}", st.error);
            let on_disk = 30.0 * written as f64 / total as f64;
            max_over = max_over.max(st.pos - on_disk);
            if st.buffering {
                waited = true;
            } else if waited && st.playing {
                resumed = true;
            }
        }
        m.stop();
        println!("max seconds past the download: {max_over:.2}, waited: {waited}, resumed: {resumed}");
        assert!(max_over < 0.0, "never played past what was on disk");
        assert!(waited && resumed, "it waited for the download and went on");
        let _ = std::fs::remove_dir_all(dir);
    }
}
