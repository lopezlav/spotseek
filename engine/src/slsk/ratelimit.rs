//! A sliding-window limit: at most `max` events in any `window`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub struct Window {
    max: usize,
    window: Duration,
    times: VecDeque<Instant>,
}

impl Window {
    pub fn new(max: usize, window: Duration) -> Self {
        Window { max, window, times: VecDeque::new() }
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
