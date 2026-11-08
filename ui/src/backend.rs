//! The engine as a child process: `spotseek-backend --cli <url> --events` (the Rust program in
//! `engine/`). It writes one JSON event per line; closing its stdin stops the run.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

/// One event from the backend. The fields match `engine/src/events.rs`.
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
    pub bitrate: u32,
    pub url: String,
    pub path: String,
    pub cover: String,
    pub stopped: bool,
    /// track (downloading): the format of the file that grows at `path`.
    pub format: String,
    /// track: the candidate it downloads.
    pub cand: Option<u32>,
    /// collection: the run waits for a pick for each track.
    pub manual: bool,
    /// candidates: the Soulseek files for track `i`, best first.
    pub cands: Option<Vec<Cand>>,
    /// candidates: no more will come.
    pub settled: bool,
}

/// One Soulseek file for a track (`engine/src/events.rs`, `CandInfo`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Cand {
    pub id: u32,
    pub user: String,
    pub file: String,
    pub folder: String,
    pub ext: String,
    pub size: u64,
    pub bitrate: u32,
    pub duration: u32,
    pub sample_rate: u32,
    pub bit_depth: u32,
    pub free: bool,
    /// Bytes per second.
    pub speed: u32,
    pub queue: u32,
    /// 0 to 100: how well the name and length fit the track.
    pub fit: u32,
    pub score: f32,
    /// Seconds the download should take.
    pub eta: u32,
    pub failed: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TrackInfo {
    pub artist: String,
    pub title: String,
    pub duration: u32,
    pub album: String,
    pub id: String,
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
    /// Wait after the collection event for [`Run::answer`]: which tracks to download.
    pub ask: bool,
    /// No Soulseek account: YouTube Music only.
    pub skip_soulseek: bool,
    /// The user picks the file for each track.
    pub manual: bool,
    /// Where the backend saves the cover image, or empty.
    pub cover_out: String,
    /// Every line the backend writes (stdout and stderr) is copied here.
    pub log: Option<PathBuf>,
}

/// A running backend. Dropping it does not stop the run; call `stop`.
pub struct Run {
    child: Arc<Mutex<Child>>,
    stdin: Option<std::process::ChildStdin>,
    #[cfg(windows)]
    job: Option<Arc<job::Job>>,
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
        if req.ask {
            cmd.arg("--ask");
        }
        if req.skip_soulseek {
            cmd.arg("--skip-soulseek");
        }
        if req.manual {
            cmd.arg("--manual");
        }
        if !req.cover_out.is_empty() {
            cmd.arg("--cover-out").arg(&req.cover_out);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            // The backend and the tools it runs (yt-dlp, ffmpeg) are console
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
        // On Windows, killing a process does not kill what it started (yt-dlp runs as a
        // parent and a worker). A job object holds the whole tree, so Stop can end it,
        // and Windows ends it when this app exits.
        #[cfg(windows)]
        let job = job::Job::holding(&child).map(Arc::new);
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(child));
        let waiter = child.clone();
        let err_tail: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = Log::open(req.log.as_deref());
        log.line(&format!("# spotseek {} {}", env!("CARGO_PKG_VERSION"), req.url));
        if let Some(stderr) = stderr {
            let tail = err_tail.clone();
            let log = log.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log.line(&format!("# stderr: {line}"));
                    let mut t = tail.lock().unwrap();
                    t.push(line);
                    let n = t.len();
                    if n > 20 {
                        t.drain(..n - 20);
                    }
                }
            });
        }
        #[cfg(windows)]
        let reader_job = job.clone();
        std::thread::spawn(move || {
            // Keep the job open until the backend has exited.
            #[cfg(windows)]
            let _job = reader_job;
            let mut ended = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                log.line(&line);
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
            log.line(&format!("# exit: {status:?}"));
            let msg = match status {
                Some(s) if !s.success() && !ended => {
                    let tail = err_tail.lock().unwrap().join("\n");
                    Some(if tail.is_empty() { format!("the backend stopped ({s})") } else { tail })
                }
                _ => None,
            };
            on_exit(msg);
        });
        Ok(Run {
            child,
            stdin,
            #[cfg(windows)]
            job,
        })
    }

    /// Answers the backend's question (`Request::ask`): download only these tracks.
    pub fn answer(&mut self, only: &[usize]) {
        if let Some(s) = self.stdin.as_mut() {
            let line = serde_json::json!({ "only": only }).to_string();
            let _ = writeln!(s, "{line}");
            let _ = s.flush();
        }
    }

    /// Downloads candidate `cand` for track `i` (None: from YouTube Music instead).
    pub fn pick(&mut self, i: usize, cand: Option<u32>) {
        if let Some(s) = self.stdin.as_mut() {
            let line = match cand {
                Some(id) => serde_json::json!({ "pick": { "i": i, "id": id } }),
                None => serde_json::json!({ "pick": { "i": i, "yt": true } }),
            };
            let _ = writeln!(s, "{line}");
            let _ = s.flush();
        }
    }

    /// Ends the downloads early: the backend still converts the finished files and sends a
    /// result (with `stopped`).
    pub fn finish(&mut self) {
        if let Some(s) = self.stdin.as_mut() {
            let _ = writeln!(s, "{{\"stop\":true}}");
            let _ = s.flush();
        }
    }

    /// Asks the backend to stop (closes its stdin), and kills it if it is still running after
    /// a few seconds. On Windows this also ends every program it started.
    pub fn stop(&mut self) {
        self.stdin.take();
        let child = self.child.clone();
        #[cfg(windows)]
        let job = self.job.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(4));
            let mut c = child.lock().unwrap();
            if let Ok(None) = c.try_wait() {
                let _ = c.kill();
            }
            #[cfg(windows)]
            if let Some(job) = job {
                job.terminate();
            }
        });
    }
}

/// A log file that several threads write lines to.
#[derive(Clone)]
struct Log(Option<Arc<Mutex<std::fs::File>>>);

impl Log {
    fn open(path: Option<&std::path::Path>) -> Log {
        let file = path.and_then(|p| {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
                prune(dir, 40);
            }
            std::fs::File::create(p).ok()
        });
        Log(file.map(|f| Arc::new(Mutex::new(f))))
    }

    fn line(&self, line: &str) {
        if let Some(f) = &self.0 {
            let _ = writeln!(f.lock().unwrap(), "{line}");
        }
    }
}

/// Deletes the oldest log files in `dir` so at most `keep` stay.
fn prune(dir: &std::path::Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut logs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect();
    logs.sort();
    while logs.len() >= keep {
        let _ = std::fs::remove_file(logs.remove(0));
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

    /// A job object that holds a process and everything it starts. Closing the last handle
    /// (also when this app exits) ends them all.
    pub struct Job(HANDLE);

    // The handle is only used through thread-safe Win32 calls.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn holding(child: &Child) -> Option<Job> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return None;
                }
                let job = Job(h);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 || AssignProcessToJobObject(h, child.as_raw_handle() as HANDLE) == 0 {
                    return None;
                }
                Some(job)
            }
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
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
