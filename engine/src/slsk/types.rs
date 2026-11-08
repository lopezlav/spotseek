//! The data the Soulseek client gives to the rest of the engine.

/// One file in a search answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharedFile {
    /// The full path on the peer, as the peer sends it (folders split by `\` or `/`).
    pub filename: String,
    pub size: u64,
    /// The extension in lower case, without the dot ("mp3", "flac"). Taken from the file name
    /// when the peer sends an empty one.
    pub ext: String,
    /// kbps (attribute 0).
    pub bitrate: Option<u32>,
    /// Seconds (attribute 1).
    pub duration: Option<u32>,
    /// Attribute 2.
    pub vbr: Option<bool>,
    /// Hz (attribute 4).
    pub sample_rate: Option<u32>,
    /// Bits (attribute 5).
    pub bit_depth: Option<u32>,
}

/// One peer's answer to a search.
#[derive(Clone, Debug, Default)]
pub struct SearchResponse {
    pub user: String,
    /// The peer has a free upload slot: a download can start at once.
    pub free_slot: bool,
    /// The peer's average upload speed, bytes per second (as the server measured it).
    pub avg_speed: u32,
    /// Uploads waiting in the peer's queue.
    pub queue_len: u32,
    /// Public files only (private, buddy-only files are left out).
    pub files: Vec<SharedFile>,
}

/// What happens to a download, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// We have a connection to the peer and asked for the file.
    Requested,
    /// The peer put us in its queue (place, when it said).
    Queued(Option<u32>),
    /// The peer started to send. The file size it said.
    Started(u64),
    /// Bytes written to the file so far.
    Bytes(u64),
}

#[derive(Debug)]
pub enum DownloadError {
    /// No connection to the peer: direct and through the server both failed.
    NoConnection(String),
    /// The peer said no (the reason it gave: "Queued", "File not shared.", "Banned", …).
    Denied(String),
    /// The transfer started and broke, or the peer reported a failure.
    Failed(String),
    /// The caller cancelled it.
    Cancelled,
    Io(std::io::Error),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::NoConnection(s) => write!(f, "no connection to the peer: {s}"),
            DownloadError::Denied(s) => write!(f, "the peer said no: {s}"),
            DownloadError::Failed(s) => write!(f, "the transfer failed: {s}"),
            DownloadError::Cancelled => write!(f, "cancelled"),
            DownloadError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<std::io::Error> for DownloadError {
    fn from(e: std::io::Error) -> Self {
        DownloadError::Io(e)
    }
}

/// The server's answer to a sign-in.
#[derive(Clone, Debug, PartialEq)]
pub struct LoginAnswer {
    pub ok: bool,
    /// The server's code when `ok` is false: INVALIDPASS, INVALIDUSERNAME, SVRFULL, …
    pub reason: String,
    pub detail: String,
}
