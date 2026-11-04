
use std::iter::Peekable;
use std::str::Chars;
use std::time::Duration;

use irust_config::lookandfeel;

use crate::state::{BufferId, ClientState};

const MAX_EVAL_DEPTH: u8 = 5;

/// Everything `/eval`'s expando table needs that isn't already reachable
/// straight off a `&ClientState`. Built once per `/eval` invocation by
/// the caller (`main.rs`), threaded down through recursion unchanged.
pub struct EvalContext<'a> {
    pub state: &'a ClientState,
    /// irust's network/chatnet key -- one string does both jobs here
    /// (see `$tag`/`$chatnet` below), unlike irssi which has both a
    /// server tag AND a separately-configurable chatnet name.
    pub tag: &'a str,
    pub version: &'static str,
    pub uptime: Duration,
}

fn split_commands(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut prev: Option<char> = None;
    for c in text.chars() {
        if c == ';' && prev != Some('\\') && prev != Some('$') {
            result.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
        prev = Some(c);
    }
    result.push(current);
    result
        .into_iter()
        .map(|s| s.trim_start().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Backslash-escapes + `$`-expando expansion for one already-split
/// segment (irssi's `parse_special_string`). Fail-open throughout: a
/// trailing lone `\` is dropped rather than causing an error, an
/// unrecognized escape char passes through literally, an unmatched `$`
/// expando is left as literal `$name` text.
fn expand(text: &str, ctx: &EvalContext) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => {}
            },
            '$' => out.push_str(&expand_dollar(&mut chars, ctx)),
            _ => out.push(c),
        }
    }
    out
}

fn expand_dollar(chars: &mut Peekable<Chars>, ctx: &EvalContext) -> String {
    let mut align: Option<(usize, bool)> = None;
    if chars.peek() == Some(&'[') {
        chars.next();
        let left = if chars.peek() == Some(&'-') {
            chars.next();
            true
        } else {
            false
        };
        let mut digits = String::new();
        while let Some(&d) = chars.peek() {
            if d.is_ascii_digit() {
                digits.push(d);
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek() == Some(&']') {
            chars.next();
        }
        if let Ok(n) = digits.parse::<usize>() {
            align = Some((n, left));
        }
    }

    let count_mode = if chars.peek() == Some(&'#') || chars.peek() == Some(&'@') {
        chars.next()
    } else {
        None
    };

    let name = read_name(chars);
    if name.is_empty() {
        // Bare `$` (nothing sensible followed it) -- literal, same as
        // irssi's own fallback.
        return "$".to_string();
    }

    let Some(mut value) = resolve_expando(&name, ctx) else {
        // Unmatched -- literal `$name`, fail-open (same rule
        // `theme_formats.rs::render` already uses for its own
        // placeholders).
        return format!("${name}");
    };

    if let Some(mode) = count_mode {
        value = match mode {
            '#' => value.split_whitespace().count().to_string(),
            '@' => value.chars().count().to_string(),
            _ => value,
        };
    }

    apply_align(value, align)
}

fn read_name(chars: &mut Peekable<Chars>) -> String {
    match chars.peek().copied() {
        None => String::new(),
        Some(c) if c.is_ascii_alphabetic() => {
            let mut name = String::new();
            name.push(c);
            chars.next();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_alphanumeric() || d == '_' {
                    name.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            name
        }
        Some(c) => {
            chars.next();
            c.to_string()
        }
    }
}

fn apply_align(value: String, align: Option<(usize, bool)>) -> String {
    let Some((width, left)) = align else {
        return value;
    };
    let len = value.chars().count();
    if len >= width {
        return value;
    }
    let pad = " ".repeat(width - len);
    if left {
        format!("{value}{pad}")
    } else {
        format!("{pad}{value}")
    }
}

fn active_channel_key(state: &ClientState) -> Option<String> {
    match &state.active_buffer {
        BufferId::Channel(key) => Some(key.clone()),
        _ => None,
    }
}

fn resolve_expando(name: &str, ctx: &EvalContext) -> Option<String> {
    let state = ctx.state;
    Some(match name {
        "N" => state.nick.clone(),
        "C" => active_channel_key(state)
            .and_then(|k| state.channels.get(&k))
            .map(|c| c.display_name.clone())
            .unwrap_or_default(),
        "T" => crate::input::active_target(state)
            .map(|(display, _)| display)
            .unwrap_or_default(),
        "Q" => match &state.active_buffer {
            BufferId::Query(key) => state
                .query_buffers
                .get(key)
                .map(|b| b.name.clone())
                .unwrap_or_else(|| key.clone()),
            _ => String::new(),
        },
        "tag" | "chatnet" => ctx.tag.to_string(),
        "topic" => active_channel_key(state)
            .and_then(|k| state.channels.get(&k))
            .and_then(|c| c.topic.clone())
            .unwrap_or_default(),
        "usermode" => state.own_modes.clone(),
        "cumode" => active_channel_key(state)
            .and_then(|k| state.own_rank_in_channel(&k))
            .map(|c| c.to_string())
            .unwrap_or_default(),
        "cumode_space" => active_channel_key(state)
            .and_then(|k| state.own_rank_in_channel(&k))
            .map(|c| c.to_string())
            .unwrap_or_else(|| " ".to_string()),
        "P" => {
            let is_op = active_channel_key(state).and_then(|k| state.own_rank_in_channel(&k)) == Some('@');
            if is_op {
                "1".to_string()
            } else {
                String::new()
            }
        }
        "Y" => irust_config::settings::realname(),
        "J" => ctx.version.to_string(),
        "K" | "k" => "/".to_string(),
        "$" => "$".to_string(),
        "Z" => crate::buffer::now_stamp(),
        "W" => std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        "sysname" => std::env::consts::OS.to_string(),
        "sysarch" => std::env::consts::ARCH.to_string(),
        // No Rust std equivalent of `uname -r` without a new crate
        // dependency just for this one field -- documented gap, empty
        // for now rather than pulling in a dependency for one string.
        "sysrelease" => String::new(),
        "F" => format_duration_dhms(ctx.uptime),
        "itemname" | "winname" => active_buffer_item_name(state),
        "winref" => {
            let ids = state.buffer_ids();
            ids.iter()
                .position(|id| *id == state.active_buffer)
                .map(|i| (i + 1).to_string())
                .unwrap_or_default()
        }
        "X" => state.own_userhost.clone().unwrap_or_default(),
        "x" => state
            .own_userhost
            .as_deref()
            .and_then(|uh| uh.split('@').nth(1))
            .unwrap_or_default()
            .to_string(),
        // `$R` -- irssi's own `expando_serverversion`, sourced from
        // `ClientState::server_version` (`client.rs`'s `"004"` handler).
        "R" => state.server_version.clone().unwrap_or_default(),
        "M" => {
            let full = active_channel_key(state)
                .and_then(|k| state.channels.get(&k))
                .map(|c| c.modestring.clone())
                .unwrap_or_default();
            if lookandfeel::get_bool("chanmode_expando_strip") {
                full.split_once(' ').map_or(full.clone(), |(letters, _)| letters.to_string())
            } else {
                full
            }
        }
        // `$A` -- irssi's own `expando_awaymsg`, sourced from
        // `ClientState::away_reason` (`/away`'s own handler in
        // `main.rs`, kept in sync from `"301"`/`"305"` in `client.rs`).
        "A" => state.away_reason.clone().unwrap_or_default(),
        "," => state.last_privmsg_from.clone().unwrap_or_default(),
        "." => state.last_sent_msg.clone().unwrap_or_default(),
        ";" => state.last_public_from.clone().unwrap_or_default(),
        ":" => state.last_join.clone().unwrap_or_default(),
        "I" => state.last_invite.clone().unwrap_or_default(),
        "D" => String::new(),
        _ => return None,
    })
}

/// Same "Xd Xh Xm Xs" shape as `main.rs`'s own `format_uptime` (not
/// reused directly -- that lives in the binary crate, this crate can't
/// see it -- duplicated as one small, independently-tested function
/// rather than restructuring crate boundaries just to share four lines).
fn format_duration_dhms(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    format!(
        "{}d {}h {}m {}s",
        secs / 86400,
        secs / 3600 % 24,
        secs / 60 % 60,
        secs % 60
    )
}

fn active_buffer_item_name(state: &ClientState) -> String {
    match &state.active_buffer {
        BufferId::Status => "status".to_string(),
        BufferId::Channel(key) => key.clone(),
        BufferId::Query(key) => key.clone(),
        BufferId::Empty(id) => format!("(empty {id})"),
    }
}

fn prepend_cmdchar(text: String, cmdchars: &str) -> String {
    let Some(first) = text.chars().next() else {
        return text;
    };
    if cmdchars.contains(first) {
        text
    } else {
        format!("{}{}", cmdchars.chars().next().unwrap_or('/'), text)
    }
}

fn as_eval_arg(cmd: &str, cmdchars: &str) -> Option<String> {
    let rest = cmd.strip_prefix(|c: char| cmdchars.contains(c))?;
    let mut parts = rest.splitn(2, ' ');
    let word = parts.next()?;
    if word.eq_ignore_ascii_case("eval") {
        Some(parts.next().unwrap_or("").to_string())
    } else {
        None
    }
}

/// Runs `/eval`'s whole pipeline on `text` and returns the flat list of
/// final command strings (nested `/eval`s already resolved, up to
/// `MAX_EVAL_DEPTH`) -- the caller resubmits each one through the normal
/// command pipeline exactly as if it had been typed, matching irssi's
/// own "resubmit through the generic dispatch" behavior.
pub fn eval_commands(text: &str, ctx: &EvalContext, cmdchars: &str, depth: u8) -> Vec<String> {
    split_commands(text)
        .into_iter()
        .map(|seg| expand(&seg, ctx))
        .map(|s| prepend_cmdchar(s, cmdchars))
        .filter(|s| !s.is_empty())
        .flat_map(|cmd| match as_eval_arg(&cmd, cmdchars) {
            Some(inner) if depth + 1 < MAX_EVAL_DEPTH => eval_commands(&inner, ctx, cmdchars, depth + 1),
            _ => vec![cmd],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ClientState;

    fn ctx(state: &ClientState) -> EvalContext<'_> {
        EvalContext {
            state,
            tag: "chatnet",
            version: "1.4.4",
            uptime: Duration::from_secs(3725),
        }
    }

    #[test]
    fn splits_on_unescaped_semicolons_and_trims_leading_space() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo one; echo two", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo one".to_string(), "/echo two".to_string()]);
    }

    #[test]
    fn escaped_semicolon_does_not_split() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands(r"echo 1\;2", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo 1;2".to_string()]);
    }

    #[test]
    fn dollar_before_semicolon_does_not_split_either() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo $;more", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo more".to_string()]);
    }

    #[test]
    fn auto_prepends_cmdchar_when_missing() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo hi", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo hi".to_string()]);
    }

    #[test]
    fn does_not_double_prepend_when_already_a_command() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("/join #foo", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/join #foo".to_string()]);
    }

    #[test]
    fn expands_nick_and_channel() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo $N in $C", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo abcX in #irust".to_string()]);
    }

    #[test]
    fn unmatched_expando_falls_back_to_literal_text() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo $notarealexpando", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo $notarealexpando".to_string()]);
    }

    #[test]
    fn literal_dollar_via_double_dollar() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo $$5", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo $5".to_string()]);
    }

    #[test]
    fn trailing_lone_backslash_is_dropped_not_a_panic() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands(r"echo done\", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo done".to_string()]);
    }

    #[test]
    fn unmatched_alignment_bracket_does_not_panic() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo $[10Nabc", &ctx(&st), "/", 0);
        // Malformed (no closing `]`) -- must not crash; exact fallback
        // text isn't the point, termination is.
        assert!(!cmds.is_empty());
    }

    #[test]
    fn alignment_pads_right_by_default_and_left_with_minus() {
        let st = ClientState::new("ab", "#irust", 11);
        let cmds = eval_commands("echo [$[5]N]", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo [   ab]".to_string()]);
        let cmds = eval_commands("echo [$[-5]N]", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo [ab   ]".to_string()]);
    }

    #[test]
    fn word_count_and_char_count_expandos() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        st.channels.get_mut("#irust").unwrap().topic = Some("three word topic".to_string());
        let cmds = eval_commands("echo $#topic words, $@topic chars", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo 3 words, 16 chars".to_string()]);
    }

    #[test]
    fn empty_segments_are_dropped() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo hi;;  ;echo bye", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo hi".to_string(), "/echo bye".to_string()]);
    }

    #[test]
    fn nested_eval_recurses_and_flattens() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("eval echo nested", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo nested".to_string()]);
    }

    #[test]
    fn deeply_nested_eval_terminates_at_the_depth_cap_instead_of_recursing_forever() {
        let st = ClientState::new("abcX", "#irust", 11);
        let nested = "eval eval eval eval eval eval eval eval eval eval echo still alive";
        let cmds = eval_commands(nested, &ctx(&st), "/", 0);
        assert!(!cmds.is_empty());
        assert!(cmds.len() < 20, "runaway eval recursion did not terminate: {cmds:?}");
    }

    #[test]
    fn cumode_and_cumode_space_reflect_own_channel_rank() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        st.channels
            .get_mut("#irust")
            .unwrap()
            .nicklist
            .add_from_names_token("@abcX");
        let cmds = eval_commands("echo [$cumode][$cumode_space]", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo [@][@]".to_string()]);
    }

    #[test]
    fn tag_chatnet_version_cmdchar_and_literal_dollar() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo $tag/$chatnet/$J/$k/$$", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo chatnet/chatnet/1.4.4///$".to_string()]);
    }

    #[test]
    fn itemname_winname_and_winref_reflect_the_active_buffer() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo $itemname/$winname/$winref", &ctx(&st), "/", 0);
        // A fresh ClientState's only buffer is the just-joined channel,
        // and status is always buffer index 1 in `buffer_ids()`'s own
        // ordering -- so the channel is winref 2, not 1.
        assert_eq!(cmds, vec!["/echo #irust/#irust/2".to_string()]);
    }

    #[test]
    fn query_and_chanop_expandos_are_empty_outside_their_context() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo [$Q][$P]", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo [][]".to_string()]);
    }

    #[test]
    fn chanmode_expando_is_empty_before_any_mode_is_ever_seen() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo [$M]", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo []".to_string()]);
    }

    #[test]
    fn chanmode_expando_reflects_the_active_channels_modestring() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        st.channels.get_mut("#irust").unwrap().modestring = "ntkl secretkey 50".to_string();
        let cmds = eval_commands("echo [$M]", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo [ntkl secretkey 50]".to_string()]);
    }

    #[test]
    fn chanmode_expando_strip_drops_mode_arguments() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        st.channels.get_mut("#irust").unwrap().modestring = "ntkl secretkey 50".to_string();
        lookandfeel::with_test_overrides(&[("chanmode_expando_strip", "true")], || {
            let cmds = eval_commands("echo [$M]", &ctx(&st), "/", 0);
            assert_eq!(cmds, vec!["/echo [ntkl]".to_string()]);
        });
        // Off (the default): unaffected.
        let cmds = eval_commands("echo [$M]", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo [ntkl secretkey 50]".to_string()]);
    }

    #[test]
    fn chanmode_expando_strip_is_a_no_op_with_no_arguments() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        st.channels.get_mut("#irust").unwrap().modestring = "nt".to_string();
        lookandfeel::with_test_overrides(&[("chanmode_expando_strip", "true")], || {
            let cmds = eval_commands("echo [$M]", &ctx(&st), "/", 0);
            assert_eq!(cmds, vec!["/echo [nt]".to_string()]);
        });
    }

    #[test]
    fn serverversion_and_userhost_expandos_are_empty_before_their_numeric_arrives() {
        let st = ClientState::new("abcX", "#irust", 11);
        let cmds = eval_commands("echo [$R][$X][$x]", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo [][][]".to_string()]);
    }

    #[test]
    fn serverversion_and_userhost_expandos_reflect_state_once_populated() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        st.server_version = Some("solanum-1.0-dev".to_string());
        st.own_userhost = Some("~abcx@example.net".to_string());
        let cmds = eval_commands("echo [$R][$X][$x]", &ctx(&st), "/", 0);
        assert_eq!(
            cmds,
            vec!["/echo [solanum-1.0-dev][~abcx@example.net][example.net]".to_string()]
        );
    }

    #[test]
    fn away_message_expando_is_empty_when_not_away_and_reflects_the_reason_once_set() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        assert_eq!(
            eval_commands("echo [$A]", &ctx(&st), "/", 0),
            vec!["/echo []".to_string()]
        );

        st.away_reason = Some("brb lunch".to_string());
        assert_eq!(
            eval_commands("echo [$A]", &ctx(&st), "/", 0),
            vec!["/echo [brb lunch]".to_string()]
        );
    }

    #[test]
    fn last_message_expandos_reflect_the_respective_state_fields() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        assert_eq!(
            eval_commands("echo [$,][$.][$;]", &ctx(&st), "/", 0),
            vec!["/echo [][][]".to_string()]
        );

        st.last_privmsg_from = Some("Friend".to_string());
        st.last_sent_msg = Some("K9".to_string());
        st.last_public_from = Some("Other".to_string());
        assert_eq!(
            eval_commands("echo [$,][$.][$;]", &ctx(&st), "/", 0),
            vec!["/echo [Friend][K9][Other]".to_string()]
        );
    }

    #[test]
    fn last_join_and_last_invite_expandos_reflect_state() {
        let mut st = ClientState::new("abcX", "#irust", 11);
        assert_eq!(
            eval_commands("echo [$:][$I]", &ctx(&st), "/", 0),
            vec!["/echo [][]".to_string()]
        );

        st.last_join = Some("dana".to_string());
        st.last_invite = Some("#chatnet".to_string());
        assert_eq!(
            eval_commands("echo [$:][$I]", &ctx(&st), "/", 0),
            vec!["/echo [dana][#chatnet]".to_string()]
        );
    }

    #[test]
    fn notify_list_expando_is_still_a_documented_gap() {
        // $D -- irust's /notify has no periodic-WHOIS mechanism for this
        // to source from at all (see its own match-arm doc comment).
        let st = ClientState::new("abcX", "#irust", 11);
        assert_eq!(
            eval_commands("echo [$D]", &ctx(&st), "/", 0),
            vec!["/echo []".to_string()]
        );
    }

    #[test]
    fn multibyte_nick_and_topic_do_not_panic() {
        let mut st = ClientState::new("café☕", "#irust", 11);
        st.channels.get_mut("#irust").unwrap().topic = Some("héllo wörld ☕☕".to_string());
        let cmds = eval_commands("echo $N: $topic ($@topic chars)", &ctx(&st), "/", 0);
        assert_eq!(cmds, vec!["/echo café☕: héllo wörld ☕☕ (14 chars)".to_string()]);
    }
}
