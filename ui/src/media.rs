//! Playback, waveforms and covers for the track details. The audio device lives on its own
//! thread; decoding runs on worker threads. Each finished job wakes the view.

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
}

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
    Seek(f64),
    Stop,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

pub struct Media {
    cmds: Mutex<Option<mpsc::Sender<Cmd>>>,
    status: Arc<Mutex<Status>>,
    waves: Arc<Mutex<HashMap<String, Wave>>>,
    covers: Arc<Mutex<HashMap<String, Cover>>>,
    wake: Wake,
}

impl Media {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Media {
        Media { cmds: Mutex::new(None), status: Arc::default(), waves: Arc::default(), covers: Arc::default(), wake: Arc::new(wake) }
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn send(&self, c: Cmd) {
        let mut cmds = self.cmds.lock().unwrap();
        if cmds.is_none() {
            let (tx, rx) = mpsc::channel();
            let (status, wake) = (self.status.clone(), self.wake.clone());
            std::thread::spawn(move || audio(rx, status, wake));
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
            self.send(Cmd::Play(path.to_string(), 0.0));
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

    pub fn stop(&self) {
        if self.cmds.lock().unwrap().is_some() {
            self.send(Cmd::Stop);
        }
    }

    /// The waveform of `path`; the first ask starts decoding it.
    pub fn wave(&self, path: &str) -> Wave {
        let mut waves = self.waves.lock().unwrap();
        if let Some(w) = waves.get(path) {
            return w.clone();
        }
        waves.insert(path.to_string(), Wave::Loading);
        let (waves, wake, p) = (self.waves.clone(), self.wake.clone(), path.to_string());
        std::thread::spawn(move || {
            let w = match peaks(&p) {
                Some((v, d)) => Wave::Ready(Arc::new(v), d),
                None => Wave::Failed,
            };
            waves.lock().unwrap().insert(p, w);
            wake();
        });
        Wave::Loading
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

fn audio(rx: mpsc::Receiver<Cmd>, status: Arc<Mutex<Status>>, wake: Wake) {
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
    // Loads `path` and plays it from `at` seconds.
    let load = |path: String, at: f64, player: &mut Option<rodio::Player>| {
        if let Some(p) = player.take() {
            p.stop();
        }
        let opened = std::fs::File::open(&path).map_err(|e| e.to_string()).and_then(|f| rodio::Decoder::try_from(f).map_err(|e| e.to_string()));
        let mut st = status.lock().unwrap();
        match opened {
            Ok(dec) => {
                use rodio::Source;
                let dur = dec.total_duration().map_or(0.0, |d| d.as_secs_f64());
                let p = rodio::Player::connect_new(sink.mixer());
                p.append(dec);
                if at > 0.0 {
                    let _ = p.try_seek(Duration::from_secs_f64(at));
                }
                p.play();
                *st = Status { path, playing: true, pos: at, dur, error: String::new() };
                *player = Some(p);
            }
            Err(e) => *st = Status { path, error: format!("cannot play: {e}"), ..Default::default() },
        }
    };
    loop {
        let cmd = rx.recv_timeout(Duration::from_millis(80));
        let mut changed = cmd.is_ok();
        match cmd {
            Ok(Cmd::Play(path, at)) => load(path, at, &mut player),
            Ok(Cmd::Toggle) => {
                if let Some(p) = &player {
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
                    let _ = p.try_seek(Duration::from_secs_f64(secs.max(0.0)));
                    p.play();
                    let mut st = status.lock().unwrap();
                    st.pos = secs;
                    st.playing = true;
                }
            }
            Ok(Cmd::Stop) => {
                if let Some(p) = player.take() {
                    p.stop();
                }
                *status.lock().unwrap() = Status::default();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if let Some(p) = &player {
            let mut st = status.lock().unwrap();
            if p.empty() {
                // The end of the song.
                st.playing = false;
                st.pos = 0.0;
                player = None;
                changed = true;
            } else if !p.is_paused() {
                st.pos = p.get_pos().as_secs_f64();
            }
        }
        if changed {
            wake();
        }
    }
}

/// The loudest sample of each bar, scaled so the loudest bar is 1, and the length in seconds.
fn peaks(path: &str) -> Option<(Vec<f32>, f64)> {
    use rodio::Source;
    let f = std::fs::File::open(path).ok()?;
    let dec = rodio::Decoder::try_from(f).ok()?;
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
    for (i, b) in blocks.iter().enumerate() {
        let k = i * BARS / blocks.len();
        bars[k] = bars[k].max(*b);
    }
    let max = bars.iter().cloned().fold(0.0f32, f32::max).max(1e-4);
    // A little compression, so quiet parts still show.
    let bars = bars.iter().map(|b| (b / max).powf(0.8)).collect();
    Some((bars, total as f64 / ch as f64 / rate))
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
        let (bars, secs) = super::peaks(&path).expect("waveform");
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
