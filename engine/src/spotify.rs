//! Reads a public Spotify playlist, album or track, with no account: the embed page has the
//! names and a short-lived token, and the token reads the full track list.

use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
use num_bigint::BigUint;
use serde::Deserialize;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131 Safari/537.36";
const B62: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: String,
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub kind: String,
    pub name: String,
    /// The album artist (albums), or the track artist.
    pub artist: String,
    /// An image URL: the album cover or the playlist image.
    pub cover: String,
    pub tracks: Vec<Track>,
}

/// `(kind, id)` from a link, or a bare id (a playlist).
pub fn detect(input: &str) -> Result<(String, String)> {
    let re = regex::Regex::new(r"/(playlist|album|track)/([A-Za-z0-9]+)").unwrap();
    if let Some(m) = re.captures(input) {
        return Ok((m[1].to_string(), m[2].to_string()));
    }
    if !input.is_empty() && input.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(("playlist".into(), input.into()));
    }
    Err(anyhow!("cannot read a Spotify id from {input:?}"))
}

/// Spotify sometimes sends `null` for a field (for example the width of a playlist image).
/// Read `null`, or a value of an unexpected type, as the default value instead of failing.
fn or_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ArtistRef {
    #[serde(deserialize_with = "or_default")]
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct TrackItem {
    #[serde(deserialize_with = "or_default")]
    uri: String,
    #[serde(deserialize_with = "or_default")]
    title: String,
    #[serde(deserialize_with = "or_default")]
    name: String,
    #[serde(deserialize_with = "or_default")]
    subtitle: String,
    #[serde(deserialize_with = "or_default")]
    duration: u64,
    #[serde(deserialize_with = "or_default")]
    artists: Vec<ArtistRef>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Image {
    #[serde(deserialize_with = "or_default")]
    url: String,
    #[serde(rename = "maxWidth")]
    #[serde(deserialize_with = "or_default")]
    max_width: i64,
    #[serde(deserialize_with = "or_default")]
    width: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Visual {
    #[serde(deserialize_with = "or_default")]
    image: Vec<Image>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CoverArt {
    #[serde(deserialize_with = "or_default")]
    sources: Vec<Image>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Entity {
    #[serde(deserialize_with = "or_default")]
    name: String,
    #[serde(deserialize_with = "or_default")]
    title: String,
    #[serde(deserialize_with = "or_default")]
    uri: String,
    #[serde(deserialize_with = "or_default")]
    subtitle: String,
    #[serde(deserialize_with = "or_default")]
    duration: u64,
    #[serde(rename = "trackList")]
    #[serde(deserialize_with = "or_default")]
    track_list: Vec<TrackItem>,
    #[serde(deserialize_with = "or_default")]
    artists: Vec<ArtistRef>,
    #[serde(rename = "visualIdentity")]
    #[serde(deserialize_with = "or_default")]
    visual: Visual,
    #[serde(rename = "coverArt")]
    #[serde(deserialize_with = "or_default")]
    cover_art: CoverArt,
}

impl Entity {
    /// The image closest to 640 px wide (smaller counts double).
    fn cover(&self) -> String {
        let mut best = (String::new(), i64::MAX);
        let imgs = self.visual.image.iter().map(|i| (&i.url, i.max_width)).chain(self.cover_art.sources.iter().map(|i| (&i.url, i.width)));
        for (url, w) in imgs {
            if url.is_empty() {
                continue;
            }
            let d = if w == 0 { 1 << 20 } else if w < 640 { (640 - w) * 2 } else { w - 640 };
            if d < best.1 {
                best = (url.clone(), d);
            }
        }
        best.0
    }

    fn order(&self) -> Vec<String> {
        self.track_list.iter().filter_map(|t| item_id(&t.uri)).collect()
    }
}

fn first_artist(s: &str) -> String {
    s.split(',').next().unwrap_or("").trim().to_string()
}

fn item_id(uri: &str) -> Option<String> {
    uri.strip_prefix("spotify:track:").map(str::to_string)
}

fn track_from_item(it: &TrackItem) -> Track {
    let artist = it.artists.first().map(|a| first_artist(&a.name)).unwrap_or_else(|| first_artist(&it.subtitle));
    let title = if it.title.is_empty() { it.name.clone() } else { it.title.clone() };
    Track { id: item_id(&it.uri).unwrap_or_default(), artist, title, album: String::new(), duration: (it.duration / 1000) as u32 }
}

fn track_from_entity(e: &Entity) -> Track {
    let artist = e.artists.first().map(|a| first_artist(&a.name)).unwrap_or_else(|| first_artist(&e.subtitle));
    let title = if e.name.is_empty() { e.title.clone() } else { e.name.clone() };
    Track { id: item_id(&e.uri).unwrap_or_default(), artist, title, album: String::new(), duration: (e.duration / 1000) as u32 }
}

pub fn b62_to_hex(s: &str) -> Result<String> {
    let mut n = BigUint::from(0u32);
    for c in s.bytes() {
        let i = B62.iter().position(|&b| b == c).ok_or_else(|| anyhow!("invalid base62 character {:?}", c as char))?;
        n = n * 62u32 + i as u32;
    }
    Ok(format!("{:0>32}", n.to_str_radix(16)))
}

pub fn hex_to_b62(h: &str) -> Result<String> {
    let mut n = BigUint::parse_bytes(h.as_bytes(), 16).ok_or_else(|| anyhow!("invalid hex {h:?}"))?;
    if n == BigUint::from(0u32) {
        return Ok("0".into());
    }
    let mut out = Vec::new();
    let base = BigUint::from(62u32);
    while n > BigUint::from(0u32) {
        let r = (&n % &base).to_u32_digits().first().copied().unwrap_or(0);
        out.push(B62[r as usize]);
        n /= &base;
    }
    out.reverse();
    Ok(String::from_utf8(out).unwrap())
}

fn next_data(html: &str) -> Result<serde_json::Value> {
    let re = regex::Regex::new(r#"(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#).unwrap();
    let m = re.captures(html).ok_or_else(|| anyhow!("no __NEXT_DATA__ in page"))?;
    serde_json::from_str(&m[1]).context("bad __NEXT_DATA__")
}

struct Http(reqwest::Client);

impl Http {
    fn new() -> Http {
        Http(reqwest::Client::builder().user_agent(USER_AGENT).timeout(std::time::Duration::from_secs(45)).build().unwrap())
    }

    async fn get(&self, url: &str, token: Option<&str>) -> Result<String> {
        let mut req = self.0.get(url);
        if let Some(t) = token {
            req = req.bearer_auth(t).header("Accept", "application/json");
        }
        let resp = req.send().await?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!("{url} returned HTTP {}", status.as_u16()));
        }
        Ok(body)
    }

    async fn embed(&self, kind: &str, id: &str) -> Result<(Entity, String)> {
        let html = self.get(&format!("https://open.spotify.com/embed/{kind}/{id}"), None).await?;
        let nd = next_data(&html)?;
        let state = &nd["props"]["pageProps"]["state"];
        let entity: Entity = serde_json::from_value(state["data"]["entity"].clone()).context("bad entity")?;
        let token = state["settings"]["session"]["accessToken"].as_str().unwrap_or("").to_string();
        Ok((entity, token))
    }

    async fn tracks_by_id(&self, ids: Vec<String>) -> HashMap<String, Track> {
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        let mut set = tokio::task::JoinSet::new();
        let client = std::sync::Arc::new(Http(self.0.clone()));
        for id in ids {
            let (sem, client) = (sem.clone(), client.clone());
            set.spawn(async move {
                let _p = sem.acquire().await;
                client.embed("track", &id).await.ok().map(|(e, _)| (id, track_from_entity(&e)))
            });
        }
        let mut out = HashMap::new();
        while let Some(r) = set.join_next().await {
            if let Ok(Some((id, t))) = r {
                out.insert(id, t);
            }
        }
        out
    }
}

fn ordered(by_id: &HashMap<String, Track>, ids: &[String]) -> Vec<Track> {
    ids.iter().filter_map(|id| by_id.get(id)).filter(|t| !t.title.is_empty()).cloned().collect()
}

/// Reads a public Spotify playlist, album or track.
pub async fn extract(input: &str) -> Result<Collection> {
    let (kind, id) = detect(input)?;
    let http = Http::new();
    let (e, token) = http.embed(&kind, &id).await?;
    let cover = e.cover();
    match kind.as_str() {
        "track" => {
            let t = track_from_entity(&e);
            Ok(Collection { kind, name: t.title.clone(), artist: t.artist.clone(), cover, tracks: vec![t] })
        }
        "album" => {
            let artist = first_artist(&e.subtitle);
            let mut by_id: HashMap<String, Track> = e.track_list.iter().filter_map(|it| item_id(&it.uri).map(|id| (id, track_from_item(it)))).collect();
            let mut ids = e.order();
            if !token.is_empty() {
                if let Ok(full) = album_ids(&http, &id, &token).await {
                    if !full.is_empty() {
                        ids = full;
                    }
                }
            }
            let missing: Vec<String> = ids.iter().filter(|i| !by_id.contains_key(*i)).cloned().collect();
            by_id.extend(http.tracks_by_id(missing).await);
            let mut tracks = ordered(&by_id, &ids);
            for t in &mut tracks {
                t.album = e.name.clone();
            }
            Ok(Collection { kind, name: e.name.clone(), artist, cover, tracks })
        }
        _ => {
            let mut by_id: HashMap<String, Track> = e.track_list.iter().filter_map(|it| item_id(&it.uri).map(|id| (id, track_from_item(it)))).collect();
            let mut ids = e.order();
            if !token.is_empty() {
                if let Ok(full) = playlist_ids(&http, &id, &token).await {
                    if !full.is_empty() {
                        ids = full;
                    }
                }
            }
            let missing: Vec<String> = ids.iter().filter(|i| !by_id.contains_key(*i)).cloned().collect();
            by_id.extend(http.tracks_by_id(missing).await);
            Ok(Collection { kind: "playlist".into(), name: e.name.clone(), artist: String::new(), cover, tracks: ordered(&by_id, &ids) })
        }
    }
}

async fn playlist_ids(http: &Http, id: &str, token: &str) -> Result<Vec<String>> {
    let raw = http.get(&format!("https://spclient.wg.spotify.com/playlist/v2/playlist/{id}"), Some(token)).await?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    Ok(v["contents"]["items"].as_array().map(|a| a.iter().filter_map(|it| it["uri"].as_str().and_then(item_id)).collect()).unwrap_or_default())
}

async fn album_ids(http: &Http, id: &str, token: &str) -> Result<Vec<String>> {
    let gid = b62_to_hex(id)?;
    let raw = http.get(&format!("https://spclient.wg.spotify.com/metadata/4/album/{gid}"), Some(token)).await?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    let mut ids = Vec::new();
    for disc in v["disc"].as_array().into_iter().flatten() {
        for t in disc["track"].as_array().into_iter().flatten() {
            if let Some(g) = t["gid"].as_str() {
                if let Ok(b) = hex_to_b62(g) {
                    ids.push(b);
                }
            }
        }
    }
    Ok(ids)
}

/// Downloads `url` to `path` (through a temporary file), at most 8 MB.
pub async fn download_file(url: &str, path: &std::path::Path) -> Result<()> {
    let http = Http::new();
    let resp = http.0.get(url).timeout(std::time::Duration::from_secs(30)).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("HTTP {}", resp.status().as_u16()));
    }
    let bytes = resp.bytes().await?;
    if bytes.len() > 8 << 20 {
        return Err(anyhow!("too big"));
    }
    if let Some(d) = path.parent() {
        tokio::fs::create_dir_all(d).await?;
    }
    let tmp = path.with_extension("part");
    tokio::fs::write(&tmp, &bytes).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(detect("https://open.spotify.com/intl-es/album/4aawyAB9vmqN3uQ7FjRGTy?si=x").unwrap(), ("album".into(), "4aawyAB9vmqN3uQ7FjRGTy".into()));
        assert!(detect("https://example.com/nothing").is_err());
        let h = b62_to_hex("4aawyAB9vmqN3uQ7FjRGTy").unwrap();
        assert_eq!(h.len(), 32);
        assert_eq!(hex_to_b62(&h).unwrap(), "4aawyAB9vmqN3uQ7FjRGTy");
    }

    #[test]
    fn reads_an_embed_page() {
        let html = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{"name":"Mix","coverArt":{"sources":[{"url":"a","width":300},{"url":"b","width":640}]},"trackList":[{"uri":"spotify:track:abc","title":"One","subtitle":"A, B","duration":200500}]}},"settings":{"session":{"accessToken":"tok"}}}}}}</script></html>"#;
        let nd = next_data(html).unwrap();
        let e: Entity = serde_json::from_value(nd["props"]["pageProps"]["state"]["data"]["entity"].clone()).unwrap();
        assert_eq!(e.cover(), "b");
        let t = track_from_item(&e.track_list[0]);
        assert_eq!((t.id.as_str(), t.artist.as_str(), t.title.as_str(), t.duration), ("abc", "A", "One", 200));
    }

    #[test]
    fn reads_null_fields() {
        // Spotify sends "width": null for some playlist images; that made every such link fail.
        let v = serde_json::json!({"name":"Mix","subtitle":null,"duration":null,"coverArt":{"sources":[{"height":null,"width":null,"url":"a"}]},
            "trackList":[{"uri":"spotify:track:abc","title":"One","subtitle":"A","duration":null,"artists":null}]});
        let e: Entity = serde_json::from_value(v).unwrap();
        assert_eq!(e.cover(), "a");
        let t = track_from_item(&e.track_list[0]);
        assert_eq!((t.id.as_str(), t.artist.as_str(), t.duration), ("abc", "A", 0));
    }
}
