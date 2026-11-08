//! The live session: the server connection, the listen socket, peer ("P") connections,
//! file ("F") connections and the download steps.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::proto::{self, Init, PeerMsg};
use super::ratelimit::{self, Window};
use super::types::{DownloadError, LoginAnswer, Progress, SearchResponse};
use super::{ConnectError, OnProgress, Search};

/// Time for a direct TCP connection to a peer (and for the server to give its address).
const DIRECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time for a peer to connect to us after we asked the server to relay.
const INDIRECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Time for the first message on a new peer connection.
const INIT_TIMEOUT: Duration = Duration::from_secs(30);
/// A peer connection with no message for this long is closed.
const PEER_IDLE: Duration = Duration::from_secs(120);
/// The open-file limit the client asks for (see `fdlimit`).
const WANT_FILES: u64 = 4096;
/// How long a relayed connection request from a peer may wait for a free socket.
const OPEN_WAIT: Duration = Duration::from_secs(5);
/// Time to connect to a peer that sent a search answer through the server. Most connect in
/// well under a second; an unreachable one must not hold a socket for long.
const RELAYED_TIMEOUT: Duration = Duration::from_secs(5);

/// How many sockets we open at most for others (mostly search answers), from the process's
/// open-file limit. Half of it; the rest is for downloads, files and the engine.
fn others_limit(files: u64) -> usize {
    (files.saturating_sub(100) / 2).clamp(50, 1000) as usize
}
/// A file transfer with no data for this long has failed.
const FILE_IDLE: Duration = Duration::from_secs(90);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(20);
const PING_EVERY: Duration = Duration::from_secs(120);
const SEARCH_MAX: usize = 34;
const SEARCH_WINDOW: Duration = Duration::from_secs(220);
/// Ask for our place in the queue when no transfer has started after this time.
const PLACE_ASK_AFTER: Duration = Duration::from_secs(4);
const PROGRESS_EVERY: Duration = Duration::from_millis(150);

pub struct Inner {
    user: String,
    listen_port: u16,
    online: AtomicBool,
    server_tx: mpsc::UnboundedSender<Vec<u8>>,
    next_token: AtomicU32,
    next_id: AtomicU64,
    limiter: tokio::sync::Mutex<Window>,
    /// Search token → where its answers go.
    searches: Mutex<HashMap<u32, mpsc::UnboundedSender<SearchResponse>>>,
    /// The newest open P connection to each user.
    peers: Mutex<HashMap<String, PeerConn>>,
    /// Our ConnectToPeer tokens that wait for the peer's PierceFirewall.
    indirect: Mutex<HashMap<u32, oneshot::Sender<TcpStream>>>,
    /// Users we asked the server for an address.
    addr_waiters: Mutex<HashMap<String, Vec<oneshot::Sender<(Ipv4Addr, u16)>>>>,
    /// (user, filename) → the download that waits for it.
    downloads: Mutex<HashMap<(String, String), DownloadSlot>>,
    /// TransferRequest token → the download that accepted it, until its F connection comes.
    transfers: Mutex<HashMap<u32, TransferSlot>>,
    /// Cancelled when the server connection ends.
    server_gone: CancellationToken,
    /// Cancelled when the last `Client` is dropped.
    shutdown: CancellationToken,
    /// Permits for the sockets we open for others (see `others_limit`).
    others: Arc<Semaphore>,
}

#[derive(Clone)]
struct PeerConn {
    id: u64,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    closed: CancellationToken,
}

#[derive(Clone)]
struct DownloadSlot {
    id: u64,
    ev: mpsc::UnboundedSender<DlEvent>,
    files: mpsc::UnboundedSender<(TcpStream, u64)>,
}

struct TransferSlot {
    files: mpsc::UnboundedSender<(TcpStream, u64)>,
    size: u64,
}

enum DlEvent {
    Queued(Option<u32>),
    Accepted,
    Denied(String),
    Failed(String),
}

/// With SLSK_DEBUG=1 in the environment, the client writes its connection steps to stderr.
fn debug_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("SLSK_DEBUG").is_some_and(|v| v != "0"))
}

macro_rules! debug {
    ($($t:tt)*) => {
        if debug_on() {
            eprintln!("slsk: {}", format_args!($($t)*));
        }
    };
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A small random number without a new dependency.
fn random_u32() -> u32 {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut x = (nanos as u64) ^ ((std::process::id() as u64) << 32) ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x >> 16) as u32
}

/// Signs in on `stream` and reads the answer.
async fn login(stream: &mut TcpStream, user: &str, pass: &str) -> std::io::Result<LoginAnswer> {
    stream.write_all(&proto::login(user, pass)).await?;
    // The server can send other messages first; read until the login answer.
    loop {
        let (code, body) = proto::read_frame(stream, proto::MAX_SERVER_FRAME).await?;
        if code == proto::server::LOGIN {
            return proto::parse_login(&body).map_err(|_| proto::bad_data("bad login answer".into()));
        }
    }
}

async fn open_server(addr: &str, user: &str, pass: &str) -> Result<(TcpStream, LoginAnswer), String> {
    let mut stream = match timeout(LOGIN_TIMEOUT, TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => return Err(e.to_string()),
        Err(_) => return Err("timed out".into()),
    };
    let _ = stream.set_nodelay(true);
    match timeout(LOGIN_TIMEOUT, login(&mut stream, user, pass)).await {
        Ok(Ok(a)) => Ok((stream, a)),
        Ok(Err(e)) => Err(format!("no answer: {e}")),
        Err(_) => Err("no answer in time".into()),
    }
}

pub async fn check_login(addr: &str, user: &str, pass: &str) -> anyhow::Result<LoginAnswer> {
    check_name(user)?;
    if pass.is_empty() {
        anyhow::bail!("the password is empty");
    }
    let (_stream, answer) = open_server(addr, user, pass).await.map_err(|e| anyhow::anyhow!("cannot reach the Soulseek server: {e}"))?;
    Ok(answer)
}

/// The rules of Soulseek names (as in the Go backend).
fn check_name(name: &str) -> anyhow::Result<()> {
    let ok = !name.is_empty() && name.len() <= 30 && name.trim() == name && name.chars().all(|c| (' '..='~').contains(&c));
    if !ok {
        anyhow::bail!("the name must be 1 to 30 letters, numbers or symbols, with no spaces at the start or end");
    }
    Ok(())
}

async fn bind_listener() -> std::io::Result<(TcpListener, u16)> {
    if let Ok(l) = TcpListener::bind(("0.0.0.0", 2234)).await {
        return Ok((l, 2234));
    }
    let mut last = None;
    for _ in 0..20 {
        let port = 50000 + (random_u32() % 10001) as u16;
        match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(l) => return Ok((l, port)),
            Err(e) => last = Some(e),
        }
    }
    let l = TcpListener::bind(("0.0.0.0", 0)).await.map_err(|e| last.unwrap_or(e))?;
    let port = l.local_addr()?.port();
    Ok((l, port))
}

impl Inner {
    pub async fn connect(addr: &str, user: &str, pass: &str) -> Result<Arc<Inner>, ConnectError> {
        let (stream, answer) = open_server(addr, user, pass).await.map_err(ConnectError::Network)?;
        if !answer.ok {
            return Err(ConnectError::Refused(answer));
        }
        let (listener, listen_port) = bind_listener().await.map_err(|e| ConnectError::Network(format!("cannot open a listen port: {e}")))?;
        let files = super::fdlimit::raise(WANT_FILES);
        let others = others_limit(files);
        debug!("open-file limit {files}: up to {others} peer sockets for others");
        let (server_tx, server_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            user: user.to_string(),
            listen_port,
            online: AtomicBool::new(true),
            server_tx,
            next_token: AtomicU32::new(random_u32() & 0x3fff_ffff),
            next_id: AtomicU64::new(1),
            limiter: tokio::sync::Mutex::new(Window::new(SEARCH_MAX, SEARCH_WINDOW)),
            searches: Mutex::default(),
            peers: Mutex::default(),
            indirect: Mutex::default(),
            addr_waiters: Mutex::default(),
            downloads: Mutex::default(),
            transfers: Mutex::default(),
            server_gone: CancellationToken::new(),
            shutdown: CancellationToken::new(),
            others: Arc::new(Semaphore::new(others)),
        });
        inner.send_server(proto::set_wait_port(listen_port));
        inner.send_server(proto::set_status_online());
        inner.send_server(proto::shared_folders_files(0, 0));
        inner.send_server(proto::have_no_parent(true));
        let (rd, wr) = stream.into_split();
        tokio::spawn(inner.clone().server_writer(wr, server_rx));
        tokio::spawn(inner.clone().server_reader(rd));
        tokio::spawn(inner.clone().accept_loop(listener));
        Ok(inner)
    }

    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    pub fn online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    fn token(&self) -> u32 {
        self.next_token.fetch_add(1, Ordering::Relaxed)
    }

    fn id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn send_server(&self, msg: Vec<u8>) {
        let _ = self.server_tx.send(msg);
    }

    fn set_offline(&self) {
        self.online.store(false, Ordering::SeqCst);
        self.server_gone.cancel();
        // Waiters for the server fail at once.
        lock(&self.indirect).clear();
        lock(&self.addr_waiters).clear();
    }

    // ---- Server --------------------------------------------------------------------------

    async fn server_writer(self: Arc<Self>, mut wr: OwnedWriteHalf, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
        let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
        loop {
            let msg = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                _ = self.server_gone.cancelled() => break,
                m = rx.recv() => match m { Some(m) => m, None => break },
                _ = ping.tick() => proto::ping(),
            };
            if wr.write_all(&msg).await.is_err() {
                break;
            }
        }
        self.set_offline();
        let _ = wr.shutdown().await;
    }

    async fn server_reader(self: Arc<Self>, rd: OwnedReadHalf) {
        let mut rd = BufReader::new(rd);
        loop {
            let r = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                _ = self.server_gone.cancelled() => break,
                r = proto::read_frame(&mut rd, proto::MAX_SERVER_FRAME) => r,
            };
            let Ok((code, body)) = r else { break };
            match code {
                proto::server::RELOGGED => break,
                proto::server::GET_PEER_ADDRESS => {
                    if let Ok(a) = proto::parse_peer_address(&body) {
                        let waiters = lock(&self.addr_waiters).remove(&a.user).unwrap_or_default();
                        for w in waiters {
                            let _ = w.send((a.ip, a.port));
                        }
                    }
                }
                proto::server::CONNECT_TO_PEER => {
                    if let Ok(c) = proto::parse_connect_to_peer(&body) {
                        tokio::spawn(self.clone().answer_connect_to_peer(c));
                    }
                }
                proto::server::CANT_CONNECT_TO_PEER => {
                    if let Some(t) = body.get(..4) {
                        // Dropping the sender tells the waiter.
                        lock(&self.indirect).remove(&u32::from_le_bytes(t.try_into().unwrap()));
                    }
                }
                _ => {}
            }
        }
        self.set_offline();
    }

    /// Another user asked (through the server) that we connect to them.
    async fn answer_connect_to_peer(self: Arc<Self>, c: proto::ConnectToPeer) {
        if c.kind != "P" && c.kind != "F" {
            return; // "D": this client is not part of the distributed search network.
        }
        let permit = if c.kind == "P" {
            match timeout(OPEN_WAIT, self.others.clone().acquire_owned()).await {
                Ok(Ok(p)) => Some(p),
                _ => {
                    self.send_server(proto::cant_connect_to_peer(c.token, &c.user));
                    return;
                }
            }
        } else {
            None
        };
        let stream = tokio::select! {
            _ = self.shutdown.cancelled() => return,
            r = timeout(if c.kind == "P" { RELAYED_TIMEOUT } else { DIRECT_TIMEOUT }, TcpStream::connect((c.ip, c.port))) => r,
        };
        let mut s = match stream {
            Ok(Ok(s)) => s,
            _ => {
                self.send_server(proto::cant_connect_to_peer(c.token, &c.user));
                return;
            }
        };
        let _ = s.set_nodelay(true);
        if s.write_all(&proto::pierce_firewall(c.token)).await.is_err() {
            return;
        }
        if c.kind == "P" {
            self.spawn_peer_conn(c.user, s, permit);
        } else {
            debug!("F connection to {} (relayed by the server)", c.user);
            self.handle_file_conn(s).await;
        }
    }

    // ---- Listen socket -------------------------------------------------------------------

    async fn accept_loop(self: Arc<Self>, listener: TcpListener) {
        loop {
            let r = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                r = listener.accept() => r,
            };
            match r {
                Ok((s, _)) => {
                    // Without a permit the socket is kept only when our downloads need it.
                    let permit = self.others.clone().try_acquire_owned().ok();
                    tokio::spawn(self.clone().handle_inbound(s, permit));
                }
                // Too many open files and similar: wait a little.
                Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }
    }

    async fn handle_inbound(self: Arc<Self>, mut s: TcpStream, permit: Option<OwnedSemaphorePermit>) {
        let _ = s.set_nodelay(true);
        // Read without a buffer: the bytes after the init message belong to the next reader.
        let wait = if permit.is_some() { INIT_TIMEOUT } else { OPEN_WAIT };
        let first = tokio::select! {
            _ = self.shutdown.cancelled() => return,
            r = timeout(wait, proto::read_init_frame(&mut s)) => r,
        };
        let Ok(Ok((code, body))) = first else { return };
        match proto::parse_init(code, &body) {
            Ok(Init::PierceFirewall(token)) => {
                let waiter = lock(&self.indirect).remove(&token);
                if let Some(w) = waiter {
                    let _ = w.send(s);
                }
            }
            Ok(Init::PeerInit { user, kind }) => match kind.as_str() {
                "P" => {
                    // Without a free socket, keep it only when a download from this user waits.
                    if permit.is_some() || lock(&self.downloads).keys().any(|(u, _)| *u == user) {
                        self.spawn_peer_conn(user, s, permit);
                    }
                }
                "F" => {
                    debug!("F connection from {user} (to our listen port)");
                    self.handle_file_conn(s).await
                }
                _ => {}
            },
            Err(_) => {}
        }
    }

    // ---- P connections -------------------------------------------------------------------

    /// Starts the tasks of a P connection. `permit` is held while it is open (None for the
    /// connections our downloads open).
    fn spawn_peer_conn(self: &Arc<Self>, user: String, s: TcpStream, permit: Option<OwnedSemaphorePermit>) -> PeerConn {
        let (tx, rx) = mpsc::unbounded_channel();
        let conn = PeerConn { id: self.id(), tx, closed: self.shutdown.child_token() };
        lock(&self.peers).insert(user.clone(), conn.clone());
        let (rd, wr) = s.into_split();
        tokio::spawn(peer_writer(wr, rx, conn.closed.clone()));
        tokio::spawn(self.clone().peer_reader(user, conn.clone(), rd, permit));
        conn
    }

    async fn peer_reader(self: Arc<Self>, user: String, conn: PeerConn, rd: OwnedReadHalf, _permit: Option<OwnedSemaphorePermit>) {
        let mut rd = BufReader::new(rd);
        loop {
            let r = tokio::select! {
                _ = conn.closed.cancelled() => break,
                r = timeout(PEER_IDLE, proto::read_frame(&mut rd, proto::MAX_PEER_FRAME)) => r,
            };
            let Ok(Ok((code, body))) = r else { break };
            self.on_peer_message(&user, &conn, code, &body);
            // A peer that only answered a search has nothing more to say: close at once, so
            // that a popular search does not keep hundreds of sockets open. Keep it when a
            // download from this user waits. The downloads lock is held while the entry
            // leaves `peers`, so a new download never takes a connection that is closing.
            if code == proto::peer::FILE_SEARCH_RESPONSE {
                let downloads = lock(&self.downloads);
                if !downloads.keys().any(|(u, _)| *u == user) {
                    self.forget_peer(&user, &conn);
                    drop(downloads);
                    return;
                }
            }
        }
        self.forget_peer(&user, &conn);
    }

    fn forget_peer(&self, user: &str, conn: &PeerConn) {
        conn.closed.cancel();
        let mut peers = lock(&self.peers);
        if peers.get(user).map(|c| c.id) == Some(conn.id) {
            peers.remove(user);
        }
    }

    fn on_peer_message(&self, user: &str, conn: &PeerConn, code: u32, body: &[u8]) {
        if code == proto::peer::FILE_SEARCH_RESPONSE {
            self.route_search_response(user, body);
            return;
        }
        let Ok(Some(msg)) = proto::parse_peer_msg(code, body) else { return };
        let event = |filename: String, ev: DlEvent| {
            let slot = lock(&self.downloads).get(&(user.to_string(), filename)).cloned();
            if let Some(slot) = slot {
                let _ = slot.ev.send(ev);
            }
        };
        match msg {
            PeerMsg::TransferRequest { direction, token, filename, size } => {
                let reply = if direction != 1 {
                    // The peer wants a file from us; we share nothing.
                    proto::transfer_response(token, false, "File not shared.")
                } else {
                    let slot = lock(&self.downloads).get(&(user.to_string(), filename)).cloned();
                    match slot {
                        Some(slot) => {
                            // Register before the answer: the F connection can come at once.
                            lock(&self.transfers).insert(token, TransferSlot { files: slot.files.clone(), size });
                            let _ = slot.ev.send(DlEvent::Accepted);
                            proto::transfer_response(token, true, "")
                        }
                        None => proto::transfer_response(token, false, "Cancelled"),
                    }
                };
                let _ = conn.tx.send(reply);
            }
            // We do not send TransferRequest, so its answer has no download to go to.
            PeerMsg::TransferResponse { .. } => {}
            PeerMsg::PlaceInQueue { filename, place } => event(filename, DlEvent::Queued(Some(place))),
            PeerMsg::UploadFailed { filename } => event(filename, DlEvent::Failed("the peer reported that the upload failed".into())),
            PeerMsg::UploadDenied { filename, reason } => event(filename, DlEvent::Denied(reason)),
        }
    }

    fn route_search_response(&self, user: &str, body: &[u8]) {
        let Ok(token) = proto::search_response_token(body) else { return };
        let tx = match lock(&self.searches).get(&token) {
            Some(tx) if !tx.is_closed() => tx.clone(),
            _ => return,
        };
        let Ok((_, mut resp)) = proto::parse_search_response(body) else { return };
        if resp.user.is_empty() {
            resp.user = user.to_string();
        }
        if tx.send(resp).is_err() {
            lock(&self.searches).remove(&token);
        }
    }

    /// An open P connection to `user`: the one we have, or a new one (direct and through the
    /// server at the same time; the first that works wins).
    async fn get_peer(self: &Arc<Self>, user: &str, cancel: &CancellationToken) -> Result<PeerConn, DownloadError> {
        if let Some(c) = lock(&self.peers).get(user) {
            if !c.closed.is_cancelled() {
                return Ok(c.clone());
            }
        }
        if !self.online() {
            return Err(DownloadError::NoConnection("not signed in to the Soulseek server".into()));
        }
        let token = self.token();
        let (itx, irx) = oneshot::channel();
        lock(&self.indirect).insert(token, itx);
        let (atx, arx) = oneshot::channel();
        lock(&self.addr_waiters).entry(user.to_string()).or_default().push(atx);
        self.send_server(proto::connect_to_peer(token, user, "P"));
        self.send_server(proto::get_peer_address(user));

        let own = self.user.clone();
        let direct = async move {
            let (ip, port) = match timeout(DIRECT_TIMEOUT, arx).await {
                Ok(Ok(a)) => a,
                Ok(Err(_)) => return Err((false, "the server connection closed".to_string())),
                Err(_) => return Err((false, "no address from the server".to_string())),
            };
            if ip.is_unspecified() || port == 0 {
                return Err((true, "the user is offline".to_string()));
            }
            let mut s = match timeout(DIRECT_TIMEOUT, TcpStream::connect((ip, port))).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return Err((false, format!("{ip}:{port}: {e}"))),
                Err(_) => return Err((false, format!("{ip}:{port}: timed out"))),
            };
            let _ = s.set_nodelay(true);
            s.write_all(&proto::peer_init(&own, "P")).await.map_err(|e| (false, e.to_string()))?;
            Ok(s)
        };
        let indirect = async move {
            match timeout(INDIRECT_TIMEOUT, irx).await {
                Ok(Ok(s)) => Ok(s),
                Ok(Err(_)) => Err("the peer could not connect to us".to_string()),
                Err(_) => Err("the peer did not connect to us in time".to_string()),
            }
        };
        tokio::pin!(direct, indirect);
        let mut direct_err: Option<String> = None;
        let mut indirect_err: Option<String> = None;
        let result = loop {
            tokio::select! {
                _ = cancel.cancelled() => break Err(DownloadError::Cancelled),
                r = &mut direct, if direct_err.is_none() => match r {
                    Ok(s) => {
                        debug!("P connection to {user}: direct");
                        break Ok(s);
                    }
                    Err((true, e)) => break Err(DownloadError::NoConnection(e)),
                    Err((false, e)) => direct_err = Some(e),
                },
                r = &mut indirect, if indirect_err.is_none() => match r {
                    Ok(s) => {
                        debug!("P connection to {user}: the peer connected to us");
                        break Ok(s);
                    }
                    Err(e) => indirect_err = Some(e),
                },
            }
            if let (Some(d), Some(i)) = (&direct_err, &indirect_err) {
                debug!("no P connection to {user}: direct: {d}; through the server: {i}");
                break Err(DownloadError::NoConnection(format!("direct: {d}; through the server: {i}")));
            }
        };
        lock(&self.indirect).remove(&token);
        Ok(self.spawn_peer_conn(user.to_string(), result?, None))
    }

    // ---- F connections and downloads -----------------------------------------------------

    /// An F connection: the uploader sends the transfer token, then we say the offset.
    async fn handle_file_conn(self: Arc<Self>, mut s: TcpStream) {
        let token = tokio::select! {
            _ = self.shutdown.cancelled() => return,
            r = timeout(INIT_TIMEOUT, s.read_u32_le()) => r,
        };
        let Ok(Ok(token)) = token else { return };
        let slot = lock(&self.transfers).remove(&token);
        match slot {
            Some(slot) => {
                let _ = slot.files.send((s, slot.size));
            }
            None => debug!("F connection with unknown token {token}"),
        }
    }

    pub async fn download(
        self: &Arc<Self>,
        user: &str,
        filename: &str,
        path: &Path,
        progress: OnProgress,
        cancel: CancellationToken,
    ) -> Result<u64, DownloadError> {
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let (files_tx, mut files_rx) = mpsc::unbounded_channel();
        let key = (user.to_string(), filename.to_string());
        let id = self.id();
        lock(&self.downloads).insert(key.clone(), DownloadSlot { id, ev: ev_tx, files: files_tx.clone() });
        let _cleanup = Cleanup { inner: self, key, id, files: files_tx };

        let mut conn = self.get_peer(user, &cancel).await?;
        if conn.tx.send(proto::queue_upload(filename)).is_err() {
            return Err(DownloadError::NoConnection("the peer connection closed".into()));
        }
        progress(Progress::Requested);
        // When the connection closes before the peer said anything, ask once more on a new
        // one (an old connection can be dead without us knowing yet).
        let mut heard = false;
        let mut retried = false;

        let place_timer = tokio::time::sleep(PLACE_ASK_AFTER);
        tokio::pin!(place_timer);
        let mut place_asked = false;
        let mut accepted = false;
        let (mut stream, size) = loop {
            tokio::select! {
                _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
                _ = &mut place_timer, if !place_asked => {
                    place_asked = true;
                    if !accepted {
                        let tx = lock(&self.peers).get(user).map(|c| c.tx.clone());
                        if let Some(tx) = tx {
                            let _ = tx.send(proto::place_in_queue_request(filename));
                        }
                    }
                }
                _ = conn.closed.cancelled(), if !heard && !retried => {
                    retried = true;
                    debug!("P connection to {user} closed before an answer: ask again");
                    conn = self.get_peer(user, &cancel).await?;
                    if conn.tx.send(proto::queue_upload(filename)).is_err() {
                        return Err(DownloadError::NoConnection("the peer connection closed".into()));
                    }
                }
                ev = ev_rx.recv() => match ev {
                    Some(DlEvent::Queued(place)) => {
                        heard = true;
                        if !accepted { progress(Progress::Queued(place)) }
                    }
                    Some(DlEvent::Accepted) => {
                        heard = true;
                        accepted = true;
                    }
                    Some(DlEvent::Denied(reason)) => return Err(DownloadError::Denied(reason)),
                    Some(DlEvent::Failed(reason)) => return Err(DownloadError::Failed(reason)),
                    None => return Err(DownloadError::Failed("the download was replaced".into())),
                },
                f = files_rx.recv() => match f {
                    Some(f) => break f,
                    None => return Err(DownloadError::Failed("the download was replaced".into())),
                },
            }
        };
        drop(files_rx);

        progress(Progress::Started(size));
        let offset = 0u64.to_le_bytes();
        let send_offset = tokio::select! {
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            r = timeout(FILE_IDLE, stream.write_all(&offset)) => r,
        };
        match send_offset {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(DownloadError::Failed(format!("cannot send the offset: {e}"))),
            Err(_) => return Err(DownloadError::Failed("cannot send the offset: timed out".into())),
        }
        let mut file = tokio::io::BufWriter::with_capacity(256 * 1024, tokio::fs::File::create(path).await?);
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = 0u64;
        let mut last = Instant::now();
        while got < size {
            let want = (size - got).min(buf.len() as u64) as usize;
            let r = tokio::select! {
                _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
                r = timeout(FILE_IDLE, stream.read(&mut buf[..want])) => r,
            };
            let n = match r {
                Ok(Ok(0)) => return Err(DownloadError::Failed(format!("the connection closed at {got} of {size} bytes"))),
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(DownloadError::Failed(format!("{e} at {got} of {size} bytes"))),
                Err(_) => return Err(DownloadError::Failed(format!("the peer stopped sending at {got} of {size} bytes"))),
            };
            file.write_all(&buf[..n]).await?;
            got += n as u64;
            if last.elapsed() >= PROGRESS_EVERY {
                last = Instant::now();
                // The preview player reads the file: report only what is on disk.
                file.flush().await?;
                progress(Progress::Bytes(got));
            }
        }
        file.flush().await?;
        drop(file);
        // The downloader closes the F connection when the file is complete.
        drop(stream);
        progress(Progress::Bytes(got));
        Ok(got)
    }

    pub async fn search(&self, query: &str) -> anyhow::Result<Search> {
        if !self.online() {
            anyhow::bail!("not signed in to the Soulseek server");
        }
        ratelimit::acquire(&self.limiter).await;
        if !self.online() {
            anyhow::bail!("not signed in to the Soulseek server");
        }
        let token = self.token();
        let (tx, rx) = mpsc::unbounded_channel();
        {
            let mut s = lock(&self.searches);
            // Forget searches whose receiver was dropped.
            s.retain(|_, t| !t.is_closed());
            s.insert(token, tx);
        }
        self.send_server(proto::file_search(token, query));
        Ok(Search { rx })
    }
}

async fn peer_writer(mut wr: OwnedWriteHalf, mut rx: mpsc::UnboundedReceiver<Vec<u8>>, closed: CancellationToken) {
    loop {
        let msg = tokio::select! {
            _ = closed.cancelled() => break,
            m = rx.recv() => match m { Some(m) => m, None => break },
        };
        if wr.write_all(&msg).await.is_err() {
            break;
        }
    }
    closed.cancel();
    let _ = wr.shutdown().await;
}

/// Removes a download's routes when it ends in any way.
struct Cleanup<'a> {
    inner: &'a Inner,
    key: (String, String),
    id: u64,
    files: mpsc::UnboundedSender<(TcpStream, u64)>,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let mut d = lock(&self.inner.downloads);
        if d.get(&self.key).map(|s| s.id) == Some(self.id) {
            d.remove(&self.key);
        }
        drop(d);
        lock(&self.inner.transfers).retain(|_, t| !t.files.same_channel(&self.files));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Client, Progress, SearchResponse};
    use super::*;
    use std::io::Write;

    const SONG: &str = "@@music\\Artist\\Song.mp3";
    const DENIED: &str = "@@music\\Artist\\Not shared.mp3";

    fn zlib(b: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    /// A fake server: login, GetPeerAddress → the fake peer, FileSearch → token to the peer.
    async fn fake_server(l: TcpListener, peer_port: u16, search_tx: mpsc::UnboundedSender<u32>) {
        let (mut s, _) = l.accept().await.unwrap();
        let (code, _) = proto::read_frame(&mut s, 1 << 20).await.unwrap();
        assert_eq!(code, proto::server::LOGIN);
        let ok = proto::Writer::default().bool(true).str("hi").u32(0x7f00_0001).str("x").bool(false).frame(1);
        s.write_all(&ok).await.unwrap();
        while let Ok((code, body)) = proto::read_frame(&mut s, 1 << 20).await {
            let mut r = proto::Reader::new(&body);
            match code {
                proto::server::GET_PEER_ADDRESS => {
                    let user = r.str().unwrap();
                    let m = proto::Writer::default().str(&user).u32(0x7f00_0001).u32(peer_port as u32).u32(0).u32(0).frame(3);
                    s.write_all(&m).await.unwrap();
                }
                proto::server::FILE_SEARCH => {
                    let _ = search_tx.send(r.u32().unwrap());
                }
                _ => {}
            }
        }
    }

    /// Serves the download requests on one P connection of the fake peer.
    async fn serve_p(mut p: TcpStream, our_port: u16, data: Arc<Vec<u8>>) {
        loop {
            let Ok((code, body)) = proto::read_frame(&mut p, 1 << 20).await else { return };
            if code != proto::peer::QUEUE_UPLOAD {
                continue;
            }
            let name = proto::Reader::new(&body).str().unwrap();
            if name == DENIED {
                p.write_all(&proto::Writer::default().str(&name).str("File not shared.").frame(50)).await.unwrap();
                continue;
            }
            if name != SONG {
                continue; // Never answers: the download waits.
            }
            p.write_all(&proto::Writer::default().str(&name).u32(3).frame(44)).await.unwrap();
            let req = proto::Writer::default().u32(1).u32(555).str(&name).u64(data.len() as u64).frame(40);
            p.write_all(&req).await.unwrap();
            let (code, body) = proto::read_frame(&mut p, 1 << 20).await.unwrap();
            assert_eq!(code, proto::peer::TRANSFER_RESPONSE);
            assert_eq!(&body[..5], &[43, 2, 0, 0, 1]);
            let mut f = TcpStream::connect(("127.0.0.1", our_port)).await.unwrap();
            f.write_all(&proto::peer_init("alice", "F")).await.unwrap();
            f.write_all(&555u32.to_le_bytes()).await.unwrap();
            assert_eq!(f.read_u64_le().await.unwrap(), 0);
            for chunk in data.chunks(7000) {
                f.write_all(chunk).await.unwrap();
            }
            // The downloader closes the connection.
            let mut rest = Vec::new();
            let _ = f.read_to_end(&mut rest).await;
        }
    }

    /// A fake peer "alice" with one song of `data`, and one file it denies.
    async fn fake_peer(l: TcpListener, mut our_port: mpsc::UnboundedReceiver<u16>, mut search_rx: mpsc::UnboundedReceiver<u32>, data: Vec<u8>) {
        let our_port = our_port.recv().await.unwrap();
        let data = Arc::new(data);
        // Search answers come on a connection the peer opens to us.
        let d = data.clone();
        tokio::spawn(async move {
            while let Some(token) = search_rx.recv().await {
                let mut s = TcpStream::connect(("127.0.0.1", our_port)).await.unwrap();
                s.write_all(&proto::peer_init("alice", "P")).await.unwrap();
                let body = proto::Writer::default()
                    .str("alice")
                    .u32(token)
                    .u32(1)
                    .u8(1)
                    .str(SONG)
                    .u64(1000)
                    .str("mp3")
                    .u32(1)
                    .u32(0)
                    .u32(320)
                    .bool(true)
                    .u32(5000)
                    .u32(0)
                    .into_inner();
                let z = zlib(&body);
                let mut f = Vec::new();
                f.extend_from_slice(&(z.len() as u32 + 4).to_le_bytes());
                f.extend_from_slice(&9u32.to_le_bytes());
                f.extend_from_slice(&z);
                s.write_all(&f).await.unwrap();
                tokio::spawn(serve_p(s, our_port, d.clone()));
            }
        });
        // Connections we open to the peer.
        loop {
            let (mut p, _) = l.accept().await.unwrap();
            let (code, body) = proto::read_init_frame(&mut p).await.unwrap();
            assert_eq!(proto::parse_init(code, &body).unwrap(), Init::PeerInit { user: "me".into(), kind: "P".into() });
            tokio::spawn(serve_p(p, our_port, data.clone()));
        }
    }

    #[tokio::test]
    async fn search_and_download_over_loopback() {
        let server_l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let peer_l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server_l.local_addr().unwrap().to_string();
        let peer_port = peer_l.local_addr().unwrap().port();
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let (search_tx, search_rx) = mpsc::unbounded_channel();
        let (port_tx, port_rx) = mpsc::unbounded_channel();
        tokio::spawn(fake_server(server_l, peer_port, search_tx));
        tokio::spawn(fake_peer(peer_l, port_rx, search_rx, data.clone()));

        let c = Client::connect(&addr, "me", "pw").await.unwrap();
        assert!(c.online());
        port_tx.send(c.listen_port()).unwrap();

        let mut s = c.search("artist song").await.unwrap();
        let r: SearchResponse = timeout(Duration::from_secs(5), s.rx.recv()).await.unwrap().unwrap();
        assert_eq!((r.user.as_str(), r.free_slot, r.avg_speed, r.files.len()), ("alice", true, 5000, 1));
        assert_eq!(r.files[0].bitrate, Some(320));

        let dir = std::env::temp_dir().join(format!("slsk_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.mp3");
        let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let seen2 = seen.clone();
        let progress: OnProgress = Arc::new(move |p| seen2.lock().unwrap().push(p));
        let n = timeout(Duration::from_secs(10), c.download("alice", SONG, 1000, &path, progress, CancellationToken::new()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(std::fs::read(&path).unwrap(), data);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(&seen[..3], &[Progress::Requested, Progress::Queued(Some(3)), Progress::Started(100_000)]);
        assert_eq!(seen.last(), Some(&Progress::Bytes(100_000)));

        // A user with no open connection: the client connects directly to the address.
        let path2 = dir.join("song2.mp3");
        let n = timeout(Duration::from_secs(10), c.download("bob", SONG, 1000, &path2, Arc::new(|_| {}), CancellationToken::new()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(n, data.len() as u64);
        assert!(lock(&c.inner.peers).contains_key("bob"));

        // The same P connection is used again; the peer says no.
        let progress: OnProgress = Arc::new(|_| {});
        let e = c.download("alice", DENIED, 1, &dir.join("x.mp3"), progress, CancellationToken::new()).await.unwrap_err();
        assert!(matches!(e, DownloadError::Denied(ref r) if r == "File not shared."), "{e}");

        // Cancel stops a download that waits.
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.cancel();
        });
        let e = c.download("alice", "never.mp3", 1, &dir.join("y.mp3"), Arc::new(|_| {}), cancel).await.unwrap_err();
        assert!(matches!(e, DownloadError::Cancelled));
        assert!(lock(&c.inner.downloads).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn others_limit_from_file_limit() {
        assert_eq!(others_limit(256), 78);
        assert_eq!(others_limit(4096), 1000);
        assert_eq!(others_limit(10), 50);
    }

    #[tokio::test]
    async fn offline_user_fails_fast() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        let (tx, _rx) = mpsc::unbounded_channel();
        // Port 0 in the address answer means the user is offline.
        tokio::spawn(fake_server(l, 0, tx));
        let c = Client::connect(&addr, "me", "pw").await.unwrap();
        let t = Instant::now();
        let e = c.download("ghost", "a.mp3", 1, Path::new("/nonexistent/x"), Arc::new(|_| {}), CancellationToken::new()).await.unwrap_err();
        assert!(matches!(e, DownloadError::NoConnection(ref r) if r.contains("offline")), "{e}");
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn refused_login() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let _ = proto::read_frame(&mut s, 1 << 20).await.unwrap();
            s.write_all(&proto::Writer::default().bool(false).str("INVALIDPASS").frame(1)).await.unwrap();
        });
        match Client::connect(&addr, "me", "pw").await {
            Err(ConnectError::Refused(a)) => assert_eq!(a.reason, "INVALIDPASS"),
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("signed in"),
        }
    }

    #[tokio::test]
    async fn relogged_goes_offline() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let _ = proto::read_frame(&mut s, 1 << 20).await.unwrap();
            s.write_all(&proto::Writer::default().bool(true).str("hi").u32(0).str("x").bool(false).frame(1)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            s.write_all(&proto::Writer::default().frame(41)).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let c = Client::connect(&addr, "me", "pw").await.unwrap();
        assert!(c.online());
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(!c.online());
        assert!(c.search("x").await.is_err());
    }
}
