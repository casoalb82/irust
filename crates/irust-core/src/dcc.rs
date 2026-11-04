//! Pure `DCC CHAT` CTCP wire-format helpers -- encoding the offer line
//! we send, decoding one we receive. The actual TCP session (listener,
//! connector, read/write loop) is I/O and lives above this crate, in
//! `main.rs`/`src/dcc.rs`, same split as everywhere else in this
//! codebase.

use std::net::Ipv4Addr;

pub fn offer_ctcp(local_ip: Ipv4Addr, port: u16) -> String {
    let ip_u32 = u32::from(local_ip);
    format!("\u{1}DCC CHAT chat {ip_u32} {port}\u{1}")
}

pub fn parse_offer(ctcp_arg: &str) -> Option<(String, u16)> {
    let mut fields = ctcp_arg.split(' ').filter(|s| !s.is_empty());
    let dcc_type = fields.next()?;
    if !dcc_type.eq_ignore_ascii_case("CHAT") {
        return None;
    }
    let _argument = fields.next()?; // historically "chat", unused
    let addr = fields.next()?;
    let port = fields.next()?.parse::<u16>().ok()?;
    let ip = addr
        .parse::<u32>()
        .map(|n| Ipv4Addr::from(n).to_string())
        .unwrap_or_else(|_| addr.to_string());
    Some((ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_ctcp_encodes_the_ip_as_a_decimal_u32() {
        let ctcp = offer_ctcp(Ipv4Addr::new(192, 168, 1, 1), 5000);
        assert_eq!(ctcp, "\u{1}DCC CHAT chat 3232235777 5000\u{1}");
    }

    #[test]
    fn parse_offer_round_trips_the_classic_encoding() {
        assert_eq!(
            parse_offer("CHAT chat 3232235777 5000"),
            Some(("192.168.1.1".to_string(), 5000))
        );
    }

    #[test]
    fn parse_offer_falls_back_to_a_literal_address() {
        assert_eq!(
            parse_offer("CHAT chat 192.168.1.1 5000"),
            Some(("192.168.1.1".to_string(), 5000))
        );
    }

    #[test]
    fn parse_offer_rejects_a_non_chat_dcc_type() {
        assert_eq!(parse_offer("SEND file.txt 3232235777 5000 1024"), None);
    }

    #[test]
    fn parse_offer_rejects_a_malformed_line() {
        assert_eq!(parse_offer("CHAT chat 3232235777"), None);
        assert_eq!(parse_offer(""), None);
    }
}
