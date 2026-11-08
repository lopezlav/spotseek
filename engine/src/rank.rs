//! Which Soulseek file to download for a track: is it the same song (name and length), is the
//! sound good enough for the mp3 we want (never less than needed when there is better), and will
//! the peer send it soon and fast.

use unicode_normalization::UnicodeNormalization;

use crate::events::CandInfo;
use crate::slsk::{SearchResponse, SharedFile};
use crate::spotify::Track;

/// Lower case, no accents, letters and digits split by single spaces.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = true;
    for c in s.nfd() {
        if unicode_normalization::char::is_combining_mark(c) {
            continue;
        }
        if c.is_alphanumeric() {
            for l in c.to_lowercase() {
                out.push(l);
            }
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    out.trim_end().to_string()
}

/// The title without the parts file names often leave out: "(feat. X)", "[Remastered]", and
/// " - Remix" style suffixes. The full title when nothing would be left.
pub fn base_title(s: &str) -> &str {
    let mut cut = s.len();
    for sep in [" (", " [", " - "] {
        if let Some(i) = s.find(sep) {
            if i > 0 && i < cut {
                cut = i;
            }
        }
    }
    let b = s[..cut].trim();
    if b.is_empty() { s } else { b }
}

/// The first artist ("A, B" and "A feat. B" give "A").
pub fn main_artist(s: &str) -> &str {
    let mut a = s.split(',').next().unwrap_or(s);
    for sep in [" feat. ", " feat ", " ft. ", " & ", " x "] {
        if let Some(i) = a.to_lowercase().find(sep) {
            a = &a[..i];
        }
    }
    a.trim()
}

/// The search words: the main artist and the base title. Soulseek finds files whose path has
/// every word, so fewer, surer words find more.
pub fn queries(t: &Track) -> Vec<String> {
    let title = norm(base_title(&t.title));
    let artist = norm(main_artist(&t.artist));
    let mut q = vec![format!("{artist} {title}").trim().to_string()];
    // Without the artist: for files named only by title, in folders named another way.
    let distinctive = title.split(' ').count() >= 2 || title.len() >= 7;
    if distinctive && !artist.is_empty() {
        q.push(title);
    }
    q.retain(|s| !s.is_empty());
    q.dedup();
    q
}

/// Words that make a different version, when the wanted title does not have them.
const VERSION_WORDS: &[&str] = &[
    "remix", "live", "karaoke", "instrumental", "acapella", "a cappella", "cover", "sped up", "slowed", "nightcore", "8d", "mashup", "bootleg",
    "extended", "club mix", "dub mix", "edit", "reprise", "demo", "acoustic", "unplugged", "version", "vip", "rework", "flip", "chopped",
];

const LOSSLESS: &[&str] = &["flac", "wav", "aif", "aiff", "ape", "wv", "alac"];
const LOSSY: &[&str] = &["mp3", "m4a", "aac", "ogg", "opus", "wma"];

pub fn is_lossless(ext: &str) -> bool {
    LOSSLESS.contains(&ext)
}

pub fn is_audio(ext: &str) -> bool {
    is_lossless(ext) || LOSSY.contains(&ext)
}

/// The file name without folders and extension.
pub fn stem(path: &str) -> &str {
    let base = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match base.rfind('.') {
        Some(i) if i > 0 => &base[..i],
        _ => base,
    }
}

fn has_words(hay: &str, words: &str) -> bool {
    let h = format!(" {hay} ");
    h.contains(&format!(" {words} "))
}

/// How well the file fits the track, 0 to 100. Below 50 it is most likely another song.
pub fn fit(t: &Track, f: &SharedFile) -> u32 {
    let path = norm(&f.filename);
    let name = norm(stem(&f.filename));
    let full_title = norm(&t.title);
    let title = norm(base_title(&t.title));
    let artist = norm(main_artist(&t.artist));
    let mut s: f32 = 0.0;

    // The title, in the file name.
    let words: Vec<&str> = title.split(' ').filter(|w| !w.is_empty()).collect();
    let mut cap = 100.0;
    if !words.is_empty() {
        if has_words(&name, &title) {
            s += 45.0;
        } else {
            let found = words.iter().filter(|w| has_words(&name, w)).count() as f32 / words.len() as f32;
            s += 30.0 * found;
            // Most of the title is not in the name: another song, whatever its length.
            if found < 0.6 {
                cap = 40.0;
            }
        }
    }
    // The artist, anywhere in the path (folders are often "Artist/Album").
    let aw: Vec<&str> = artist.split(' ').filter(|w| !w.is_empty()).collect();
    if !aw.is_empty() {
        if has_words(&path, &artist) {
            s += 25.0;
        } else {
            let found = aw.iter().filter(|w| has_words(&path, w)).count();
            s += 15.0 * found as f32 / aw.len() as f32;
        }
    } else {
        s += 15.0;
    }
    // The length.
    match (f.duration, t.duration) {
        (Some(d), w) if w > 0 && d > 0 => {
            let diff = (d as i64 - w as i64).unsigned_abs();
            s += match diff {
                0..=2 => 30.0,
                3..=5 => 24.0,
                6..=10 => 12.0,
                11..=20 => 0.0,
                _ => -40.0,
            };
        }
        _ => s += 10.0,
    }
    // Another version of the song.
    for w in VERSION_WORDS {
        if has_words(&name, w) && !has_words(&full_title, w) {
            s -= 35.0;
            break;
        }
    }
    s.clamp(0.0, cap) as u32
}

/// The kbps of the file: what the peer said, else from the size and the length.
pub fn kbps(f: &SharedFile) -> u32 {
    if let Some(b) = f.bitrate.filter(|b| *b > 0) {
        return b;
    }
    match f.duration {
        Some(d) if d > 0 => (f.size * 8 / d as u64 / 1000) as u32,
        _ => 0,
    }
}

/// How good the sound is for an mp3 of `target` kbps, 0 to 1. Lossless and mp3 at or above the
/// target are 1; lower mp3 counts by its bitrate; other lossy formats count a little less.
pub fn quality(f: &SharedFile, target: u32) -> f32 {
    if is_lossless(&f.ext) {
        return 1.0;
    }
    let b = kbps(f);
    if b == 0 {
        return 0.5;
    }
    let q = (b as f32 / target as f32).min(1.0);
    if f.ext == "mp3" { q } else { q * 0.85 }
}

/// One file for a track, with its scores.
#[derive(Clone, Debug)]
pub struct Cand {
    pub id: u32,
    pub user: String,
    pub file: SharedFile,
    pub free: bool,
    pub speed: u32,
    pub queue: u32,
    pub fit: u32,
    pub score: f32,
    pub failed: bool,
}

impl Cand {
    /// Seconds to download at the peer's speed (a slow floor when it is not known).
    pub fn eta(&self) -> u32 {
        let speed = self.speed.max(40_000) as u64;
        (self.file.size / speed) as u32
    }

    pub fn info(&self) -> CandInfo {
        let f = &self.file;
        let parts: Vec<&str> = f.filename.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
        let file = parts.last().copied().unwrap_or("").to_string();
        let folder = if parts.len() >= 3 { format!("{}/{}", parts[parts.len() - 3], parts[parts.len() - 2]) } else if parts.len() == 2 { parts[0].to_string() } else { String::new() };
        CandInfo {
            id: self.id,
            user: self.user.clone(),
            file,
            folder,
            ext: f.ext.clone(),
            size: f.size,
            bitrate: kbps(f),
            duration: f.duration.unwrap_or(0),
            sample_rate: f.sample_rate.unwrap_or(0),
            bit_depth: f.bit_depth.unwrap_or(0),
            free: self.free,
            speed: self.speed,
            queue: self.queue,
            fit: self.fit,
            score: (self.score * 10.0).round() / 10.0,
            eta: if self.speed > 0 { self.eta() } else { 0 },
            failed: self.failed,
        }
    }
}

/// The rank score: the fit first, then the sound, then how soon and how fast it comes.
pub fn score(fit: u32, f: &SharedFile, free: bool, speed: u32, queue: u32, target: u32) -> f32 {
    let mut s = fit as f32;
    s += 40.0 * quality(f, target);
    // An mp3 at the target bitrate is copied as it is: no conversion.
    if f.ext == "mp3" && kbps(f) == target {
        s += 5.0;
    }
    if free {
        s += 25.0;
    } else {
        // Every upload in the queue is a wait of a minute or more.
        s -= 10.0 + (queue.min(60) as f32) * 0.8;
    }
    // Time to download: a big lossless file from a slow peer takes long.
    let eta = f.size as f32 / speed.max(40_000) as f32;
    s -= (eta / 4.0).min(30.0);
    // A known fast peer.
    s += (speed as f32 / 200_000.0).min(8.0);
    s
}

/// The files of the answers that may be the track, best first, without repeats. `next_id`
/// gives each new file an id; `known` keeps the ids of files seen before.
pub struct Ranker {
    pub track: Track,
    pub target: u32,
    pub cands: Vec<Cand>,
    next_id: u32,
    /// What the app window was last sent, and when (see `to_send`).
    sent: Vec<CandInfo>,
    sent_at: Option<std::time::Instant>,
    sent_settled: bool,
}

impl Ranker {
    pub fn new(track: Track, target: u32) -> Ranker {
        Ranker { track, target, cands: Vec::new(), next_id: 0, sent: Vec::new(), sent_at: None, sent_settled: false }
    }

    /// Adds a peer's answer. True when a new file fits.
    pub fn add(&mut self, r: &SearchResponse) -> bool {
        let mut added = false;
        for f in &r.files {
            if !is_audio(&f.ext) || f.size < 500_000 {
                continue;
            }
            let fit = fit(&self.track, f);
            if fit < 50 {
                continue;
            }
            if self.cands.iter().any(|c| c.user == r.user && c.file.filename == f.filename) {
                continue;
            }
            let score = score(fit, f, r.free_slot, r.avg_speed, r.queue_len, self.target);
            self.cands.push(Cand { id: self.next_id, user: r.user.clone(), file: f.clone(), free: r.free_slot, speed: r.avg_speed, queue: r.queue_len, fit, score, failed: false });
            self.next_id += 1;
            added = true;
        }
        if added {
            self.sort();
        }
        added
    }

    fn sort(&mut self) {
        self.cands.sort_by(|a, b| a.failed.cmp(&b.failed).then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)));
    }

    pub fn mark_failed(&mut self, id: u32) {
        if let Some(c) = self.cands.iter_mut().find(|c| c.id == id) {
            c.failed = true;
        }
        self.sort();
    }

    pub fn get(&self, id: u32) -> Option<&Cand> {
        self.cands.iter().find(|c| c.id == id)
    }

    /// Good candidates: likely the song, from a peer with a free slot.
    pub fn good(&self) -> usize {
        self.cands.iter().filter(|c| !c.failed && c.free && c.fit >= 75).count()
    }

    /// Very good candidates: surely the song, a free peer, the full sound, and a short download.
    pub fn excellent(&self) -> usize {
        self.cands.iter().filter(|c| !c.failed && c.free && c.fit >= 90 && quality(&c.file, self.target) >= 0.99 && c.eta() <= 60).count()
    }

    /// The list for the app window when it changed, at most every half second (always when
    /// `force`, or when it becomes settled).
    pub fn to_send(&mut self, settled: bool, force: bool) -> Option<Vec<CandInfo>> {
        let infos = self.infos(12);
        if infos == self.sent && settled == self.sent_settled {
            return None;
        }
        let recent = self.sent_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(500));
        if recent && !force && settled == self.sent_settled {
            return None;
        }
        self.sent = infos.clone();
        self.sent_at = Some(std::time::Instant::now());
        self.sent_settled = settled;
        Some(infos)
    }

    /// The first `n`, for the app window.
    pub fn infos(&self, n: usize) -> Vec<CandInfo> {
        self.cands.iter().take(n).map(Cand::info).collect()
    }

    /// The next one to try: the best that did not fail.
    pub fn next(&self) -> Option<&Cand> {
        self.cands.iter().find(|c| !c.failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(a: &str, t: &str, d: u32) -> Track {
        Track { artist: a.into(), title: t.into(), duration: d, ..Default::default() }
    }

    fn file(name: &str, size: u64, br: Option<u32>, dur: Option<u32>) -> SharedFile {
        let ext = name.rsplit('.').next().unwrap().to_lowercase();
        SharedFile { filename: name.into(), size, ext, bitrate: br, duration: dur, ..Default::default() }
    }

    #[test]
    fn normalizes() {
        assert_eq!(norm("  Beyoncé — Halo (Live!) "), "beyonce halo live");
        assert_eq!(base_title("Halo (feat. X) - Remastered"), "Halo");
        assert_eq!(main_artist("Calvin Harris feat. Rihanna"), "Calvin Harris");
        assert_eq!(stem(r"@@music\Daft Punk\Discovery\01 - One More Time.flac"), "01 - One More Time");
    }

    #[test]
    fn fits_the_same_song_not_a_remix() {
        let t = track("Daft Punk", "One More Time", 320);
        let good = file(r"music\Daft Punk\Discovery\01 - One More Time.mp3", 12_000_000, Some(320), Some(321));
        let remix = file(r"music\Daft Punk\01 - One More Time (Remix).mp3", 12_000_000, Some(320), Some(321));
        let long = file(r"music\Daft Punk\01 - One More Time.mp3", 12_000_000, Some(320), Some(600));
        let other = file(r"music\Daft Punk\02 - Aerodynamic.mp3", 12_000_000, Some(320), Some(320));
        assert!(fit(&t, &good) >= 95, "{}", fit(&t, &good));
        assert!(fit(&t, &remix) < 75);
        assert!(fit(&t, &long) < 75);
        assert!(fit(&t, &other) < 50);
        // A remix that is wanted is not a penalty.
        let tr = track("Daft Punk", "One More Time (Remix)", 321);
        assert!(fit(&tr, &remix) >= 90);
    }

    #[test]
    fn quality_never_counts_low_files_as_good() {
        let flac = file("a.flac", 30_000_000, None, Some(200));
        let mp3_320 = file("a.mp3", 8_000_000, Some(320), Some(200));
        let mp3_128 = file("a.mp3", 3_200_000, Some(128), Some(200));
        let m4a = file("a.m4a", 6_000_000, Some(256), Some(200));
        assert_eq!(quality(&flac, 320), 1.0);
        assert_eq!(quality(&mp3_320, 320), 1.0);
        assert!(quality(&mp3_128, 320) < 0.5);
        assert!(quality(&m4a, 320) < quality(&mp3_320, 320));
        assert_eq!(quality(&mp3_128, 128), 1.0);
        // No bitrate: from size and length.
        assert_eq!(kbps(&file("a.mp3", 8_000_000, None, Some(200))), 320);
    }

    #[test]
    fn a_fast_free_peer_beats_a_queued_one_and_failures_go_last() {
        let t = track("Daft Punk", "One More Time", 320);
        let mut r = Ranker::new(t, 320);
        let f = file(r"music\Daft Punk\01 - One More Time.mp3", 12_800_000, Some(320), Some(320));
        r.add(&SearchResponse { user: "queued".into(), free_slot: false, avg_speed: 2_000_000, queue_len: 30, files: vec![f.clone()] });
        r.add(&SearchResponse { user: "free".into(), free_slot: true, avg_speed: 900_000, queue_len: 0, files: vec![f.clone()] });
        r.add(&SearchResponse { user: "slowflac".into(), free_slot: true, avg_speed: 60_000, queue_len: 0, files: vec![file(r"x\Daft Punk - One More Time.flac", 40_000_000, None, Some(320))] });
        r.add(&SearchResponse { user: "junk".into(), free_slot: true, avg_speed: 900_000, queue_len: 0, files: vec![file(r"x\cover.jpg", 900_000, None, None), file(r"x\other.mp3", 9_000_000, Some(320), Some(200))] });
        let order: Vec<&str> = r.cands.iter().map(|c| c.user.as_str()).collect();
        assert_eq!(order, ["free", "slowflac", "queued"]);
        let id = r.next().unwrap().id;
        r.mark_failed(id);
        assert_eq!(r.next().unwrap().user, "slowflac");
        assert_eq!(r.cands.last().unwrap().user, "free");
        assert_eq!(r.good(), 1);
        let info = r.infos(10);
        assert_eq!(info.len(), 3);
        assert_eq!(info[0].folder, "x");
        assert_eq!(info[2].folder, "music/Daft Punk");
        assert!(info[2].failed);
    }

    #[test]
    fn queries_use_the_main_artist_and_the_base_title() {
        let t = track("Calvin Harris, Rihanna", "This Is What You Came For (feat. Rihanna)", 222);
        assert_eq!(queries(&t), ["calvin harris this is what you came for", "this is what you came for"]);
        let t = track("Enya", "Orinoco Flow - Remastered", 266);
        assert_eq!(queries(&t), ["enya orinoco flow", "orinoco flow"]);
        assert_eq!(queries(&track("ABBA", "SOS", 200)), ["abba sos"]);
    }
}
