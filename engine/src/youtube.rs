//! The fallback: find the track on YouTube Music with yt-dlp and download its best audio. The
//! converter makes the mp3 (never above the bitrate YouTube has).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::Deserialize;

use crate::convert::command;
use crate::rank::{base_title, main_artist, norm};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidate {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: u32,
    pub id: String,
    pub url: String,
    pub score: f32,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Entry {
    id: String,
    url: String,
    ie_key: String,
    title: String,
    artist: String,
    channel: String,
    uploader: String,
    album: String,
    duration: f64,
}

impl Entry {
    fn is_video(&self) -> bool {
        let id_ok = self.id.len() == 11 && self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !id_ok || (!self.ie_key.is_empty() && self.ie_key != "Youtube") {
            return false;
        }
        self.url.is_empty() || self.url == self.id || self.url.contains(&format!("v={}", self.id)) || self.url.ends_with(&format!("youtu.be/{}", self.id))
    }

    fn candidate(&self) -> Candidate {
        let artist = [&self.artist, &self.channel, &self.uploader].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
        Candidate {
            title: self.title.clone(),
            artist,
            album: self.album.clone(),
            duration: self.duration as u32,
            id: self.id.clone(),
            url: format!("https://www.youtube.com/watch?v={}", self.id),
            score: 0.0,
        }
    }
}

const BAD_TERMS: &[&str] = &[
    "lyrics", "lyric", "letra", "subtitulad", "subtitulado", "traduccion", "reaction", "review", "tutorial", "cover", "karaoke", "instrumental", "remix", "sped up",
    "slowed", "nightcore", "8d", "live", "en vivo", "mashup", "type beat", "full album", "backstage",
];

fn has(hay: &str, w: &str) -> bool {
    format!(" {hay} ").contains(&format!(" {w} "))
}

fn bad_video(video: &str, want: &str) -> bool {
    let (t, w) = (norm(video), norm(want));
    if BAD_TERMS.iter().any(|term| has(&t, term) && !has(&w, term)) {
        return true;
    }
    let official = ["official video", "official music video", "videoclip", "video oficial"].iter().any(|p| has(&t, p));
    official && !w.contains("official")
}

/// The same score as before the rewrite (max 12).
pub fn score(c: &Candidate, title: &str, artist: &str, length: u32) -> f32 {
    let (vt, va, nt, na) = (norm(&c.title), norm(&c.artist), norm(title), norm(artist));
    let mut s = 0.0;
    if !nt.is_empty() {
        if vt.contains(&nt) {
            s += 4.0;
        } else {
            let words: Vec<&str> = nt.split(' ').collect();
            if words.iter().all(|w| w.chars().count() <= 2 || vt.contains(w)) {
                s += 2.0;
            }
        }
    }
    if !na.is_empty() && va.contains(&na) {
        s += 3.0;
    }
    if !c.album.is_empty() {
        s += 1.0;
    }
    if length > 0 && c.duration > 0 {
        let diff = (c.duration as i64 - length as i64).unsigned_abs();
        s += match diff {
            0..=3 => 4.0,
            4..=10 => 2.0,
            11..=45 => 0.0,
            _ => -4.0,
        };
    }
    if bad_video(&c.title, title) {
        s -= 8.0;
    }
    s
}

fn parse(out: &str) -> Vec<Entry> {
    out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

pub struct YtDlp {
    pub bin: String,
}

impl YtDlp {
    async fn run(&self, timeout: Duration, args: &[&str]) -> Result<String> {
        let mut c = command(&self.bin);
        c.args(["--no-warnings", "--ignore-config"]).args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = tokio::time::timeout(timeout, c.output()).await.map_err(|_| anyhow!("yt-dlp took too long"))??;
        if !out.status.success() && out.stdout.is_empty() {
            return Err(anyhow!("yt-dlp: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// The best videos for the track, best first. Flat results from YouTube Music and YouTube
    /// come first; the top few are then read in full (artist, album, exact length).
    pub async fn search(&self, artist: &str, title: &str, length: u32) -> Result<Vec<Candidate>> {
        let q = format!("{} {}", main_artist(artist), base_title(title)).trim().to_string();
        let music = format!("https://music.youtube.com/search?q={}#songs", urlencode(&q));
        let video = format!("ytsearch5:{q}");
        let flat = ["--skip-download", "--flat-playlist", "--dump-json", "--playlist-end", "5"];
        let (args_a, args_b) = ([&flat[..], &[music.as_str()]].concat(), [&flat[..], &[video.as_str()]].concat());
        let (a, b) = tokio::join!(self.run(Duration::from_secs(90), &args_a), self.run(Duration::from_secs(90), &args_b));
        let mut seen = std::collections::HashSet::new();
        let mut cands: Vec<Candidate> = Vec::new();
        for out in [a.as_deref().unwrap_or(""), b.as_deref().unwrap_or("")] {
            for e in parse(out) {
                if e.is_video() && seen.insert(e.id.clone()) {
                    let mut c = e.candidate();
                    c.score = score(&c, title, artist, length);
                    cands.push(c);
                }
            }
        }
        if cands.is_empty() {
            return Err(a.err().or(b.err()).unwrap_or_else(|| anyhow!("no results for {q:?}")));
        }
        cands.sort_by(|x, y| y.score.partial_cmp(&x.score).unwrap_or(std::cmp::Ordering::Equal));
        // Read the top three in full, at the same time.
        let top: Vec<Candidate> = cands.iter().take(3).cloned().collect();
        let mut set = tokio::task::JoinSet::new();
        for c in top {
            let bin = self.bin.clone();
            set.spawn(async move {
                let y = YtDlp { bin };
                let out = y.run(Duration::from_secs(60), &["--skip-download", "--no-playlist", "--dump-json", &c.url]).await.ok()?;
                parse(&out).into_iter().find(|e| e.id == c.id).map(|e| e.candidate())
            });
        }
        let mut full = Vec::new();
        while let Some(r) = set.join_next().await {
            if let Ok(Some(mut c)) = r {
                c.score = score(&c, title, artist, length);
                full.push(c);
            }
        }
        if full.is_empty() {
            full = cands;
        }
        full.sort_by(|x, y| y.score.partial_cmp(&x.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(full)
    }

    /// Downloads the best audio of `c` into `dir` as `name.<ext>`. Returns the file.
    pub async fn download(&self, c: &Candidate, dir: &Path, name: &str) -> Result<PathBuf> {
        tokio::fs::create_dir_all(dir).await?;
        let template = dir.join(format!("{name}.%(ext)s"));
        let template = template.to_string_lossy().into_owned();
        let attempts: [&[&str]; 3] = [&[], &["--extractor-args", "youtube:player_client=default,web_safari"], &["--extractor-args", "youtube:player_client=web_safari,tv"]];
        let mut last = anyhow!("yt-dlp failed");
        for extra in attempts {
            let mut args = vec!["-f", "bestaudio/best", "--no-playlist", "--no-part", "-o", &template, "--print", "after_move:filepath"];
            args.extend_from_slice(extra);
            args.push(&c.url);
            match self.run(Duration::from_secs(600), &args).await {
                Ok(out) => {
                    if let Some(p) = out.lines().map(str::trim).filter(|l| !l.is_empty()).last() {
                        let p = PathBuf::from(p);
                        if p.is_file() {
                            return Ok(p);
                        }
                    }
                    // Older yt-dlp: look for the file.
                    if let Some(p) = std::fs::read_dir(dir).ok().into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.file_stem().is_some_and(|s| s == name)) {
                        return Ok(p);
                    }
                    last = anyhow!("yt-dlp gave no file");
                }
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_like_before() {
        let c = Candidate { title: "Enya - Only Time (Official Video)".into(), artist: "Enya".into(), duration: 218, ..Default::default() };
        assert!(score(&c, "Only Time", "Enya", 218) < 4.0, "official videos are penalized");
        let c = Candidate { title: "Only Time".into(), artist: "Enya".into(), album: "A Day Without Rain".into(), duration: 218, ..Default::default() };
        assert_eq!(score(&c, "Only Time", "Enya", 218), 12.0);
        let c = Candidate { title: "Only Time (Karaoke)".into(), artist: "Sing".into(), duration: 218, ..Default::default() };
        assert!(score(&c, "Only Time", "Enya", 218) <= 0.0);
        assert_eq!(urlencode("a b&c"), "a+b%26c");
    }

    #[test]
    fn reads_flat_entries() {
        let out = "{\"id\":\"dQw4w9WgXcQ\",\"url\":\"https://www.youtube.com/watch?v=dQw4w9WgXcQ\",\"title\":\"x\",\"channel\":\"Rick\",\"duration\":212.0}\n{\"id\":\"PLxxx\",\"ie_key\":\"YoutubeTab\"}\nnot json";
        let es = parse(out);
        assert_eq!(es.len(), 2);
        assert!(es[0].is_video() && !es[1].is_video());
        assert_eq!(es[0].candidate().artist, "Rick");
    }
}
