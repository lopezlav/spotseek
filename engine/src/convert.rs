//! Turns a downloaded file into the mp3 in the output folder. The sound never goes up: an mp3
//! at or below the wanted bitrate is copied as it is; a better mp3 or a lossless file is encoded
//! down to the wanted bitrate; another lossy format is encoded at no more than its own bitrate.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::process::Command;

/// mp3 bitrates, kbps.
const STEPS: [u32; 8] = [96, 112, 128, 160, 192, 224, 256, 320];

/// What the converter does with a file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plan {
    /// Copy the mp3 as it is.
    Copy,
    /// Encode to mp3 at this many kbps.
    Encode(u32),
}

/// The audio facts ffprobe gives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probe {
    pub codec: String,
    /// kbps of the audio stream (0: not known).
    pub kbps: u32,
    pub lossless: bool,
}

/// The smallest mp3 step at or above `kbps`.
pub fn step_up(kbps: u32) -> u32 {
    STEPS.iter().copied().find(|s| *s + 4 >= kbps).unwrap_or(320)
}

pub fn plan(p: &Probe, target: u32) -> Plan {
    if p.lossless {
        return Plan::Encode(target);
    }
    if p.codec == "mp3" {
        // A few kbps over (VBR, rounding) is still the same quality.
        return if p.kbps == 0 || p.kbps <= target + 8 { Plan::Copy } else { Plan::Encode(target) };
    }
    // Another lossy format: no higher than what it has.
    if p.kbps == 0 { Plan::Encode(target.min(192)) } else { Plan::Encode(target.min(step_up(p.kbps))) }
}

fn lossless_codec(c: &str) -> bool {
    c == "flac" || c == "alac" || c == "ape" || c == "wavpack" || c == "tta" || c == "mlp" || c.starts_with("pcm_")
}

/// Reads the codec and bitrate of the first audio stream.
pub async fn probe(ffprobe: &str, path: &Path) -> Result<Probe> {
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        command(ffprobe)
            .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=codec_name,bit_rate:format=bit_rate", "-of", "json"])
            .arg(path)
            .output(),
    )
    .await??;
    if !out.status.success() {
        return Err(anyhow!("ffprobe: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let s = &v["streams"][0];
    let codec = s["codec_name"].as_str().unwrap_or("").to_string();
    if codec.is_empty() {
        return Err(anyhow!("no audio stream"));
    }
    let br = |x: &serde_json::Value| x.as_str().and_then(|b| b.parse::<u64>().ok()).unwrap_or(0);
    let mut bps = br(&s["bit_rate"]);
    if bps == 0 {
        bps = br(&v["format"]["bit_rate"]);
    }
    Ok(Probe { lossless: lossless_codec(&codec), codec, kbps: (bps / 1000) as u32 })
}

pub fn command(program: &str) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Characters no file system takes, as spaces; runs of spaces as one.
pub fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c }).collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ").trim_end_matches('.').to_string()
}

/// `dir/base.mp3`, or `dir/base (1).mp3`, … when it is taken. Creates an empty file there, so
/// two conversions at once never pick the same name.
pub fn unique_dest(dir: &Path, base: &str) -> PathBuf {
    let mut i = 0;
    loop {
        let name = if i == 0 { format!("{base}.mp3") } else { format!("{base} ({i}).mp3") };
        let p = dir.join(name);
        if std::fs::OpenOptions::new().write(true).create_new(true).open(&p).is_ok() {
            return p;
        }
        i += 1;
    }
}

/// Tags written into the mp3.
#[derive(Clone, Debug, Default)]
pub struct Tags {
    pub artist: String,
    pub title: String,
    pub album: String,
}

pub struct Tools {
    pub ffmpeg: String,
    pub ffprobe: String,
}

/// Converts `src` into a new mp3 in `dir` named `base`, at most `target` kbps. Returns the mp3
/// and the bitrate it has.
pub async fn to_mp3(tools: &Tools, src: &Path, dir: &Path, base: &str, target: u32, tags: &Tags) -> Result<(PathBuf, u32)> {
    tokio::fs::create_dir_all(dir).await?;
    let p = probe(&tools.ffprobe, src).await?;
    let dest = unique_dest(dir, base);
    let result = async {
        match plan(&p, target) {
            Plan::Copy => {
                tokio::fs::copy(src, &dest).await?;
                let d = dest.clone();
                let t = tags.clone();
                let _ = tokio::task::spawn_blocking(move || fill_tags(&d, &t)).await;
                Ok(p.kbps)
            }
            Plan::Encode(kbps) => {
                // Keep the cover picture; a picture ffmpeg cannot put in an mp3 is left out.
                if encode(tools, src, &dest, kbps, tags, true).await.is_err() {
                    encode(tools, src, &dest, kbps, tags, false).await?;
                }
                Ok(kbps)
            }
        }
    }
    .await;
    match result {
        Ok(kbps) => Ok((dest, kbps)),
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            Err(e)
        }
    }
}

async fn encode(tools: &Tools, src: &Path, dest: &Path, kbps: u32, tags: &Tags, cover: bool) -> Result<()> {
    let mut c = command(&tools.ffmpeg);
    c.args(["-y", "-loglevel", "error", "-i"]).arg(src);
    if cover {
        c.args(["-map", "0:a:0", "-map", "0:v:0?", "-c:v", "copy", "-disposition:v:0", "attached_pic"]);
    } else {
        c.arg("-vn");
    }
    c.args(["-map_metadata", "0", "-id3v2_version", "3", "-c:a", "libmp3lame", "-b:a", &format!("{kbps}k")]);
    // Spotify's names, so the file is found again by its tags.
    for (k, v) in [("artist", &tags.artist), ("title", &tags.title), ("album", &tags.album)] {
        if !v.is_empty() {
            c.arg("-metadata").arg(format!("{k}={v}"));
        }
    }
    c.args(["-f", "mp3"]).arg(dest);
    let out = tokio::time::timeout(Duration::from_secs(600), c.stdout(Stdio::null()).stderr(Stdio::piped()).output()).await??;
    if !out.status.success() {
        return Err(anyhow!("ffmpeg: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

/// Adds the artist, title and album to a copied mp3 when it has none.
fn fill_tags(path: &Path, t: &Tags) -> Result<()> {
    use id3::TagLike;
    let mut tag = id3::Tag::read_from_path(path).unwrap_or_default();
    let mut changed = false;
    if tag.artist().is_none_or(str::is_empty) && !t.artist.is_empty() {
        tag.set_artist(&t.artist);
        changed = true;
    }
    if tag.title().is_none_or(str::is_empty) && !t.title.is_empty() {
        tag.set_title(&t.title);
        changed = true;
    }
    if tag.album().is_none_or(str::is_empty) && !t.album.is_empty() {
        tag.set_album(&t.album);
        changed = true;
    }
    if changed {
        tag.write_to_path(path, id3::Version::Id3v23)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(codec: &str, kbps: u32) -> Probe {
        Probe { codec: codec.into(), kbps, lossless: lossless_codec(codec) }
    }

    #[test]
    fn never_converts_up() {
        assert_eq!(plan(&p("mp3", 320), 320), Plan::Copy);
        assert_eq!(plan(&p("mp3", 192), 320), Plan::Copy, "a 192 mp3 is not made 320");
        assert_eq!(plan(&p("mp3", 245), 320), Plan::Copy);
        assert_eq!(plan(&p("mp3", 320), 192), Plan::Encode(192), "a better mp3 goes down");
        assert_eq!(plan(&p("flac", 900), 320), Plan::Encode(320));
        assert_eq!(plan(&p("pcm_s16le", 1411), 256), Plan::Encode(256));
        assert_eq!(plan(&p("opus", 135), 320), Plan::Encode(160), "YouTube opus: no more than it has");
        assert_eq!(plan(&p("aac", 256), 320), Plan::Encode(256));
        assert_eq!(plan(&p("aac", 256), 192), Plan::Encode(192));
        assert_eq!(plan(&p("vorbis", 0), 320), Plan::Encode(192));
    }

    #[test]
    fn names() {
        assert_eq!(safe_name("AC/DC: Back in Black?"), "AC DC Back in Black");
        let dir = std::env::temp_dir().join(format!("spotseek-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_dest(&dir, "x");
        let b = unique_dest(&dir, "x");
        assert_eq!(a.file_name().unwrap(), "x.mp3");
        assert_eq!(b.file_name().unwrap(), "x (1).mp3");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Needs ffmpeg and ffprobe on the PATH.
    #[tokio::test]
    async fn converts_real_files() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("no ffmpeg: skipped");
            return;
        }
        let tools = Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into() };
        let dir = std::env::temp_dir().join(format!("spotseek-conv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mk = |name: &str, args: &[&str]| {
            let path = dir.join(name);
            let st = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=2"]).args(args).arg(&path).status().unwrap();
            assert!(st.success());
            path
        };
        let flac = mk("a.flac", &[]);
        let mp3 = mk("b.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]);
        let out = dir.join("out");
        let tags = Tags { artist: "Art".into(), title: "Song".into(), album: "Alb".into() };
        let (f, k) = to_mp3(&tools, &flac, &out, "Art - Song", 320, &tags).await.unwrap();
        assert_eq!(k, 320);
        assert_eq!(probe("ffprobe", &f).await.unwrap().codec, "mp3");
        let (m, k) = to_mp3(&tools, &mp3, &out, "Art - Song", 320, &tags).await.unwrap();
        assert!(k <= 130, "copied, not raised: {k}");
        assert_eq!(m.file_name().unwrap(), "Art - Song (1).mp3");
        use id3::TagLike;
        assert_eq!(id3::Tag::read_from_path(&m).unwrap().title(), Some("Song"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
