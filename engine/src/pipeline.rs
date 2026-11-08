//! One run: read the Spotify list, then for each track at the same time: search Soulseek, rank
//! the answers, download the best file (the next one when it fails or is too slow), convert it
//! to mp3 at once, and fall back to YouTube Music when Soulseek has nothing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::convert::{self, Tags, Tools, safe_name};
use crate::events::{Emitter, Event, TrackInfo};
use crate::rank::{self, Ranker};
use crate::slsk::{self, Client, ConnectError, Progress};
use crate::spotify::{self, Track};
use crate::youtube::YtDlp;

pub struct Options {
    pub input: String,
    pub out_dir: PathBuf,
    pub bitrate: u32,
    pub fallback: bool,
    /// The Soulseek account, or None for YouTube Music only.
    pub account: Option<(String, String)>,
    pub server: String,
    /// Wait for the user's pick for each track.
    pub manual: bool,
    /// Wait for `Control::Only` after the collection event.
    pub ask: bool,
    pub cover_out: Option<PathBuf>,
    pub limit: usize,
    pub ffmpeg: String,
    pub ffprobe: String,
    pub ytdlp: String,
    /// Downloads at the same time.
    pub parallel: usize,
}

/// Messages from the app window (stdin lines).
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// Download only these tracks (None: all).
    Only(Option<Vec<usize>>),
    /// End the downloads; keep and convert what is finished.
    Stop,
    /// Download candidate `cand` for track `i`, or (None) take it from YouTube Music.
    Pick { i: usize, cand: Option<u32> },
}

/// What a pick asks for.
type Pick = Option<u32>;

struct Ctx {
    o: Options,
    emit: Emitter,
    client: Option<Client>,
    tools: Tools,
    yt: YtDlp,
    downloads: Semaphore,
    converts: Semaphore,
    youtube: Semaphore,
    /// Ends the searches and downloads (STOP); conversions still finish.
    stop: CancellationToken,
    work: PathBuf,
    files: AtomicUsize,
    missing: AtomicUsize,
}

/// How a track ended.
#[derive(Debug, PartialEq)]
enum End {
    Done,
    Missing,
    Stopped,
}

pub async fn run(o: Options, emit: Emitter, mut controls: mpsc::UnboundedReceiver<Control>) -> Result<()> {
    let started = Instant::now();
    tokio::fs::create_dir_all(&o.out_dir).await?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let work = o.out_dir.join(".work").join(format!("run-{stamp}"));
    tokio::fs::create_dir_all(&work).await?;

    // Sign in to Soulseek while Spotify answers.
    let login = o.account.clone().map(|(u, p)| {
        let server = o.server.clone();
        tokio::spawn(async move { Client::connect(&server, &u, &p).await })
    });

    // The controls: the answer to "which tracks", STOP, and picks.
    let stop = CancellationToken::new();
    let (only_tx, only_rx) = oneshot::channel::<Option<Vec<usize>>>();
    let picks: Arc<Mutex<Vec<Option<mpsc::UnboundedSender<Pick>>>>> = Arc::default();
    {
        let (stop, picks) = (stop.clone(), picks.clone());
        let emit = emit.clone();
        tokio::spawn(async move {
            let mut only_tx = Some(only_tx);
            while let Some(c) = controls.recv().await {
                match c {
                    Control::Only(v) => {
                        if let Some(tx) = only_tx.take() {
                            let _ = tx.send(v);
                        }
                    }
                    Control::Stop => {
                        if !stop.is_cancelled() {
                            emit.emit(Event::log("Stopping: converting the finished tracks..."));
                        }
                        stop.cancel();
                    }
                    Control::Pick { i, cand } => {
                        if let Some(Some(tx)) = picks.lock().unwrap().get(i) {
                            let _ = tx.send(cand);
                        }
                    }
                }
            }
        });
    }

    emit.emit(Event::phase("spotify"));
    emit.emit(Event::log("Reading Spotify metadata..."));
    let coll = spotify::extract(&o.input).await.map_err(|e| anyhow!("spotify: {e}"))?;
    let mut tracks = coll.tracks.clone();
    if o.limit > 0 && tracks.len() > o.limit {
        tracks.truncate(o.limit);
    }
    if tracks.is_empty() {
        return Err(anyhow!("no tracks found"));
    }
    emit.emit(Event::log(format!("Collection: {} ({} tracks)", coll.name, tracks.len())));
    emit.emit(Event {
        name: coll.name.clone(),
        kind: coll.kind.clone(),
        artist: coll.artist.clone(),
        cover: coll.cover.clone(),
        manual: o.manual,
        tracks: tracks.iter().map(|t| TrackInfo { artist: t.artist.clone(), title: t.title.clone(), duration: t.duration, album: t.album.clone(), id: t.id.clone() }).collect(),
        ..Event::new("collection")
    });
    if let (Some(out), false) = (o.cover_out.clone(), coll.cover.is_empty()) {
        let (url, emit) = (coll.cover.clone(), emit.clone());
        tokio::spawn(async move {
            if spotify::download_file(&url, &out).await.is_ok() {
                emit.emit(Event { path: out.to_string_lossy().into_owned(), ..Event::new("cover") });
            }
        });
    }

    let mut selected: Vec<usize> = (0..tracks.len()).collect();
    if o.ask {
        match only_rx.await {
            Ok(Some(only)) => {
                let mut seen = std::collections::HashSet::new();
                selected = only.into_iter().filter(|&i| i < tracks.len() && seen.insert(i)).collect();
            }
            Ok(None) => {}
            Err(_) => return Err(anyhow!("stopped")),
        }
    }
    if selected.is_empty() {
        emit.emit(Event::phase("done"));
        emit.emit(Event { name: coll.name.clone(), dir: o.out_dir.to_string_lossy().into_owned(), ..Event::new("result") });
        let _ = tokio::fs::remove_dir_all(&work).await;
        return Ok(());
    }

    // Soulseek, if the account works.
    let client = match login {
        None => {
            emit.emit(Event::log("No Soulseek account: using YouTube Music"));
            None
        }
        Some(h) => match h.await {
            Ok(Ok(c)) => {
                emit.emit(Event::log(format!("Signed in to Soulseek in {:.1} s", started.elapsed().as_secs_f32())));
                Some(c)
            }
            Ok(Err(ConnectError::Refused(a))) => {
                let m = format!("Soulseek login failed: {} {}", a.reason, a.detail).trim().to_string();
                emit.emit(Event { message: m.clone(), ..Event::new("login") });
                emit.emit(Event::log(m));
                None
            }
            Ok(Err(e)) => {
                emit.emit(Event::log(format!("{e}; using YouTube Music")));
                None
            }
            Err(e) => {
                emit.emit(Event::log(format!("Soulseek: {e}")));
                None
            }
        },
    };
    emit.emit(Event::phase(if client.is_some() { "soulseek" } else { "youtube" }));

    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let ctx = Arc::new(Ctx {
        tools: Tools { ffmpeg: o.ffmpeg.clone(), ffprobe: o.ffprobe.clone() },
        yt: YtDlp { bin: o.ytdlp.clone() },
        downloads: Semaphore::new(o.parallel.max(1)),
        converts: Semaphore::new((cpus / 2).clamp(2, 6)),
        youtube: Semaphore::new(2),
        stop: stop.clone(),
        work: work.clone(),
        files: AtomicUsize::new(0),
        missing: AtomicUsize::new(0),
        client,
        emit: emit.clone(),
        o,
    });

    let mut jobs = tokio::task::JoinSet::new();
    {
        let mut p = picks.lock().unwrap();
        p.resize(tracks.len(), None);
        for &i in &selected {
            let (tx, rx) = mpsc::unbounded_channel();
            p[i] = Some(tx);
            let (ctx, t) = (ctx.clone(), tracks[i].clone());
            jobs.spawn(async move { track_job(ctx, i, t, rx).await });
        }
    }
    while jobs.join_next().await.is_some() {}

    let files = ctx.files.load(Ordering::SeqCst);
    let missing = ctx.missing.load(Ordering::SeqCst);
    emit.emit(Event::log(format!("Done. {files} files in {} ({:.0} s)", ctx.o.out_dir.display(), started.elapsed().as_secs_f32())));
    emit.emit(Event::phase("done"));
    emit.emit(Event {
        files,
        count: selected.len(),
        missing,
        dir: ctx.o.out_dir.to_string_lossy().into_owned(),
        name: coll.name.clone(),
        stopped: stop.is_cancelled(),
        ..Event::new("result")
    });
    let _ = tokio::fs::remove_dir_all(&work).await;
    if let Some(p) = work.parent() {
        // Removes `.work` when no other run uses it.
        let _ = tokio::fs::remove_dir(p).await;
    }
    Ok(())
}

/// Searches Soulseek and sends every answer to `tx` until `window` is over.
async fn start_search(client: &Client, query: &str, window: Duration, stop: CancellationToken, tx: mpsc::UnboundedSender<slsk::SearchResponse>) -> Result<()> {
    let mut s = tokio::select! {
        r = client.search(query) => r?,
        _ = stop.cancelled() => return Ok(()),
    };
    tokio::spawn(async move {
        let end = tokio::time::sleep(window);
        tokio::pin!(end);
        loop {
            tokio::select! {
                r = s.rx.recv() => match r {
                    Some(r) => if tx.send(r).is_err() { break },
                    None => break,
                },
                _ = &mut end => break,
                _ = stop.cancelled() => break,
            }
        }
    });
    Ok(())
}

/// One track from start to end.
async fn track_job(ctx: Arc<Ctx>, i: usize, t: Track, mut picks: mpsc::UnboundedReceiver<Pick>) {
    let end = match ctx.client.clone() {
        Some(c) if c.online() => soulseek(&ctx, &c, i, &t, &mut picks).await,
        _ => None,
    };
    let end = match end {
        Some(e) => e,
        None if ctx.stop.is_cancelled() => End::Stopped,
        None if ctx.o.fallback => youtube(&ctx, i, &t).await,
        None => End::Missing,
    };
    if end == End::Missing {
        ctx.missing.fetch_add(1, Ordering::SeqCst);
        ctx.emit.emit(Event::track(i, "missing"));
    }
}

/// Sends the candidates of track `i`.
fn send_cands(ctx: &Ctx, i: usize, r: &Ranker, settled: bool) {
    ctx.emit.emit(Event { i: Some(i), cands: Some(r.infos(12)), settled, ..Event::new("candidates") });
}

/// The Soulseek part. None: not found here (try YouTube).
async fn soulseek(ctx: &Arc<Ctx>, c: &Client, i: usize, t: &Track, picks: &mut mpsc::UnboundedReceiver<Pick>) -> Option<End> {
    let manual = ctx.o.manual;
    ctx.emit.emit(Event::track(i, "searching"));
    let mut ranker = Ranker::new(t.clone(), ctx.o.bitrate);
    let (tx, mut answers) = mpsc::unbounded_channel();
    let queries = rank::queries(t);
    let t0 = Instant::now();
    // Answers keep coming in while the track waits or downloads: more choices if one fails.
    let window = Duration::from_secs(if manual { 30 } else { 20 });
    if let Err(e) = start_search(c, &queries[0], window, ctx.stop.clone(), tx.clone()).await {
        ctx.emit.emit(Event::log(format!("search failed: {e}")));
        return None;
    }
    let mut second = queries.get(1).cloned();
    let mut open = true;
    let mut last_sent = Instant::now() - Duration::from_secs(5);
    let mut dirty = false;
    // 1. Collect answers: stop early once there are enough good ones.
    loop {
        let el = t0.elapsed();
        let enough = (el >= Duration::from_millis(1500) && ranker.good() >= 5) || (el >= Duration::from_secs(4) && ranker.good() >= 1) || el >= Duration::from_secs(7);
        if enough && !(ranker.cands.is_empty() && second.is_some()) {
            break;
        }
        if el >= Duration::from_secs(4) && ranker.cands.is_empty() {
            if let Some(q) = second.take() {
                // Nothing yet: also search without the artist.
                let _ = start_search(c, &q, window, ctx.stop.clone(), tx.clone()).await;
            } else if el >= Duration::from_secs(9) {
                break;
            }
        }
        tokio::select! {
            r = answers.recv(), if open => match r {
                Some(r) => dirty |= ranker.add(&r),
                None => open = false,
            },
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            _ = ctx.stop.cancelled() => return Some(End::Stopped),
        }
        if dirty && last_sent.elapsed() >= Duration::from_millis(500) {
            send_cands(ctx, i, &ranker, false);
            last_sent = Instant::now();
            dirty = false;
        }
    }
    drop(tx);
    send_cands(ctx, i, &ranker, !manual);
    ctx.emit.emit(Event::log(format!("{} - {}: {} files in {:.1} s", t.artist, t.title, ranker.cands.len(), t0.elapsed().as_secs_f32())));
    if ranker.cands.is_empty() {
        ctx.emit.emit(Event::track(i, "failed"));
        return None;
    }

    // 2. Download: the best, then the next ones (or what the user picks).
    let mut pending: Option<Pick> = None;
    let mut tries = 0;
    loop {
        let pick: Option<u32> = if manual {
            // Wait for the user, while more answers come in.
            let p = match pending.take() {
                Some(p) => p,
                None => loop {
                    tokio::select! {
                        p = picks.recv() => break p.unwrap_or(None),
                        r = answers.recv(), if open => match r {
                            Some(r) => if ranker.add(&r) { send_cands(ctx, i, &ranker, false) },
                            None => { open = false; send_cands(ctx, i, &ranker, true) },
                        },
                        _ = ctx.stop.cancelled() => return Some(End::Stopped),
                    }
                },
            };
            match p {
                Some(id) => Some(id),
                // "From YouTube".
                None => return None,
            }
        } else {
            while let Ok(p) = picks.try_recv() {
                pending = Some(p);
            }
            while let Ok(r) = answers.try_recv() {
                ranker.add(&r);
            }
            match pending.take() {
                Some(None) => return None,
                Some(Some(id)) => Some(id),
                None => ranker.next().map(|c| c.id),
            }
        };
        let Some(id) = pick else {
            ctx.emit.emit(Event::track(i, "failed"));
            return None;
        };
        let Some(cand) = ranker.get(id).cloned() else { continue };
        tries += 1;
        if !manual && tries > 6 {
            ctx.emit.emit(Event::track(i, "failed"));
            return None;
        }
        let others = ranker.cands.iter().filter(|x| !x.failed && x.id != id).count();
        match fetch(ctx, c, i, &cand, picks, others, (&mut answers, &mut open), &mut ranker).await {
            Fetch::Done(path) => {
                ctx.emit.emit(Event { peer: cand.user.clone(), source: "soulseek".into(), total: cand.file.size, bitrate: rank::kbps(&cand.file), cand: Some(id), ..Event::track(i, "done") });
                return Some(finish(ctx, i, t, &path).await);
            }
            Fetch::Switch(p) => {
                pending = Some(p);
            }
            Fetch::Failed(why) => {
                ctx.emit.emit(Event::log(format!("{} - {}: {} failed: {why}", t.artist, t.title, cand.user)));
                ranker.mark_failed(id);
                send_cands(ctx, i, &ranker, !manual || !open);
                ctx.emit.emit(Event::track(i, "searching"));
            }
            Fetch::Stopped => return Some(End::Stopped),
        }
    }
}

enum Fetch {
    Done(PathBuf),
    /// The user picked another file (or YouTube) while this one downloaded.
    Switch(Pick),
    Failed(String),
    Stopped,
}

/// Downloads one candidate, with a watchdog: it must start soon, and not be much too slow when
/// there are other choices.
#[allow(clippy::too_many_arguments)]
async fn fetch(ctx: &Arc<Ctx>, c: &Client, i: usize, cand: &rank::Cand, picks: &mut mpsc::UnboundedReceiver<Pick>, others: usize, answers: (&mut mpsc::UnboundedReceiver<slsk::SearchResponse>, &mut bool), ranker: &mut Ranker) -> Fetch {
    let (answers, open) = answers;
    let manual = ctx.o.manual;
    // A download slot; a pick while waiting for one counts.
    let _permit = tokio::select! {
        p = ctx.downloads.acquire() => match p { Ok(p) => p, Err(_) => return Fetch::Stopped },
        p = picks.recv() => return Fetch::Switch(p.unwrap_or(None)),
        _ = ctx.stop.cancelled() => return Fetch::Stopped,
    };
    let ext = if cand.file.ext.is_empty() { "mp3".to_string() } else { cand.file.ext.clone() };
    let path = ctx.work.join("soulseek").join(format!("{i}-{}.{ext}", cand.id));
    if let Some(d) = path.parent() {
        let _ = tokio::fs::create_dir_all(d).await;
    }
    ctx.emit.emit(Event {
        peer: cand.user.clone(),
        source: "soulseek".into(),
        total: cand.file.size,
        bitrate: rank::kbps(&cand.file),
        path: path.to_string_lossy().into_owned(),
        format: ext.clone(),
        cand: Some(cand.id),
        ..Event::track(i, "downloading")
    });
    let cancel = ctx.stop.child_token();
    let (ptx, mut prog) = mpsc::unbounded_channel::<Progress>();
    let on: slsk::OnProgress = Arc::new(move |p| {
        let _ = ptx.send(p);
    });
    let dl = {
        let (c, user, file, size, path, cancel) = (c.clone(), cand.user.clone(), cand.file.filename.clone(), cand.file.size, path.clone(), cancel.clone());
        tokio::spawn(async move { c.download(&user, &file, size, &path, on, cancel).await })
    };
    tokio::pin!(dl);
    let t0 = Instant::now();
    let mut started: Option<Instant> = None;
    let mut queued = false;
    let (mut bytes, mut last_bytes_at, mut sent_at) = (0u64, Instant::now(), Instant::now() - Duration::from_secs(1));
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let fail = |cancel: &CancellationToken, why: &str| {
        cancel.cancel();
        Fetch::Failed(why.to_string())
    };
    loop {
        tokio::select! {
            r = &mut dl => {
                return match r {
                    Ok(Ok(n)) => {
                        ctx.emit.emit(Event { i: Some(i), bytes: n, total: cand.file.size, ..Event::new("bytes") });
                        Fetch::Done(path)
                    }
                    Ok(Err(slsk::DownloadError::Cancelled)) if ctx.stop.is_cancelled() => Fetch::Stopped,
                    Ok(Err(e)) => Fetch::Failed(e.to_string()),
                    Err(e) => Fetch::Failed(e.to_string()),
                };
            }
            Some(p) = prog.recv() => match p {
                Progress::Requested => {}
                Progress::Queued(place) => {
                    queued = true;
                    ctx.emit.emit(Event::log(format!("{}: queued{}", cand.user, place.map(|p| format!(" (place {p})")).unwrap_or_default())));
                }
                Progress::Started(_) => {
                    started = Some(Instant::now());
                    last_bytes_at = Instant::now();
                }
                Progress::Bytes(n) => {
                    if n > bytes {
                        last_bytes_at = Instant::now();
                    }
                    bytes = n;
                    if sent_at.elapsed() >= Duration::from_millis(250) {
                        sent_at = Instant::now();
                        ctx.emit.emit(Event { i: Some(i), bytes, total: cand.file.size, ..Event::new("bytes") });
                    }
                }
            },
            p = picks.recv() => {
                let p = p.unwrap_or(None);
                if p == Some(cand.id) {
                    continue;
                }
                cancel.cancel();
                return Fetch::Switch(p);
            }
            r = answers.recv(), if *open => match r {
                Some(r) => if ranker.add(&r) {
                    send_cands(ctx, i, ranker, false);
                },
                None => *open = false,
            },
            _ = ctx.stop.cancelled() => {
                cancel.cancel();
                return Fetch::Stopped;
            }
            _ = tick.tick() => {
                let waited = t0.elapsed().as_secs();
                match started {
                    None => {
                        // It must start: soon from a free slot, a little later from a queue.
                        let limit = if manual { 120 } else if queued { 25 } else { 15 };
                        if waited > limit && (others > 0 || manual || waited > 60) {
                            return fail(&cancel, if queued { "still queued" } else { "did not start" });
                        }
                    }
                    Some(s) => {
                        if last_bytes_at.elapsed() > Duration::from_secs(30) {
                            return fail(&cancel, "stalled");
                        }
                        // Much too slow, and there is something else to try.
                        let secs = s.elapsed().as_secs_f64();
                        if !manual && others > 0 && secs > 15.0 {
                            let rate = bytes as f64 / secs;
                            let left = cand.file.size.saturating_sub(bytes) as f64 / rate.max(1.0);
                            if rate < 25_000.0 && left > 90.0 {
                                return fail(&cancel, &format!("too slow ({:.0} KB/s)", rate / 1000.0));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Converts the downloaded file into the output folder.
async fn finish(ctx: &Arc<Ctx>, i: usize, t: &Track, src: &Path) -> End {
    let _permit = ctx.converts.acquire().await;
    let base = {
        let b = safe_name(&format!("{} - {}", t.artist, t.title));
        if b.is_empty() { "track".to_string() } else { b }
    };
    let tags = Tags { artist: t.artist.clone(), title: t.title.clone(), album: t.album.clone() };
    match convert::to_mp3(&ctx.tools, src, &ctx.o.out_dir, &base, ctx.o.bitrate, &tags).await {
        Ok((dest, kbps)) => {
            ctx.files.fetch_add(1, Ordering::SeqCst);
            ctx.emit.emit(Event { i: Some(i), path: dest.to_string_lossy().into_owned(), bitrate: kbps, ..Event::new("file") });
            End::Done
        }
        Err(e) => {
            ctx.emit.emit(Event::log(format!("{} - {}: conversion failed: {e}", t.artist, t.title)));
            End::Missing
        }
    }
}

/// The YouTube Music fallback.
async fn youtube(ctx: &Arc<Ctx>, i: usize, t: &Track) -> End {
    let _permit = tokio::select! {
        p = ctx.youtube.acquire() => p,
        _ = ctx.stop.cancelled() => return End::Stopped,
    };
    ctx.emit.emit(Event { source: "youtube".into(), ..Event::track(i, "yt_search") });
    let found = tokio::select! {
        r = ctx.yt.search(&t.artist, &t.title, t.duration) => r,
        _ = ctx.stop.cancelled() => return End::Stopped,
    };
    let best = match found {
        Ok(c) if !c.is_empty() && c[0].score >= 6.0 => c[0].clone(),
        Ok(c) => {
            ctx.emit.emit(Event::log(format!("{} - {}: no good YouTube match (best {:.0})", t.artist, t.title, c.first().map_or(0.0, |c| c.score))));
            return End::Missing;
        }
        Err(e) => {
            ctx.emit.emit(Event::log(format!("{} - {}: YouTube search failed: {e}", t.artist, t.title)));
            return End::Missing;
        }
    };
    ctx.emit.emit(Event { source: "youtube".into(), peer: "YouTube Music".into(), url: best.url.clone(), ..Event::track(i, "yt_download") });
    let dir = ctx.work.join("youtube");
    let name = i.to_string();
    let got = tokio::select! {
        r = ctx.yt.download(&best, &dir, &name) => r,
        _ = ctx.stop.cancelled() => return End::Stopped,
    };
    match got {
        Ok(path) => {
            let total = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            ctx.emit.emit(Event { source: "youtube".into(), peer: "YouTube Music".into(), url: best.url.clone(), total, ..Event::track(i, "done") });
            finish(ctx, i, t, &path).await
        }
        Err(e) => {
            ctx.emit.emit(Event::log(format!("{} - {}: YouTube download failed: {e}", t.artist, t.title)));
            End::Missing
        }
    }
}
