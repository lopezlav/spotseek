//! The Soulseek account. The name is in settings.json; the password is in the system's secure
//! store (the macOS Keychain, the Windows Credential Manager), never in a file of ours. An
//! account in sockseek.conf (the config file of the Sockseek tool) also counts; the engine reads it.
//!
//! Soulseek has no sign-up page: signing in with a name that is not taken creates the account.
//! `check` asks the backend to sign in once (`spotseek-backend --cli --check-login`).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::settings::home;

const SERVICE: &str = "Spotseek Soulseek";

/// Where the account for a run comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Account {
    /// Signed in from this app: the name, and the password from the secure store.
    App(String, String),
    /// sockseek.conf has the name (and the password); the engine reads it itself.
    Conf(String),
    None,
}

impl Account {
    pub fn name(&self) -> Option<&str> {
        match self {
            Account::App(n, _) | Account::Conf(n) => Some(n),
            Account::None => None,
        }
    }
}

/// The account to use with the user name from the settings.
pub fn current(user: &str) -> Account {
    if !user.is_empty() {
        if let Some(pass) = password(user) {
            return Account::App(user.to_string(), pass);
        }
    }
    match conf_user() {
        Some(n) => Account::Conf(n),
        None => Account::None,
    }
}

fn entry(user: &str) -> Option<keyring::Entry> {
    keyring::Entry::new(SERVICE, user).ok()
}

pub fn password(user: &str) -> Option<String> {
    entry(user)?.get_password().ok().filter(|p| !p.is_empty())
}

pub fn save(user: &str, pass: &str) -> Result<(), String> {
    entry(user).ok_or("no secure store")?.set_password(pass).map_err(|e| e.to_string())
}

pub fn forget(user: &str) {
    if let Some(e) = entry(user) {
        let _ = e.delete_credential();
    }
}

/// The places of sockseek.conf (the engine reads the same ones).
fn conf_paths() -> Vec<PathBuf> {
    let mut v = vec![home().join(".config").join("sockseek").join("sockseek.conf")];
    if let Some(a) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(a).join("sockseek").join("sockseek.conf"));
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        v.push(PathBuf::from(x).join("sockseek").join("sockseek.conf"));
    }
    v
}

/// The user name in sockseek.conf, when it has both a user name and a password.
pub fn conf_user() -> Option<String> {
    for p in conf_paths() {
        let Ok(f) = std::fs::File::open(&p) else { continue };
        let (mut user, mut pass) = (None, false);
        for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let (k, v) = (k.trim(), v.trim());
                match k {
                    "username" | "user" if !v.is_empty() => user = Some(v.to_string()),
                    "password" | "pass" if !v.is_empty() => pass = true,
                    _ => {}
                }
            }
        }
        if let (Some(u), true) = (user, pass) {
            return Some(u);
        }
    }
    None
}

/// The server's answer to a sign-in.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    Ok,
    /// The name is taken, or the password is wrong.
    WrongPassword,
    BadName,
    ServerFull,
    Error(String),
}

/// Signs in once with the backend. Blocks; run it on a thread.
pub fn check(user: &str, pass: &str) -> Check {
    let bin = crate::backend::binary();
    let mut cmd = Command::new(&bin);
    cmd.arg("--cli").arg("--check-login").arg("--user").arg(user);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Check::Error(format!("cannot start {}: {e}", bin.display())),
    };
    if let Some(mut s) = child.stdin.take() {
        let _ = writeln!(s, "{pass}");
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return Check::Error(e.to_string()),
    };
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Answer {
        ok: bool,
        reason: String,
        message: String,
    }
    let a: Answer = match serde_json::from_slice(&out.stdout) {
        Ok(a) => a,
        Err(_) => return Check::Error(String::from_utf8_lossy(&out.stdout).trim().to_string()),
    };
    match (a.ok, a.reason.as_str()) {
        (true, _) => Check::Ok,
        (false, "INVALIDPASS") => Check::WrongPassword,
        (false, "INVALIDUSERNAME") => Check::BadName,
        (false, "SVRFULL") => Check::ServerFull,
        (false, r) if r == "ERROR" && a.message.starts_with("the name") => Check::BadName,
        (false, r) => Check::Error(if a.message.is_empty() { r.to_string() } else { a.message }),
    }
}

#[cfg(test)]
mod tests {
    /// Writes, reads and deletes a password in the real secure store.
    /// `cargo test --release keychain -- --ignored`
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let user = "spotseek-test-user";
        super::save(user, "s3cret").expect("save");
        assert_eq!(super::password(user).as_deref(), Some("s3cret"));
        super::forget(user);
        assert_eq!(super::password(user), None);
    }
}
