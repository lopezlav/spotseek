//! A sliding-window limit: at most `max` events in any `window`. With a journal file, the times
//! are kept on disk, so the next run counts the events of the run before.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Window {
    max: usize,
    window: Duration,
    times: VecDeque<Instant>,
    journal: Option<PathBuf>,
}

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl Window {
    pub fn new(max: usize, window: Duration) -> Self {
        Window { max, window, times: VecDeque::new(), journal: None }
    }

    /// Like `new`, and reads the times of earlier runs from `path` (a JSON list of Unix seconds);
    /// each take writes it again.
    pub fn with_journal(max: usize, window: Duration, path: PathBuf) -> Self {
        let mut w = Window::new(max, window);
        let saved: Vec<f64> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let (now, unix) = (Instant::now(), unix_now());
        let mut ages: Vec<f64> = saved.into_iter().map(|t| unix - t).filter(|a| *a >= 0.0 && *a < window.as_secs_f64()).collect();
        ages.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        w.times = ages.into_iter().filter_map(|a| now.checked_sub(Duration::from_secs_f64(a))).collect();
        w.journal = Some(path);
        w
    }

    fn save(&self) {
        let Some(path) = &self.journal else { return };
        let (now, unix) = (Instant::now(), unix_now());
        let times: Vec<f64> = self.times.iter().map(|t| unix - now.saturating_duration_since(*t).as_secs_f64()).collect();
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = path.with_extension("tmp");
        if let Ok(b) = serde_json::to_vec(&times) {
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }

    /// How long a take at `now` would wait (zero when a slot is free).
    pub fn wait(&self, now: Instant) -> Duration {
        let live: Vec<&Instant> = self.times.iter().filter(|t| now.saturating_duration_since(**t) < self.window).collect();
        if live.len() < self.max {
            return Duration::ZERO;
        }
        (*live[live.len() - self.max] + self.window).saturating_duration_since(now)
    }

    /// Takes a slot at `now`, or says how long to wait for the next free one.
    pub fn try_take(&mut self, now: Instant) -> Result<(), Duration> {
        while let Some(&t) = self.times.front() {
            if now.saturating_duration_since(t) >= self.window {
                self.times.pop_front();
            } else {
                break;
            }
        }
        if self.times.len() < self.max {
            self.times.push_back(now);
            self.save();
            return Ok(());
        }
        let oldest = *self.times.front().expect("max is at least one");
        Err((oldest + self.window).saturating_duration_since(now).max(Duration::from_millis(1)))
    }
}

/// Waits until `w` has a free slot, then takes it.
pub async fn acquire(w: &tokio::sync::Mutex<Window>) {
    loop {
        let wait = match w.lock().await.try_take(Instant::now()) {
            Ok(()) => return,
            Err(d) => d,
        };
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_window() {
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let mut w = Window::new(3, s(10));
        assert!(w.try_take(t0).is_ok());
        assert!(w.try_take(t0 + s(1)).is_ok());
        assert!(w.try_take(t0 + s(2)).is_ok());
        // Full: the first slot frees at t0+10.
        assert_eq!(w.try_take(t0 + s(3)), Err(s(7)));
        assert_eq!(w.try_take(t0 + s(9)), Err(s(1)));
        assert_eq!(w.wait(t0 + s(9)), s(1), "wait says the same without taking");
        assert_eq!(w.wait(t0 + s(10)), Duration::ZERO);
        assert!(w.try_take(t0 + s(10)).is_ok());
        // Now the slots are at 1, 2 and 10; the next frees at 11.
        assert_eq!(w.try_take(t0 + s(10)), Err(s(1)));
        assert!(w.try_take(t0 + s(12)).is_ok());
        // 10, 12 and the new one; after a long pause all slots are free.
        let later = t0 + s(100);
        for _ in 0..3 {
            assert!(w.try_take(later).is_ok());
        }
        assert!(w.try_take(later).is_err());
    }

    #[test]
    fn the_next_run_counts_the_searches_of_the_last_one() {
        let path = std::env::temp_dir().join(format!("spotseek-searches-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let s = Duration::from_secs;
        let mut a = Window::with_journal(3, s(220), path.clone());
        let now = Instant::now();
        assert!(a.try_take(now).is_ok());
        assert!(a.try_take(now).is_ok());
        // A new run (a new process) reads them back: one slot is left.
        let mut b = Window::with_journal(3, s(220), path.clone());
        assert!(b.try_take(Instant::now()).is_ok());
        let wait = b.try_take(Instant::now()).unwrap_err();
        assert!(wait > s(200), "{wait:?}");
        // Times older than the window are dropped.
        std::fs::write(&path, serde_json::to_vec(&[unix_now() - 500.0]).unwrap()).unwrap();
        assert_eq!(Window::with_journal(3, s(220), path.clone()).wait(Instant::now()), Duration::ZERO);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn acquire_waits() {
        let w = tokio::sync::Mutex::new(Window::new(2, Duration::from_millis(150)));
        let start = Instant::now();
        acquire(&w).await;
        acquire(&w).await;
        assert!(start.elapsed() < Duration::from_millis(100));
        acquire(&w).await;
        assert!(start.elapsed() >= Duration::from_millis(150));
    }
}
