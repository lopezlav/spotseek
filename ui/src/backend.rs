//! The Go backend as a child process: `spotseek-backend --cli <url> --events`. It writes one
//! JSON event per line; closing its stdin stops the run.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

/// One event from the backend. The fields match `internal/pipeline/events.go`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Event {
    pub ev: String,
    pub phase: String,
    pub name: String,
    pub kind: String,
    pub artist: String,
    pub tracks: Vec<TrackInfo>,
    pub i: Option<usize>,
    pub state: String,
    pub peer: String,
    pub source: String,
    pub bytes: u64,
    pub total: u64,
    pub done: usize,
    pub count: usize,
    pub files: usize,
    pub missing: usize,
    pub dir: String,
    pub line: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TrackInfo {
    pub artist: String,
    pub title: String,
    pub duration: u32,
}

pub fn parse(line: &str) -> Option<Event> {
    serde_json::from_str(line.trim()).ok()
}

/// What a run needs from the settings.
pub struct Request {
    pub url: String,
    pub dir: String,
    pub bitrate: String,
    pub fallback: bool,
    pub user: String,
    pub pass: String,
}

/// A running backend. Dropping it does not stop the run; call `stop`.
pub struct Run {
    child: Arc<Mutex<Child>>,
    stdin: Option<std::process::ChildStdin>,
}

/// The backend binary: `SPOTSEEK_BACKEND`, else `spotseek-backend` next to this program.
pub fn binary() -> PathBuf {
    if let Some(p) = std::env::var_os("SPOTSEEK_BACKEND") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    let name = if cfg!(windows) { "spotseek-backend.exe" } else { "spotseek-backend" };
    dir.join(name)
}

/// Opens the classic (Fyne) window: the backend started with no arguments. It has the
/// YouTube Music search and the My Spotify library.
pub fn open_classic() -> Result<(), String> {
    let bin = binary();
    let mut cmd = Command::new(&bin);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console behind it
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("cannot start {}: {e}", bin.display()))
}

impl Run {
    /// Starts the backend. `on_event` runs on a reader thread for each event, and `on_exit`
    /// once when the process has ended (with a message when it failed to report one).
    pub fn start(
        req: &Request,
        on_event: impl Fn(Event) + Send + 'static,
        on_exit: impl FnOnce(Option<String>) + Send + 'static,
    ) -> Result<Run, String> {
        let bin = binary();
        let mut cmd = Command::new(&bin);
        cmd.arg("--cli").arg(&req.url).arg("-o").arg(&req.dir).arg("--bitrate").arg(&req.bitrate).arg("--events");
        if !req.fallback {
            cmd.arg("--no-fallback");
        }
        let with_pass = !req.user.is_empty() && !req.pass.is_empty();
        if !req.user.is_empty() {
            cmd.arg("--user").arg(&req.user);
        }
        if with_pass {
            cmd.arg("--pass-stdin");
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            // The backend and the tools it runs (sockseek, yt-dlp, ffmpeg) are console
            // programs. Give them one hidden console, so no window opens for each.
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", bin.display()))?;
        let mut stdin = child.stdin.take();
        if with_pass {
            if let Some(s) = stdin.as_mut() {
                let _ = writeln!(s, "{}", req.pass);
            }
        }
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let waiter = child.clone();
        let err_tail: Arc<Mutex<Vec<String>>> = Arc::default();
        if let Some(stderr) = stderr {
            let tail = err_tail.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut t = tail.lock().unwrap();
                    t.push(line);
                    let n = t.len();
                    if n > 20 {
                        t.drain(..n - 20);
                    }
                }
            });
        }
        std::thread::spawn(move || {
            let mut ended = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(e) = parse(&line) {
                    ended |= e.ev == "result" || e.ev == "error";
                    on_event(e);
                }
            }
            let status = loop {
                if let Ok(Some(s)) = waiter.lock().unwrap().try_wait() {
                    break Some(s);
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            let msg = match status {
                Some(s) if !s.success() && !ended => {
                    let tail = err_tail.lock().unwrap().join("\n");
                    Some(if tail.is_empty() { format!("the backend stopped ({s})") } else { tail })
                }
                _ => None,
            };
            on_exit(msg);
        });
        Ok(Run { child, stdin })
    }

    /// Asks the backend to stop (closes its stdin), and kills it if it is still running after
    /// a few seconds.
    pub fn stop(&mut self) {
        self.stdin.take();
        let child = self.child.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(4));
            let mut c = child.lock().unwrap();
            if let Ok(None) = c.try_wait() {
                let _ = c.kill();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_line_of_a_real_run() {
        let text = include_str!("../tests/fixtures/real-run.jsonl");
        let events: Vec<Event> = text.lines().filter_map(parse).collect();
        assert_eq!(events.len(), text.lines().count());
        let coll = events.iter().find(|e| e.ev == "collection").unwrap();
        assert_eq!(coll.tracks.len(), 3);
        assert!(events.iter().any(|e| e.ev == "bytes" && e.i == Some(0) && e.total > 0));
        assert!(events.iter().any(|e| e.ev == "track" && e.state == "yt_download"));
        assert_eq!(events.last().unwrap().ev, "result");
    }
}
