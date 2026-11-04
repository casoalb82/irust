
/// Scans a raw wire line (bytes, not yet decoded) for the target its
/// command addresses -- the first parameter, if that parameter isn't
/// itself the trailing (`:`-prefixed) one. Returns `None` for a command
/// with no leading (non-trailing) parameter at all (e.g. a bare `PING
/// :token`), matching "no specific target to look up an override for."
pub fn extract_target(line: &[u8]) -> Option<&[u8]> {
    let mut rest = line;
    if rest.first() == Some(&b':') {
        let sp = rest.iter().position(|&b| b == b' ')?;
        rest = &rest[sp + 1..];
    }
    let sp = rest.iter().position(|&b| b == b' ')?;
    rest = &rest[sp + 1..];
    let first_non_space = rest.iter().position(|&b| b != b' ')?;
    rest = &rest[first_non_space..];
    if rest.first() == Some(&b':') {
        return None;
    }
    let end = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
    Some(&rest[..end])
}

pub fn decode_incoming(line: &[u8], network_tag: &str) -> String {
    use irust_config::settings_net::{bool_of, str_of};
    // irssi `recode_in` (`src/core/recode.c`), minus the terminal-charset
    // half (irust's terminal side is always UTF-8):
    if !bool_of("recode") {
        return String::from_utf8_lossy(line).into_owned();
    }
    let utf8_valid = std::str::from_utf8(line).is_ok();
    // `recode_autodetect_utf8`: text that is already valid UTF-8 is trusted
    // over a configured conversion.
    if utf8_valid && bool_of("recode_autodetect_utf8") {
        return String::from_utf8_lossy(line).into_owned();
    }
    let target = extract_target(line).and_then(|t| std::str::from_utf8(t).ok());
    let decode_with = |label: &str| encoding_rs::Encoding::for_label(label.as_bytes()).map(|e| e.decode(line).0.into_owned());
    if let Some(text) = irust_config::recode::resolve(network_tag, target).and_then(|c| decode_with(&c)) {
        return text;
    }
    if utf8_valid {
        return String::from_utf8_lossy(line).into_owned();
    }
    // Not UTF-8 and no usable conversion: `recode_fallback` (default CP1252).
    decode_with(&str_of("recode_fallback")).unwrap_or_else(|| String::from_utf8_lossy(line).into_owned())
}

pub fn encode_outgoing(line: &str, network_tag: &str) -> Vec<u8> {
    if !irust_config::settings_net::bool_of("recode") {
        return line.as_bytes().to_vec();
    }
    let target = extract_target(line.as_bytes()).and_then(|t| std::str::from_utf8(t).ok());
    let charset = irust_config::recode::resolve(network_tag, target)
        .or_else(|| Some(irust_config::settings::recode_out_default_charset()).filter(|d| !d.is_empty()));
    match charset {
        Some(charset) => match encoding_rs::Encoding::for_label(charset.as_bytes()) {
            Some(encoding) => encoding.encode(line).0.into_owned(),
            None => line.as_bytes().to_vec(),
        },
        None => line.as_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_recode_default() -> std::sync::MutexGuard<'static, ()> {
        crate::NET_SETTINGS_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn extract_target_finds_the_channel_in_a_privmsg() {
        assert_eq!(extract_target(b"PRIVMSG #chan :hello there"), Some(&b"#chan"[..]));
    }

    #[test]
    fn extract_target_finds_the_target_past_a_prefix() {
        assert_eq!(
            extract_target(b":nick!user@host PRIVMSG #chan :hello there"),
            Some(&b"#chan"[..])
        );
    }

    #[test]
    fn extract_target_finds_a_notice_target() {
        assert_eq!(extract_target(b"NOTICE someone :a notice"), Some(&b"someone"[..]));
    }

    #[test]
    fn extract_target_is_none_for_a_command_whose_only_param_is_trailing() {
        assert_eq!(extract_target(b"PING :1234567"), None);
    }

    #[test]
    fn extract_target_is_none_for_a_bare_command_with_no_params() {
        assert_eq!(extract_target(b"MOTD"), None);
    }

    #[test]
    fn decode_incoming_falls_back_to_utf8_lossy_with_no_override_configured() {
        let _guard = lock_recode_default();
        // No /recode override exists for this made-up tag in a fresh
        // test environment, so this exercises the fallback path.
        let decoded = decode_incoming(b"PRIVMSG #chan :plain ascii", "recode_test_tag_no_override_zzz");
        assert_eq!(decoded, "PRIVMSG #chan :plain ascii");
    }

    #[test]
    fn encode_outgoing_is_a_no_op_with_no_override_configured() {
        let _guard = lock_recode_default();
        let encoded = encode_outgoing("PRIVMSG #chan :hello", "recode_test_tag_no_override_zzz");
        assert_eq!(encoded, b"PRIVMSG #chan :hello");
    }

    #[test]
    fn decode_incoming_uses_the_configured_charset_for_the_target() {
        let _guard = lock_recode_default();
        let key = "#recode_net_test_cp1251chan_zzz".to_string();
        irust_config::recode::update_cache_entry(key.clone(), "windows-1251".to_string());
        // 0xE0 0xE1 0xE2 in windows-1251 is "абв".
        let mut line = b"PRIVMSG #recode_net_test_cp1251chan_zzz :".to_vec();
        line.extend_from_slice(&[0xE0, 0xE1, 0xE2]);
        let decoded = decode_incoming(&line, "recode_test_tag_zzz");
        assert_eq!(decoded, "PRIVMSG #recode_net_test_cp1251chan_zzz :абв");
        irust_config::recode::remove_cache_entry(&key);
    }

    #[test]
    fn decode_incoming_has_no_default_charset_fallback() {
        let _guard = lock_recode_default();
        let mut s = irust_config::settings::Settings::default();
        s.set("recode_out_default_charset", "windows-1251").unwrap();
        let mut line = b"PRIVMSG #recode_test_no_incoming_default_zzz :".to_vec();
        line.extend_from_slice(&[0xE0, 0xE1, 0xE2]);
        let decoded = decode_incoming(&line, "recode_test_tag_zzz");
        assert_eq!(
            decoded, "PRIVMSG #recode_test_no_incoming_default_zzz :àáâ",
            "must use recode_fallback (CP1252), never the outgoing default charset"
        );
        s.set("recode_out_default_charset", "").unwrap();
    }

    #[test]
    fn encode_outgoing_falls_back_to_the_default_charset_when_no_override_resolves() {
        let _guard = lock_recode_default();
        let mut s = irust_config::settings::Settings::default();
        s.set("recode_out_default_charset", "windows-1251").unwrap();
        let encoded = encode_outgoing("PRIVMSG #recode_test_default_charset_zzz :абв", "recode_test_tag_zzz");
        let mut expected = b"PRIVMSG #recode_test_default_charset_zzz :".to_vec();
        expected.extend_from_slice(&[0xE0, 0xE1, 0xE2]);
        assert_eq!(encoded, expected);
        s.set("recode_out_default_charset", "").unwrap();
    }

    #[test]
    fn encode_outgoing_ignores_an_empty_default_charset() {
        let _guard = lock_recode_default();
        // Empty (unset) must behave exactly like today, before this
        // fallback tier existed: plain UTF-8 bytes, unchanged.
        let encoded = encode_outgoing("PRIVMSG #recode_test_empty_default_zzz :hello", "recode_test_tag_zzz");
        assert_eq!(encoded, b"PRIVMSG #recode_test_empty_default_zzz :hello");
    }

    #[test]
    fn a_per_target_override_still_wins_over_the_default_charset() {
        let _guard = lock_recode_default();
        let key = "#recode_test_override_beats_default_zzz".to_string();
        irust_config::recode::update_cache_entry(key.clone(), "windows-1251".to_string());
        let mut s = irust_config::settings::Settings::default();
        s.set("recode_out_default_charset", "iso-8859-1").unwrap();
        let encoded = encode_outgoing(
            "PRIVMSG #recode_test_override_beats_default_zzz :абв",
            "recode_test_tag_zzz",
        );
        let mut expected = b"PRIVMSG #recode_test_override_beats_default_zzz :".to_vec();
        expected.extend_from_slice(&[0xE0, 0xE1, 0xE2]);
        assert_eq!(
            encoded, expected,
            "the per-target windows-1251 override must win, not the iso-8859-1 default"
        );
        irust_config::recode::remove_cache_entry(&key);
        s.set("recode_out_default_charset", "").unwrap();
    }

    #[test]
    fn encode_outgoing_uses_the_configured_charset_for_the_target() {
        let _guard = lock_recode_default();
        let key = "#recode_net_test_encode_cp1251chan_zzz".to_string();
        irust_config::recode::update_cache_entry(key.clone(), "windows-1251".to_string());
        let encoded = encode_outgoing(
            "PRIVMSG #recode_net_test_encode_cp1251chan_zzz :абв",
            "recode_test_tag_zzz",
        );
        let mut expected = b"PRIVMSG #recode_net_test_encode_cp1251chan_zzz :".to_vec();
        expected.extend_from_slice(&[0xE0, 0xE1, 0xE2]);
        assert_eq!(encoded, expected);
        irust_config::recode::remove_cache_entry(&key);
    }

    #[test]
    fn valid_utf8_beats_a_configured_conversion_unless_autodetect_is_off() {
        let _guard = lock_recode_default();
        let key = "#recode_net_test_autodetect_zzz".to_string();
        irust_config::recode::update_cache_entry(key.clone(), "windows-1251".to_string());
        let line = "PRIVMSG #recode_net_test_autodetect_zzz :é".as_bytes();
        assert_eq!(decode_incoming(line, "t"), "PRIVMSG #recode_net_test_autodetect_zzz :é");
        let mut n = irust_config::settings_net::NetSettings::default();
        n.set("recode_autodetect_utf8", "off").unwrap();
        // "é" is 0xC3 0xA9; read as windows-1251 that is "Г©".
        assert_eq!(decode_incoming(line, "t"), "PRIVMSG #recode_net_test_autodetect_zzz :Г©");
        irust_config::settings_net::NetSettings::default().publish();
        irust_config::recode::remove_cache_entry(&key);
    }

    #[test]
    fn recode_off_passes_bytes_through_untouched_both_ways() {
        let _guard = lock_recode_default();
        let mut s = irust_config::settings::Settings::default();
        s.set("recode", "off").unwrap();
        let mut line = b"PRIVMSG #x :".to_vec();
        line.push(0xE0);
        assert!(decode_incoming(&line, "t").contains('\u{FFFD}'), "no CP1252 fallback when recode is off");
        s.set("recode_out_default_charset", "windows-1251").unwrap();
        assert_eq!(encode_outgoing("PRIVMSG #x :абв", "t"), "PRIVMSG #x :абв".as_bytes());
        s.set("recode_out_default_charset", "").unwrap();
        irust_config::settings::Settings::default().set("recode", "on").unwrap();
    }

    #[test]
    fn recode_fallback_setting_picks_the_charset_for_non_utf8_input() {
        let _guard = lock_recode_default();
        let mut n = irust_config::settings_net::NetSettings::default();
        n.set("recode_fallback", "windows-1251").unwrap();
        let mut line = b"PRIVMSG #recode_fb_zzz :".to_vec();
        line.extend_from_slice(&[0xE0, 0xE1, 0xE2]);
        assert_eq!(decode_incoming(&line, "t"), "PRIVMSG #recode_fb_zzz :абв");
        irust_config::settings_net::NetSettings::default().publish();
    }
}
