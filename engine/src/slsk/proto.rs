//! The Soulseek wire format: framing, the messages this client sends and reads, and the
//! parsers. Field order follows the Nicotine+ protocol document (doc/SLSKPROTOCOL.md).
//!
//! All numbers are little endian. A string is a u32 length and the bytes.

use std::io::Read;
use std::net::Ipv4Addr;

use md5::{Digest, Md5};
use tokio::io::{AsyncRead, AsyncReadExt};

use super::types::{LoginAnswer, SearchResponse, SharedFile};

pub const MAJOR_VERSION: u32 = 170;
pub const MINOR_VERSION: u32 = 1;

/// Server message codes.
pub mod server {
    pub const LOGIN: u32 = 1;
    pub const SET_WAIT_PORT: u32 = 2;
    pub const GET_PEER_ADDRESS: u32 = 3;
    pub const CONNECT_TO_PEER: u32 = 18;
    pub const FILE_SEARCH: u32 = 26;
    pub const SET_STATUS: u32 = 28;
    pub const PING: u32 = 32;
    pub const SHARED_FOLDERS_FILES: u32 = 35;
    pub const RELOGGED: u32 = 41;
    pub const HAVE_NO_PARENT: u32 = 71;
    pub const CANT_CONNECT_TO_PEER: u32 = 1001;
}

/// Peer message codes (on "P" connections).
pub mod peer {
    pub const FILE_SEARCH_RESPONSE: u32 = 9;
    pub const TRANSFER_REQUEST: u32 = 40;
    pub const TRANSFER_RESPONSE: u32 = 41;
    pub const QUEUE_UPLOAD: u32 = 43;
    pub const PLACE_IN_QUEUE_RESPONSE: u32 = 44;
    pub const UPLOAD_FAILED: u32 = 46;
    pub const UPLOAD_DENIED: u32 = 50;
    pub const PLACE_IN_QUEUE_REQUEST: u32 = 51;
}

/// Peer init codes (the first message on a new peer connection).
pub const INIT_PIERCE_FIREWALL: u8 = 0;
pub const INIT_PEER_INIT: u8 = 1;

/// The largest frame on the server connection (room lists can be a few MB).
pub const MAX_SERVER_FRAME: u32 = 64 << 20;
/// The largest frame on a peer connection (compressed search answers).
pub const MAX_PEER_FRAME: u32 = 64 << 20;
/// The largest peer init frame (a name, a type and a token).
pub const MAX_INIT_FRAME: u32 = 4096;
/// The largest search answer after decompression.
pub const MAX_INFLATED: u64 = 64 << 20;

/// Builds the body of one message.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn u8(mut self, v: u8) -> Self {
        self.buf.push(v);
        self
    }
    pub fn bool(self, v: bool) -> Self {
        self.u8(v as u8)
    }
    pub fn u32(mut self, v: u32) -> Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    #[cfg(test)]
    pub fn u64(mut self, v: u64) -> Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn str(self, s: &str) -> Self {
        self.bytes(s.as_bytes())
    }
    pub fn bytes(mut self, b: &[u8]) -> Self {
        self.buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
        self.buf.extend_from_slice(b);
        self
    }
    /// A server or peer frame: u32 length, u32 code, body.
    pub fn frame(self, code: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.buf.len() + 8);
        out.extend_from_slice(&(self.buf.len() as u32 + 4).to_le_bytes());
        out.extend_from_slice(&code.to_le_bytes());
        out.extend_from_slice(&self.buf);
        out
    }
    /// A peer init frame: u32 length, u8 code, body.
    pub fn init_frame(self, code: u8) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.buf.len() + 5);
        out.extend_from_slice(&(self.buf.len() as u32 + 1).to_le_bytes());
        out.push(code);
        out.extend_from_slice(&self.buf);
        out
    }
    #[cfg(test)]
    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

/// The message was shorter than its fields, or a field made no sense.
#[derive(Debug, PartialEq)]
pub struct Malformed;

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("malformed message")
    }
}

impl std::error::Error for Malformed {}

pub type PResult<T> = Result<T, Malformed>;

/// Reads the fields of one message body.
pub struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b }
    }
    pub fn remaining(&self) -> usize {
        self.b.len()
    }
    fn take(&mut self, n: usize) -> PResult<&'a [u8]> {
        if self.b.len() < n {
            return Err(Malformed);
        }
        let (head, rest) = self.b.split_at(n);
        self.b = rest;
        Ok(head)
    }
    pub fn u8(&mut self) -> PResult<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> PResult<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u32(&mut self) -> PResult<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    #[cfg(test)]
    pub fn u64(&mut self) -> PResult<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    /// A file size. Soulseek NS sends sizes over 2 GiB as a u32 and four 0xFF bytes.
    pub fn file_size(&mut self) -> PResult<u64> {
        let b = self.take(8)?;
        if b[7] == 0xFF {
            Ok(u32::from_le_bytes(b[..4].try_into().unwrap()) as u64)
        } else {
            Ok(u64::from_le_bytes(b.try_into().unwrap()))
        }
    }
    /// An IPv4 address, sent as a little-endian u32 whose big-endian bytes are the address.
    pub fn ip(&mut self) -> PResult<Ipv4Addr> {
        Ok(Ipv4Addr::from(self.u32()?))
    }
    pub fn raw_str(&mut self) -> PResult<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    /// A string: UTF-8, or Latin-1 for old clients.
    pub fn str(&mut self) -> PResult<String> {
        Ok(decode_str(self.raw_str()?))
    }
}

pub fn decode_str(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => b.iter().map(|&c| c as char).collect(),
    }
}

/// Reads one frame with a u32 code (server and peer connections). Returns (code, body).
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, max: u32) -> std::io::Result<(u32, Vec<u8>)> {
    let n = r.read_u32_le().await?;
    if n < 4 || n > max {
        return Err(bad_data(format!("bad message length {n}")));
    }
    let code = r.read_u32_le().await?;
    let body = read_body(r, n - 4).await?;
    Ok((code, body))
}

/// Reads one peer init frame (u8 code). Returns (code, body).
pub async fn read_init_frame<R: AsyncRead + Unpin>(r: &mut R) -> std::io::Result<(u8, Vec<u8>)> {
    let n = r.read_u32_le().await?;
    if n < 1 || n > MAX_INIT_FRAME {
        return Err(bad_data(format!("bad init message length {n}")));
    }
    let code = r.read_u8().await?;
    let body = read_body(r, n - 1).await?;
    Ok((code, body))
}

/// Reads `n` bytes. The buffer grows as data comes, so a false length does not allocate it all.
async fn read_body<R: AsyncRead + Unpin>(r: &mut R, n: u32) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::with_capacity((n as usize).min(64 * 1024));
    let got = (&mut *r).take(n as u64).read_to_end(&mut body).await?;
    if got != n as usize {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(body)
}

pub fn bad_data(msg: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

// ---- Messages we send -------------------------------------------------------------------

pub fn login(user: &str, pass: &str) -> Vec<u8> {
    let sum = Md5::digest(format!("{user}{pass}").as_bytes());
    let hex: String = sum.iter().map(|b| format!("{b:02x}")).collect();
    Writer::default().str(user).str(pass).u32(MAJOR_VERSION).str(&hex).u32(MINOR_VERSION).frame(server::LOGIN)
}

pub fn set_wait_port(port: u16) -> Vec<u8> {
    Writer::default().u32(port as u32).frame(server::SET_WAIT_PORT)
}

pub fn set_status_online() -> Vec<u8> {
    Writer::default().u32(2).frame(server::SET_STATUS)
}

pub fn shared_folders_files(folders: u32, files: u32) -> Vec<u8> {
    Writer::default().u32(folders).u32(files).frame(server::SHARED_FOLDERS_FILES)
}

pub fn have_no_parent(no_parent: bool) -> Vec<u8> {
    Writer::default().bool(no_parent).frame(server::HAVE_NO_PARENT)
}

pub fn ping() -> Vec<u8> {
    Writer::default().frame(server::PING)
}

pub fn get_peer_address(user: &str) -> Vec<u8> {
    Writer::default().str(user).frame(server::GET_PEER_ADDRESS)
}

pub fn connect_to_peer(token: u32, user: &str, kind: &str) -> Vec<u8> {
    Writer::default().u32(token).str(user).str(kind).frame(server::CONNECT_TO_PEER)
}

pub fn cant_connect_to_peer(token: u32, user: &str) -> Vec<u8> {
    Writer::default().u32(token).str(user).frame(server::CANT_CONNECT_TO_PEER)
}

pub fn file_search(token: u32, query: &str) -> Vec<u8> {
    Writer::default().u32(token).str(query).frame(server::FILE_SEARCH)
}

pub fn peer_init(own_user: &str, kind: &str) -> Vec<u8> {
    // The token is always zero today.
    Writer::default().str(own_user).str(kind).u32(0).init_frame(INIT_PEER_INIT)
}

pub fn pierce_firewall(token: u32) -> Vec<u8> {
    Writer::default().u32(token).init_frame(INIT_PIERCE_FIREWALL)
}

pub fn queue_upload(filename: &str) -> Vec<u8> {
    Writer::default().str(filename).frame(peer::QUEUE_UPLOAD)
}

pub fn place_in_queue_request(filename: &str) -> Vec<u8> {
    Writer::default().str(filename).frame(peer::PLACE_IN_QUEUE_REQUEST)
}

/// Our answer to a peer's TransferRequest.
pub fn transfer_response(token: u32, allowed: bool, reason: &str) -> Vec<u8> {
    let w = Writer::default().u32(token).bool(allowed);
    let w = if allowed { w } else { w.str(reason) };
    w.frame(peer::TRANSFER_RESPONSE)
}

// ---- Messages we read -------------------------------------------------------------------

pub fn parse_login(body: &[u8]) -> PResult<LoginAnswer> {
    let mut r = Reader::new(body);
    let ok = r.bool()?;
    let msg = r.str()?;
    if ok {
        return Ok(LoginAnswer { ok: true, reason: String::new(), detail: String::new() });
    }
    let detail = if r.remaining() > 0 { r.str().unwrap_or_default() } else { String::new() };
    Ok(LoginAnswer { ok: false, reason: msg, detail })
}

#[derive(Debug, PartialEq)]
pub struct PeerAddress {
    pub user: String,
    pub ip: Ipv4Addr,
    pub port: u16,
}

pub fn parse_peer_address(body: &[u8]) -> PResult<PeerAddress> {
    let mut r = Reader::new(body);
    let user = r.str()?;
    let ip = r.ip()?;
    let port = r.u32()?;
    // Obfuscation fields follow; this client does not use them.
    Ok(PeerAddress { user, ip, port: u16::try_from(port).unwrap_or(0) })
}

#[derive(Debug, PartialEq)]
pub struct ConnectToPeer {
    pub user: String,
    pub kind: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub token: u32,
}

pub fn parse_connect_to_peer(body: &[u8]) -> PResult<ConnectToPeer> {
    let mut r = Reader::new(body);
    let user = r.str()?;
    let kind = r.str()?;
    let ip = r.ip()?;
    let port = u16::try_from(r.u32()?).map_err(|_| Malformed)?;
    let token = r.u32()?;
    Ok(ConnectToPeer { user, kind, ip, port, token })
}

/// The first message on an inbound peer connection.
#[derive(Debug, PartialEq)]
pub enum Init {
    PierceFirewall(u32),
    PeerInit { user: String, kind: String },
}

pub fn parse_init(code: u8, body: &[u8]) -> PResult<Init> {
    let mut r = Reader::new(body);
    match code {
        INIT_PIERCE_FIREWALL => Ok(Init::PierceFirewall(r.u32()?)),
        INIT_PEER_INIT => {
            let user = r.str()?;
            let kind = r.str()?;
            Ok(Init::PeerInit { user, kind })
        }
        _ => Err(Malformed),
    }
}

/// The peer messages a download listens for.
#[derive(Debug, PartialEq)]
pub enum PeerMsg {
    /// direction 1 = the peer uploads to us.
    TransferRequest { direction: u32, token: u32, filename: String, size: u64 },
    TransferResponse { token: u32, allowed: bool, reason: String },
    PlaceInQueue { filename: String, place: u32 },
    UploadFailed { filename: String },
    UploadDenied { filename: String, reason: String },
}

pub fn parse_peer_msg(code: u32, body: &[u8]) -> PResult<Option<PeerMsg>> {
    let mut r = Reader::new(body);
    Ok(Some(match code {
        peer::TRANSFER_REQUEST => {
            let direction = r.u32()?;
            let token = r.u32()?;
            let filename = r.str()?;
            let size = if direction == 1 { r.file_size()? } else { 0 };
            PeerMsg::TransferRequest { direction, token, filename, size }
        }
        peer::TRANSFER_RESPONSE => {
            let token = r.u32()?;
            let allowed = r.bool()?;
            let reason = if !allowed && r.remaining() >= 4 { r.str()? } else { String::new() };
            PeerMsg::TransferResponse { token, allowed, reason }
        }
        peer::PLACE_IN_QUEUE_RESPONSE => {
            let filename = r.str()?;
            let place = r.u32()?;
            PeerMsg::PlaceInQueue { filename, place }
        }
        peer::UPLOAD_FAILED => PeerMsg::UploadFailed { filename: r.str()? },
        peer::UPLOAD_DENIED => {
            let filename = r.str()?;
            let reason = r.str()?;
            PeerMsg::UploadDenied { filename, reason }
        }
        _ => return Ok(None),
    }))
}

/// Reads the user name and token at the start of a compressed FileSearchResponse, so that
/// answers to searches nobody waits for are not inflated in full.
pub fn search_response_token(compressed: &[u8]) -> PResult<u32> {
    let mut z = flate2::read::ZlibDecoder::new(compressed);
    let mut n = [0u8; 4];
    z.read_exact(&mut n).map_err(|_| Malformed)?;
    let n = u32::from_le_bytes(n) as u64;
    if n > 4096 {
        return Err(Malformed);
    }
    std::io::copy(&mut (&mut z).take(n), &mut std::io::sink()).map_err(|_| Malformed)?;
    let mut t = [0u8; 4];
    z.read_exact(&mut t).map_err(|_| Malformed)?;
    Ok(u32::from_le_bytes(t))
}

/// Parses a whole FileSearchResponse (zlib). Returns the token and the answer, with the
/// public files only.
pub fn parse_search_response(compressed: &[u8]) -> PResult<(u32, SearchResponse)> {
    let mut data = Vec::new();
    flate2::read::ZlibDecoder::new(compressed)
        .take(MAX_INFLATED + 1)
        .read_to_end(&mut data)
        .map_err(|_| Malformed)?;
    if data.len() as u64 > MAX_INFLATED {
        return Err(Malformed);
    }
    let mut r = Reader::new(&data);
    let user = r.str()?;
    let token = r.u32()?;
    let files = parse_file_list(&mut r)?;
    let free_slot = r.bool()?;
    let avg_speed = r.u32()?;
    let queue_len = r.u32()?;
    // Then an optional u32 (always 0) and an optional list of private files: left out.
    Ok((token, SearchResponse { user, free_slot, avg_speed, queue_len, files }))
}

fn parse_file_list(r: &mut Reader) -> PResult<Vec<SharedFile>> {
    let count = r.u32()? as usize;
    // Each file takes at least 21 bytes; do not trust the count for the allocation.
    let mut files = Vec::with_capacity(count.min(r.remaining() / 21));
    for _ in 0..count {
        let _code = r.u8()?;
        let filename = r.str()?;
        let size = r.file_size()?;
        let ext = r.str()?;
        let mut f = SharedFile { ext: normalize_ext(&ext, &filename), filename, size, ..Default::default() };
        let attrs = r.u32()?;
        for _ in 0..attrs {
            let code = r.u32()?;
            let value = r.u32()?;
            match code {
                0 => f.bitrate = Some(value),
                1 => f.duration = Some(value),
                2 => f.vbr = Some(value != 0),
                4 => f.sample_rate = Some(value),
                5 => f.bit_depth = Some(value),
                _ => {}
            }
        }
        files.push(f);
    }
    Ok(files)
}

/// The extension in lower case without the dot; from the file name when `ext` is empty.
fn normalize_ext(ext: &str, filename: &str) -> String {
    let ext = ext.trim().trim_start_matches('.');
    if !ext.is_empty() {
        return ext.to_lowercase();
    }
    let base = filename.rsplit(['\\', '/']).next().unwrap_or(filename);
    match base.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() && !e.is_empty() && e.len() <= 8 => e.to_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zlib(b: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    fn file(w: Writer, name: &str, size: u64, ext: &str, attrs: &[(u32, u32)]) -> Writer {
        let mut w = w.u8(1).str(name).u64(size).str(ext).u32(attrs.len() as u32);
        for &(c, v) in attrs {
            w = w.u32(c).u32(v);
        }
        w
    }

    fn search_body(with_private: bool) -> Vec<u8> {
        let mut w = Writer::default().str("alice").u32(77).u32(2);
        w = file(w, "@@music\\Daft Punk\\Around the World.MP3", 7_000_000, "", &[(0, 320), (1, 425), (2, 0)]);
        w = file(w, "@@music\\Daft Punk\\Around.flac", 30_000_000, "FLAC", &[(1, 425), (4, 44100), (5, 16), (9, 1)]);
        w = w.bool(true).u32(123_456).u32(3);
        if with_private {
            w = w.u32(0).u32(1);
            w = file(w, "@@private\\secret.mp3", 1, "mp3", &[]);
        }
        w.into_inner()
    }

    #[test]
    fn frame_round_trip() {
        let f = Writer::default().u32(5).str("héllo").u64(1 << 40).bool(true).frame(26);
        assert_eq!(u32::from_le_bytes(f[..4].try_into().unwrap()) as usize, f.len() - 4);
        let mut slice = &f[..];
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let (code, body) = rt.block_on(read_frame(&mut slice, MAX_PEER_FRAME)).unwrap();
        assert_eq!(code, 26);
        let mut r = Reader::new(&body);
        assert_eq!(r.u32().unwrap(), 5);
        assert_eq!(r.str().unwrap(), "héllo");
        assert_eq!(r.u64().unwrap(), 1 << 40);
        assert!(r.bool().unwrap());
        assert_eq!(r.remaining(), 0);
        assert_eq!(r.u8(), Err(Malformed));
    }

    #[test]
    fn frame_too_long_is_refused() {
        let mut b = Vec::new();
        b.extend_from_slice(&(MAX_PEER_FRAME + 1).to_le_bytes());
        b.extend_from_slice(&[0; 8]);
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        assert!(rt.block_on(read_frame(&mut &b[..], MAX_PEER_FRAME)).is_err());
        // A length longer than the data is an early end, not a large allocation.
        let mut b = Vec::new();
        b.extend_from_slice(&1000u32.to_le_bytes());
        b.extend_from_slice(&9u32.to_le_bytes());
        assert!(rt.block_on(read_frame(&mut &b[..], MAX_PEER_FRAME)).is_err());
    }

    #[test]
    fn init_round_trip() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let f = peer_init("me", "P");
        let (code, body) = rt.block_on(read_init_frame(&mut &f[..])).unwrap();
        assert_eq!(parse_init(code, &body).unwrap(), Init::PeerInit { user: "me".into(), kind: "P".into() });
        let f = pierce_firewall(0xdeadbeef);
        assert_eq!(f, [5, 0, 0, 0, 0, 0xef, 0xbe, 0xad, 0xde]);
        let (code, body) = rt.block_on(read_init_frame(&mut &f[..])).unwrap();
        assert_eq!(parse_init(code, &body).unwrap(), Init::PierceFirewall(0xdeadbeef));
        assert!(parse_init(7, &body).is_err());
    }

    #[test]
    fn login_message_matches_the_document() {
        // The example in the protocol document uses major version 177; ours is 170.
        let f = login("username", "password");
        assert_eq!(&f[..4], &[0x48, 0, 0, 0]);
        assert_eq!(&f[4..8], &[1, 0, 0, 0]);
        let s = String::from_utf8_lossy(&f);
        assert!(s.contains("d51c9a7e9353746a6020f9602d452929"));
        assert_eq!(&f[32..36], &170u32.to_le_bytes());
        assert_eq!(&f[f.len() - 4..], &[1, 0, 0, 0]);
    }

    #[test]
    fn login_answers() {
        let ok = Writer::default().bool(true).str("welcome").u32(0x7f000001).str("abc").bool(false).into_inner();
        assert_eq!(parse_login(&ok).unwrap(), LoginAnswer { ok: true, reason: String::new(), detail: String::new() });
        let bad = Writer::default().bool(false).str("INVALIDPASS").into_inner();
        let a = parse_login(&bad).unwrap();
        assert!(!a.ok);
        assert_eq!(a.reason, "INVALIDPASS");
        assert_eq!(a.detail, "");
        let name = Writer::default().bool(false).str("INVALIDUSERNAME").str("Nickname is empty.").into_inner();
        assert_eq!(parse_login(&name).unwrap().detail, "Nickname is empty.");
        assert!(parse_login(&[]).is_err());
    }

    #[test]
    fn ip_byte_order() {
        // 1.2.3.4 arrives as the bytes 04 03 02 01.
        let body = Writer::default().str("bob").u8(4).u8(3).u8(2).u8(1).u32(2234).u32(0).into_inner();
        let mut body = body;
        body.extend_from_slice(&0u16.to_le_bytes());
        let a = parse_peer_address(&body).unwrap();
        assert_eq!(a, PeerAddress { user: "bob".into(), ip: Ipv4Addr::new(1, 2, 3, 4), port: 2234 });
    }

    #[test]
    fn connect_to_peer_round_trip() {
        let body = Writer::default().str("bob").str("F").u32(0x0a000001).u32(50000).u32(99).bool(false).u32(0).u32(0).into_inner();
        let c = parse_connect_to_peer(&body).unwrap();
        assert_eq!(c, ConnectToPeer { user: "bob".into(), kind: "F".into(), ip: Ipv4Addr::new(10, 0, 0, 1), port: 50000, token: 99 });
        assert!(parse_connect_to_peer(&body[..10]).is_err());
    }

    #[test]
    fn search_response_with_private_files() {
        let c = zlib(&search_body(true));
        assert_eq!(search_response_token(&c).unwrap(), 77);
        let (token, r) = parse_search_response(&c).unwrap();
        assert_eq!(token, 77);
        assert_eq!(r.user, "alice");
        assert!(r.free_slot);
        assert_eq!(r.avg_speed, 123_456);
        assert_eq!(r.queue_len, 3);
        assert_eq!(r.files.len(), 2, "private files are left out");
        let f = &r.files[0];
        assert_eq!(f.filename, "@@music\\Daft Punk\\Around the World.MP3");
        assert_eq!((f.size, f.ext.as_str(), f.bitrate, f.duration, f.vbr), (7_000_000, "mp3", Some(320), Some(425), Some(false)));
        let f = &r.files[1];
        assert_eq!((f.ext.as_str(), f.sample_rate, f.bit_depth, f.bitrate), ("flac", Some(44100), Some(16), None));
    }

    #[test]
    fn search_response_without_optional_tail() {
        let (_, r) = parse_search_response(&zlib(&search_body(false))).unwrap();
        assert_eq!(r.files.len(), 2);
    }

    #[test]
    fn search_response_malformed() {
        let body = search_body(true);
        assert!(parse_search_response(&zlib(&body[..40])).is_err());
        assert!(parse_search_response(b"not zlib").is_err());
        assert!(search_response_token(b"not zlib").is_err());
        // A huge file count with no data must fail, not allocate.
        let w = Writer::default().str("x").u32(1).u32(u32::MAX).into_inner();
        assert!(parse_search_response(&zlib(&w)).is_err());
    }

    #[test]
    fn soulseek_ns_large_file_size() {
        let mut b = Vec::new();
        b.extend_from_slice(&3_000_000_000u32.to_le_bytes());
        b.extend_from_slice(&[0xFF; 4]);
        assert_eq!(Reader::new(&b).file_size().unwrap(), 3_000_000_000);
    }

    #[test]
    fn peer_messages() {
        let b = Writer::default().u32(1).u32(42).str("a\\b.mp3").u64(1234).into_inner();
        assert_eq!(
            parse_peer_msg(40, &b).unwrap(),
            Some(PeerMsg::TransferRequest { direction: 1, token: 42, filename: "a\\b.mp3".into(), size: 1234 })
        );
        let b = Writer::default().u32(0).u32(42).str("a\\b.mp3").into_inner();
        assert!(matches!(parse_peer_msg(40, &b).unwrap(), Some(PeerMsg::TransferRequest { direction: 0, size: 0, .. })));
        let b = Writer::default().str("f").str("Queued").into_inner();
        assert_eq!(parse_peer_msg(50, &b).unwrap(), Some(PeerMsg::UploadDenied { filename: "f".into(), reason: "Queued".into() }));
        let b = Writer::default().str("f").u32(7).into_inner();
        assert_eq!(parse_peer_msg(44, &b).unwrap(), Some(PeerMsg::PlaceInQueue { filename: "f".into(), place: 7 }));
        let b = Writer::default().u32(42).bool(false).str("Cancelled").into_inner();
        assert_eq!(
            parse_peer_msg(41, &b).unwrap(),
            Some(PeerMsg::TransferResponse { token: 42, allowed: false, reason: "Cancelled".into() })
        );
        assert_eq!(parse_peer_msg(15, &[]).unwrap(), None);
        assert!(parse_peer_msg(46, &[1, 0]).is_err());
    }

    #[test]
    fn our_transfer_response() {
        let f = transfer_response(9, true, "");
        assert_eq!(f, Writer::default().u32(9).bool(true).frame(41));
        let f = transfer_response(9, false, "Cancelled");
        assert_eq!(f, Writer::default().u32(9).bool(false).str("Cancelled").frame(41));
    }

    #[test]
    fn latin1_fallback() {
        assert_eq!(decode_str(&[b'c', 0xe9]), "cé");
        assert_eq!(decode_str("cé".as_bytes()), "cé");
    }

    #[test]
    fn ext_from_name() {
        assert_eq!(normalize_ext("", "a\\b\\Song.Mp3"), "mp3");
        assert_eq!(normalize_ext(".FLAC", "x"), "flac");
        assert_eq!(normalize_ext("", "a\\b.c\\noext"), "");
        assert_eq!(normalize_ext("", "a/.hidden"), "");
    }
}
