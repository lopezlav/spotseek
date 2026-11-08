//! New versions: at launch the engine asks GitHub for the newest release
//! (`spotseek-backend --latest`). When it is newer, a banner offers it. UPDATE runs
//! `spotseek-backend --update`, which downloads and checks the file and puts it in place; then this
//! app starts what it says (the new version, or the Windows installer) and quits.

use std::io::BufRead;
use std::process::{Command, Stdio};

use serde::Deserialize;

/// One line from `--latest` or `--update`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Line {
    pub state: String,
    pub newer: bool,
    pub version: String,
    pub page: String,
    /// "app", "setup", "appimage", "tar" or "manual" (see `engine/src/update.rs`).
    pub kind: String,
    pub bytes: u64,
    pub total: u64,
    pub run: String,
    pub args: Vec<String>,
    pub message: String,
}

/// The update banner.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Up {
    #[default]
    None,
    /// A newer version: `kind` "manual" opens its page instead.
    Ready { version: String, page: String, kind: String },
    Downloading { version: String, page: String, frac: f32 },
    Installing { version: String },
    Failed { version: String, page: String, message: String },
}

impl Up {
    pub fn version(&self) -> &str {
        match self {
            Up::None => "",
            Up::Ready { version, .. } | Up::Downloading { version, .. } | Up::Installing { version } | Up::Failed { version, .. } => version,
        }
    }

    pub fn page(&self) -> String {
        match self {
            Up::Ready { page, .. } | Up::Downloading { page, .. } | Up::Failed { page, .. } => page.clone(),
            _ => format!("https://github.com/{}/releases/latest", REPO),
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self, Up::Downloading { .. } | Up::Installing { .. })
    }
}

pub const REPO: &str = "lopezlav/spotseek";

fn backend() -> Command {
    let mut c = Command::new(crate::backend::binary());
    c.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Asks for the newest release (blocks; run it on a thread).
pub fn check() -> Option<Line> {
    let out = backend().arg("--latest").output().ok()?;
    let line = String::from_utf8_lossy(&out.stdout).lines().find_map(|l| serde_json::from_str::<Line>(l).ok())?;
    (line.state == "latest").then_some(line)
}

/// Downloads and installs the newest release; `on_line` gets each line (blocks).
pub fn install(mut on_line: impl FnMut(Line)) {
    let child = backend().arg("--update").stdout(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return on_line(Line { state: "failed".into(), message: e.to_string(), ..Default::default() }),
    };
    let mut last = String::new();
    if let Some(out) = child.stdout.take() {
        for l in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if let Ok(line) = serde_json::from_str::<Line>(&l) {
                last = line.state.clone();
                on_line(line);
            }
        }
    }
    let _ = child.wait();
    if last != "ready" && last != "failed" {
        on_line(Line { state: "failed".into(), message: "the updater stopped".into(), ..Default::default() });
    }
}

/// Starts `run args` on its own, a moment after this app has quit (macOS and Linux: so the old
/// and the new copy never run together). The Windows installer closes this app itself.
pub fn launch(run: &str, args: &[String]) -> std::io::Result<()> {
    let mut c = if cfg!(windows) {
        let mut c = Command::new(run);
        c.args(args);
        c
    } else {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", "sleep 1; exec \"$0\" \"$@\"", run]).args(args);
        c
    };
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    c.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_engine_lines() {
        let l: Line = serde_json::from_str(r#"{"asset":"Spotseek-macOS-arm64.zip","current":"0.2.3","ev":"update","kind":"app","newer":true,"page":"https://x/v0.2.4","state":"latest","version":"0.2.4"}"#).unwrap();
        assert!(l.newer);
        assert_eq!((l.kind.as_str(), l.version.as_str()), ("app", "0.2.4"));
        let l: Line = serde_json::from_str(r#"{"ev":"update","state":"ready","run":"/usr/bin/open","args":["-n","/Applications/Spotseek.app"]}"#).unwrap();
        assert_eq!(l.args.len(), 2);
    }
}
