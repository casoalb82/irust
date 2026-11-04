
const NICK: u8 = 0x01;
const USER: u8 = 0x02;
const HOST: u8 = 0x04;
const DOMAIN: u8 = 0x08;

const NORMAL: u8 = USER | DOMAIN;

/// Port of `parse_ban_type`/`parse_custom_ban`. An unrecognised value
/// yields the `normal` type, which is what irssi falls back to when
/// parsing gives <= 0 (`default_ban_type = IRC_MASK_USER|IRC_MASK_DOMAIN`).
fn parse(setting: &str) -> u8 {
    let setting = setting.trim();
    let first = setting.chars().next().map(|c| c.to_ascii_uppercase());
    let flags = match first {
        Some('N') => NORMAL,
        Some('U') => USER,
        Some('H') => HOST | DOMAIN,
        Some('D') => DOMAIN,
        Some('C') => setting
            .split_once(' ')
            .map(|(_, rest)| {
                rest.split(' ').fold(0u8, |acc, w| {
                    acc | match w.chars().next().map(|c| c.to_ascii_uppercase()) {
                        Some('N') => NICK,
                        Some('U') => USER,
                        Some('H') => HOST | DOMAIN,
                        Some('D') => DOMAIN,
                        _ => 0,
                    }
                })
            })
            .unwrap_or(0),
        _ => 0,
    };
    if flags == 0 {
        NORMAL
    } else {
        flags
    }
}

/// `irc_get_mask()` + the 10-character user-field trim from
/// `ban_get_mask()`.
pub fn mask(nick: &str, user: &str, host: &str, setting: &str) -> String {
    let flags = parse(setting);
    let user = user.strip_prefix(['^', '~', '+', '=', '-']).unwrap_or(user);
    let host_part = if flags & HOST != 0 {
        host.to_string()
    } else if flags & DOMAIN != 0 {
        crate::nicklist::domain_mask(host)
    } else {
        "*".to_string()
    };
    let mut user_part = format!("*{user}");
    // Max 10 chars in a ban's user field: "*" + 9 keeps the tail.
    if user_part.chars().count() >= 10 && flags & USER != 0 {
        let chars: Vec<char> = user_part.chars().collect();
        let size = chars.len();
        user_part = std::iter::once('*').chain(chars[size - 9..].iter().copied()).collect();
    }
    format!(
        "{}!{}@{}",
        if flags & NICK != 0 { nick } else { "*" },
        if flags & USER != 0 { user_part.as_str() } else { "*" },
        host_part
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_keep_the_right_parts() {
        let m = |t: &str| mask("Nick", "~user", "dyn1.blah.net", t);
        assert_eq!(m("normal"), "*!*user@*.blah.net");
        assert_eq!(m("user"), "*!*user@*");
        assert_eq!(m("host"), "*!*@dyn1.blah.net");
        assert_eq!(m("domain"), "*!*@*.blah.net");
        assert_eq!(m("custom nick host"), "Nick!*@dyn1.blah.net");
        assert_eq!(m("custom nick user domain"), "Nick!*user@*.blah.net");
    }

    #[test]
    fn bad_values_fall_back_to_normal() {
        assert_eq!(mask("n", "u", "a.b.c", "garbage"), "*!*u@*.b.c");
        assert_eq!(mask("n", "u", "a.b.c", "custom"), "*!*u@*.b.c");
        assert_eq!(mask("n", "u", "a.b.c", ""), "*!*u@*.b.c");
    }

    #[test]
    fn long_user_field_keeps_the_last_nine_chars() {
        assert_eq!(mask("n", "1234567890", "h", "user"), "*!*234567890@*");
    }
}
