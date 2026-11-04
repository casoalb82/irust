
use irust_config::levels;
use irust_config::lookandfeel;

use crate::buffer::{BufferLine, SystemKind};

/// How loud a line is for the window list, quietest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    None,
    Text,
    Msg,
    Hilight,
}

fn is_channel_name(name: &str) -> bool {
    name.starts_with(['#', '&', '!', '+'])
}

pub fn line_level(line: &BufferLine, buffer_name: &str) -> u32 {
    let highlighted = matches!(
        line,
        BufferLine::Chat { highlight: true, .. } | BufferLine::Action { highlight: true, .. }
    );
    let base = if is_channel_name(buffer_name) || !highlighted { levels::PUBLIC } else { levels::MSGS };
    match line {
        BufferLine::Chat { highlight, .. } => base | if *highlight { levels::HILIGHT } else { 0 },
        BufferLine::Action { highlight, .. } => {
            base | levels::ACTIONS | if *highlight { levels::HILIGHT } else { 0 }
        }
        BufferLine::System { kind, .. } => match kind {
            SystemKind::Join => levels::JOINS,
            SystemKind::Leave => levels::PARTS | levels::QUITS | levels::KICKS,
            SystemKind::Topic => levels::TOPICS,
            SystemKind::Whois => levels::CRAP,
            SystemKind::Notice => levels::NOTICES,
            SystemKind::Server => levels::SNOTES,
            SystemKind::Warning => levels::CLIENTERROR,
            // irssi prints the midnight marker with MSGLEVEL_NEVER.
            SystemKind::DayChange => levels::NEVER,
            SystemKind::Wallops => levels::WALLOPS,
            SystemKind::Other => levels::CRAP,
        },
    }
}

/// `activity_hide_targets`: space-separated `target` or `tag/target`
/// entries. A buffer knows no network tag, so a `tag/` prefix is dropped
/// and only the target part is compared (case-insensitive).
fn target_is_hidden(targets: &str, buffer_name: &str) -> bool {
    targets.split(' ').filter(|t| !t.is_empty()).any(|entry| {
        let target = entry.rsplit_once('/').map_or(entry, |(_, t)| t);
        target.eq_ignore_ascii_case(buffer_name)
    })
}

/// irssi's `sig_hilight_text` decision for a line of `level` in
/// `buffer_name`, reading the four activity settings.
pub fn tier_for(level: u32, buffer_name: &str) -> Tier {
    let hide_level = levels::NEVER | levels::NO_ACT | lookandfeel::get_level("activity_hide_level");
    if level & hide_level != 0 {
        return Tier::None;
    }
    // `activity_hide_window_hidelevel`: on top of `activity_hide_level`
    // above, a line matching the window's own hidden-level (no
    // per-window override yet, so the one global
    // `window_default_hidelevel` applies uniformly) raises no activity
    // either -- irssi's `sig_window_hilight_check`.
    if lookandfeel::get_bool("activity_hide_window_hidelevel")
        && level & lookandfeel::get_level("window_default_hidelevel") != 0
    {
        return Tier::None;
    }
    let hilight_level = levels::HILIGHT | lookandfeel::get_level("activity_hilight_level");
    let tier = if level & hilight_level != 0 {
        Tier::Hilight
    } else if level & lookandfeel::get_level("activity_msg_level") != 0 {
        Tier::Msg
    } else {
        Tier::Text
    };
    // A line that itself matched a hilight is never hidden by target.
    if level & levels::HILIGHT == 0 {
        let targets = lookandfeel::get("activity_hide_targets");
        if !targets.is_empty() && target_is_hidden(&targets, buffer_name) {
            return Tier::None;
        }
    }
    tier
}

pub fn tier_of_line(line: &BufferLine, buffer_name: &str) -> Tier {
    tier_for(line_level(line, buffer_name), buffer_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn with_settings<T>(pairs: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
        lookandfeel::with_test_overrides(pairs, f)
    }

    fn chat(highlight: bool) -> BufferLine {
        BufferLine::Chat {
            nick: "n".into(),
            color_index: 0,
            rank: None,
            text: "t".into(),
            highlight,
            highlight_color: None,
            timestamp: String::new(),
            time: None,
        }
    }

    fn system(kind: SystemKind) -> BufferLine {
        BufferLine::System { text: "x".into(), timestamp: String::new(), time: None, kind }
    }

    #[test]
    fn defaults_match_irssi_for_channel_and_query_traffic() {
        with_settings(&[], || {
            assert_eq!(tier_of_line(&chat(false), "#c"), Tier::Msg, "PUBLIC is in activity_msg_level");
            assert_eq!(tier_of_line(&chat(true), "#c"), Tier::Hilight, "a hilight is the loudest tier");
            assert_eq!(tier_of_line(&chat(true), "somenick"), Tier::Hilight, "an incoming DM is a hilight");
            assert_eq!(tier_of_line(&chat(false), "somenick"), Tier::Msg, "our own/echoed query line stays a plain message");
            assert_eq!(tier_of_line(&system(SystemKind::Notice), "status"), Tier::Msg, "NOTICES is in activity_msg_level");
            assert_eq!(tier_of_line(&system(SystemKind::Join), "#c"), Tier::Text);
            assert_eq!(tier_of_line(&system(SystemKind::Topic), "#c"), Tier::Text);
        });
    }

    #[test]
    fn the_midnight_marker_never_triggers_activity() {
        with_settings(&[], || assert_eq!(tier_of_line(&system(SystemKind::DayChange), "#c"), Tier::None));
    }

    #[test]
    fn activity_hide_level_silences_only_those_levels() {
        with_settings(&[("activity_hide_level", "JOINS")], || {
            assert_eq!(tier_of_line(&system(SystemKind::Join), "#c"), Tier::None);
            assert_eq!(tier_of_line(&system(SystemKind::Topic), "#c"), Tier::Text);
            assert_eq!(tier_of_line(&chat(false), "#c"), Tier::Msg);
        });
    }

    #[test]
    fn activity_msg_and_hilight_levels_reassign_tiers() {
        with_settings(&[("activity_msg_level", "PUBLIC NOTICES JOINS")], || {
            assert_eq!(tier_of_line(&system(SystemKind::Join), "#c"), Tier::Msg);
        });
        with_settings(&[("activity_hilight_level", "MSGS PUBLIC")], || {
            assert_eq!(tier_of_line(&chat(false), "#c"), Tier::Hilight, "every public line now counts as a hilight");
        });
        with_settings(&[("activity_msg_level", "")], || {
            assert_eq!(tier_of_line(&chat(false), "#c"), Tier::Text, "no longer a message tier");
        });
    }

    #[test]
    fn window_default_hidelevel_is_a_no_op_by_its_own_default() {
        with_settings(&[], || {
            assert_eq!(tier_of_line(&system(SystemKind::Join), "#c"), Tier::Text);
            assert_eq!(tier_of_line(&chat(false), "#c"), Tier::Msg);
        });
    }

    #[test]
    fn window_default_hidelevel_hides_activity_on_top_of_activity_hide_level() {
        with_settings(&[("window_default_hidelevel", "JOINS")], || {
            assert_eq!(tier_of_line(&system(SystemKind::Join), "#c"), Tier::None);
            assert_eq!(tier_of_line(&system(SystemKind::Topic), "#c"), Tier::Text, "unrelated level still shows");
        });
    }

    #[test]
    fn activity_hide_window_hidelevel_off_leaves_window_default_hidelevel_without_effect() {
        with_settings(&[("window_default_hidelevel", "JOINS"), ("activity_hide_window_hidelevel", "false")], || {
            assert_eq!(tier_of_line(&system(SystemKind::Join), "#c"), Tier::Text, "flag off, hidelevel ignored");
        });
    }

    #[test]
    fn activity_hide_targets_hides_a_target_but_not_a_hilight_in_it() {
        with_settings(&[("activity_hide_targets", "#Noisy libera/#other")], || {
            assert_eq!(tier_of_line(&chat(false), "#noisy"), Tier::None, "case-insensitive");
            assert_eq!(tier_of_line(&chat(false), "#other"), Tier::None, "tag/ prefix is dropped");
            assert_eq!(tier_of_line(&chat(true), "#noisy"), Tier::Hilight, "a hilighted line still shows");
            assert_eq!(tier_of_line(&chat(false), "#quiet"), Tier::Msg);
        });
    }

    #[test]
    fn buffer_push_sets_the_flags_the_tier_says() {
        with_settings(&[("activity_hide_level", "JOINS"), ("activity_msg_level", "")], || {
            let mut buf = Buffer::new("#c");
            buf.push_system_join("-- a joined");
            assert!(!buf.has_unread && !buf.has_unread_join, "hidden level leaves the window quiet");
            buf.push_system_leave("-- a left");
            assert!(buf.has_unread && !buf.has_unread_message);
            buf.push_chat("n", 0, None, "hi", false);
            assert!(buf.has_unread && !buf.has_unread_message, "msg_level emptied: a chat line is only text tier");
        });
        with_settings(&[], || {
            let mut buf = Buffer::new("#c");
            buf.push_chat("n", 0, None, "hi", false);
            assert!(buf.has_unread_message && !buf.has_unread_highlight);
            buf.mark_read();
            buf.push_chat("n", 0, None, "yo me", true);
            assert!(buf.has_unread_highlight && buf.has_unread_message, "hilight implies message");
        });
    }
}
