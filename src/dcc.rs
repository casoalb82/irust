
use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

use irust_core::Servers;

/// One line to relay out over the DCC socket, or a request to tear the
/// session down locally (from `/wc` on its buffer) -- distinct from the
/// socket just closing on its own (peer quit, network drop), which
/// leaves a "-- ... closed" line behind; a local `/wc` doesn't, since
/// the user already knows.
pub enum DccOutbound {
    Line(String),
    Close,
}

pub struct DccHandle {
    pub write_tx: mpsc::UnboundedSender<DccOutbound>,
}

/// Keyed by (server tag, dcc buffer key) -- a nick could plausibly have
/// a live DCC CHAT on more than one connected server at once.
pub type DccSessions = Arc<Mutex<HashMap<(String, String), DccHandle>>>;

pub fn dcc_key(nick: &str) -> String {
    format!("dcc:{}", nick.to_lowercase())
}

pub async fn run_session(
    stream: TcpStream,
    tag: String,
    peer_nick: String,
    servers: Arc<Mutex<Servers>>,
    redraw_tx: mpsc::UnboundedSender<()>,
    sessions: DccSessions,
) {
    let key = dcc_key(&peer_nick);
    let display = format!("DCC:{peer_nick}");
    let (write_tx, mut write_rx) = mpsc::unbounded_channel::<DccOutbound>();
    sessions
        .lock()
        .await
        .insert((tag.clone(), key.clone()), DccHandle { write_tx });

    {
        let mut guard = servers.lock().await;
        if let Some(st) = guard.state_mut(&tag) {
            st.get_or_create_query_buffer(&key, &display)
                .push_system(format!("-- DCC CHAT established with {peer_nick}"));
        }
    }
    let _ = redraw_tx.send(());

    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let mut closed_locally = false;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(text)) => {
                        let unprefixed = text
                            .strip_prefix("CTCP_MESSAGE ")
                            .or_else(|| text.strip_prefix("CTCP_REPLY "))
                            .unwrap_or(&text);
                        let action_text = unprefixed
                            .strip_prefix('\u{1}')
                            .and_then(|s| s.strip_suffix('\u{1}'))
                            .and_then(|s| s.strip_prefix("ACTION"))
                            .map(|s| s.strip_prefix(' ').unwrap_or(s).to_string());
                        let mut guard = servers.lock().await;
                        if let Some(st) = guard.state_mut(&tag) {
                            let color_index = st.nick_colors.resolve(&peer_nick);
                            let buf = st.get_or_create_query_buffer(&key, &display);
                            match action_text {
                                Some(a) => buf.push_action(peer_nick.clone(), color_index, None, a, false),
                                None => buf.push_chat(peer_nick.clone(), color_index, None, text, false),
                            }
                        }
                        drop(guard);
                        let _ = redraw_tx.send(());
                    }
                    _ => break,
                }
            }
            outbound = write_rx.recv() => {
                match outbound {
                    Some(DccOutbound::Line(text)) => {
                        if write_half.write_all(text.as_bytes()).await.is_err() {
                            break;
                        }
                        if write_half.write_all(b"\r\n").await.is_err() {
                            break;
                        }
                        let _ = write_half.flush().await;
                    }
                    Some(DccOutbound::Close) | None => {
                        closed_locally = true;
                        break;
                    }
                }
            }
        }
    }

    sessions.lock().await.remove(&(tag.clone(), key.clone()));
    if !closed_locally {
        let mut guard = servers.lock().await;
        if let Some(st) = guard.state_mut(&tag) {
            st.get_or_create_query_buffer(&key, &display)
                .push_system(format!("-- DCC CHAT with {peer_nick} closed"));
        }
        drop(guard);
        let _ = redraw_tx.send(());
    }
}
