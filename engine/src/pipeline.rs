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
    session: Session,
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
    /// Peers that did not start a download in this run (queued or no answer): tried last.
    stuck: Mutex<std::collections::HashSet<String>>,
    /// Tracks whose search waits for the server's search limit.
    waiting_search: AtomicUsize,
    /// The run has more tracks than the server's search limit allows at once (34): every
    /// search counts, so no second search for a track.
    long_list: bool,
}

/// The Soulseek session, which can be replaced after a reconnect.
#[derive(Default)]
struct Session {
    client: Mutex<Option<Client>>,
    changed: tokio::sync::Notify,
    /// No account, or it was refused: there will be no session.
    never: std::sync::atomic::AtomicBool,
}

impl Session {
    fn get(&self) -> Option<Client> {
        self.client.lock().unwrap().clone()
    }

    fn set(&self, c: Client) {
        *self.client.lock().unwrap() = Some(c);
        self.changed.notify_waiters();
    }

    /// The session when it is online, waiting up to `max` for a reconnect.
    async fn ready(&self, stop: &CancellationToken, max: Duration) -> Option<Client> {
        let end = tokio::time::Instant::now() + max;
        loop {
            if self.never.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(c) = self.get().filter(|c| c.online()) {
                return Some(c);
            }
            tokio::select! {
                _ = self.changed.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                _ = tokio::time::sleep_until(end) => return None,
                _ = stop.cancelled() => return None,
            }
        }
    }
}

impl Ctx {
    /// A note for the app window's log: `code` says what happened (the window words it for
    /// people), `line` is the same in plain English for the technical log.
    fn note(&self, code: &str, level: &str, i: Option<usize>, peer: &str, message: &str, count: usize, secs: u32, line: String) {
        self.emit.emit(Event { code: code.into(), level: level.into(), i, peer: peer.into(), message: message.into(), count, secs, line, ..Event::new("note") });
    }
}

/// How a track ended.
#[derive(Debug, PartialEq)]
enum End {
    Done,
    Missing,
    Stopped,
}

pub async fn run(o: Options, emit: Emitter, controls: mpsc::UnboundedReceiver<Control>) -> Result<()> {
    tokio::fs::create_dir_all(&o.out_dir).await?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let work = o.out_dir.join(".work").join(format!("run-{stamp}"));
    tokio::fs::create_dir_all(&work).await?;
    let r = run_in(o, emit, controls, work.clone()).await;
    // The work folder goes, also after an error.
    let _ = tokio::fs::remove_dir_all(&work).await;
    if let Some(p) = work.parent() {
        // Removes `.work` when no other run uses it.
        let _ = tokio::fs::remove_dir(p).await;
    }
    r
}

async fn run_in(o: Options, emit: Emitter, mut controls: mpsc::UnboundedReceiver<Control>, work: PathBuf) -> Result<()> {
    let started = Instant::now();

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
                            emit.emit(Event { code: "stopping".into(), level: "info".into(), line: "Stopping: converting the finished tracks...".into(), ..Event::new("note") });
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
        return Ok(());
    }

    // Soulseek, if the account works.
    let note = |code: &str, level: &str, message: &str, line: String| {
        emit.emit(Event { code: code.into(), level: level.into(), message: message.into(), line, ..Event::new("note") });
    };
    let session = Session::default();
    let mut retry = false;
    match login {
        None => {
            session.never.store(true, Ordering::SeqCst);
            note("no_account", "warn", "", "No Soulseek account: using YouTube Music".into());
        }
        Some(h) => match h.await {
            Ok(Ok(c)) => {
                note("signed_in", "good", "", format!("Signed in to Soulseek in {:.1} s", started.elapsed().as_secs_f32()));
                session.set(c);
                retry = true;
            }
            Ok(Err(ConnectError::Refused(a))) => {
                let m = format!("Soulseek login failed: {} {}", a.reason, a.detail).trim().to_string();
                emit.emit(Event { message: m.clone(), ..Event::new("login") });
                session.never.store(true, Ordering::SeqCst);
                note("login_failed", "bad", &a.reason, m);
            }
            Ok(Err(e)) => {
                note("offline", "bad", &e.to_string(), format!("{e}; trying again"));
                retry = true;
            }
            Err(e) => {
                session.never.store(true, Ordering::SeqCst);
                note("offline", "bad", &e.to_string(), format!("Soulseek: {e}"));
            }
        },
    }
    emit.emit(Event::phase(if session.get().is_some() { "soulseek" } else { "youtube" }));

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
        stuck: Mutex::default(),
        waiting_search: AtomicUsize::new(0),
        long_list: selected.len() > 30,
        session,
        emit: emit.clone(),
        o,
    });
    // Keeps the session up: signs in again when the server connection drops.
    let watch = CancellationToken::new();
    if retry {
        tokio::spawn(keep_online(ctx.clone(), watch.clone()));
    }
    tokio::spawn(watch_search_limit(ctx.clone(), watch.clone()));

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
    watch.cancel();

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
    Ok(())
}

/// Signs in again when the server connection drops. After "relogged" (the account signed in
/// somewhere else) it waits a minute first, so two apps do not take the account from each
/// other in a loop.
async fn keep_online(ctx: Arc<Ctx>, done: CancellationToken) {
    let Some((user, pass)) = ctx.o.account.clone() else { return };
    let mut fails = 0u32;
    loop {
        // Wait for the session to drop.
        loop {
            if ctx.session.get().is_none_or(|c| !c.online()) {
                break;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                _ = done.cancelled() => return,
                _ = ctx.stop.cancelled() => return,
            }
        }
        let reason = ctx.session.get().map(|c| c.offline_reason()).unwrap_or_default();
        let relogged = reason == "relogged";
        let wait = if relogged { 60 } else { [2u32, 5, 10, 30, 60][fails.min(4) as usize] };
        if fails == 0 && !reason.is_empty() {
            let line = if relogged { "Soulseek: the account signed in somewhere else".to_string() } else { format!("Soulseek disconnected: {reason}") };
            ctx.note("offline", "bad", None, "", &reason, 0, 0, line);
        }
        ctx.note("reconnecting", "warn", None, "", "", 0, wait, format!("Signing in to Soulseek again in {wait} s"));
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(wait as u64)) => {}
            _ = done.cancelled() => return,
            _ = ctx.stop.cancelled() => return,
        }
        match Client::connect(&ctx.o.server, &user, &pass).await {
            Ok(c) => {
                fails = 0;
                ctx.session.set(c);
                ctx.note("reconnected", "good", None, "", "", 0, 0, "Signed in to Soulseek again".into());
            }
            Err(ConnectError::Refused(a)) => {
                ctx.session.never.store(true, Ordering::SeqCst);
                ctx.session.changed.notify_waiters();
                ctx.note("login_failed", "bad", None, "", &a.reason, 0, 0, format!("Soulseek login failed: {} {}", a.reason, a.detail));
                return;
            }
            Err(e) => {
                fails += 1;
                ctx.note("offline", "bad", None, "", &e.to_string(), 0, 0, format!("{e}"));
            }
        }
    }
}

/// Tells the app window when searches wait for the server's limit (34 every 220 s), once each
/// time the wait starts.
async fn watch_search_limit(ctx: Arc<Ctx>, done: CancellationToken) {
    let mut told = false;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = done.cancelled() => return,
            _ = ctx.stop.cancelled() => return,
        }
        let waiting = ctx.waiting_search.load(Ordering::SeqCst);
        let wait = slsk::search_wait();
        if waiting > 0 && wait > Duration::from_secs(2) {
            if !told {
                told = true;
                let secs = wait.as_secs() as u32;
                ctx.note("search_limit", "warn", None, "", "", waiting, secs, format!("Soulseek allows 34 searches every 3:40: {waiting} songs wait for their search (the next in {secs} s)"));
            }
        } else if waiting == 0 {
            told = false;
        }
    }
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
    let end = match ctx.session.ready(&ctx.stop, Duration::from_secs(90)).await {
        Some(c) => soulseek(&ctx, c, i, &t, &mut picks).await,
        None => None,
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
        ctx.note("missing", "bad", Some(i), "", "", 0, 0, format!("{} - {}: not found anywhere", t.artist, t.title));
    }
}

/// Sends the candidates of track `i` when they changed (at most every half second unless `force`).
fn send_cands(ctx: &Ctx, i: usize, r: &mut Ranker, settled: bool, force: bool) {
    if let Some(cands) = r.to_send(settled, force) {
        ctx.emit.emit(Event { i: Some(i), cands: Some(cands), settled, ..Event::new("candidates") });
    }
}

/// The Soulseek part. None: not found here (try YouTube).
async fn soulseek(ctx: &Arc<Ctx>, client: Client, i: usize, t: &Track, picks: &mut mpsc::UnboundedReceiver<Pick>) -> Option<End> {
    let mut client = client;
    let c = &mut client;
    let manual = ctx.o.manual;
    ctx.emit.emit(Event::track(i, "searching"));
    let mut ranker = Ranker::new(t.clone(), ctx.o.bitrate);
    let (tx, mut answers) = mpsc::unbounded_channel();
    let queries = rank::queries(t);
    // Answers keep coming in while the track waits or downloads: more choices if one fails.
    let window = Duration::from_secs(if manual { 30 } else { 20 });
    // This can wait a long time for the server's search limit; the clock starts when it is sent.
    ctx.waiting_search.fetch_add(1, Ordering::SeqCst);
    let mut sent = start_search(c, &queries[0], window, ctx.stop.clone(), tx.clone()).await;
    if sent.is_err() {
        // The session dropped: wait for the reconnect, then once more.
        if let Some(n) = ctx.session.ready(&ctx.stop, Duration::from_secs(120)).await {
            *c = n;
            sent = start_search(c, &queries[0], window, ctx.stop.clone(), tx.clone()).await;
        }
    }
    ctx.waiting_search.fetch_sub(1, Ordering::SeqCst);
    if let Err(e) = sent {
        ctx.emit.emit(Event::log(format!("search failed: {e}")));
        return None;
    }
    if ctx.stop.is_cancelled() {
        return Some(End::Stopped);
    }
    let mut t0 = Instant::now();
    // A second search (without the artist) only for short lists: on long ones it takes the
    // place of another song's first search.
    let mut second = if ctx.long_list { None } else { queries.get(1).cloned() };
    let mut open = true;
    // 1. Collect answers. Most come in the first seconds; stop early once there are enough
    // very good ones, else take what there is.
    loop {
        let el = t0.elapsed();
        let enough = (el >= Duration::from_millis(2500) && ranker.excellent() >= 3) || (el >= Duration::from_secs(5) && ranker.good() >= 1) || el >= Duration::from_secs(8);
        if enough && !(ranker.cands.is_empty() && second.is_some()) {
            break;
        }
        if el >= Duration::from_secs(4) && ranker.cands.is_empty() {
            if let Some(q) = second.take() {
                // Nothing yet: also search without the artist (a new clock for it).
                let _ = start_search(c, &q, window, ctx.stop.clone(), tx.clone()).await;
                t0 = Instant::now();
                continue;
            } else if el >= Duration::from_secs(9) {
                break;
            }
        }
        tokio::select! {
            r = answers.recv(), if open => match r {
                Some(r) => { ranker.add(&r); }
                None => open = false,
            },
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
            _ = ctx.stop.cancelled() => return Some(End::Stopped),
        }
        send_cands(ctx, i, &mut ranker, false, false);
    }
    drop(tx);
    // What already came in counts.
    while let Ok(r) = answers.try_recv() {
        ranker.add(&r);
    }
    send_cands(ctx, i, &mut ranker, !manual, true);
    let line = format!("{} - {}: {} files in {:.1} s", t.artist, t.title, ranker.cands.len(), t0.elapsed().as_secs_f32());
    if ranker.cands.is_empty() {
        ctx.note("no_files", "warn", Some(i), "", "", 0, 0, line);
        ctx.emit.emit(Event::track(i, "failed"));
        return None;
    }
    ctx.note("found", "info", Some(i), "", "", ranker.cands.len(), t0.elapsed().as_secs() as u32, line);

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
                            Some(r) => { ranker.add(&r); }
                            None => { open = false; send_cands(ctx, i, &mut ranker, true, true) },
                        },
                        _ = tokio::time::sleep(Duration::from_millis(250)) => {}
                        _ = ctx.stop.cancelled() => return Some(End::Stopped),
                    }
                    send_cands(ctx, i, &mut ranker, !open, false);
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
                None => {
                    let stuck = ctx.stuck.lock().unwrap();
                    ranker.cands.iter().find(|c| !c.failed && !stuck.contains(&c.user)).or_else(|| ranker.next()).map(|c| c.id)
                }
            }
        };
        let Some(id) = pick else {
            ctx.emit.emit(Event::track(i, "failed"));
            return None;
        };
        let Some(cand) = ranker.get(id).cloned() else { continue };
        tries += 1;
        if !manual && tries > 6 {
            ctx.note("gave_up", "warn", Some(i), "", "", 6, 0, format!("{} - {}: 6 files failed, giving up on Soulseek", t.artist, t.title));
            ctx.emit.emit(Event::track(i, "failed"));
            return None;
        }
        let others = ranker.cands.iter().filter(|x| !x.failed && x.id != id).count();
        match fetch(ctx, &c.clone(), i, &cand, picks, others, (&mut answers, &mut open), &mut ranker).await {
            Fetch::Done(path, secs) => {
                // It sends after all (it was only busy with another of our files).
                ctx.stuck.lock().unwrap().remove(&cand.user);
                ctx.note("downloaded", "good", Some(i), &cand.user, "", 0, secs, format!("{} - {}: downloaded from {} in {secs} s", t.artist, t.title, cand.user));
                ctx.emit.emit(Event { peer: cand.user.clone(), source: "soulseek".into(), total: cand.file.size, bitrate: rank::kbps(&cand.file), cand: Some(id), ..Event::track(i, "done") });
                return Some(finish(ctx, i, t, &path).await);
            }
            Fetch::Switch(p) => {
                pending = Some(p);
            }
            Fetch::Failed(code, why) => {
                let line = format!("{} - {}: {} failed: {why}", t.artist, t.title, cand.user);
                if code == "connect" && !c.online() {
                    // Not the peer's fault: the session dropped. Try the same file once it is back.
                    ctx.emit.emit(Event::log(line));
                    ctx.emit.emit(Event::track(i, "searching"));
                    match ctx.session.ready(&ctx.stop, Duration::from_secs(150)).await {
                        Some(n) => {
                            *c = n;
                            tries -= 1;
                            pending = Some(Some(id));
                            continue;
                        }
                        None if ctx.stop.is_cancelled() => return Some(End::Stopped),
                        None => return None,
                    }
                }
                ctx.note("peer_failed", "warn", Some(i), &cand.user, code, 0, 0, line);
                ranker.mark_failed(id);
                if code == "queued" || code == "no_start" {
                    ctx.stuck.lock().unwrap().insert(cand.user.clone());
                }
                send_cands(ctx, i, &mut ranker, !manual || !open, true);
                ctx.emit.emit(Event::track(i, "searching"));
            }
            Fetch::Stopped => return Some(End::Stopped),
        }
    }
}

enum Fetch {
    /// The file, and the seconds it took.
    Done(PathBuf, u32),
    /// The user picked another file (or YouTube) while this one downloaded.
    Switch(Pick),
    /// A short code (queued, no_start, stalled, slow, faster, connect, denied, broken, disk) and
    /// the details.
    Failed(&'static str, String),
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
    let fail = |cancel: &CancellationToken, code: &'static str, why: &str| {
        cancel.cancel();
        Fetch::Failed(code, why.to_string())
    };
    loop {
        tokio::select! {
            r = &mut dl => {
                return match r {
                    Ok(Ok(n)) => {
                        ctx.emit.emit(Event { i: Some(i), bytes: n, total: cand.file.size, ..Event::new("bytes") });
                        Fetch::Done(path, t0.elapsed().as_secs() as u32)
                    }
                    Ok(Err(slsk::DownloadError::Cancelled)) if ctx.stop.is_cancelled() => Fetch::Stopped,
                    Ok(Err(e)) => {
                        let code = match &e {
                            slsk::DownloadError::NoConnection(_) => "connect",
                            slsk::DownloadError::Denied(_) => "denied",
                            slsk::DownloadError::Failed(_) => "broken",
                            slsk::DownloadError::Cancelled => "broken",
                            slsk::DownloadError::Io(_) => "disk",
                        };
                        Fetch::Failed(code, e.to_string())
                    }
                    Err(e) => Fetch::Failed("broken", e.to_string()),
                };
            }
            Some(p) = prog.recv() => match p {
                Progress::Requested => {}
                Progress::Queued(place) => {
                    queued = true;
                    ctx.note("queued", "info", Some(i), &cand.user, "", place.unwrap_or(0) as usize, 0, format!("{}: queued{}", cand.user, place.map(|p| format!(" (place {p})")).unwrap_or_default()));
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
                Some(r) => { ranker.add(&r); }
                None => *open = false,
            },
            _ = ctx.stop.cancelled() => {
                cancel.cancel();
                return Fetch::Stopped;
            }
            _ = tick.tick() => {
                send_cands(ctx, i, ranker, !manual || !*open, false);
                let waited = t0.elapsed().as_secs();
                match started {
                    None => {
                        // It must start: soon from a free slot, a little later from a queue.
                        let limit = if manual { 120 } else if queued { 20 } else { 15 };
                        if waited > limit && (others > 0 || manual || waited > 60) {
                            return if queued { fail(&cancel, "queued", "still queued") } else { fail(&cancel, "no_start", "did not start") };
                        }
                    }
                    Some(s) => {
                        if last_bytes_at.elapsed() > Duration::from_secs(30) {
                            return fail(&cancel, "stalled", "stalled");
                        }
                        // Much too slow, and there is something else to try.
                        let secs = s.elapsed().as_secs_f64();
                        let others = ranker.cands.iter().filter(|x| !x.failed && x.id != cand.id).count().max(others);
                        if !manual && others > 0 && secs > 15.0 {
                            let rate = bytes as f64 / secs;
                            let left = cand.file.size.saturating_sub(bytes) as f64 / rate.max(1.0);
                            if rate < 25_000.0 && left > 90.0 {
                                return fail(&cancel, "slow", &format!("too slow ({:.0} KB/s)", rate / 1000.0));
                            }
                            // A much faster file came in after this one started.
                            if left > 60.0 {
                                let better = ranker.cands.iter().filter(|x| !x.failed && x.id != cand.id && x.free && x.fit >= 90).map(|x| x.eta()).min();
                                if better.is_some_and(|eta| (eta as f64) * 3.0 < left) {
                                    return fail(&cancel, "faster", "a much faster file came in");
                                }
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
            ctx.note("convert_failed", "bad", Some(i), "", &e.to_string(), 0, 0, format!("{} - {}: conversion failed: {e}", t.artist, t.title));
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
            ctx.note("yt_no_match", "warn", Some(i), "", "", 0, 0, format!("{} - {}: no good YouTube match (best {:.0})", t.artist, t.title, c.first().map_or(0.0, |c| c.score)));
            return End::Missing;
        }
        Err(e) => {
            let code = if yt_blocked(&e.to_string()) { "yt_blocked" } else { "yt_failed" };
            ctx.note(code, "bad", Some(i), "", "", 0, 0, format!("{} - {}: YouTube search failed: {e}", t.artist, t.title));
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
            ctx.note("yt_done", "good", Some(i), "", "", 0, 0, format!("{} - {}: downloaded from YouTube Music", t.artist, t.title));
            finish(ctx, i, t, &path).await
        }
        Err(e) => {
            let code = if yt_blocked(&e.to_string()) { "yt_blocked" } else { "yt_failed" };
            ctx.note(code, "bad", Some(i), "", "", 0, 0, format!("{} - {}: YouTube download failed: {e}", t.artist, t.title));
            End::Missing
        }
    }
}

/// YouTube asks yt-dlp to prove it is not a bot (too many requests from this address).
fn yt_blocked(e: &str) -> bool {
    e.contains("not a bot") || e.contains("Sign in to confirm") || e.contains("HTTP Error 429")
}
