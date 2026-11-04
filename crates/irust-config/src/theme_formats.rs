
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::custom_themes::CustomTheme;

pub const DEFAULT_JOIN: &str = "-- $nick$hostsuffix has joined $channel";
pub const DEFAULT_JOIN_SELF: &str = "-- $nick$hostsuffix has joined $channel";
pub const DEFAULT_PART: &str = "-- $nick$hostsuffix has left $channel [$reason]";
pub const DEFAULT_PART_SELF: &str = "-- $nick$hostsuffix has left $channel [$reason]";
pub const DEFAULT_QUIT: &str = "-- $nick$hostsuffix has quit [$reason]";
pub const DEFAULT_KICK: &str = "-- $kicked was kicked from $channel by $nick [$reason]";
pub const DEFAULT_KICK_SELF: &str = "-- you were kicked from $channel by $nick [$reason]";
pub const DEFAULT_NICK: &str = "-- $nick is now known as $newnick";
pub const DEFAULT_NICK_SELF: &str = "-- You're now known as $newnick";
pub const DEFAULT_TOPIC: &str = "-- $nick changed the topic of $channel to: $topic";
pub const DEFAULT_MODE_CHANNEL: &str = "-- mode/$channel [$mode] by $nick";
pub const DEFAULT_MODE_USER: &str = "-- Mode change [$mode] for user $nick";
pub const DEFAULT_CHGHOST: &str = "-- $nick [$user@$host] has changed host";
pub const DEFAULT_ACCOUNT_LOGIN: &str = "-- $nick is now authenticated as $account";
pub const DEFAULT_ACCOUNT_LOGOUT: &str = "-- $nick is no longer authenticated";
pub const DEFAULT_SETNAME: &str = "-- $nick changed their real name to $realname";
pub const DEFAULT_INVITE: &str = "-- $nick invites you to $channel";
pub const DEFAULT_INVITE_NOTIFY: &str = "-- $nick invited $invitee to $channel";

pub const DEFAULT_DCC_SEND_START: &str = "-- sending $filename to $peer$resume_note";
pub const DEFAULT_DCC_GET_START: &str = "-- receiving $filename from $peer$resume_note";
pub const DEFAULT_DCC_TRANSFER_COMPLETE: &str = "-- $filename: transfer complete ($bytes)";
pub const DEFAULT_DCC_TRANSFER_CANCELLED: &str = "-- $filename: transfer cancelled";
pub const DEFAULT_DCC_TRANSFER_FAILED: &str = "-- $filename: transfer failed: $reason";
/// irssi's own `whois` theme abstract: `%# $[8]0 : $1-` -- `label` is
/// already left-padded to 8 columns by the caller before this
/// substitutes it, same as the original hardcoded `{label:<8}` did,
/// since a `$placeholder` template has no `{:<8}`-style width syntax of
/// its own to express that padding.
pub const DEFAULT_WHOIS_LINE: &str = "-- $label : $value";
pub const DEFAULT_WHOIS_HEADER: &str = "-- $nick $userhost";
pub const DEFAULT_WHOIS_END: &str = "-- End of WHOIS";
pub const DEFAULT_WHOWAS_END: &str = "-- End of WHOWAS";
pub const DEFAULT_WHOIS_NO_SUCH_NICK: &str = "-- There is no such nick $nick";
pub const DEFAULT_WHOWAS_NO_SUCH_NICK: &str = "-- There was no such nickname $nick";

type FormatGetter = fn(&CustomTheme) -> &Option<String>;
type FormatSetter = fn(&mut CustomTheme) -> &mut Option<String>;
pub const FORMAT_TABLE: &[(&str, &str, FormatGetter, FormatSetter)] = &[
    ("join", DEFAULT_JOIN, |c| &c.format_join, |c| &mut c.format_join),
    (
        "join_self",
        DEFAULT_JOIN_SELF,
        |c| &c.format_join_self,
        |c| &mut c.format_join_self,
    ),
    ("part", DEFAULT_PART, |c| &c.format_part, |c| &mut c.format_part),
    (
        "part_self",
        DEFAULT_PART_SELF,
        |c| &c.format_part_self,
        |c| &mut c.format_part_self,
    ),
    ("quit", DEFAULT_QUIT, |c| &c.format_quit, |c| &mut c.format_quit),
    ("kick", DEFAULT_KICK, |c| &c.format_kick, |c| &mut c.format_kick),
    (
        "kick_self",
        DEFAULT_KICK_SELF,
        |c| &c.format_kick_self,
        |c| &mut c.format_kick_self,
    ),
    ("nick", DEFAULT_NICK, |c| &c.format_nick, |c| &mut c.format_nick),
    (
        "nick_self",
        DEFAULT_NICK_SELF,
        |c| &c.format_nick_self,
        |c| &mut c.format_nick_self,
    ),
    ("topic", DEFAULT_TOPIC, |c| &c.format_topic, |c| &mut c.format_topic),
    (
        "mode_channel",
        DEFAULT_MODE_CHANNEL,
        |c| &c.format_mode_channel,
        |c| &mut c.format_mode_channel,
    ),
    (
        "mode_user",
        DEFAULT_MODE_USER,
        |c| &c.format_mode_user,
        |c| &mut c.format_mode_user,
    ),
    (
        "chghost",
        DEFAULT_CHGHOST,
        |c| &c.format_chghost,
        |c| &mut c.format_chghost,
    ),
    (
        "account_login",
        DEFAULT_ACCOUNT_LOGIN,
        |c| &c.format_account_login,
        |c| &mut c.format_account_login,
    ),
    (
        "account_logout",
        DEFAULT_ACCOUNT_LOGOUT,
        |c| &c.format_account_logout,
        |c| &mut c.format_account_logout,
    ),
    (
        "setname",
        DEFAULT_SETNAME,
        |c| &c.format_setname,
        |c| &mut c.format_setname,
    ),
    ("invite", DEFAULT_INVITE, |c| &c.format_invite, |c| &mut c.format_invite),
    (
        "invite_notify",
        DEFAULT_INVITE_NOTIFY,
        |c| &c.format_invite_notify,
        |c| &mut c.format_invite_notify,
    ),
    (
        "dcc_send_start",
        DEFAULT_DCC_SEND_START,
        |c| &c.format_dcc_send_start,
        |c| &mut c.format_dcc_send_start,
    ),
    (
        "dcc_get_start",
        DEFAULT_DCC_GET_START,
        |c| &c.format_dcc_get_start,
        |c| &mut c.format_dcc_get_start,
    ),
    (
        "dcc_transfer_complete",
        DEFAULT_DCC_TRANSFER_COMPLETE,
        |c| &c.format_dcc_transfer_complete,
        |c| &mut c.format_dcc_transfer_complete,
    ),
    (
        "dcc_transfer_cancelled",
        DEFAULT_DCC_TRANSFER_CANCELLED,
        |c| &c.format_dcc_transfer_cancelled,
        |c| &mut c.format_dcc_transfer_cancelled,
    ),
    (
        "dcc_transfer_failed",
        DEFAULT_DCC_TRANSFER_FAILED,
        |c| &c.format_dcc_transfer_failed,
        |c| &mut c.format_dcc_transfer_failed,
    ),
    (
        "whois_line",
        DEFAULT_WHOIS_LINE,
        |c| &c.format_whois_line,
        |c| &mut c.format_whois_line,
    ),
    (
        "whois_header",
        DEFAULT_WHOIS_HEADER,
        |c| &c.format_whois_header,
        |c| &mut c.format_whois_header,
    ),
    (
        "whois_end",
        DEFAULT_WHOIS_END,
        |c| &c.format_whois_end,
        |c| &mut c.format_whois_end,
    ),
    (
        "whowas_end",
        DEFAULT_WHOWAS_END,
        |c| &c.format_whowas_end,
        |c| &mut c.format_whowas_end,
    ),
    (
        "whois_no_such_nick",
        DEFAULT_WHOIS_NO_SUCH_NICK,
        |c| &c.format_whois_no_such_nick,
        |c| &mut c.format_whois_no_such_nick,
    ),
    (
        "whowas_no_such_nick",
        DEFAULT_WHOWAS_NO_SUCH_NICK,
        |c| &c.format_whowas_no_such_nick,
        |c| &mut c.format_whowas_no_such_nick,
    ),
];

pub fn resolve_by_name(name: &str) -> Option<String> {
    FORMAT_TABLE
        .iter()
        .find(|(n, _, _, _)| *n == name)
        .map(|(_, default, getter, _)| resolve_template(default, *getter).into_owned())
}

pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(dollar_pos) = rest.find('$') {
        out.push_str(&rest[..dollar_pos]);
        rest = &rest[dollar_pos + 1..];
        let name_len = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if name_len == 0 {
            // A bare trailing `$`, or `$` followed by punctuation/
            // whitespace -- not a placeholder, kept literal.
            out.push('$');
            continue;
        }
        let name = &rest[..name_len];
        match vars.iter().find(|(n, _)| *n == name) {
            Some((_, value)) => out.push_str(value),
            None => {
                out.push('$');
                out.push_str(name);
            }
        }
        rest = &rest[name_len..];
    }
    out.push_str(rest);
    out
}

fn custom_themes_cache() -> &'static RwLock<Arc<HashMap<String, CustomTheme>>> {
    static CACHE: OnceLock<RwLock<Arc<HashMap<String, CustomTheme>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(Arc::new(crate::custom_themes::load_custom_themes())))
}

fn cached_custom_themes() -> Arc<HashMap<String, CustomTheme>> {
    custom_themes_cache().read().unwrap().clone()
}

pub fn update_cached_theme(name: String, theme: CustomTheme) {
    let cache = custom_themes_cache();
    let mut guard = cache.write().unwrap();
    let mut updated = (**guard).clone();
    updated.insert(name, theme);
    *guard = Arc::new(updated);
}

pub fn remove_cached_theme(name: &str) {
    let cache = custom_themes_cache();
    let mut guard = cache.write().unwrap();
    let mut updated = (**guard).clone();
    updated.remove(name);
    *guard = Arc::new(updated);
}

fn resolve_template(default: &'static str, extract: impl Fn(&CustomTheme) -> &Option<String>) -> Cow<'static, str> {
    let theme_name = crate::settings::theme();
    if crate::settings::THEME_NAMES.contains(&theme_name.as_str()) {
        return Cow::Borrowed(default);
    }
    resolve_from(cached_custom_themes().get(&theme_name), default, extract)
}

fn resolve_from(
    theme: Option<&CustomTheme>,
    default: &'static str,
    extract: impl Fn(&CustomTheme) -> &Option<String>,
) -> Cow<'static, str> {
    match theme.and_then(|c| extract(c).as_deref()) {
        Some(custom) => Cow::Owned(custom.to_string()),
        None => Cow::Borrowed(default),
    }
}

pub fn join(nick: &str, hostsuffix: &str, channel: &str) -> String {
    render(
        &resolve_template(DEFAULT_JOIN, |c| &c.format_join),
        &[("nick", nick), ("hostsuffix", hostsuffix), ("channel", channel)],
    )
}

pub fn join_self(nick: &str, hostsuffix: &str, channel: &str) -> String {
    render(
        &resolve_template(DEFAULT_JOIN_SELF, |c| &c.format_join_self),
        &[("nick", nick), ("hostsuffix", hostsuffix), ("channel", channel)],
    )
}

pub fn part(nick: &str, hostsuffix: &str, channel: &str, reason: &str) -> String {
    render(
        &resolve_template(DEFAULT_PART, |c| &c.format_part),
        &[
            ("nick", nick),
            ("hostsuffix", hostsuffix),
            ("channel", channel),
            ("reason", reason),
        ],
    )
}

pub fn part_self(nick: &str, hostsuffix: &str, channel: &str, reason: &str) -> String {
    render(
        &resolve_template(DEFAULT_PART_SELF, |c| &c.format_part_self),
        &[
            ("nick", nick),
            ("hostsuffix", hostsuffix),
            ("channel", channel),
            ("reason", reason),
        ],
    )
}

pub fn quit(nick: &str, hostsuffix: &str, reason: &str) -> String {
    render(
        &resolve_template(DEFAULT_QUIT, |c| &c.format_quit),
        &[("nick", nick), ("hostsuffix", hostsuffix), ("reason", reason)],
    )
}

pub fn kick(kicked: &str, channel: &str, nick: &str, reason: &str) -> String {
    render(
        &resolve_template(DEFAULT_KICK, |c| &c.format_kick),
        &[
            ("kicked", kicked),
            ("channel", channel),
            ("nick", nick),
            ("reason", reason),
        ],
    )
}

pub fn kick_self(channel: &str, nick: &str, reason: &str) -> String {
    render(
        &resolve_template(DEFAULT_KICK_SELF, |c| &c.format_kick_self),
        &[("channel", channel), ("nick", nick), ("reason", reason)],
    )
}

pub fn nick(nick: &str, newnick: &str) -> String {
    render(
        &resolve_template(DEFAULT_NICK, |c| &c.format_nick),
        &[("nick", nick), ("newnick", newnick)],
    )
}

pub fn nick_self(newnick: &str) -> String {
    render(
        &resolve_template(DEFAULT_NICK_SELF, |c| &c.format_nick_self),
        &[("newnick", newnick)],
    )
}

pub fn topic(nick: &str, channel: &str, topic: &str) -> String {
    render(
        &resolve_template(DEFAULT_TOPIC, |c| &c.format_topic),
        &[("nick", nick), ("channel", channel), ("topic", topic)],
    )
}

pub fn mode_channel(channel: &str, mode: &str, nick: &str) -> String {
    render(
        &resolve_template(DEFAULT_MODE_CHANNEL, |c| &c.format_mode_channel),
        &[("channel", channel), ("mode", mode), ("nick", nick)],
    )
}

pub fn mode_user(mode: &str, nick: &str) -> String {
    render(
        &resolve_template(DEFAULT_MODE_USER, |c| &c.format_mode_user),
        &[("mode", mode), ("nick", nick)],
    )
}

pub fn chghost(nick: &str, user: &str, host: &str) -> String {
    render(
        &resolve_template(DEFAULT_CHGHOST, |c| &c.format_chghost),
        &[("nick", nick), ("user", user), ("host", host)],
    )
}

pub fn account_login(nick: &str, account: &str) -> String {
    render(
        &resolve_template(DEFAULT_ACCOUNT_LOGIN, |c| &c.format_account_login),
        &[("nick", nick), ("account", account)],
    )
}

pub fn account_logout(nick: &str) -> String {
    render(
        &resolve_template(DEFAULT_ACCOUNT_LOGOUT, |c| &c.format_account_logout),
        &[("nick", nick)],
    )
}

pub fn setname(nick: &str, realname: &str) -> String {
    render(
        &resolve_template(DEFAULT_SETNAME, |c| &c.format_setname),
        &[("nick", nick), ("realname", realname)],
    )
}

pub fn invite(nick: &str, channel: &str) -> String {
    render(
        &resolve_template(DEFAULT_INVITE, |c| &c.format_invite),
        &[("nick", nick), ("channel", channel)],
    )
}

pub fn invite_notify(nick: &str, invitee: &str, channel: &str) -> String {
    render(
        &resolve_template(DEFAULT_INVITE_NOTIFY, |c| &c.format_invite_notify),
        &[("nick", nick), ("invitee", invitee), ("channel", channel)],
    )
}

pub fn dcc_send_start(filename: &str, peer: &str, resume_note: &str) -> String {
    render(
        &resolve_template(DEFAULT_DCC_SEND_START, |c| &c.format_dcc_send_start),
        &[("filename", filename), ("peer", peer), ("resume_note", resume_note)],
    )
}

pub fn dcc_get_start(filename: &str, peer: &str, resume_note: &str) -> String {
    render(
        &resolve_template(DEFAULT_DCC_GET_START, |c| &c.format_dcc_get_start),
        &[("filename", filename), ("peer", peer), ("resume_note", resume_note)],
    )
}

pub fn dcc_transfer_complete(filename: &str, bytes: &str) -> String {
    render(
        &resolve_template(DEFAULT_DCC_TRANSFER_COMPLETE, |c| &c.format_dcc_transfer_complete),
        &[("filename", filename), ("bytes", bytes)],
    )
}

pub fn dcc_transfer_cancelled(filename: &str) -> String {
    render(
        &resolve_template(DEFAULT_DCC_TRANSFER_CANCELLED, |c| &c.format_dcc_transfer_cancelled),
        &[("filename", filename)],
    )
}

pub fn dcc_transfer_failed(filename: &str, reason: &str) -> String {
    render(
        &resolve_template(DEFAULT_DCC_TRANSFER_FAILED, |c| &c.format_dcc_transfer_failed),
        &[("filename", filename), ("reason", reason)],
    )
}

/// `label` must already be left-padded to 8 columns by the caller (see
/// [`DEFAULT_WHOIS_LINE`]'s own doc comment).
pub fn whois_line(label: &str, value: &str) -> String {
    render(
        &resolve_template(DEFAULT_WHOIS_LINE, |c| &c.format_whois_line),
        &[("label", label), ("value", value)],
    )
}

pub fn whois_header(nick: &str, userhost: &str) -> String {
    render(
        &resolve_template(DEFAULT_WHOIS_HEADER, |c| &c.format_whois_header),
        &[("nick", nick), ("userhost", userhost)],
    )
}

pub fn whois_end() -> String {
    resolve_template(DEFAULT_WHOIS_END, |c| &c.format_whois_end).into_owned()
}

pub fn whowas_end() -> String {
    resolve_template(DEFAULT_WHOWAS_END, |c| &c.format_whowas_end).into_owned()
}

pub fn whois_no_such_nick(nick: &str) -> String {
    render(
        &resolve_template(DEFAULT_WHOIS_NO_SUCH_NICK, |c| &c.format_whois_no_such_nick),
        &[("nick", nick)],
    )
}

pub fn whowas_no_such_nick(nick: &str) -> String {
    render(
        &resolve_template(DEFAULT_WHOWAS_NO_SUCH_NICK, |c| &c.format_whowas_no_such_nick),
        &[("nick", nick)],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_every_placeholder() {
        let out = render(
            "-- $nick has joined $channel",
            &[("nick", "abcX"), ("channel", "#chatnet")],
        );
        assert_eq!(out, "-- abcX has joined #chatnet");
    }

    #[test]
    fn render_leaves_an_unmatched_placeholder_as_literal_text() {
        let out = render("-- $nick did $thing", &[("nick", "abcX")]);
        assert_eq!(out, "-- abcX did $thing");
    }

    #[test]
    fn render_handles_a_bare_dollar_with_no_name() {
        let out = render("costs $5", &[]);
        assert_eq!(out, "costs $5");
    }

    #[test]
    fn render_handles_adjacent_placeholders_with_no_separator() {
        let out = render("$a$b", &[("a", "X"), ("b", "Y")]);
        assert_eq!(out, "XY");
    }

    #[test]
    fn render_is_a_no_op_on_a_template_with_no_placeholders() {
        assert_eq!(render("plain text", &[("unused", "x")]), "plain text");
    }

    #[test]
    fn format_table_has_exactly_one_row_per_default_constant() {
        let all_defaults = [
            DEFAULT_JOIN,
            DEFAULT_JOIN_SELF,
            DEFAULT_PART,
            DEFAULT_PART_SELF,
            DEFAULT_QUIT,
            DEFAULT_KICK,
            DEFAULT_KICK_SELF,
            DEFAULT_NICK,
            DEFAULT_NICK_SELF,
            DEFAULT_TOPIC,
            DEFAULT_MODE_CHANNEL,
            DEFAULT_MODE_USER,
            DEFAULT_CHGHOST,
            DEFAULT_ACCOUNT_LOGIN,
            DEFAULT_ACCOUNT_LOGOUT,
            DEFAULT_SETNAME,
            DEFAULT_INVITE,
            DEFAULT_INVITE_NOTIFY,
            DEFAULT_DCC_SEND_START,
            DEFAULT_DCC_GET_START,
            DEFAULT_DCC_TRANSFER_COMPLETE,
            DEFAULT_DCC_TRANSFER_CANCELLED,
            DEFAULT_DCC_TRANSFER_FAILED,
            DEFAULT_WHOIS_LINE,
            DEFAULT_WHOIS_HEADER,
            DEFAULT_WHOIS_END,
            DEFAULT_WHOWAS_END,
            DEFAULT_WHOIS_NO_SUCH_NICK,
            DEFAULT_WHOWAS_NO_SUCH_NICK,
        ];
        assert_eq!(FORMAT_TABLE.len(), all_defaults.len());
        let table_defaults: Vec<&str> = FORMAT_TABLE.iter().map(|(_, default, _, _)| *default).collect();
        for default in all_defaults {
            assert!(
                table_defaults.contains(&default),
                "missing FORMAT_TABLE row for {default:?}"
            );
        }
        let names: std::collections::HashSet<&str> = FORMAT_TABLE.iter().map(|(name, _, _, _)| *name).collect();
        assert_eq!(names.len(), FORMAT_TABLE.len(), "FORMAT_TABLE has a duplicate name");
    }

    /// `update_cached_theme` must be visible to `resolve_template`'s
    /// callers on the very next call, no restart -- the whole point of
    /// switching the cache to `OnceLock<RwLock<Arc<...>>>`.
    #[test]
    fn update_cached_theme_is_visible_without_reloading_from_disk() {
        let name = "format_table_test_theme_zzz".to_string();
        update_cached_theme(
            name.clone(),
            CustomTheme {
                format_join: Some("$nick arrived in style!".to_string()),
                ..Default::default()
            },
        );
        let resolved = resolve_from(cached_custom_themes().get(&name), DEFAULT_JOIN, |c| &c.format_join);
        assert_eq!(resolved, "$nick arrived in style!");
    }

    /// `remove_cached_theme` must be visible to `resolve_template`'s
    /// callers on the very next call too, the removal counterpart of
    /// `update_cached_theme_is_visible_without_reloading_from_disk`.
    #[test]
    fn remove_cached_theme_is_visible_without_reloading_from_disk() {
        let name = "format_table_test_theme_removal".to_string();
        update_cached_theme(
            name.clone(),
            CustomTheme {
                format_join: Some("$nick arrived in style!".to_string()),
                ..Default::default()
            },
        );
        assert!(cached_custom_themes().contains_key(&name));
        remove_cached_theme(&name);
        assert!(!cached_custom_themes().contains_key(&name));
    }

    #[test]
    fn resolve_from_uses_the_override_when_the_theme_has_one() {
        let custom = CustomTheme {
            format_join: Some("$nick arrived!".to_string()),
            ..Default::default()
        };
        let resolved = resolve_from(Some(&custom), DEFAULT_JOIN, |c| &c.format_join);
        assert_eq!(resolved, "$nick arrived!");
    }

    #[test]
    fn resolve_from_falls_back_to_default_when_that_field_is_unset() {
        let custom = CustomTheme {
            format_part: Some("$nick left, whatever".to_string()),
            ..Default::default()
        };
        // The theme overrides a DIFFERENT format (`format_part`) -- this
        // one (`format_join`) must still fall back to the built-in
        // default, not silently blank or reuse the other override.
        let resolved = resolve_from(Some(&custom), DEFAULT_JOIN, |c| &c.format_join);
        assert_eq!(resolved, DEFAULT_JOIN);
    }

    #[test]
    fn resolve_from_falls_back_to_default_with_no_theme_at_all() {
        let resolved = resolve_from(None, DEFAULT_JOIN, |c| &c.format_join);
        assert_eq!(resolved, DEFAULT_JOIN);
    }

    #[test]
    fn default_theme_formats_match_the_original_hardcoded_wording() {
        assert_eq!(join("abcX", " [u@h]", "#chatnet"), "-- abcX [u@h] has joined #chatnet");
        assert_eq!(join_self("abcX", " [u@h]", "#chatnet"), "-- abcX [u@h] has joined #chatnet");
        assert_eq!(part("abcX", "", "#chatnet", "bye"), "-- abcX has left #chatnet [bye]");
        assert_eq!(
            part_self("abcX", " [u@h]", "#chatnet", "bye"),
            "-- abcX [u@h] has left #chatnet [bye]"
        );
        assert_eq!(quit("abcX", "", "gone"), "-- abcX has quit [gone]");
        assert_eq!(
            kick("dana", "#chatnet", "abcX", "spam"),
            "-- dana was kicked from #chatnet by abcX [spam]"
        );
        assert_eq!(
            kick_self("#chatnet", "abcX", "spam"),
            "-- you were kicked from #chatnet by abcX [spam]"
        );
        assert_eq!(nick("abcX", "abcX_"), "-- abcX is now known as abcX_");
        assert_eq!(nick_self("abcX_"), "-- You're now known as abcX_");
        assert_eq!(
            topic("abcX", "#chatnet", "new topic"),
            "-- abcX changed the topic of #chatnet to: new topic"
        );
        assert_eq!(
            mode_channel("#chatnet", "+o abcX", "K9"),
            "-- mode/#chatnet [+o abcX] by K9"
        );
        assert_eq!(mode_user("+i", "abcX"), "-- Mode change [+i] for user abcX");
        assert_eq!(chghost("abcX", "u", "h"), "-- abcX [u@h] has changed host");
        assert_eq!(account_login("abcX", "abcX"), "-- abcX is now authenticated as abcX");
        assert_eq!(account_logout("abcX"), "-- abcX is no longer authenticated");
        assert_eq!(setname("abcX", "Alex"), "-- abcX changed their real name to Alex");
        assert_eq!(invite("dana", "#chatnet"), "-- dana invites you to #chatnet");
        assert_eq!(
            invite_notify("dana", "sh", "#chatnet"),
            "-- dana invited sh to #chatnet"
        );
        assert_eq!(dcc_send_start("file.txt", "dana", ""), "-- sending file.txt to dana");
        assert_eq!(
            dcc_send_start("file.txt", "dana", " (resuming from 1.0 KiB)"),
            "-- sending file.txt to dana (resuming from 1.0 KiB)"
        );
        assert_eq!(dcc_get_start("file.txt", "dana", ""), "-- receiving file.txt from dana");
        assert_eq!(
            dcc_transfer_complete("file.txt", "1.0 MiB"),
            "-- file.txt: transfer complete (1.0 MiB)"
        );
        assert_eq!(dcc_transfer_cancelled("file.txt"), "-- file.txt: transfer cancelled");
        assert_eq!(
            dcc_transfer_failed("file.txt", "connection reset"),
            "-- file.txt: transfer failed: connection reset"
        );
        assert_eq!(whois_line("ircname", "Alex"), "-- ircname : Alex");
        assert_eq!(whois_header("abcX", "u@h"), "-- abcX u@h");
        assert_eq!(whois_end(), "-- End of WHOIS");
        assert_eq!(whowas_end(), "-- End of WHOWAS");
        assert_eq!(whois_no_such_nick("ghost"), "-- There is no such nick ghost");
        assert_eq!(whowas_no_such_nick("ghost"), "-- There was no such nickname ghost");
    }
}
