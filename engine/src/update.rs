//! Updates: the newest Spotseek release on GitHub, and putting it in place of this copy.
//!
//! `spotseek-backend --latest` prints one line:
//! `{"ev":"update","state":"latest","newer":true,"version":"0.2.4","page":"…","kind":"app","asset":"…"}`.
//! `kind` says how this copy updates itself: "app" (macOS: the .app is replaced), "setup" (Windows:
//! the installer runs), "appimage" (Linux: the AppImage file is replaced), "tar" (Linux: the
//! install.sh of the new folder runs), or "manual" (a copy we cannot replace: the page opens).
//!
//! `spotseek-backend --update` downloads the file for this copy, checks it against the release's
//! SHA256SUMS.txt, installs it and prints `{"state":"downloading","bytes":…,"total":…}` lines, then
//! `{"state":"ready","run":"…","args":[…]}`: the app window starts that program and quits (it
//! starts the new version, or the Windows installer). On a problem: `{"state":"failed","message":…}`.
//!
//! `SPOTSEEK_UPDATE_URL` replaces the GitHub address (a URL, or a path to a JSON file in the same
//! form, for tests); asset addresses may be local paths too.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::json;

pub const REPO: &str = "lopezlav/spotseek";

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize, Clone)]
struct Asset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// How this copy was installed, and so how it is replaced.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// macOS: the .app bundle at this path.
    App(PathBuf),
    /// Windows: installed by the Inno Setup installer in this folder.
    Setup(PathBuf),
    /// Linux: the AppImage file at this path.
    AppImage(PathBuf),
    /// Linux: the folder install.sh made (~/.local/share/spotseek).
    Tar(PathBuf),
    Manual,
}

impl Kind {
    fn name(&self) -> &'static str {
        match self {
            Kind::App(_) => "app",
            Kind::Setup(_) => "setup",
            Kind::AppImage(_) => "appimage",
            Kind::Tar(_) => "tar",
            Kind::Manual => "manual",
        }
    }

    /// The release file this copy needs.
    pub fn asset(&self) -> Option<String> {
        let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
        match self {
            Kind::App(_) => Some(format!("Spotseek-macOS-{arch}.zip")),
            Kind::Setup(_) => Some(format!("Spotseek-Setup-windows-{arch}.exe")),
            Kind::AppImage(_) => Some(format!("Spotseek-linux-{arch}.AppImage")),
            Kind::Tar(_) => Some(format!("Spotseek-linux-{arch}.tar.gz")),
            Kind::Manual => None,
        }
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".spotseek-write-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// How the copy with the program at `exe` was installed (`appimage`: the `APPIMAGE` variable).
pub fn kind_of(exe: &Path, appimage: Option<PathBuf>) -> Kind {
    let dir = exe.parent().unwrap_or(Path::new("."));
    if cfg!(target_os = "macos") {
        // Spotseek.app/Contents/MacOS/spotseek-backend
        let app = dir.parent().and_then(Path::parent);
        if let Some(app) = app.filter(|a| dir.ends_with("Contents/MacOS") && a.extension().is_some_and(|e| e == "app")) {
            if app.parent().is_some_and(writable) {
                return Kind::App(app.to_path_buf());
            }
        }
        return Kind::Manual;
    }
    if cfg!(windows) {
        return if dir.join("unins000.exe").is_file() { Kind::Setup(dir.to_path_buf()) } else { Kind::Manual };
    }
    if let Some(a) = appimage.filter(|a| a.is_file() && a.parent().is_some_and(writable)) {
        return Kind::AppImage(a);
    }
    if dir.join("install.sh").is_file() && dir.ends_with("spotseek") && writable(dir) {
        return Kind::Tar(dir.to_path_buf());
    }
    Kind::Manual
}

fn this_kind() -> Kind {
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok()).unwrap_or_default();
    kind_of(&exe, std::env::var_os("APPIMAGE").map(PathBuf::from))
}

/// "v0.2.10" → (0, 2, 10).
pub fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("spotseek/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .build()?)
}

fn local(url: &str) -> Option<PathBuf> {
    if let Some(p) = url.strip_prefix("file://") {
        return Some(PathBuf::from(p));
    }
    (!url.starts_with("http://") && !url.starts_with("https://")).then(|| PathBuf::from(url))
}

async fn get_text(url: &str) -> Result<String> {
    if let Some(p) = local(url) {
        return Ok(tokio::fs::read_to_string(&p).await.with_context(|| p.display().to_string())?);
    }
    let r = client()?.get(url).header("Accept", "application/vnd.github+json").timeout(Duration::from_secs(30)).send().await?;
    if !r.status().is_success() {
        bail!("{url}: HTTP {}", r.status());
    }
    Ok(r.text().await?)
}

async fn latest_release() -> Result<Release> {
    let url = std::env::var("SPOTSEEK_UPDATE_URL").unwrap_or_else(|_| format!("https://api.github.com/repos/{REPO}/releases/latest"));
    Ok(serde_json::from_str(&get_text(&url).await?)?)
}

fn say(v: serde_json::Value) {
    use std::io::Write;
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

/// `--latest`.
pub async fn latest() -> i32 {
    match latest_release().await {
        Ok(r) => {
            let kind = this_kind();
            let asset = kind.asset().filter(|a| r.assets.iter().any(|x| &x.name == a));
            let kind_name = if asset.is_some() { kind.name() } else { "manual" };
            let newer = parse_version(&r.tag_name) > parse_version(env!("CARGO_PKG_VERSION"));
            say(json!({
                "ev": "update", "state": "latest", "newer": newer,
                "version": r.tag_name.trim_start_matches('v'),
                "current": env!("CARGO_PKG_VERSION"),
                "page": r.html_url, "kind": kind_name, "asset": asset.unwrap_or_default(),
            }));
            0
        }
        Err(e) => {
            say(json!({ "ev": "update", "state": "failed", "message": e.to_string() }));
            1
        }
    }
}

/// `--update`.
pub async fn update() -> i32 {
    match install().await {
        Ok((run, args)) => {
            say(json!({ "ev": "update", "state": "ready", "run": run, "args": args }));
            0
        }
        Err(e) => {
            say(json!({ "ev": "update", "state": "failed", "message": format!("{e:#}") }));
            1
        }
    }
}

/// Downloads `a` into `dest`, with progress lines, and returns its sha256 (hex).
async fn download(a: &Asset, dest: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;
    let mut hash = Sha256::new();
    let mut file = tokio::fs::File::create(dest).await.with_context(|| dest.display().to_string())?;
    if let Some(p) = local(&a.browser_download_url) {
        let bytes = tokio::fs::read(&p).await.with_context(|| p.display().to_string())?;
        hash.update(&bytes);
        file.write_all(&bytes).await?;
    } else {
        let mut r = client()?.get(&a.browser_download_url).timeout(Duration::from_secs(1800)).send().await?;
        if !r.status().is_success() {
            bail!("{}: HTTP {}", a.name, r.status());
        }
        let total = r.content_length().unwrap_or(a.size);
        let (mut got, mut said) = (0u64, std::time::Instant::now());
        while let Some(chunk) = r.chunk().await? {
            hash.update(&chunk);
            file.write_all(&chunk).await?;
            got += chunk.len() as u64;
            if said.elapsed() > Duration::from_millis(250) {
                said = std::time::Instant::now();
                say(json!({ "ev": "update", "state": "downloading", "bytes": got, "total": total }));
            }
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    Ok(format!("{:x}", hash.finalize()))
}

/// The sha256 of `name` in a SHA256SUMS.txt (`<hex>  <name>` lines).
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut w = l.split_whitespace();
        let (h, n) = (w.next()?, w.next()?);
        (n.trim_start_matches('*') == name).then(|| h.to_lowercase())
    })
}

fn run_status(cmd: &mut std::process::Command, what: &str) -> Result<()> {
    let out = cmd.output().with_context(|| what.to_string())?;
    if !out.status.success() {
        bail!("{what}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

async fn install() -> Result<(String, Vec<String>)> {
    let kind = this_kind();
    let want = kind.asset().ok_or_else(|| anyhow!("this copy cannot update itself; download the new version from the page"))?;
    let r = latest_release().await?;
    let asset = r.assets.iter().find(|a| a.name == want).cloned().ok_or_else(|| anyhow!("the release has no {want}"))?;
    let sums = r.assets.iter().find(|a| a.name == "SHA256SUMS.txt").ok_or_else(|| anyhow!("the release has no SHA256SUMS.txt"))?;
    let expected = sum_for(&get_text(&sums.browser_download_url).await?, &want).ok_or_else(|| anyhow!("SHA256SUMS.txt has no line for {want}"))?;

    // Next to what it replaces, so the last step is a rename on the same disk.
    let work_parent = match &kind {
        Kind::App(app) => app.parent().map(Path::to_path_buf),
        Kind::AppImage(f) => f.parent().map(Path::to_path_buf),
        _ => None,
    }
    .unwrap_or_else(std::env::temp_dir);
    let work = work_parent.join(format!(".spotseek-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = async {
        let file = work.join(&want);
        let got = download(&asset, &file).await?;
        if got != expected {
            bail!("the download of {want} is damaged (its checksum does not match)");
        }
        say(json!({ "ev": "update", "state": "installing" }));
        put_in_place(&kind, &file, &work)
    }
    .await;
    // The Windows installer still has to run from there.
    if !matches!(kind, Kind::Setup(_)) || result.is_err() {
        let _ = std::fs::remove_dir_all(&work);
    }
    result
}

fn put_in_place(kind: &Kind, file: &Path, work: &Path) -> Result<(String, Vec<String>)> {
    match kind {
        Kind::App(app) => {
            let unpacked = work.join("new");
            run_status(std::process::Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(file).arg(&unpacked), "unzip")?;
            let new_app = std::fs::read_dir(&unpacked)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().is_some_and(|e| e == "app"))
                .ok_or_else(|| anyhow!("the zip has no app"))?;
            let old = work.join("old.app");
            std::fs::rename(app, &old).context("move the old app aside")?;
            if let Err(e) = std::fs::rename(&new_app, app) {
                let _ = std::fs::rename(&old, app);
                return Err(anyhow!(e).context("put the new app in place"));
            }
            let _ = std::process::Command::new("/usr/bin/xattr").args(["-dr", "com.apple.quarantine"]).arg(app).output();
            Ok(("/usr/bin/open".into(), vec!["-n".into(), app.display().to_string()]))
        }
        Kind::AppImage(dest) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))?;
            }
            std::fs::rename(file, dest).context("replace the AppImage")?;
            Ok((dest.display().to_string(), vec![]))
        }
        Kind::Tar(dir) => {
            run_status(std::process::Command::new("tar").arg("-xzf").arg(file).arg("-C").arg(work), "unpack")?;
            let script = work.join("Spotseek").join("install.sh");
            run_status(std::process::Command::new("sh").arg(&script), "install.sh")?;
            Ok((dir.join("spotseek").display().to_string(), vec![]))
        }
        Kind::Setup(dir) => {
            // An install in Program Files needs the installer for all users (it asks for admin).
            let all = std::env::var_os("ProgramFiles").is_some_and(|pf| dir.starts_with(PathBuf::from(pf)));
            let args = ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/RELAUNCH=1", if all { "/ALLUSERS" } else { "/CURRENTUSER" }];
            Ok((file.display().to_string(), args.iter().map(|s| s.to_string()).collect()))
        }
        Kind::Manual => Err(anyhow!("this copy cannot update itself")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_sums() {
        assert_eq!(parse_version("v0.2.10"), Some((0, 2, 10)));
        assert!(parse_version("0.2.10") > parse_version("0.2.9"));
        assert_eq!(parse_version("x"), None);
        let sums = "abc123  Spotseek-macOS-arm64.zip\nDEF456 *Spotseek-Setup-windows-x64.exe\n";
        assert_eq!(sum_for(sums, "Spotseek-macOS-arm64.zip").as_deref(), Some("abc123"));
        assert_eq!(sum_for(sums, "Spotseek-Setup-windows-x64.exe").as_deref(), Some("def456"));
        assert_eq!(sum_for(sums, "Spotseek-linux-x64.tar.gz"), None);
    }

    #[test]
    fn a_dev_build_updates_by_hand() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(kind_of(&exe, None), Kind::Manual, "target/release is no install");
        assert_eq!(Kind::Manual.asset(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_app_is_replaced() {
        let dir = std::env::temp_dir().join(format!("spotseek-kind-{}", std::process::id()));
        let macos = dir.join("Spotseek.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let k = kind_of(&macos.join("spotseek-backend"), None);
        assert_eq!(k, Kind::App(dir.join("Spotseek.app")));
        assert!(k.asset().unwrap().starts_with("Spotseek-macOS-"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
