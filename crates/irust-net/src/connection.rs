use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::StreamExt;
use tokio::io::{split, AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::time::{Duration, Instant};
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_util::codec::FramedRead;

use crate::codec::BytesLinesCodec;
use crate::tls;

enum MaybeTlsStream {
    Tls(TlsStream<TcpStream>),
    Plain(TcpStream),
}

impl AsyncRead for MaybeTlsStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTlsStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Tls(s) => Pin::new(s).poll_flush(cx),
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

fn strip_line_breaks(line: &str) -> std::borrow::Cow<'_, str> {
    if line.contains(['\r', '\n', '\0']) {
        line.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).collect::<String>().into()
    } else {
        line.into()
    }
}

#[cfg(test)]
const MIN_WRITE_INTERVAL: Duration = Duration::from_millis(600);

fn pace_delay(last_write: Option<Instant>, min_interval: Duration, now: Instant) -> Option<Duration> {
    let elapsed = now.saturating_duration_since(last_write?);
    (elapsed < min_interval).then(|| min_interval - elapsed)
}

async fn tls_handshake(
    tcp: TcpStream,
    host: &str,
    verify_tls: bool,
    tls_fingerprint: Option<&str>,
    client_auth: Option<(&std::path::Path, &std::path::Path)>,
) -> anyhow::Result<MaybeTlsStream> {
    // A pinned fingerprint wins over `verify_tls` -- pinning is its own,
    // stricter verification mode (exact-cert match, not "trust
    // anything"), orthogonal to the CA-chain trust-or-skip choice
    // `verify_tls` makes. See `tls::connector_pinned`'s own doc comment.
    let connector = match tls_fingerprint {
        Some(fp) => tls::connector_pinned(fp, client_auth)?,
        None if verify_tls => tls::connector(client_auth)?,
        None => tls::connector_insecure(client_auth)?,
    };
    let server_name = ServerName::try_from(host.to_string())?;
    Ok(MaybeTlsStream::Tls(connector.connect(server_name, tcp).await?))
}

pub struct Connection {
    reader: FramedRead<ReadHalf<MaybeTlsStream>, BytesLinesCodec>,
    writer: WriteHalf<MaybeTlsStream>,
    /// This socket's local address, captured before the TLS handshake
    /// consumes the plain `TcpStream` (its split halves don't expose it
    /// back out). Best-effort input for DCC CHAT's own IP-address guess
    /// -- the address the ircd sees us connect from, which is at least
    /// as good a guess as any client without NAT-PMP/UPnP can make.
    local_addr: Option<std::net::SocketAddr>,
    tag: String,
    /// When the last line actually went out -- `None` until the first
    /// `write_line` call. Drives `write_line`'s own flood-safety
    /// pacing (`pace_delay`); every caller in the codebase gets this
    /// for free, no call-site changes needed anywhere.
    last_write: Option<Instant>,
    rawlog: Option<std::fs::File>,
}

/// Everything `Connection::connect` needs to open one link -- bundled
/// into a struct because the individual fields (address family
/// override, TLS client cert, fingerprint pin, ...) are each only
/// meaningful together with the others, not independent overloads.
pub struct ConnectOptions<'a> {
    pub host: &'a str,
    pub port: u16,
    pub tls: bool,
    pub verify_tls: bool,
    pub ip_version: Option<u8>,
    pub client_auth: Option<(&'a std::path::Path, &'a std::path::Path)>,
    pub tag: &'a str,
    pub tls_fingerprint: Option<&'a str>,
}

fn tune_tcp(tcp: &TcpStream) {
    let _ = tcp.set_nodelay(true);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(std::time::Duration::from_secs(60))
        .with_interval(std::time::Duration::from_secs(20));
    let _ = socket2::SockRef::from(tcp).set_tcp_keepalive(&keepalive);
}

async fn dial(host: &str, port: u16, ip_version: Option<u8>, source: &str) -> anyhow::Result<TcpStream> {
    let source_addrs: Vec<std::net::IpAddr> = if source.is_empty() {
        Vec::new()
    } else {
        tokio::net::lookup_host((source, 0)).await?.map(|a| a.ip()).collect()
    };
    let mut last_err: Option<anyhow::Error> = None;
    let mut tried = false;
    for addr in tokio::net::lookup_host((host, port)).await? {
        if let Some(want) = ip_version {
            if (want == 6) != addr.is_ipv6() {
                continue;
            }
        }
        tried = true;
        let socket = if addr.is_ipv6() { tokio::net::TcpSocket::new_v6()? } else { tokio::net::TcpSocket::new_v4()? };
        if let Some(ip) = source_addrs.iter().find(|ip| ip.is_ipv6() == addr.is_ipv6()) {
            socket.bind(std::net::SocketAddr::new(*ip, 0))?;
        }
        match socket.connect(addr).await {
            Ok(tcp) => return Ok(tcp),
            Err(e) => last_err = Some(e.into()),
        }
    }
    match (last_err, ip_version) {
        (Some(e), _) => Err(e),
        (None, Some(want)) if !tried => Err(anyhow::anyhow!("{host} has no IPv{want} address")),
        (None, _) => Err(anyhow::anyhow!("could not resolve {host}")),
    }
}

impl Connection {
    pub async fn connect(opts: ConnectOptions<'_>) -> anyhow::Result<Self> {
        let ConnectOptions {
            host,
            port,
            tls,
            verify_tls,
            ip_version,
            client_auth,
            tag,
            tls_fingerprint,
        } = opts;
        // irssi's `server_connect_timeout` bounds the whole connect (TCP +
        // TLS handshake), not just the SYN.
        let timeout = Duration::from_millis(irust_config::settings_net::time_ms_of("server_connect_timeout"));
        let (local_addr, stream) = tokio::time::timeout(timeout, async {
            let (dial_host, dial_port) = crate::proxy::tcp_target().unwrap_or_else(|| (host.to_string(), port));
            let source = irust_config::settings_net::str_of("hostname");
            let tcp = dial(&dial_host, dial_port, ip_version, &source).await?;
            tune_tcp(&tcp);
            let local_addr = tcp.local_addr().ok();
            let stream = if tls {
                tls_handshake(tcp, host, verify_tls, tls_fingerprint, client_auth).await?
            } else {
                MaybeTlsStream::Plain(tcp)
            };
            anyhow::Ok((local_addr, stream))
        })
        .await
        .map_err(|_| anyhow::anyhow!("connection to {host}:{port} timed out after {}s", timeout.as_secs()))??;

        let (read_half, write_half) = split(stream);
        let reader = FramedRead::new(read_half, BytesLinesCodec);

        Ok(Self {
            reader,
            writer: write_half,
            local_addr,
            tag: tag.to_string(),
            last_write: None,
            rawlog: None,
        })
    }

    pub fn local_addr(&self) -> Option<std::net::SocketAddr> {
        self.local_addr
    }

    /// `/rawlog open <file>` -- starts appending every raw line (TX
    /// redacted the same way `debug_log` is, RX as-is) to `path` from
    /// now on. Opens in append mode so a repeated `/rawlog open` onto
    /// the same file doesn't clobber what's already there.
    pub fn open_rawlog(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        self.rawlog = Some(file);
        Ok(())
    }

    /// `/rawlog close` -- stops appending, drops the file handle.
    /// `true` if a rawlog was actually active (matches `LogToggle`'s
    /// own already-off-is-a-no-op-but-still-reported convention).
    pub fn close_rawlog(&mut self) -> bool {
        self.rawlog.take().is_some()
    }

    pub fn rawlog_active(&self) -> bool {
        self.rawlog.is_some()
    }

    fn write_rawlog(&mut self, direction: &str, line: &str) {
        use std::io::Write;
        if let Some(file) = &mut self.rawlog {
            let _ = writeln!(file, "{direction} {line}");
        }
    }

    pub async fn upgrade_to_tls(
        self,
        host: &str,
        verify_tls: bool,
        tls_fingerprint: Option<&str>,
        client_auth: Option<(&std::path::Path, &std::path::Path)>,
    ) -> anyhow::Result<Self> {
        if !self.reader.read_buffer().is_empty() {
            anyhow::bail!(
                "STARTTLS: server sent data past the 670 reply before the handshake -- refusing to upgrade and lose it"
            );
        }
        let read_half = self.reader.into_inner();
        let stream = read_half.unsplit(self.writer);
        let MaybeTlsStream::Plain(tcp) = stream else {
            anyhow::bail!("STARTTLS: connection is already TLS-wrapped, nothing to upgrade");
        };

        let upgraded = tls_handshake(tcp, host, verify_tls, tls_fingerprint, client_auth).await?;
        let (read_half, write_half) = split(upgraded);
        Ok(Self {
            reader: FramedRead::new(read_half, BytesLinesCodec),
            writer: write_half,
            local_addr: self.local_addr,
            tag: self.tag,
            last_write: self.last_write,
            rawlog: self.rawlog,
        })
    }

    pub async fn read_line(&mut self) -> Option<anyhow::Result<String>> {
        let result = match self.reader.next().await {
            Some(Ok(bytes)) => Some(Ok(crate::recode::decode_incoming(&bytes, &self.tag))),
            Some(Err(err)) => Some(Err(err.into())),
            None => None,
        };
        if let Some(Ok(line)) = &result {
            irust_config::debug_log::line("RX", &self.tag, line);
            self.write_rawlog("RX", line);
        }
        result
    }

    pub async fn write_line(&mut self, line: &str) -> anyhow::Result<()> {
        let min_interval =
            Duration::from_millis(irust_config::settings::FLOOD_INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed));
        if let Some(delay) = pace_delay(self.last_write, min_interval, Instant::now()) {
            tokio::time::sleep(delay).await;
        }
        self.last_write = Some(Instant::now());
        // One write is one IRC line: drop CR/LF/NUL so nothing built from
        // server- or user-supplied text can smuggle in a second command.
        let clean = strip_line_breaks(line);
        let line = clean.as_ref();
        let redacted = redact_for_debug_log(line);
        irust_config::debug_log::line("TX", &self.tag, &redacted);
        self.write_rawlog("TX", &redacted);
        let encoded = crate::recode::encode_outgoing(line, &self.tag);
        self.writer.write_all(&encoded).await?;
        self.writer.write_all(b"\r\n").await?;
        self.writer.flush().await?;
        Ok(())
    }
}

fn redact_for_debug_log(line: &str) -> std::borrow::Cow<'_, str> {
    let upper_cmd = line.split(' ').next().unwrap_or("").to_ascii_uppercase();
    match upper_cmd.as_str() {
        "PASS" | "OPER" => return std::borrow::Cow::Owned(format!("{upper_cmd} [redacted]")),
        "AUTHENTICATE" => {
            // Only the mechanism-selection line ("AUTHENTICATE PLAIN"/
            // "AUTHENTICATE EXTERNAL") and EXTERNAL's empty "+" response
            // carry no secret -- anything else is the actual credential
            // payload (PLAIN's base64 account+password).
            let arg = line.strip_prefix("AUTHENTICATE ").unwrap_or("");
            if !matches!(arg, "+" | "PLAIN" | "EXTERNAL") {
                return std::borrow::Cow::Borrowed("AUTHENTICATE [redacted]");
            }
        }
        _ => {}
    }
    if matches!(upper_cmd.as_str(), "PRIVMSG" | "NOTICE") {
        let lower = line.to_ascii_lowercase();
        let mut parts = lower.splitn(3, ' ');
        parts.next(); // PRIVMSG/NOTICE
        let target = parts.next().unwrap_or("");
        let text = parts.next().unwrap_or("").trim_start_matches(':');
        if matches!(target, "nickserv" | "ns" | "cs")
            && (text.starts_with("identify") || text.starts_with("register"))
        {
            let sub = text.split_whitespace().next().unwrap_or("");
            return std::borrow::Cow::Owned(format!("{upper_cmd} {target} :{sub} [redacted]"));
        }
    }
    std::borrow::Cow::Borrowed(line)
}

#[cfg(test)]
mod tests {
    #[test]
    fn strip_line_breaks_removes_cr_lf_nul_and_borrows_clean_lines() {
        assert_eq!(strip_line_breaks("JOIN #a\r\nQUIT :x\0"), "JOIN #aQUIT :x");
        assert!(matches!(strip_line_breaks("PRIVMSG #a :hi"), std::borrow::Cow::Borrowed(_)));
    }

    use super::*;

    #[test]
    fn pace_delay_is_none_for_the_very_first_write() {
        assert_eq!(pace_delay(None, MIN_WRITE_INTERVAL, Instant::now()), None);
    }

    #[tokio::test]
    async fn upgrade_to_tls_against_a_non_tls_listener_fails_cleanly_not_a_panic_or_hang() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = stream.write_all(b"not a tls server hello at all").await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });

        let conn = Connection::connect(ConnectOptions {
            host: "127.0.0.1",
            port,
            tls: false,
            verify_tls: true,
            ip_version: None,
            client_auth: None,
            tag: "test",
            tls_fingerprint: None,
        })
        .await
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            conn.upgrade_to_tls("127.0.0.1", true, None, None),
        )
        .await;
        match result {
            Ok(upgrade_result) => assert!(
                upgrade_result.is_err(),
                "a non-TLS peer must fail the handshake, not succeed"
            ),
            Err(_) => panic!("upgrade_to_tls hung instead of failing the handshake"),
        }
    }

    #[tokio::test]
    async fn upgrade_to_tls_refuses_when_the_reader_has_buffered_bytes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                // No trailing newline -- LinesCodec can't decode this
                // into a complete line yet, so it stays in its internal
                // buffer rather than ever reaching `read_line`'s caller.
                let _ = stream.write_all(b"partial-line-no-newline").await;
                // Keep the socket open so the read side doesn't see EOF
                // and misinterpret the partial bytes as a complete line.
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });

        let mut conn = Connection::connect(ConnectOptions {
            host: "127.0.0.1",
            port,
            tls: false,
            verify_tls: true,
            ip_version: None,
            client_auth: None,
            tag: "test",
            tls_fingerprint: None,
        })
        .await
        .unwrap();
        let _ = tokio::time::timeout(Duration::from_millis(200), conn.read_line()).await;
        assert!(
            !conn.reader.read_buffer().is_empty(),
            "test setup: the partial line must actually be buffered"
        );

        let result = conn.upgrade_to_tls("127.0.0.1", true, None, None).await;
        assert!(
            result.is_err(),
            "must refuse to upgrade with unread bytes still buffered"
        );
    }

    #[test]
    fn pace_delay_is_none_once_enough_time_has_passed() {
        let last = Instant::now();
        let now = last + MIN_WRITE_INTERVAL + Duration::from_millis(1);
        assert_eq!(pace_delay(Some(last), MIN_WRITE_INTERVAL, now), None);
    }

    #[test]
    fn pace_delay_returns_the_exact_remaining_wait_when_called_too_soon() {
        let last = Instant::now();
        let now = last + Duration::from_millis(200);
        assert_eq!(
            pace_delay(Some(last), MIN_WRITE_INTERVAL, now),
            Some(MIN_WRITE_INTERVAL - Duration::from_millis(200))
        );
    }

    #[tokio::test]
    async fn read_line_survives_a_non_utf8_line_instead_of_erroring() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                use tokio::io::AsyncWriteExt;
                let _ = stream.write_all(b"PRIVMSG #chan :\xff\xfe not valid utf8\r\n").await;
                let _ = stream.write_all(b"PRIVMSG #chan :still alive\r\n").await;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });

        let mut conn = Connection::connect(ConnectOptions {
            host: "127.0.0.1",
            port,
            tls: false,
            verify_tls: true,
            ip_version: None,
            client_auth: None,
            tag: "test",
            tls_fingerprint: None,
        })
        .await
        .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), conn.read_line())
            .await
            .expect("must not hang")
            .expect("connection must not close")
            .expect("must decode, not error, on invalid UTF-8");
        // 0xFF 0xFE is not UTF-8, so `recode_fallback` (CP1252 by default,
        // like irssi) reads it as "ÿþ".
        assert_eq!(first, "PRIVMSG #chan :ÿþ not valid utf8");

        let second = tokio::time::timeout(Duration::from_secs(2), conn.read_line())
            .await
            .expect("must not hang")
            .expect("connection must still be open for the next line")
            .expect("must decode cleanly");
        assert_eq!(second, "PRIVMSG #chan :still alive");
    }

    #[test]
    fn pace_delay_at_exactly_the_boundary_needs_no_wait() {
        let last = Instant::now();
        let now = last + MIN_WRITE_INTERVAL;
        assert_eq!(pace_delay(Some(last), MIN_WRITE_INTERVAL, now), None);
    }

    #[test]
    fn redact_hides_the_real_bouncer_password() {
        assert_eq!(redact_for_debug_log("PASS abcX:hunter2"), "PASS [redacted]");
    }

    #[test]
    fn redact_hides_the_sasl_plain_payload_but_keeps_mechanism_selection() {
        assert_eq!(redact_for_debug_log("AUTHENTICATE PLAIN"), "AUTHENTICATE PLAIN");
        assert_eq!(redact_for_debug_log("AUTHENTICATE +"), "AUTHENTICATE +");
        assert_eq!(
            redact_for_debug_log("AUTHENTICATE YWJjWABhYmNYAGh1bnRlcjI="),
            "AUTHENTICATE [redacted]"
        );
    }

    #[test]
    fn redact_hides_the_oper_password() {
        assert_eq!(redact_for_debug_log("OPER abcX hunter2"), "OPER [redacted]");
    }

    #[test]
    fn redact_hides_nickserv_identify_but_keeps_the_command_shape() {
        assert_eq!(
            redact_for_debug_log("PRIVMSG NickServ :identify hunter2"),
            "PRIVMSG nickserv :identify [redacted]"
        );
        assert_eq!(
            redact_for_debug_log("PRIVMSG NS :IDENTIFY hunter2"),
            "PRIVMSG ns :identify [redacted]"
        );
    }

    #[test]
    fn redact_leaves_ordinary_lines_alone() {
        assert_eq!(
            redact_for_debug_log("PRIVMSG #chatnet :hello there"),
            "PRIVMSG #chatnet :hello there"
        );
        assert_eq!(redact_for_debug_log("JOIN #chatnet"), "JOIN #chatnet");
        assert_eq!(
            redact_for_debug_log("PRIVMSG NickServ :info someone"),
            "PRIVMSG NickServ :info someone"
        );
    }
}

#[cfg(test)]
mod tcp_tuning_tests {
    use super::*;

    #[tokio::test]
    async fn chat_connections_get_nodelay_and_keepalive() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let tcp = TcpStream::connect(addr).await.unwrap();
        assert!(!tcp.nodelay().unwrap(), "precondition: OS default has Nagle on");
        tune_tcp(&tcp);
        assert!(tcp.nodelay().unwrap());
        assert!(socket2::SockRef::from(&tcp).keepalive().unwrap());
    }

    /// irssi's `hostname` setting: the source address of the outgoing
    /// connection. 127.0.0.2 is a valid loopback source on Linux, so this
    /// runs without any real network.
    #[tokio::test]
    async fn hostname_setting_binds_the_source_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let tcp = dial("127.0.0.1", addr.port(), None, "127.0.0.2").await.unwrap();
        assert_eq!(tcp.local_addr().unwrap().ip().to_string(), "127.0.0.2");
        // No address of the target's family among the source addresses: unbound.
        let tcp = dial("127.0.0.1", addr.port(), None, "::1").await.unwrap();
        assert_eq!(tcp.local_addr().unwrap().ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn dial_forced_to_the_wrong_family_reports_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let err = dial("127.0.0.1", port, Some(6), "").await.unwrap_err();
        assert!(err.to_string().contains("no IPv6 address"), "{err}");
    }
}
