//! Turns a downloaded file into the file in the output folder, in the format the user chose (mp3,
//! m4a or flac). The sound never goes up: a file already in that format at or below the wanted
//! bitrate is copied as it is; a better one or a lossless file is encoded down to the wanted
//! bitrate; another lossy format is encoded at no more than its own bitrate. FLAC is lossless:
//! a FLAC is copied, anything else is encoded to FLAC (a lossy source keeps its own sound).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow};
use tokio::process::Command;

/// mp3 bitrates, kbps.
const STEPS: [u32; 8] = [96, 112, 128, 160, 192, 224, 256, 320];

/// The format of the files in the output folder.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Format {
    #[default]
    Mp3,
    /// AAC in an .m4a file.
    M4a,
    Flac,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Mp3, Format::M4a, Format::Flac];

    pub fn parse(s: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|f| f.ext().eq_ignore_ascii_case(s.trim()))
    }

    /// The file extension, also the name on the command line.
    pub fn ext(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::M4a => "m4a",
            Format::Flac => "flac",
        }
    }

    /// The codec name in `ffmpeg -i` output.
    fn codec(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::M4a => "aac",
            Format::Flac => "flac",
        }
    }

    pub fn lossless(self) -> bool {
        self == Format::Flac
    }

    /// A Soulseek file with this extension is already in the format (copied, not encoded).
    pub fn same_ext(self, ext: &str) -> bool {
        match self {
            Format::M4a => ext == "m4a" || ext == "aac",
            f => ext == f.ext(),
        }
    }

    /// The ffmpeg muxer.
    fn muxer(self) -> &'static str {
        match self {
            Format::Mp3 => "mp3",
            Format::M4a => "ipod",
            Format::Flac => "flac",
        }
    }
}

/// What the converter does with a file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plan {
    /// Keep the audio as it is (an mp3 is copied; m4a and flac are put in a new file with the tags).
    Copy,
    /// Encode at this many kbps (0 for FLAC: lossless, no bitrate).
    Encode(u32),
}

/// The audio facts of a file (see `probe`).
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

pub fn plan(p: &Probe, format: Format, target: u32) -> Plan {
    if format.lossless() {
        return if p.codec == format.codec() { Plan::Copy } else { Plan::Encode(0) };
    }
    if p.lossless {
        return Plan::Encode(target);
    }
    if p.codec == format.codec() {
        // A few kbps over (VBR, rounding) is still the same quality.
        return if p.kbps == 0 || p.kbps <= target + 8 { Plan::Copy } else { Plan::Encode(target) };
    }
    // Another lossy format: no higher than what it has.
    if p.kbps == 0 { Plan::Encode(target.min(192)) } else { Plan::Encode(target.min(step_up(p.kbps))) }
}

fn lossless_codec(c: &str) -> bool {
    c == "flac" || c == "alac" || c == "ape" || c == "wavpack" || c == "tta" || c == "mlp" || c.starts_with("pcm_")
}

/// Reads the codec and bitrate of the first audio stream. `ffmpeg -i` with no output prints the
/// same facts ffprobe gives (the stream's bitrate, else the whole file's), so no ffprobe is bundled.
pub async fn probe(ffmpeg: &str, path: &Path) -> Result<Probe> {
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        command(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(path).stdout(Stdio::null()).stderr(Stdio::piped()).output(),
    )
    .await??;
    // It exits with an error (no output file was given); the facts are on stderr all the same.
    let text = String::from_utf8_lossy(&out.stderr);
    parse_probe(&text).ok_or_else(|| anyhow!("no audio stream: {}", text.lines().last().unwrap_or("").trim()))
}

/// The length of the audio in seconds, from `ffmpeg -i`.
pub async fn duration(ffmpeg: &str, path: &Path) -> Option<u32> {
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        command(ffmpeg).args(["-hide_banner", "-nostdin", "-i"]).arg(path).stdout(Stdio::null()).stderr(Stdio::piped()).output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_duration(&String::from_utf8_lossy(&out.stderr))
}

/// `  Duration: 00:03:21.03, start: …` → 201.
pub fn parse_duration(text: &str) -> Option<u32> {
    let l = text.lines().find(|l| l.trim_start().starts_with("Duration:"))?;
    let v = l.trim_start()["Duration:".len()..].trim_start().split(',').next()?.trim();
    let mut it = v.split(':');
    let (h, m, s): (u32, u32, f64) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    Some(h * 3600 + m * 60 + s.round() as u32)
}

/// "123 kb/s" (or "123 kb/s (default)") → 123.
fn kbps_in(part: &str) -> Option<u32> {
    let mut w = part.split_whitespace();
    let n = w.next()?.parse().ok()?;
    (w.next()? == "kb/s").then_some(n)
}

/// The first audio stream in `ffmpeg -i` output:
/// `Stream #0:0[0x1](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 123 kb/s (default)`.
pub fn parse_probe(text: &str) -> Option<Probe> {
    let line = text.lines().find(|l| l.trim_start().starts_with("Stream #") && l.contains(": Audio: "))?;
    let rest = &line[line.find(": Audio: ")? + ": Audio: ".len()..];
    let codec: String = rest.chars().take_while(|c| !matches!(c, ' ' | ',')).collect();
    if codec.is_empty() {
        return None;
    }
    let file_kbps = || {
        let l = text.lines().find(|l| l.trim_start().starts_with("Duration:"))?;
        kbps_in(&l[l.find("bitrate: ")? + "bitrate: ".len()..])
    };
    let kbps = rest.split(", ").find_map(kbps_in).or_else(file_kbps).unwrap_or(0);
    Some(Probe { lossless: lossless_codec(&codec), codec, kbps })
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

/// `dir/base.mp3`, or `dir/base (1).mp3`, … when it is taken (with the extension `ext`). Creates
/// an empty file there, so two conversions at once never pick the same name.
pub fn unique_dest(dir: &Path, base: &str, ext: &str) -> PathBuf {
    let mut i = 0;
    loop {
        let name = if i == 0 { format!("{base}.{ext}") } else { format!("{base} ({i}).{ext}") };
        let p = dir.join(name);
        if std::fs::OpenOptions::new().write(true).create_new(true).open(&p).is_ok() {
            return p;
        }
        i += 1;
    }
}

/// Tags written into the file.
#[derive(Clone, Debug, Default)]
pub struct Tags {
    pub artist: String,
    pub title: String,
    pub album: String,
}

pub struct Tools {
    pub ffmpeg: String,
}

/// Converts `src` into a new file in `format` in `dir` named `base`, at most `target` kbps.
/// Returns the file and the bitrate it has (for FLAC: the bitrate of the source's sound).
pub async fn convert(tools: &Tools, src: &Path, dir: &Path, base: &str, format: Format, target: u32, tags: &Tags) -> Result<(PathBuf, u32)> {
    tokio::fs::create_dir_all(dir).await?;
    let p = probe(&tools.ffmpeg, src).await?;
    let dest = unique_dest(dir, base, format.ext());
    let result = async {
        let pl = plan(&p, format, target);
        if pl == Plan::Copy && format == Format::Mp3 {
            tokio::fs::copy(src, &dest).await?;
            let d = dest.clone();
            let t = tags.clone();
            let _ = tokio::task::spawn_blocking(move || fill_tags(&d, &t)).await;
            return Ok(p.kbps);
        }
        // Keep the cover picture; a picture ffmpeg cannot put in the file is left out.
        if encode(tools, src, &dest, format, pl, !p.lossless, tags, true).await.is_err() {
            encode(tools, src, &dest, format, pl, !p.lossless, tags, false).await?;
        }
        Ok(match pl {
            Plan::Encode(k) if k > 0 => k,
            _ => p.kbps,
        })
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

#[allow(clippy::too_many_arguments)]
async fn encode(tools: &Tools, src: &Path, dest: &Path, format: Format, plan: Plan, lossy_src: bool, tags: &Tags, cover: bool) -> Result<()> {
    let mut c = command(&tools.ffmpeg);
    c.args(["-y", "-loglevel", "error", "-i"]).arg(src);
    if cover {
        c.args(["-map", "0:a:0", "-map", "0:v:0?", "-c:v", "copy", "-disposition:v:0", "attached_pic"]);
    } else {
        c.arg("-vn");
    }
    c.args(["-map_metadata", "0"]);
    match (plan, format) {
        (Plan::Copy, _) => {
            c.args(["-c:a", "copy"]);
        }
        (Plan::Encode(kbps), Format::Mp3) => {
            c.args(["-id3v2_version", "3", "-c:a", "libmp3lame", "-b:a", &format!("{kbps}k")]);
        }
        (Plan::Encode(kbps), Format::M4a) => {
            c.args(["-c:a", "aac", "-b:a", &format!("{kbps}k")]);
        }
        (Plan::Encode(_), Format::Flac) => {
            c.args(["-c:a", "flac"]);
            // A lossy source has no bit depth: 16 bits, like a CD (else it would be 24 or 32).
            if lossy_src {
                c.args(["-sample_fmt", "s16"]);
            }
        }
    }
    // Spotify's names, so the file is found again by its tags.
    for (k, v) in [("artist", &tags.artist), ("title", &tags.title), ("album", &tags.album)] {
        if !v.is_empty() {
            c.arg("-metadata").arg(format!("{k}={v}"));
        }
    }
    c.args(["-f", format.muxer()]).arg(dest);
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
    fn formats() {
        use Format::*;
        assert_eq!(Format::parse("FLAC"), Some(Flac));
        assert_eq!(Format::parse("m4a"), Some(M4a));
        assert_eq!(Format::parse("ogg"), None);
        assert_eq!(plan(&p("aac", 256), M4a, 320), Plan::Copy, "an aac is not encoded again");
        assert_eq!(plan(&p("aac", 320), M4a, 192), Plan::Encode(192));
        assert_eq!(plan(&p("mp3", 320), M4a, 256), Plan::Encode(256));
        assert_eq!(plan(&p("flac", 900), M4a, 256), Plan::Encode(256));
        assert_eq!(plan(&p("flac", 900), Flac, 320), Plan::Copy);
        assert_eq!(plan(&p("alac", 900), Flac, 320), Plan::Encode(0));
        assert_eq!(plan(&p("mp3", 320), Flac, 320), Plan::Encode(0));
        assert!(M4a.same_ext("aac") && !Mp3.same_ext("m4a"));
    }

    #[test]
    fn never_converts_up() {
        assert_eq!(plan(&p("mp3", 320), Format::Mp3, 320), Plan::Copy);
        assert_eq!(plan(&p("mp3", 192), Format::Mp3, 320), Plan::Copy, "a 192 mp3 is not made 320");
        assert_eq!(plan(&p("mp3", 245), Format::Mp3, 320), Plan::Copy);
        assert_eq!(plan(&p("mp3", 320), Format::Mp3, 192), Plan::Encode(192), "a better mp3 goes down");
        assert_eq!(plan(&p("flac", 900), Format::Mp3, 320), Plan::Encode(320));
        assert_eq!(plan(&p("pcm_s16le", 1411), Format::Mp3, 256), Plan::Encode(256));
        assert_eq!(plan(&p("opus", 135), Format::Mp3, 320), Plan::Encode(160), "YouTube opus: no more than it has");
        assert_eq!(plan(&p("aac", 256), Format::Mp3, 320), Plan::Encode(256));
        assert_eq!(plan(&p("aac", 256), Format::Mp3, 192), Plan::Encode(192));
        assert_eq!(plan(&p("vorbis", 0), Format::Mp3, 320), Plan::Encode(192));
    }

    #[test]
    fn reads_ffmpeg_output() {
        let m4a = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'c.m4a':\n  Duration: 00:00:03.00, start: 0.000000, bitrate: 127 kb/s\n  Stream #0:0[0x1](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, mono, fltp, 123 kb/s (default)\nAt least one output file must be specified";
        assert_eq!(parse_probe(m4a), Some(p("aac", 123)));
        let webm = "  Duration: 00:00:03.01, start: -0.007000, bitrate: 152 kb/s\n  Stream #0:0: Audio: opus, 48000 Hz, mono, fltp";
        assert_eq!(parse_probe(webm), Some(p("opus", 152)), "no stream bitrate: the file's");
        let wav = "Guessed Channel Layout for Input Stream #0.0 : mono\n  Duration: 00:00:03.00, bitrate: 705 kb/s\n  Stream #0:0: Audio: pcm_s16le ([1][0][0][0] / 0x0001), 44100 Hz, 1 channels, s16, 705 kb/s";
        assert_eq!(parse_probe(wav), Some(p("pcm_s16le", 705)));
        let mp3 = "  Duration: 00:03:21.03, start: 0.025057, bitrate: 330 kb/s\n  Stream #0:0: Audio: mp3 (mp3float), 44100 Hz, stereo, fltp, 320 kb/s\n  Stream #0:1: Video: mjpeg (Baseline), yuvj420p, 500x500, 90k tbr (attached pic)";
        assert_eq!(parse_probe(mp3), Some(p("mp3", 320)), "the cover does not count");
        let flac = "  Duration: 00:00:03.00, start: 0.000000, bitrate: 118 kb/s\n  Stream #0:0: Audio: flac, 44100 Hz, mono, s16";
        assert!(parse_probe(flac).unwrap().lossless);
        assert_eq!(parse_probe("x.jpg: Invalid data found when processing input"), None);
        assert_eq!(parse_duration(mp3), Some(201));
        assert_eq!(parse_duration("  Duration: 01:00:29.60, start: 0"), Some(3630));
        assert_eq!(parse_duration("  Duration: N/A, bitrate: N/A"), None);
    }

    #[test]
    fn names() {
        assert_eq!(safe_name("AC/DC: Back in Black?"), "AC DC Back in Black");
        let dir = std::env::temp_dir().join(format!("spotseek-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_dest(&dir, "x", "mp3");
        let b = unique_dest(&dir, "x", "mp3");
        assert_eq!(a.file_name().unwrap(), "x.mp3");
        assert_eq!(b.file_name().unwrap(), "x (1).mp3");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Needs a full ffmpeg on the PATH (it makes the test files). `SPOTSEEK_TEST_FFMPEG` names the
    /// ffmpeg that converts and probes (for example the small one of scripts/build-ffmpeg.sh).
    #[tokio::test]
    async fn converts_real_files() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            eprintln!("no ffmpeg: skipped");
            return;
        }
        let ff = std::env::var("SPOTSEEK_TEST_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
        let tools = Tools { ffmpeg: ff.clone() };
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
        let (f, k) = convert(&tools, &flac, &out, "Art - Song", Format::Mp3, 320, &tags).await.unwrap();
        assert_eq!(k, 320);
        let got = probe(&ff, &f).await.unwrap();
        assert_eq!((got.codec.as_str(), got.lossless), ("mp3", false));
        assert!((300..=330).contains(&got.kbps), "{got:?}");
        let got = probe(&ff, &flac).await.unwrap();
        assert_eq!((got.codec.as_str(), got.lossless), ("flac", true));
        let (m, k) = convert(&tools, &mp3, &out, "Art - Song", Format::Mp3, 320, &tags).await.unwrap();
        assert!(k <= 130, "copied, not raised: {k}");
        assert_eq!(m.file_name().unwrap(), "Art - Song (1).mp3");
        use id3::TagLike;
        assert_eq!(id3::Tag::read_from_path(&m).unwrap().title(), Some("Song"));
        let title = |text: &str| text.lines().any(|l| l.trim_start().starts_with("title") && l.trim_end().ends_with(": Song"));
        let info = |path: &Path| String::from_utf8_lossy(&std::process::Command::new(&ff).arg("-hide_banner").arg("-i").arg(path).output().unwrap().stderr).into_owned();
        // m4a: AAC, no higher than the mp3 had.
        let (a, k) = convert(&tools, &mp3, &out, "Art - Song", Format::M4a, 320, &tags).await.unwrap();
        assert_eq!(a.file_name().unwrap(), "Art - Song.m4a");
        assert!(k <= 128, "{k}");
        let got = probe(&ff, &a).await.unwrap();
        assert_eq!(got.codec, "aac");
        assert!(title(&info(&a)), "{}", info(&a));
        let (a2, _) = convert(&tools, &a, &out, "Art - Song", Format::M4a, 320, &tags).await.unwrap();
        assert_eq!(probe(&ff, &a2).await.unwrap().codec, "aac", "an m4a is copied");
        // flac: the flac is kept lossless; an mp3 becomes a 16-bit flac.
        let (l, _) = convert(&tools, &flac, &out, "Art - Song", Format::Flac, 320, &tags).await.unwrap();
        assert_eq!(l.file_name().unwrap(), "Art - Song.flac");
        assert!(probe(&ff, &l).await.unwrap().lossless);
        assert!(title(&info(&l)), "{}", info(&l));
        let (l2, _) = convert(&tools, &mp3, &out, "Art - Song", Format::Flac, 320, &tags).await.unwrap();
        assert_eq!(probe(&ff, &l2).await.unwrap().codec, "flac");
        assert!(info(&l2).contains(", s16"), "{}", info(&l2));
        let _ = std::fs::remove_dir_all(dir);
    }
}
