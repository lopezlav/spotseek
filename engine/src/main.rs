//! spotseek-backend: the download engine of the Spotseek app window.
//!
//! `spotseek-backend --cli <spotify-or-soundcloud-url> -o <dir> --events` writes one JSON event per line (see
//! `events.rs`) and stops when its stdin closes. Stdin lines it reads: the Soulseek password
//! first (with `--pass-stdin`), then `{"only":[0,2]}` (with `--ask`), `{"stop":true}`, and
//! `{"pick":{"i":3,"id":7}}` or `{"pick":{"i":3,"yt":true}}`.
//!
//! `spotseek-backend --serve --events [--user NAME --pass-stdin]` runs many links at the same
//! time on one Soulseek session (the app window's tabs). Stdin lines name a job:
//! `{"job":1,"add":{"url":…,"out":…,"format":"mp3","bitrate":320,"fallback":true,"ask":true,
//! "manual":false,"cover_out":…}}` starts one, `{"job":1,"cancel":true}` ends it at once, and the
//! controls above take a `"job"` too. Each event carries its `"job"` (0: the whole engine).
//!
//! `spotseek-backend --cli --check-login --user NAME` (password on stdin) signs in once and
//! prints `{"ok":true}` or `{"ok":false,"reason":"INVALIDPASS","message":"…"}`.
//!
//! `spotseek-backend --latest` and `--update`: the newest release, and installing it (`update.rs`).

mod convert;
mod events;
mod pipeline;
mod rank;
mod slsk;
mod soundcloud;
mod spotify;
mod store;
mod update;
mod youtube;

use std::io::BufRead;
use std::path::PathBuf;

use events::{Emitter, Event};
use pipeline::{Control, Options};

const HELP: &str = "spotseek-backend --cli <spotify-or-soundcloud-url> -o <dir> [options]

  --format F         the files made: mp3, m4a (AAC) or flac (default mp3)
  --bitrate N        mp3 and m4a kbps: 320, 256, 192 or 128 (default 320)
  --events           JSON events on stdout, one per line; stop when stdin closes
  --ask              with --events: read {\"only\":[indexes]} (or {}) after the collection event
  --manual           with --events: wait for {\"pick\":{\"i\":N,\"id\":K}} for each track
  --no-fallback      do not use YouTube Music
  --skip-soulseek    no Soulseek account: YouTube Music only
  --user NAME        Soulseek user name (else the one in sockseek.conf)
  --pass-stdin       read the Soulseek password from the first stdin line
  --cover-out FILE   with --events: save the collection cover there
  --limit N          only the first N tracks
  --parallel N       downloads at the same time (default 12; YouTube at most 2)
  --state-dir DIR    keep the peer memory and the search cache here (default: the app's folder)
  --serve            with --events: no link; read {\"job\":N,\"add\":{...}} lines and run
                     every link at the same time on one Soulseek session

spotseek-backend --cli --check-login --user NAME   (password on stdin)
spotseek-backend --latest | --update               (the newest release; install it)
";

#[derive(Default, Debug, PartialEq)]
struct Args {
    url: String,
    out: String,
    format: convert::Format,
    bitrate: u32,
    events: bool,
    ask: bool,
    manual: bool,
    no_fallback: bool,
    skip_soulseek: bool,
    user: String,
    pass_stdin: bool,
    cover_out: String,
    limit: usize,
    parallel: usize,
    check_login: bool,
    serve: bool,
    help: bool,
    state_dir: String,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut a = Args { bitrate: 320, parallel: 12, ..Default::default() };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with('-') => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| inline.clone().or_else(|| it.next().cloned()).ok_or(format!("{name} needs a value"));
        match flag.trim_start_matches('-') {
            "cli" if flag.starts_with('-') => {}
            "o" | "out" => a.out = value("-o")?,
            "format" => a.format = convert::Format::parse(&value("--format")?).ok_or("--format must be mp3, m4a or flac")?,
            "bitrate" => a.bitrate = value("--bitrate")?.parse().map_err(|_| "bad --bitrate")?,
            "events" => a.events = true,
            "ask" => a.ask = true,
            "manual" => a.manual = true,
            "no-fallback" => a.no_fallback = true,
            "skip-soulseek" => a.skip_soulseek = true,
            "user" => a.user = value("--user")?,
            "pass-stdin" => a.pass_stdin = true,
            "cover-out" => a.cover_out = value("--cover-out")?,
            "limit" => a.limit = value("--limit")?.parse().map_err(|_| "bad --limit")?,
            "parallel" => a.parallel = value("--parallel")?.parse().map_err(|_| "bad --parallel")?,
            "check-login" => a.check_login = true,
            "state-dir" => a.state_dir = value("--state-dir")?,
            "h" | "help" => a.help = true,
            // Old flags of the Go backend that mean nothing here.
            "serve" => a.serve = true,
            "timings" | "extract-only" => {}
            _ if flag.starts_with('-') => return Err(format!("unknown flag {flag}")),
            _ => a.url = arg.clone(),
        }
    }
    if !matches!(a.bitrate, 128 | 192 | 256 | 320) {
        return Err("--bitrate must be 320, 256, 192 or 128".into());
    }
    Ok(a)
}

/// One stdin line from the app window.
fn parse_control(line: &str) -> Option<Control> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("stop").is_some() {
        return Some(Control::Stop);
    }
    if let Some(p) = v.get("pick") {
        let i = p.get("i")?.as_u64()? as usize;
        if p.get("yt").and_then(|y| y.as_bool()) == Some(true) {
            return Some(Control::Pick { i, cand: None });
        }
        return Some(Control::Pick { i, cand: Some(p.get("id")?.as_u64()? as u32) });
    }
    if v.is_object() {
        return Some(Control::Only(v.get("only").and_then(|o| o.as_array()).map(|a| a.iter().filter_map(|x| x.as_u64().map(|n| n as usize)).collect())));
    }
    None
}

/// The app's config folder (the same as the window's): `SPOTSEEK_CONFIG_DIR`, else the system's.
fn default_state_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SPOTSEEK_CONFIG_DIR") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)?;
    let base = if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"))
    };
    Some(base.join("Spotseek"))
}

/// The account in Sockseek's config file, when it has a user name and a password.
fn conf_account() -> Option<(String, String)> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let mut paths = vec![home.join(".config").join("sockseek").join("sockseek.conf")];
    if let Some(a) = std::env::var_os("APPDATA") {
        paths.push(PathBuf::from(a).join("sockseek").join("sockseek.conf"));
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        paths.push(PathBuf::from(x).join("sockseek").join("sockseek.conf"));
    }
    for p in paths {
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let (mut user, mut pass) = (String::new(), String::new());
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "username" | "user" => user = v.trim().to_string(),
                    "password" | "pass" => pass = v.trim().to_string(),
                    _ => {}
                }
            }
        }
        if !user.is_empty() && !pass.is_empty() {
            return Some((user, pass));
        }
    }
    None
}

/// Adds the usual install folders and the app's own tool folder to PATH. An app started from
/// Finder or Explorer does not get the shell's PATH. The app's tools come last, so tools the
/// user installed keep priority.
fn ensure_path() {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    let mut front: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            front.push(PathBuf::from(l).join("Microsoft").join("WinGet").join("Links"));
        }
        front.push(home.join("scoop").join("shims"));
    } else if cfg!(target_os = "macos") {
        front.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    } else {
        front.push(PathBuf::from("/usr/local/bin"));
    }
    front.push(home.join(".local").join("bin"));
    let mut paths: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for d in front.into_iter().rev() {
        if !paths.contains(&d) {
            paths.insert(0, d);
        }
    }
    for d in bundled_tool_dirs() {
        if !paths.contains(&d) {
            paths.push(d);
        }
    }
    if let Ok(p) = std::env::join_paths(paths) {
        // SAFETY: called at the start of main, before any other thread runs.
        unsafe { std::env::set_var("PATH", p) };
    }
}

/// `bin` next to the program, and `Contents/Resources/bin` in a macOS app.
fn bundled_tool_dirs() -> Vec<PathBuf> {
    let Ok(exe) = std::env::current_exe() else { return Vec::new() };
    let Some(dir) = exe.parent() else { return Vec::new() };
    let mut c = vec![dir.join("bin")];
    if cfg!(target_os = "macos") {
        c.push(dir.join("..").join("Resources").join("bin"));
    }
    c.into_iter().filter(|d| d.is_dir()).collect()
}

/// macOS kills unsigned downloaded tools that still carry the quarantine flag. The user
/// already allowed the app, so the flag on its own tools is not needed.
fn clear_quarantine() {
    if cfg!(target_os = "macos") {
        for d in bundled_tool_dirs() {
            let _ = std::process::Command::new("xattr").arg("-dr").arg("com.apple.quarantine").arg(d).status();
        }
    }
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("--version") {
        println!("spotseek-backend {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Some(cmd @ ("--latest" | "--update")) = raw.first().map(String::as_str) {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
        let code = rt.block_on(async { if cmd == "--latest" { update::latest().await } else { update::update().await } });
        std::process::exit(code);
    }
    let args = match parse_args(&raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{HELP}");
            std::process::exit(2);
        }
    };
    if args.help || raw.is_empty() {
        print!("{HELP}");
        return;
    }
    ensure_path();
    clear_quarantine();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
    if args.check_login {
        let mut pass = String::new();
        let _ = std::io::stdin().lock().read_line(&mut pass);
        let pass = pass.trim_end_matches(['\r', '\n']).to_string();
        let out = rt.block_on(slsk::check_login(slsk::SERVER, &args.user, &pass));
        let v = match out {
            Ok(a) if a.ok => serde_json::json!({ "ok": true }),
            Ok(a) => serde_json::json!({ "ok": false, "reason": a.reason, "message": a.detail }),
            Err(e) => serde_json::json!({ "ok": false, "reason": "ERROR", "message": e.to_string() }),
        };
        println!("{v}");
        return;
    }
    if args.serve {
        std::process::exit(rt.block_on(serve(args)));
    }
    if args.url.is_empty() || args.out.is_empty() {
        eprintln!("usage: spotseek-backend --cli <spotify-url> -o <dir>");
        std::process::exit(2);
    }
    std::process::exit(rt.block_on(run(args)));
}

async fn run(args: Args) -> i32 {
    let (ctl_tx, ctl_rx) = tokio::sync::mpsc::unbounded_channel();
    // Stdin: the password, then the controls. Its end stops everything.
    let (pass_tx, pass_rx) = std::sync::mpsc::channel::<String>();
    let pass_stdin = args.pass_stdin;
    let events = args.events;
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut lines = stdin.lock().lines();
        if pass_stdin {
            let p = lines.next().and_then(Result::ok).unwrap_or_default();
            let _ = pass_tx.send(p.trim_end_matches('\r').to_string());
        }
        drop(pass_tx);
        for line in lines.map_while(Result::ok) {
            if let Some(c) = parse_control(&line) {
                let _ = ctl_tx.send(c);
            }
        }
        if events {
            // The app window is gone: stop at once.
            std::process::exit(1);
        }
    });
    let pass = if pass_stdin { pass_rx.recv().unwrap_or_default() } else { String::new() };
    let account = account_of(&args, pass);
    let emit = if args.events {
        Emitter::stdout()
    } else {
        Emitter::new(|e: &Event| match e.ev.as_str() {
            "log" => println!("{}", e.line),
            "track" => println!("  [{}] {}{}", e.i.unwrap_or(0) + 1, e.state, if e.peer.is_empty() { String::new() } else { format!(" ({})", e.peer) }),
            "file" => println!("  -> {}", e.path),
            _ => {}
        })
    };
    let o = Options {
        input: args.url,
        out_dir: PathBuf::from(args.out),
        format: args.format,
        bitrate: args.bitrate,
        fallback: !args.no_fallback,
        account,
        server: std::env::var("SPOTSEEK_SLSK_SERVER").unwrap_or_else(|_| slsk::SERVER.to_string()),
        manual: args.manual && args.events,
        ask: args.ask && args.events,
        cover_out: (!args.cover_out.is_empty()).then(|| PathBuf::from(args.cover_out)),
        limit: args.limit,
        ffmpeg: "ffmpeg".into(),
        ytdlp: "yt-dlp".into(),
        parallel: args.parallel.clamp(1, 16),
        state_dir: if args.state_dir.is_empty() { default_state_dir() } else { Some(PathBuf::from(&args.state_dir)) },
    };
    match pipeline::run(o, emit.clone(), ctl_rx).await {
        Ok(()) => 0,
        Err(e) => {
            if args.events {
                emit.emit(Event { message: e.to_string(), ..Event::new("error") });
            } else {
                eprintln!("error: {e}");
            }
            1
        }
    }
}

/// The account to sign in with: `--user` and the password from stdin, else Sockseek's file.
fn account_of(args: &Args, pass: String) -> Option<(String, String)> {
    if args.skip_soulseek {
        None
    } else if !args.user.is_empty() && !pass.is_empty() {
        Some((args.user.clone(), pass))
    } else {
        conf_account().filter(|(u, _)| args.user.is_empty() || *u == args.user)
    }
}

/// The options of one `--serve` job: the `add` message over the command line's.
fn job_options(args: &Args, account: Option<(String, String)>, add: &serde_json::Value) -> Result<Options, String> {
    let s = |k: &str| add.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let b = |k: &str, d: bool| add.get(k).and_then(|v| v.as_bool()).unwrap_or(d);
    let url = s("url");
    let out = s("out");
    if url.is_empty() || out.is_empty() {
        return Err("add needs a url and an out folder".into());
    }
    let format = match add.get("format").and_then(|v| v.as_str()) {
        Some(f) => convert::Format::parse(f).ok_or("format must be mp3, m4a or flac")?,
        None => args.format,
    };
    let bitrate = match add.get("bitrate") {
        Some(v) => v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).ok_or("bad bitrate")? as u32,
        None => args.bitrate,
    };
    if !matches!(bitrate, 128 | 192 | 256 | 320) {
        return Err("bitrate must be 320, 256, 192 or 128".into());
    }
    let cover = s("cover_out");
    Ok(Options {
        input: url,
        out_dir: PathBuf::from(out),
        format,
        bitrate,
        fallback: b("fallback", !args.no_fallback),
        account,
        server: std::env::var("SPOTSEEK_SLSK_SERVER").unwrap_or_else(|_| slsk::SERVER.to_string()),
        manual: b("manual", args.manual),
        ask: b("ask", args.ask),
        cover_out: (!cover.is_empty()).then(|| PathBuf::from(cover)),
        limit: add.get("limit").and_then(|v| v.as_u64()).unwrap_or(args.limit as u64) as usize,
        ffmpeg: "ffmpeg".into(),
        ytdlp: "yt-dlp".into(),
        parallel: args.parallel.clamp(1, 16),
        state_dir: if args.state_dir.is_empty() { default_state_dir() } else { Some(PathBuf::from(&args.state_dir)) },
    })
}

/// `--serve`: one Soulseek session for every link the app window adds, all at the same time.
async fn serve(args: Args) -> i32 {
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (pass_tx, pass_rx) = std::sync::mpsc::channel::<String>();
    let pass_stdin = args.pass_stdin;
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut lines = stdin.lock().lines();
        if pass_stdin {
            let p = lines.next().and_then(Result::ok).unwrap_or_default();
            let _ = pass_tx.send(p.trim_end_matches('\r').to_string());
        }
        drop(pass_tx);
        for line in lines.map_while(Result::ok) {
            let _ = line_tx.send(line);
        }
        // The app window is gone: stop at once.
        std::process::exit(1);
    });
    let pass = if pass_stdin { pass_rx.recv().unwrap_or_default() } else { String::new() };
    let account = account_of(&args, pass);
    let emit = Emitter::stdout();
    let state_dir = if args.state_dir.is_empty() { default_state_dir() } else { Some(PathBuf::from(&args.state_dir)) };
    let server = std::env::var("SPOTSEEK_SLSK_SERVER").unwrap_or_else(|_| slsk::SERVER.to_string());
    let sh = pipeline::Shared::new(account.clone(), server, args.parallel.clamp(1, 16), state_dir, emit.clone());
    let mut jobs: std::collections::HashMap<u64, (tokio::sync::mpsc::UnboundedSender<pipeline::Control>, tokio::task::AbortHandle)> = Default::default();
    while let Some(line) = line_rx.recv().await {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
        let Some(job) = v.get("job").and_then(|j| j.as_u64()).filter(|j| *j > 0) else { continue };
        let jemit = emit.with_job(job);
        if let Some(add) = v.get("add") {
            let o = match job_options(&args, account.clone(), add) {
                Ok(o) => o,
                Err(e) => {
                    jemit.emit(Event { message: e, ..Event::new("error") });
                    continue;
                }
            };
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            let sh = sh.clone();
            let task = tokio::spawn(async move {
                if let Err(e) = pipeline::run_shared(o, jemit.clone(), rx, sh).await {
                    jemit.emit(Event { message: e.to_string(), ..Event::new("error") });
                }
            });
            jobs.insert(job, (tx, task.abort_handle()));
        } else if v.get("cancel").is_some() {
            if let Some((_, h)) = jobs.remove(&job) {
                h.abort();
            }
        } else if let Some(c) = parse_control(&line) {
            if let Some((tx, _)) = jobs.get(&job) {
                let _ = tx.send(c);
            }
        }
        jobs.retain(|_, (_, h)| !h.is_finished());
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn reads_the_app_windows_command_line() {
        let a = parse_args(&s(&["--cli", "https://open.spotify.com/track/x", "-o", "/out", "--bitrate", "256", "--events", "--user", "me", "--pass-stdin", "--ask", "--cover-out", "/c.jpg", "--manual"])).unwrap();
        assert_eq!(a.url, "https://open.spotify.com/track/x");
        assert_eq!((a.out.as_str(), a.bitrate, a.user.as_str(), a.cover_out.as_str()), ("/out", 256, "me", "/c.jpg"));
        assert!(a.events && a.pass_stdin && a.ask && a.manual && !a.no_fallback);
        assert!(parse_args(&s(&["--cli", "--check-login", "--user", "x"])).unwrap().check_login);
        assert!(parse_args(&s(&["--bitrate", "999"])).is_err());
        assert_eq!(parse_args(&s(&["--format", "flac"])).unwrap().format, convert::Format::Flac);
        assert_eq!(parse_args(&s(&[])).unwrap().format, convert::Format::Mp3);
        assert!(parse_args(&s(&["--format", "ogg"])).is_err());
        assert!(parse_args(&s(&["--serve", "--events"])).unwrap().serve);
        let a = parse_args(&s(&["--serve", "--events", "--format", "m4a"])).unwrap();
        let add = serde_json::json!({"url": "https://soundcloud.com/a/b", "out": "/o", "format": "flac", "bitrate": "256", "manual": true});
        let o = job_options(&a, None, &add).unwrap();
        assert_eq!((o.format, o.bitrate, o.manual, o.fallback), (convert::Format::Flac, 256, true, true));
        let o = job_options(&a, None, &serde_json::json!({"url": "u", "out": "/o"})).unwrap();
        assert_eq!(o.format, convert::Format::M4a, "the command line's format when the job names none");
        assert!(job_options(&a, None, &serde_json::json!({"url": "u"})).is_err());
        assert!(parse_args(&s(&["--what"])).is_err());
    }

    #[test]
    fn reads_controls() {
        assert_eq!(parse_control(r#"{"only":[0,2]}"#), Some(Control::Only(Some(vec![0, 2]))));
        assert_eq!(parse_control("{}"), Some(Control::Only(None)));
        assert_eq!(parse_control(r#"{"stop":true}"#), Some(Control::Stop));
        assert_eq!(parse_control(r#"{"pick":{"i":3,"id":7}}"#), Some(Control::Pick { i: 3, cand: Some(7) }));
        assert_eq!(parse_control(r#"{"pick":{"i":3,"yt":true}}"#), Some(Control::Pick { i: 3, cand: None }));
        assert_eq!(parse_control("junk"), None);
    }
}
