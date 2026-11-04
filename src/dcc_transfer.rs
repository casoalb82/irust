
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};

use irust_core::Servers;

const CHUNK_SIZE: usize = 64 * 1024;
/// A progress line is pushed into the transfer's buffer at most this
/// often even on a fast transfer -- see `run_send_session`/
/// `run_get_session`'s progress-throttling below.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Send,
    Get,
}

/// A transfer is only ever registered (and so only ever shows up in
/// `DccTransfers`/`/dcc list`) once its socket already exists --
/// there's no `Connecting` state here because the connect/accept/
/// passive-handshake phase before that is entirely covered by the
/// offer still sitting in `ClientState::pending_dcc_send`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    Transferring,
    Done,
    Failed(String),
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TransferProgress {
    pub id: u64,
    pub tag: String,
    pub peer_nick: String,
    pub filename: String,
    pub direction: Direction,
    pub size: u64,
    pub bytes_done: u64,
    pub state: TransferState,
}

pub struct TransferHandle {
    pub cancel_tx: mpsc::UnboundedSender<()>,
    pub progress: Arc<Mutex<TransferProgress>>,
}

pub type DccTransfers = Arc<Mutex<HashMap<(String, u64), TransferHandle>>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WaitKey {
    Port(u16),
    Token(String),
}

pub type DccResumeWaiters = Arc<Mutex<HashMap<WaitKey, oneshot::Sender<u64>>>>;

/// The query-buffer key for transfer `id` -- deliberately per-id, not
/// per-nick like DCC CHAT's `dcc:<nick>`, since more than one
/// simultaneous transfer with the same nick must not collide.
pub fn dcc_xfer_key(id: u64) -> String {
    format!("dccxfer:{id}")
}

fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

async fn announce(servers: &Arc<Mutex<Servers>>, tag: &str, key: &str, display: &str, line: String) {
    let mut guard = servers.lock().await;
    if let Some(st) = guard.state_mut(tag) {
        st.get_or_create_query_buffer(key, display).push_system(line);
    }
}

/// Everything a transfer session needs beyond the already-connected
/// socket -- bundled together since `run_send_session`/`run_get_session`
/// pass the same shape and every field is only meaningful alongside the
/// others, not an independent overload.
pub struct TransferParams {
    pub tag: String,
    pub peer_nick: String,
    pub id: u64,
    pub filename: String,
    pub path: PathBuf,
    pub size: u64,
    pub resume_from: u64,
    pub servers: Arc<Mutex<Servers>>,
    pub redraw_tx: mpsc::UnboundedSender<()>,
    pub transfers: DccTransfers,
}

pub async fn run_send_session(mut stream: TcpStream, params: TransferParams) {
    let key = dcc_xfer_key(params.id);
    let display = format!("DCC:{}", params.filename);
    let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel::<()>();
    let progress = Arc::new(Mutex::new(TransferProgress {
        id: params.id,
        tag: params.tag.clone(),
        peer_nick: params.peer_nick.clone(),
        filename: params.filename.clone(),
        direction: Direction::Send,
        size: params.size,
        bytes_done: params.resume_from,
        state: TransferState::Transferring,
    }));
    params.transfers.lock().await.insert(
        (params.tag.clone(), params.id),
        TransferHandle {
            cancel_tx,
            progress: progress.clone(),
        },
    );

    let resume_note = if params.resume_from > 0 {
        format!(" (resuming from {})", format_bytes(params.resume_from))
    } else {
        String::new()
    };
    announce(
        &params.servers,
        &params.tag,
        &key,
        &display,
        irust_config::theme_formats::dcc_send_start(&params.filename, &params.peer_nick, &resume_note),
    )
    .await;
    let _ = params.redraw_tx.send(());

    let outcome = async {
        let mut file = tokio::fs::File::open(&params.path).await.map_err(|e| e.to_string())?;
        if params.resume_from > 0 {
            file.seek(std::io::SeekFrom::Start(params.resume_from))
                .await
                .map_err(|e| e.to_string())?;
        }
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut bytes_done = params.resume_from;
        let mut last_push = Instant::now();
        let mut last_pct_reported = bytes_done_pct(bytes_done, params.size);
        loop {
            let read = tokio::select! {
                r = file.read(&mut buf) => r.map_err(|e| e.to_string())?,
                _ = cancel_rx.recv() => return Err("cancelled".to_string()),
            };
            if read == 0 {
                break;
            }
            if let Err(e) = stream.write_all(&buf[..read]).await {
                return Err(e.to_string());
            }
            bytes_done += read as u64;
            progress.lock().await.bytes_done = bytes_done;
            let pct = bytes_done_pct(bytes_done, params.size);
            if last_push.elapsed() >= PROGRESS_INTERVAL || pct >= last_pct_reported + 10 {
                announce(
                    &params.servers,
                    &params.tag,
                    &key,
                    &display,
                    format!(
                        "-- {}: {}/{} ({pct}%)",
                        params.filename,
                        format_bytes(bytes_done),
                        format_bytes(params.size)
                    ),
                )
                .await;
                let _ = params.redraw_tx.send(());
                last_push = Instant::now();
                last_pct_reported = pct;
            }
        }
        let _ = stream.flush().await;
        Ok(bytes_done)
    }
    .await;

    finish_session(&params, &key, &display, &progress, outcome).await;
}

/// Receives into `params.path` over `stream`, mirroring
/// `run_send_session` in every way except the read/write direction.
/// `params.path`'s filename component must already be sanitized by the
/// caller (`irust_core::dcc_transfer::sanitize_filename`) -- this
/// function trusts it completely and just opens it.
pub async fn run_get_session(mut stream: TcpStream, params: TransferParams) {
    let key = dcc_xfer_key(params.id);
    let display = format!("DCC:{}", params.filename);
    let (cancel_tx, mut cancel_rx) = mpsc::unbounded_channel::<()>();
    let progress = Arc::new(Mutex::new(TransferProgress {
        id: params.id,
        tag: params.tag.clone(),
        peer_nick: params.peer_nick.clone(),
        filename: params.filename.clone(),
        direction: Direction::Get,
        size: params.size,
        bytes_done: params.resume_from,
        state: TransferState::Transferring,
    }));
    params.transfers.lock().await.insert(
        (params.tag.clone(), params.id),
        TransferHandle {
            cancel_tx,
            progress: progress.clone(),
        },
    );

    let resume_note = if params.resume_from > 0 {
        format!(" (resuming from {})", format_bytes(params.resume_from))
    } else {
        String::new()
    };
    announce(
        &params.servers,
        &params.tag,
        &key,
        &display,
        irust_config::theme_formats::dcc_get_start(&params.filename, &params.peer_nick, &resume_note),
    )
    .await;
    let _ = params.redraw_tx.send(());

    let outcome = async {
        if let Some(parent) = params.path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
        }
        let mut open_opts = OpenOptions::new();
        open_opts.write(true).create(true);
        // irssi `dcc_file_create_mode`: applied to files this transfer creates
        // (a resumed file keeps the mode it already has).
        #[cfg(unix)]
        if params.resume_from == 0 {
            open_opts.mode(irust_core::dcc_transfer::file_create_mode());
        }
        if params.resume_from == 0 {
            open_opts.truncate(true);
        }
        let mut file = open_opts.open(&params.path).await.map_err(|e| e.to_string())?;
        if params.resume_from > 0 {
            file.seek(std::io::SeekFrom::Start(params.resume_from))
                .await
                .map_err(|e| e.to_string())?;
        }
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut bytes_done = params.resume_from;
        let mut last_push = Instant::now();
        let mut last_pct_reported = bytes_done_pct(bytes_done, params.size);
        while bytes_done < params.size {
            let read = tokio::select! {
                r = stream.read(&mut buf) => r.map_err(|e| e.to_string())?,
                _ = cancel_rx.recv() => return Err("cancelled".to_string()),
            };
            if read == 0 {
                return Err("connection closed before the transfer finished".to_string());
            }
            if let Err(e) = file.write_all(&buf[..read]).await {
                return Err(e.to_string());
            }
            bytes_done += read as u64;
            progress.lock().await.bytes_done = bytes_done;
            let pct = bytes_done_pct(bytes_done, params.size);
            if last_push.elapsed() >= PROGRESS_INTERVAL || pct >= last_pct_reported + 10 {
                announce(
                    &params.servers,
                    &params.tag,
                    &key,
                    &display,
                    format!(
                        "-- {}: {}/{} ({pct}%)",
                        params.filename,
                        format_bytes(bytes_done),
                        format_bytes(params.size)
                    ),
                )
                .await;
                let _ = params.redraw_tx.send(());
                last_push = Instant::now();
                last_pct_reported = pct;
            }
        }
        let _ = file.flush().await;
        Ok(bytes_done)
    }
    .await;

    finish_session(&params, &key, &display, &progress, outcome).await;
}

fn bytes_done_pct(bytes_done: u64, size: u64) -> u64 {
    bytes_done.saturating_mul(100).checked_div(size).unwrap_or(100).min(100)
}

async fn finish_session(
    params: &TransferParams,
    key: &str,
    display: &str,
    progress: &Arc<Mutex<TransferProgress>>,
    outcome: Result<u64, String>,
) {
    let (state, line) = match outcome {
        Ok(bytes_done) => (
            TransferState::Done,
            irust_config::theme_formats::dcc_transfer_complete(&params.filename, &format_bytes(bytes_done)),
        ),
        Err(reason) if reason == "cancelled" => (
            TransferState::Cancelled,
            irust_config::theme_formats::dcc_transfer_cancelled(&params.filename),
        ),
        Err(reason) => (
            TransferState::Failed(reason.clone()),
            irust_config::theme_formats::dcc_transfer_failed(&params.filename, &reason),
        ),
    };
    progress.lock().await.state = state;
    announce(&params.servers, &params.tag, key, display, line).await;
    let _ = params.redraw_tx.send(());
    params.transfers.lock().await.remove(&(params.tag.clone(), params.id));
}
