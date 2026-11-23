//! SoundCloud links: a track, a set (playlist or album), or a profile's tracks or likes. yt-dlp
//! reads them (it is already there for YouTube Music). The tracks are then searched on Soulseek
//! like Spotify tracks; when Soulseek has nothing, the track's own SoundCloud audio is downloaded.

use std::time::Duration;

use anyhow::{Result, anyhow};
use serde::Deserialize;

use crate::spotify::{Collection, Track};
use crate::youtube::YtDlp;

/// At most this many tracks from one link (a profile can have thousands).
const MAX_TRACKS: usize = 500;

/// A SoundCloud web link (soundcloud.com, m.soundcloud.com, on.soundcloud.com short links).
pub fn is_link(input: &str) -> bool {
    let t = input.trim();
    let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")).unwrap_or(t);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").to_ascii_lowercase();
    matches!(host.as_str(), "soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com" | "on.soundcloud.com")
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct Entry {
    #[serde(rename = "_type")]
    kind: Option<String>,
    id: Option<serde_json::Value>,
    url: Option<String>,
    webpage_url: Option<String>,
    title: Option<String>,
    track: Option<String>,
    uploader: Option<String>,
    artist: Option<String>,
    artists: Option<Vec<String>>,
    album: Option<String>,
    duration: Option<f64>,
    thumbnail: Option<String>,
    thumbnails: Option<Vec<Thumb>>,
    entries: Option<Vec<Entry>>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct Thumb {
    url: Option<String>,
}

impl Entry {
    fn id(&self) -> String {
        match &self.id {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Number(n)) => n.to_string(),
            _ => String::new(),
        }
    }

    fn page(&self) -> String {
        self.webpage_url.clone().or_else(|| self.url.clone()).unwrap_or_default()
    }

    /// Enough to search for it: a title and a length.
    fn complete(&self) -> bool {
        self.title.as_deref().is_some_and(|t| !t.is_empty()) && self.duration.is_some_and(|d| d > 0.0)
    }

    fn cover(&self) -> String {
        let t = self.thumbnail.clone().or_else(|| self.thumbnails.as_ref().and_then(|v| v.iter().rev().find_map(|t| t.url.clone()))).unwrap_or_default();
        big_cover(&t)
    }

    fn track(&self) -> Track {
        let meta_artist = self.artist.clone().filter(|a| !a.is_empty()).or_else(|| self.artists.as_ref().filter(|v| !v.is_empty()).map(|v| v.join(", ")));
        let name = self.track.clone().filter(|t| !t.is_empty()).or_else(|| self.title.clone()).unwrap_or_default();
        let (artist, title) = split_title(&name, self.uploader.as_deref().unwrap_or(""), meta_artist.as_deref());
        let id = self.id();
        Track {
            id: if id.is_empty() { String::new() } else { format!("soundcloud:{id}") },
            artist,
            title,
            album: self.album.clone().unwrap_or_default(),
            duration: self.duration.unwrap_or(0.0).round() as u32,
            url: self.page(),
        }
    }
}

/// SoundCloud covers come as `…-large.jpg` (100×100); `-t500x500` is the same picture, bigger.
fn big_cover(url: &str) -> String {
    if url.contains("sndcdn.com") { url.replace("-large.", "-t500x500.") } else { url.to_string() }
}

/// Words in brackets that are about the upload, not the song: "[FREE DOWNLOAD]", "(Out Now)".
const UPLOAD_WORDS: &[&str] = &["free download", "free dl", "out now", "premiere", "buy", "click", "support", "preview"];

fn clean(title: &str) -> String {
    let mut out = String::new();
    let mut rest = title;
    while let Some(start) = rest.find(['[', '(']) {
        let close = if rest.as_bytes()[start] == b'[' { ']' } else { ')' };
        let Some(len) = rest[start..].find(close) else { break };
        let inside = rest[start + 1..start + len].to_lowercase();
        out.push_str(&rest[..start]);
        if !UPLOAD_WORDS.iter().any(|w| inside.contains(w)) {
            out.push_str(&rest[start..=start + len]);
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The artist and the title of a SoundCloud upload. Many titles are "Artist - Title" (the
/// uploader is often a label or a channel); else the uploader is the artist.
pub fn split_title(name: &str, uploader: &str, meta_artist: Option<&str>) -> (String, String) {
    let name = clean(name);
    let dash = [" - ", " – ", " — "].iter().filter_map(|d| name.find(d).map(|i| (i, d.len()))).min();
    if let Some(a) = meta_artist.filter(|a| !a.trim().is_empty()) {
        // The title may still start with the artist.
        let title = match dash {
            Some((i, n)) if name[..i].trim().eq_ignore_ascii_case(a.trim()) => name[i + n..].trim().to_string(),
            _ => name.clone(),
        };
        return (a.trim().to_string(), title);
    }
    if let Some((i, n)) = dash {
        let (a, t) = (name[..i].trim(), name[i + n..].trim());
        if !a.is_empty() && !t.is_empty() {
            return (a.to_string(), t.to_string());
        }
    }
    (uploader.trim().to_string(), name)
}

fn parse_lines(out: &str) -> Vec<Entry> {
    out.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Reads a SoundCloud link: one track, or the tracks of a set or profile.
pub async fn extract(yt: &YtDlp, input: &str, limit: usize) -> Result<Collection> {
    let url = input.trim();
    let end = if limit > 0 { limit.min(MAX_TRACKS) } else { MAX_TRACKS }.to_string();
    let out = yt.run(Duration::from_secs(120), &["--flat-playlist", "-J", "--playlist-end", &end, url]).await?;
    let top: Entry = serde_json::from_str(out.trim()).map_err(|e| anyhow!("not a SoundCloud track or set ({e})"))?;
    collection(yt, top).await
}

async fn collection(yt: &YtDlp, top: Entry) -> Result<Collection> {
    let Some(entries) = top.entries.clone().filter(|_| top.kind.as_deref() == Some("playlist")) else {
        let t = top.track();
        return Ok(Collection { kind: "track".into(), name: t.title.clone(), artist: t.artist.clone(), cover: top.cover(), tracks: vec![t] });
    };
    // A flat list may lack the title or the length: read those tracks in full.
    let missing: Vec<String> = entries.iter().filter(|e| !e.complete()).map(Entry::page).filter(|u| !u.is_empty()).collect();
    let full = read_full(yt, &missing).await;
    let entries: Vec<Entry> = entries
        .into_iter()
        .filter_map(|e| {
            if e.complete() {
                return Some(e);
            }
            let (id, page) = (e.id(), e.page());
            let found = full.iter().find(|f| (!id.is_empty() && f.id() == id) || (!page.is_empty() && f.page() == page)).cloned();
            // Not read in full: a title alone still works (the length then does not count).
            found.or_else(|| e.title.as_deref().is_some_and(|t| !t.is_empty()).then_some(e))
        })
        .collect();
    let tracks: Vec<Track> = entries.iter().map(Entry::track).filter(|t| !t.title.is_empty()).collect();
    let cover = Some(top.cover()).filter(|c| !c.is_empty()).or_else(|| entries.first().map(Entry::cover)).unwrap_or_default();
    Ok(Collection { kind: "playlist".into(), name: top.title.clone().unwrap_or_default(), artist: top.uploader.clone().unwrap_or_default(), cover, tracks })
}

/// The full facts of these tracks: a few yt-dlp runs at the same time, many links each.
async fn read_full(yt: &YtDlp, urls: &[String]) -> Vec<Entry> {
    if urls.is_empty() {
        return Vec::new();
    }
    let per = urls.len().div_ceil(4).max(1);
    let mut set = tokio::task::JoinSet::new();
    for chunk in urls.chunks(per) {
        let bin = yt.bin.clone();
        let chunk = chunk.to_vec();
        set.spawn(async move {
            let y = YtDlp { bin };
            let mut args = vec!["--skip-download", "--no-playlist", "--ignore-errors", "-j"];
            args.extend(chunk.iter().map(String::as_str));
            y.run(Duration::from_secs(300), &args).await.map(|o| parse_lines(&o)).unwrap_or_default()
        });
    }
    let mut all = Vec::new();
    while let Some(r) = set.join_next().await {
        all.extend(r.unwrap_or_default());
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        assert!(is_link("https://soundcloud.com/forss/flickermood"));
        assert!(is_link(" soundcloud.com/forss/sets/soulhack "));
        assert!(is_link("https://m.soundcloud.com/forss/flickermood"));
        assert!(is_link("https://on.soundcloud.com/abc123"));
        assert!(!is_link("https://open.spotify.com/track/4aawyAB9vmqN3uQ7FjRGTy"));
        assert!(!is_link("https://soundcloud.com.evil.example/x"));
    }

    #[test]
    fn artist_and_title() {
        let s = |n: &str, u: &str, a: Option<&str>| split_title(n, u, a);
        assert_eq!(s("Fred again.. - Delilah (pull me out of this)", "Fred again..", None), ("Fred again..".into(), "Delilah (pull me out of this)".into()));
        assert_eq!(s("Flickermood", "Forss", None), ("Forss".into(), "Flickermood".into()));
        assert_eq!(s("Bicep – Glue [FREE DOWNLOAD]", "Some Label", None), ("Bicep".into(), "Glue".into()));
        assert_eq!(s("Glue (Out Now on Ninja Tune)", "Bicep", None), ("Bicep".into(), "Glue".into()));
        assert_eq!(s("Bicep - Glue (Extended Mix)", "Label", Some("Bicep")), ("Bicep".into(), "Glue (Extended Mix)".into()));
        assert_eq!(s("Glue", "Label", Some("Bicep")), ("Bicep".into(), "Glue".into()));
        assert_eq!(s("- Untitled", "Me", None), ("Me".into(), "- Untitled".into()));
    }

    #[test]
    fn reads_yt_dlp_json() {
        let track = r#"{"id":"293","title":"Flickermood","uploader":"Forss","duration":213.04,"webpage_url":"https://soundcloud.com/forss/flickermood","thumbnail":"https://i1.sndcdn.com/artworks-000-large.jpg"}"#;
        let e: Entry = serde_json::from_str(track).unwrap();
        let t = e.track();
        assert_eq!((t.artist.as_str(), t.title.as_str(), t.duration), ("Forss", "Flickermood", 213));
        assert_eq!(t.id, "soundcloud:293");
        assert_eq!(t.url, "https://soundcloud.com/forss/flickermood");
        assert_eq!(e.cover(), "https://i1.sndcdn.com/artworks-000-t500x500.jpg");
        let set = r#"{"_type":"playlist","id":"2284613","title":"Soulhack","uploader":"Forss","entries":[
            {"_type":"url","id":293,"url":"https://soundcloud.com/forss/flickermood","ie_key":"Soundcloud"},
            {"_type":"url","id":294,"url":"https://soundcloud.com/forss/journeyman","title":"Journeyman","duration":180.5,"uploader":"Forss"}]}"#;
        let e: Entry = serde_json::from_str(set).unwrap();
        let es = e.entries.unwrap();
        assert!(!es[0].complete() && es[1].complete());
        assert_eq!(es[0].id(), "293");
        assert_eq!(es[1].track().title, "Journeyman");
    }
}
