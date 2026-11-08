//! A native Soulseek client: sign in, search, and download files from peers.
//!
//! This file is the contract the rest of the engine uses. The wire format is in `proto`,
//! the live session (server, listen socket, peer and file connections) in `client`.

mod client;
mod fdlimit;
mod proto;
mod ratelimit;
mod types;

use std::path::Path;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::{CancellationToken, DropGuard};

pub use types::*;
pub use client::search_wait;

/// The Soulseek server.
pub const SERVER: &str = "server.slsknet.org:2271";

/// Called for each step of a download.
pub type OnProgress = Arc<dyn Fn(Progress) + Send + Sync>;

#[derive(Debug)]
pub enum ConnectError {
    /// The server said no (wrong password, name taken, …).
    Refused(LoginAnswer),
    /// The server could not be reached, or it closed the connection.
    Network(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Refused(a) => write!(f, "Soulseek login failed: {} {}", a.reason, a.detail),
            ConnectError::Network(s) => write!(f, "cannot reach the Soulseek server: {s}"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// Signs in once, reads the answer, and disconnects. A new name creates the account.
pub async fn check_login(addr: &str, user: &str, pass: &str) -> anyhow::Result<LoginAnswer> {
    client::check_login(addr, user, pass).await
}

/// A signed-in session. Clones share it. When the last clone is dropped, the session ends.
#[derive(Clone)]
pub struct Client {
    inner: Arc<client::Inner>,
    _guard: Arc<DropGuard>,
}

/// The answers to one search. Dropping it stops the routing of more answers.
pub struct Search {
    pub rx: mpsc::UnboundedReceiver<SearchResponse>,
}

impl Client {
    /// Connects to `addr`, signs in, opens a listen port for peers and tells the server about it.
    pub async fn connect(addr: &str, user: &str, pass: &str) -> Result<Client, ConnectError> {
        let inner = client::Inner::connect(addr, user, pass).await?;
        let guard = Arc::new(inner.shutdown_token().drop_guard());
        Ok(Client { inner, _guard: guard })
    }

    /// Sends a search to the network. The client keeps the server's limit (34 searches in 220
    /// seconds): when it is reached, this waits for a free slot.
    pub async fn search(&self, query: &str) -> anyhow::Result<Search> {
        self.inner.search(query).await
    }

    /// Downloads `filename` (as the search answer named it) from `user` into `path` (created,
    /// or truncated). Returns the bytes written once the whole file is there.
    pub async fn download(&self, user: &str, filename: &str, size: u64, path: &Path, progress: OnProgress, cancel: CancellationToken) -> Result<u64, DownloadError> {
        // The peer says the size again in its TransferRequest; that value is the one used.
        let _ = size;
        self.inner.download(user, filename, path, progress, cancel).await
    }

    /// Why the session ended: "relogged" (the account signed in somewhere else), or what broke.
    pub fn offline_reason(&self) -> String {
        self.inner.offline_reason()
    }

    /// True while the server connection is up.
    pub fn online(&self) -> bool {
        self.inner.online()
    }

    /// The TCP port where this session takes connections from peers.
    #[allow(dead_code)] // For tools and logs (the probe prints it).
    pub fn listen_port(&self) -> u16 {
        self.inner.listen_port()
    }
}
