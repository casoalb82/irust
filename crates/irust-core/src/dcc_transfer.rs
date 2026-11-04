
use std::net::Ipv4Addr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendOffer {
    pub filename: String,
    pub ip: String,
    pub port: u16,
    pub size: u64,
    pub token: Option<String>,
}

/// A `DCC SEND` offer once it's been recorded into `ClientState`.
/// Unlike DCC CHAT's `pending_dcc` (one slot per nick -- CHAT is 1:1),
/// SEND offers are keyed by a monotonic id (`ClientState::next_dcc_id`)
/// since a nick can have more than one outstanding offer at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSendOffer {
    pub id: u64,
    pub peer_nick: String,
    pub filename: String,
    pub ip: String,
    pub port: u16,
    pub size: u64,
    pub token: Option<String>,
    /// `true`: they offered it to us (`/dcc get <id>` accepts it).
    /// `false`: we offered it to them (ours, waiting on their accept).
    pub incoming: bool,
}

/// A `DCC RESUME` request, either direction of the handshake (receiver
/// sends it out; the peer who receives one off the wire parses it back
/// into this same shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DccResumeRequest {
    pub filename: String,
    pub port: u16,
    pub position: u64,
    pub token: Option<String>,
}

/// A `DCC ACCEPT` reply, mirroring `DccResumeRequest` in every way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DccAcceptReply {
    pub filename: String,
    pub port: u16,
    pub position: u64,
    pub token: Option<String>,
}

fn maybe_quote(filename: &str) -> String {
    if filename.contains(' ') {
        format!("\"{filename}\"")
    } else {
        filename.to_string()
    }
}

fn build_offer_wire(filename: &str, ip: Ipv4Addr, port: u16, size: u64, token: Option<&str>) -> String {
    let ip_u32 = u32::from(ip);
    let name = maybe_quote(filename);
    match token {
        Some(t) => format!("\u{1}DCC SEND {name} {ip_u32} {port} {size} {t}\u{1}"),
        None => format!("\u{1}DCC SEND {name} {ip_u32} {port} {size}\u{1}"),
    }
}

/// Active offer: we're listening on `port`, no resume/passive token.
pub fn send_offer_ctcp(filename: &str, local_ip: Ipv4Addr, port: u16, size: u64) -> String {
    build_offer_wire(filename, local_ip, port, size, None)
}

/// Passive offer: we have no listener (behind NAT) -- `port` is
/// literally `0`, `token` is what correlates the eventual
/// reverse-offer-back to this offer.
pub fn passive_offer_ctcp(filename: &str, local_ip: Ipv4Addr, size: u64, token: &str) -> String {
    build_offer_wire(filename, local_ip, 0, size, Some(token))
}

pub fn reverse_offer_ctcp(filename: &str, local_ip: Ipv4Addr, port: u16, size: u64, token: &str) -> String {
    build_offer_wire(filename, local_ip, port, size, Some(token))
}

fn build_resume_wire(dcc_type: &str, filename: &str, port: u16, position: u64, token: Option<&str>) -> String {
    let name = maybe_quote(filename);
    match token {
        Some(t) => format!("\u{1}DCC {dcc_type} {name} {port} {position} {t}\u{1}"),
        None => format!("\u{1}DCC {dcc_type} {name} {port} {position}\u{1}"),
    }
}

/// Active `DCC RESUME <filename> <port> <position>` -- receiver ->
/// sender, sent over the IRC connection before the data socket exists.
pub fn resume_ctcp(filename: &str, port: u16, position: u64) -> String {
    build_resume_wire("RESUME", filename, port, position, None)
}

pub fn passive_resume_ctcp(filename: &str, position: u64, token: &str) -> String {
    build_resume_wire("RESUME", filename, 0, position, Some(token))
}

/// Active `DCC ACCEPT <filename> <port> <position>` -- sender ->
/// receiver, confirms the seek position before the receiver connects.
pub fn accept_ctcp(filename: &str, port: u16, position: u64) -> String {
    build_resume_wire("ACCEPT", filename, port, position, None)
}

/// Passive `DCC ACCEPT <filename> 0 <position> <token>`.
pub fn passive_accept_ctcp(filename: &str, position: u64, token: &str) -> String {
    build_resume_wire("ACCEPT", filename, 0, position, Some(token))
}

/// Pulls one filename token off the front of `s`: a double-quoted run
/// (mIRC's own convention for a filename containing a space) up to the
/// next `"`, or otherwise everything up to the next space. Returns the
/// filename and whatever's left (with any single separating space
/// already consumed, ready for the next `split_whitespace()`).
fn take_filename(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        let (name, after) = rest.split_at(end);
        if name.is_empty() {
            return None;
        }
        Some((name.to_string(), &after[1..]))
    } else {
        let end = s.find(' ').unwrap_or(s.len());
        let (name, after) = s.split_at(end);
        if name.is_empty() {
            return None;
        }
        Some((name.to_string(), after))
    }
}

fn decimal_or_literal_ip(field: &str) -> String {
    field
        .parse::<u32>()
        .map(|n| Ipv4Addr::from(n).to_string())
        .unwrap_or_else(|_| field.to_string())
}

pub fn parse_send_offer(ctcp_arg: &str) -> Option<SendOffer> {
    let mut it = ctcp_arg.splitn(2, ' ');
    let dcc_type = it.next()?;
    if !dcc_type.eq_ignore_ascii_case("SEND") {
        return None;
    }
    let rest = it.next()?;
    let (filename, rest) = take_filename(rest)?;
    let mut fields = rest.split_whitespace();
    let ip = decimal_or_literal_ip(fields.next()?);
    let port = fields.next()?.parse::<u16>().ok()?;
    let size = fields.next()?.parse::<u64>().ok()?;
    let token = fields.next().map(str::to_string);
    Some(SendOffer {
        filename,
        ip,
        port,
        size,
        token,
    })
}

/// Parses a `DCC RESUME` argument (active or passive).
pub fn parse_resume(ctcp_arg: &str) -> Option<DccResumeRequest> {
    let mut it = ctcp_arg.splitn(2, ' ');
    let dcc_type = it.next()?;
    if !dcc_type.eq_ignore_ascii_case("RESUME") {
        return None;
    }
    let rest = it.next()?;
    let (filename, rest) = take_filename(rest)?;
    let mut fields = rest.split_whitespace();
    let port = fields.next()?.parse::<u16>().ok()?;
    let position = fields.next()?.parse::<u64>().ok()?;
    let token = fields.next().map(str::to_string);
    Some(DccResumeRequest {
        filename,
        port,
        position,
        token,
    })
}

/// Parses a `DCC ACCEPT` argument (active or passive).
pub fn parse_accept(ctcp_arg: &str) -> Option<DccAcceptReply> {
    let mut it = ctcp_arg.splitn(2, ' ');
    let dcc_type = it.next()?;
    if !dcc_type.eq_ignore_ascii_case("ACCEPT") {
        return None;
    }
    let rest = it.next()?;
    let (filename, rest) = take_filename(rest)?;
    let mut fields = rest.split_whitespace();
    let port = fields.next()?.parse::<u16>().ok()?;
    let position = fields.next()?.parse::<u64>().ok()?;
    let token = fields.next().map(str::to_string);
    Some(DccAcceptReply {
        filename,
        port,
        position,
        token,
    })
}

pub fn sanitize_filename(filename: &str) -> Option<String> {
    let name = std::path::Path::new(filename).file_name()?.to_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// Permission bits for a freshly received file: irssi's
/// `dcc_file_create_mode`, an int whose DIGITS are the octal mode
/// (`644` -> 0o644, `octal2dec` in `core/misc.c`).
pub fn file_create_mode() -> u32 {
    let n = irust_config::settings_net::int_of("dcc_file_create_mode");
    let mut mode = 0u32;
    let mut mult = 1u32;
    let mut rest = n.max(0) as u64;
    while rest != 0 {
        mode += mult * (rest % 10) as u32;
        rest /= 10;
        mult *= 8;
    }
    mode
}

/// `dcc_send_replace_space_with_underscore`: the name announced in a DCC
/// SEND offer (`dcc-send.c`).
pub fn outgoing_filename(name: &str) -> String {
    if irust_config::settings_net::bool_of("dcc_send_replace_space_with_underscore") {
        name.replace(' ', "_")
    } else {
        name.to_string()
    }
}

/// `dcc_upload_path`: a relative `/dcc send` path is looked up under it
/// (`dcc_send_get_file`); an absolute or `~/` one is used as given.
pub fn resolve_upload_path(arg: &str) -> std::path::PathBuf {
    let expand = |p: &str| -> std::path::PathBuf {
        match (p.strip_prefix("~/"), p == "~") {
            (Some(rest), _) => home().join(rest),
            (None, true) => home(),
            _ => std::path::PathBuf::from(p),
        }
    };
    let path = expand(arg);
    if path.is_absolute() {
        return path;
    }
    let base = irust_config::settings_net::str_of("dcc_upload_path");
    if base.is_empty() {
        path
    } else {
        expand(&base).join(arg)
    }
}

fn home() -> std::path::PathBuf {
    std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default()
}

/// `dcc_autorename`: when the target file exists, `name.1`, `name.2`, ...
/// (`dcc_get_rename_file`). Unchanged if autorename is off or the file is
/// not there yet.
pub fn autorename(path: &std::path::Path) -> std::path::PathBuf {
    if !irust_config::settings_net::bool_of("dcc_autorename") || !path.exists() {
        return path.to_path_buf();
    }
    (1u32..)
        .map(|n| {
            let mut os = path.as_os_str().to_owned();
            os.push(format!(".{n}"));
            std::path::PathBuf::from(os)
        })
        .find(|p| !p.exists())
        .expect("u32 range exhausted")
}

/// `dcc_timeout`: how long an unanswered offer / pending connection may
/// wait before being dropped.
pub fn offer_timeout() -> std::time::Duration {
    std::time::Duration::from_millis(irust_config::settings_net::time_ms_of("dcc_timeout"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_offer_round_trips() {
        let ctcp = send_offer_ctcp("file.txt", Ipv4Addr::new(192, 168, 1, 1), 5000, 1024);
        assert_eq!(ctcp, "\u{1}DCC SEND file.txt 3232235777 5000 1024\u{1}");
        let arg = ctcp.trim_matches('\u{1}').strip_prefix("DCC ").unwrap();
        let offer = parse_send_offer(arg).unwrap();
        assert_eq!(
            offer,
            SendOffer {
                filename: "file.txt".to_string(),
                ip: "192.168.1.1".to_string(),
                port: 5000,
                size: 1024,
                token: None
            }
        );
    }

    #[test]
    fn passive_offer_round_trips_with_port_zero_and_token() {
        let ctcp = passive_offer_ctcp("movie.mkv", Ipv4Addr::new(10, 0, 0, 1), 999_999, "abc123");
        let arg = ctcp.trim_matches('\u{1}').strip_prefix("DCC ").unwrap();
        let offer = parse_send_offer(arg).unwrap();
        assert_eq!(offer.port, 0);
        assert_eq!(offer.token.as_deref(), Some("abc123"));
        assert_eq!(offer.size, 999_999);
    }

    #[test]
    fn reverse_offer_back_carries_the_same_token_with_a_real_port() {
        let ctcp = reverse_offer_ctcp("movie.mkv", Ipv4Addr::new(203, 0, 113, 5), 6000, 999_999, "abc123");
        let arg = ctcp.trim_matches('\u{1}').strip_prefix("DCC ").unwrap();
        let offer = parse_send_offer(arg).unwrap();
        assert_eq!(offer.port, 6000);
        assert_eq!(offer.token.as_deref(), Some("abc123"));
    }

    #[test]
    fn quoted_filename_with_a_space_round_trips() {
        let ctcp = send_offer_ctcp("my file.txt", Ipv4Addr::new(127, 0, 0, 1), 1234, 10);
        assert!(ctcp.contains("\"my file.txt\""));
        let arg = ctcp.trim_matches('\u{1}').strip_prefix("DCC ").unwrap();
        let offer = parse_send_offer(arg).unwrap();
        assert_eq!(offer.filename, "my file.txt");
    }

    #[test]
    fn non_send_or_malformed_offer_is_rejected() {
        assert!(parse_send_offer("CHAT chat 3232235777 5000").is_none());
        assert!(parse_send_offer("SEND file.txt notanip 5000 1024").is_some()); // falls back to literal token, still parses
        assert!(parse_send_offer("SEND file.txt 3232235777 notaport 1024").is_none());
        assert!(parse_send_offer("SEND").is_none());
    }

    #[test]
    fn resume_and_accept_round_trip_active_and_passive() {
        let resume = resume_ctcp("file.txt", 5000, 4096);
        let arg = resume.trim_matches('\u{1}').strip_prefix("DCC ").unwrap();
        assert_eq!(
            parse_resume(arg).unwrap(),
            DccResumeRequest {
                filename: "file.txt".to_string(),
                port: 5000,
                position: 4096,
                token: None
            }
        );

        let presume = passive_resume_ctcp("file.txt", 4096, "tok1");
        let arg = presume.trim_matches('\u{1}').strip_prefix("DCC ").unwrap();
        let parsed = parse_resume(arg).unwrap();
        assert_eq!(parsed.port, 0);
        assert_eq!(parsed.token.as_deref(), Some("tok1"));

        let accept = accept_ctcp("file.txt", 5000, 4096);
        let arg = accept.trim_matches('\u{1}').strip_prefix("DCC ").unwrap();
        assert_eq!(
            parse_accept(arg).unwrap(),
            DccAcceptReply {
                filename: "file.txt".to_string(),
                port: 5000,
                position: 4096,
                token: None
            }
        );

        let paccept = passive_accept_ctcp("file.txt", 4096, "tok1");
        let arg = paccept.trim_matches('\u{1}').strip_prefix("DCC ").unwrap();
        let parsed = parse_accept(arg).unwrap();
        assert_eq!(parsed.port, 0);
        assert_eq!(parsed.token.as_deref(), Some("tok1"));
    }

    #[test]
    fn resume_rejects_wrong_dcc_type() {
        assert!(parse_resume("ACCEPT file.txt 5000 4096").is_none());
        assert!(parse_accept("RESUME file.txt 5000 4096").is_none());
    }

    #[test]
    fn sanitize_filename_strips_traversal_and_absolute_paths() {
        assert_eq!(sanitize_filename("report.pdf"), Some("report.pdf".to_string()));
        assert_eq!(sanitize_filename("../../etc/passwd"), Some("passwd".to_string()));
        assert_eq!(sanitize_filename("/etc/passwd"), Some("passwd".to_string()));
        assert_eq!(sanitize_filename("a/b/../../c.txt"), Some("c.txt".to_string()));
        assert_eq!(sanitize_filename(".."), None);
        assert_eq!(sanitize_filename(""), None);
    }

    #[test]
    fn create_mode_reads_the_digits_as_octal() {
        let _net = crate::test_support::net_guard();
        assert_eq!(file_create_mode(), 0o644);
        crate::test_support::net_set("dcc_file_create_mode", "600");
        assert_eq!(file_create_mode(), 0o600);
        crate::test_support::net_set("dcc_file_create_mode", "0");
        assert_eq!(file_create_mode(), 0);
    }

    #[test]
    fn outgoing_filename_and_upload_path_follow_their_settings() {
        let _net = crate::test_support::net_guard();
        assert_eq!(outgoing_filename("a b.txt"), "a b.txt");
        crate::test_support::net_set("dcc_send_replace_space_with_underscore", "on");
        assert_eq!(outgoing_filename("a b.txt"), "a_b.txt");
        assert_eq!(resolve_upload_path("/abs/f"), std::path::PathBuf::from("/abs/f"));
        assert_eq!(resolve_upload_path("rel"), home().join("rel"), "default upload path is ~");
        crate::test_support::net_set("dcc_upload_path", "/srv/up");
        assert_eq!(resolve_upload_path("rel"), std::path::PathBuf::from("/srv/up/rel"));
        crate::test_support::net_set("dcc_upload_path", "");
        assert_eq!(resolve_upload_path("rel"), std::path::PathBuf::from("rel"));
    }

    #[test]
    fn autorename_appends_a_counter_only_when_enabled_and_needed() {
        let _net = crate::test_support::net_guard();
        let dir = std::env::temp_dir().join(format!("irust-dcc-rename-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("f.txt");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(autorename(&f), f, "off by default");
        crate::test_support::net_set("dcc_autorename", "on");
        assert_eq!(autorename(&f), dir.join("f.txt.1"));
        std::fs::write(dir.join("f.txt.1"), b"x").unwrap();
        assert_eq!(autorename(&f), dir.join("f.txt.2"));
        assert_eq!(autorename(&dir.join("new.txt")), dir.join("new.txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
