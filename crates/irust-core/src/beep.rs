
use irust_config::levels;
use irust_config::lookandfeel;
use irust_proto::RawMessage;

use crate::state::{BufferId, ClientState};

fn is_channel(target: &str) -> bool {
    target.starts_with(['#', '&', '!', '+'])
}

/// The MSGLEVEL an incoming server message would be printed with, and the
/// buffer it lands in (`None` = status/no single buffer).
fn incoming_level(msg: &RawMessage, our_nick: &str) -> Option<(u32, Option<String>)> {
    let sender = msg.prefix.as_ref().map(|p| p.name.as_str());
    if sender.is_some_and(|s| s.eq_ignore_ascii_case(our_nick)) {
        return None; // our own echo never beeps
    }
    let target = msg.params.first().map(String::as_str).unwrap_or("");
    let text = msg.params.get(1).map(String::as_str).unwrap_or("");
    // A DM lands in the sender's query buffer, anything else in its target.
    let buffer = |t: &str| Some(if is_channel(t) { t.to_lowercase() } else { sender.unwrap_or(t).to_lowercase() });
    match msg.command.to_ascii_uppercase().as_str() {
        "PRIVMSG" => {
            let base = if is_channel(target) { levels::PUBLIC } else { levels::MSGS };
            let level = match text.strip_prefix('\u{1}') {
                Some(ctcp) if ctcp.starts_with("ACTION") => levels::ACTIONS | base,
                Some(_) => levels::CTCPS,
                None => base,
            };
            Some((level, buffer(target)))
        }
        "NOTICE" => {
            let level = if text.starts_with('\u{1}') {
                levels::CTCPS
            } else if is_channel(target) {
                levels::PUBNOTICES
            } else {
                levels::NOTICES
            };
            Some((level, buffer(target)))
        }
        "JOIN" => Some((levels::JOINS, Some(target.to_lowercase()))),
        "PART" => Some((levels::PARTS, Some(target.to_lowercase()))),
        "KICK" => Some((levels::KICKS, Some(target.to_lowercase()))),
        "MODE" if is_channel(target) => Some((levels::MODES, Some(target.to_lowercase()))),
        "TOPIC" => Some((levels::TOPICS, Some(target.to_lowercase()))),
        "QUIT" => Some((levels::QUITS, None)),
        "NICK" => Some((levels::NICKS, None)),
        "INVITE" => Some((levels::INVITES, None)),
        "WALLOPS" => Some((levels::WALLOPS, None)),
        _ => None,
    }
}

fn is_active(state: &ClientState, buffer: Option<&str>) -> bool {
    match (&state.active_buffer, buffer) {
        (BufferId::Channel(key), Some(b)) | (BufferId::Query(key), Some(b)) => key.eq_ignore_ascii_case(b),
        _ => false,
    }
}

/// Should this incoming message ring the bell under the `beep_*` settings?
pub fn msg_beep_wanted(state: &ClientState, msg: &RawMessage) -> bool {
    let Some((level, buffer)) = incoming_level(msg, &state.nick) else { return false };
    let wanted = lookandfeel::get_level("beep_msg_level");
    if level == 0 || level & levels::NO_ACT != 0 || wanted & level == 0 {
        return false;
    }
    if !lookandfeel::get_bool("beep_when_away") && state.away {
        return false;
    }
    if !lookandfeel::get_bool("beep_when_window_active") && is_active(state, buffer.as_deref()) {
        return false;
    }
    // "-level" entries are an ignore list on top of the positive levels
    levels::level2bits_negative(&lookandfeel::get("beep_msg_level")) & level == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use irust_proto::parse;

    fn with<T>(settings: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
        lookandfeel::with_test_overrides(settings, f)
    }

    fn state() -> ClientState {
        let mut st = ClientState::new("abcX", "#a", 11);
        st.ensure_channel("#b");
        st
    }

    fn beeps(st: &ClientState, line: &str) -> bool {
        msg_beep_wanted(st, &parse(line).unwrap())
    }

    const PUBLIC_B: &str = ":Other!u@h PRIVMSG #b :hello";
    const DM: &str = ":Other!u@h PRIVMSG abcX :hello";

    #[test]
    fn nothing_beeps_by_default() {
        with(&[], || {
            let st = state();
            assert!(!beeps(&st, PUBLIC_B) && !beeps(&st, DM));
        });
    }

    #[test]
    fn beep_msg_level_selects_what_beeps() {
        with(&[("beep_msg_level", "MSGS")], || {
            let st = state();
            assert!(beeps(&st, DM));
            assert!(!beeps(&st, PUBLIC_B), "PUBLIC is not in the list");
        });
        with(&[("beep_msg_level", "ALL -PUBLIC")], || {
            let st = state();
            assert!(beeps(&st, DM));
            assert!(!beeps(&st, PUBLIC_B), "-PUBLIC is an ignore entry");
        });
    }

    #[test]
    fn actions_and_ctcps_have_their_own_levels() {
        with(&[("beep_msg_level", "ACTIONS")], || {
            let st = state();
            assert!(beeps(&st, ":Other!u@h PRIVMSG #b :\u{1}ACTION waves\u{1}"));
            assert!(!beeps(&st, ":Other!u@h PRIVMSG #b :\u{1}VERSION\u{1}"));
            assert!(!beeps(&st, PUBLIC_B));
        });
    }

    #[test]
    fn our_own_echo_never_beeps() {
        with(&[("beep_msg_level", "ALL")], || {
            assert!(!beeps(&state(), ":abcX!u@h PRIVMSG #b :me talking"));
        });
    }

    #[test]
    fn beep_when_away_off_is_silent_while_away() {
        with(&[("beep_msg_level", "MSGS"), ("beep_when_away", "off")], || {
            let mut st = state();
            assert!(beeps(&st, DM));
            st.away = true;
            assert!(!beeps(&st, DM));
        });
    }

    #[test]
    fn beep_when_window_active_off_skips_the_window_you_are_looking_at() {
        with(&[("beep_msg_level", "PUBLIC"), ("beep_when_window_active", "off")], || {
            let mut st = state();
            st.active_buffer = BufferId::Channel("#b".to_string());
            assert!(!beeps(&st, PUBLIC_B), "#b is the active window");
            assert!(beeps(&st, ":Other!u@h PRIVMSG #a :hello"), "#a is in the background");
        });
    }
}
