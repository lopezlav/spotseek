//! A live test of the native Soulseek client: sign in, search once, download one small mp3.
//!
//! cd engine && cargo run --release --example slsk_probe -- [query]
//!
//! The account comes from ~/.config/sockseek/sockseek.conf (`username = …`, `password = …`).
//! Only one session per account can be online: do not run two at once.

#[allow(dead_code)]
#[path = "../src/slsk/mod.rs"]
mod slsk;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slsk::{Client, DownloadError, Progress, SearchResponse};
use tokio_util::sync::CancellationToken;

fn account() -> anyhow::Result<(String, String)> {
    let home = std::env::var("HOME")?;
    let text = std::fs::read_to_string(format!("{home}/.config/sockseek/sockseek.conf"))?;
    let mut user = None;
    let mut pass = None;
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "username" => user = Some(v.trim().to_string()),
                "password" => pass = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    match (user, pass) {
        (Some(u), Some(p)) if !u.is_empty() && !p.is_empty() => Ok((u, p)),
        _ => anyhow::bail!("no username or password in sockseek.conf"),
    }
}

fn ms(d: Duration) -> String {
    format!("{} ms", d.as_millis())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let query = if args.is_empty() { "daft punk around the world".to_string() } else { args.join(" ") };
    let (user, pass) = account()?;

    let t0 = Instant::now();
    let client = match Client::connect(slsk::SERVER, &user, &pass).await {
        Ok(c) => c,
        Err(e) => anyhow::bail!("{e}"),
    };
    println!("signed in as {user} in {}; listen port {}", ms(t0.elapsed()), client.listen_port());

    let t_search = Instant::now();
    let mut search = client.search(&query).await?;
    println!("search sent: {query:?}");
    let mut answers: Vec<SearchResponse> = Vec::new();
    let mut first: Option<Duration> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while let Ok(Some(r)) = tokio::time::timeout_at(deadline, search.rx.recv()).await {
        first.get_or_insert(t_search.elapsed());
        answers.push(r);
    }
    drop(search);
    let files: usize = answers.iter().map(|a| a.files.len()).sum();
    println!(
        "answers in 8 s: {} peers, {} files; first answer after {}",
        answers.len(),
        files,
        first.map(ms).unwrap_or_else(|| "-".into())
    );

    let mut rows: Vec<(&SearchResponse, &slsk::SharedFile)> = answers.iter().flat_map(|a| a.files.iter().map(move |f| (a, f))).collect();
    rows.sort_by_key(|(a, _)| (!a.free_slot, std::cmp::Reverse(a.avg_speed), a.queue_len));
    println!("top 10:");
    for (a, f) in rows.iter().take(10) {
        println!(
            "  {:<18} free={} speed={:>8} queue={:<3} {:<4} {:>4}kbps {:>4}s {:>9} B  {}",
            a.user,
            a.free_slot as u8,
            a.avg_speed,
            a.queue_len,
            f.ext,
            f.bitrate.map(|b| b.to_string()).unwrap_or_else(|| "-".into()),
            f.duration.map(|b| b.to_string()).unwrap_or_else(|| "-".into()),
            f.size,
            f.filename
        );
    }

    // The smallest mp3 files from peers with a free slot, one per user.
    let mut cands: Vec<(&SearchResponse, &slsk::SharedFile)> =
        rows.iter().copied().filter(|(a, f)| a.free_slot && f.ext == "mp3" && f.size > 100_000).collect();
    cands.sort_by_key(|(_, f)| f.size);
    let mut seen = std::collections::HashSet::new();
    cands.retain(|(a, _)| seen.insert(a.user.clone()));

    for (i, (a, f)) in cands.iter().take(3).enumerate() {
        println!("download {}: {} from {} ({} B)", i + 1, f.filename, a.user, f.size);
        let path = std::env::temp_dir().join(format!("slsk_probe_{i}.mp3"));
        let started: Arc<Mutex<Option<Duration>>> = Arc::default();
        let first_bytes: Arc<Mutex<Option<Duration>>> = Arc::default();
        let t = Instant::now();
        let progress: slsk::OnProgress = {
            let started = started.clone();
            let first_bytes = first_bytes.clone();
            Arc::new(move |p: Progress| {
                match &p {
                    Progress::Started(_) => {
                        started.lock().unwrap().get_or_insert(t.elapsed());
                    }
                    Progress::Bytes(n) if *n > 0 => {
                        first_bytes.lock().unwrap().get_or_insert(t.elapsed());
                    }
                    _ => {}
                }
                if !matches!(p, Progress::Bytes(_)) {
                    println!("  {:>6} {p:?}", ms(t.elapsed()));
                }
            })
        };
        let cancel = CancellationToken::new();
        // Cancel when the transfer does not start within 20 s, or takes more than 90 s.
        let watchdog = {
            let cancel = cancel.clone();
            let started = started.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(20)).await;
                if started.lock().unwrap().is_none() {
                    println!("  no start after 20 s: cancel");
                    cancel.cancel();
                    return;
                }
                tokio::time::sleep(Duration::from_secs(70)).await;
                cancel.cancel();
            })
        };
        let r = client.download(&a.user, &f.filename, f.size, &path, progress, cancel).await;
        watchdog.abort();
        match r {
            Ok(n) => {
                let on_disk = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                println!(
                    "  done: {n} bytes ({on_disk} on disk) in {}; transfer start after {}; first bytes after {}",
                    ms(t.elapsed()),
                    started.lock().unwrap().map(ms).unwrap_or_default(),
                    first_bytes.lock().unwrap().map(ms).unwrap_or_default()
                );
                let _ = std::fs::remove_file(&path);
                break;
            }
            Err(DownloadError::Cancelled) => println!("  cancelled after {}", ms(t.elapsed())),
            Err(e) => println!("  failed after {}: {e}", ms(t.elapsed())),
        }
        let _ = std::fs::remove_file(&path);
    }
    println!("online at the end: {}", client.online());
    Ok(())
}
