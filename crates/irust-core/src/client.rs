use irust_proto::{Prefix, RawMessage};

use crate::netsplit;
use crate::nicklist::{apply_channel_modestring, apply_mode_line, extract_ban_changes, ChanModesTable, PrefixTable};
use crate::service_notices::{is_service_nick, route_service_notice};
use crate::state::{BufferId, ClientState, PendingAutoban};

const MAX_NICK_REGISTRATION_RETRIES: u8 = 5;

fn host_suffix(prefix: &Prefix) -> String {
    match (&prefix.user, &prefix.host) {
        (Some(user), Some(host)) => format!(" [{user}@{host}]"),
        _ => String::new(),
    }
}

fn quit_display_lines(
    nick: &str,
    hostsuffix: &str,
    reason: &str,
    shared: &[String],
    collapse: bool,
) -> Vec<(String, String)> {
    if shared.is_empty() {
        return Vec::new();
    }
    if !collapse {
        return shared
            .iter()
            .map(|key| (key.clone(), irust_config::theme_formats::quit(nick, hostsuffix, reason)))
            .collect();
    }
    let mut line = irust_config::theme_formats::quit(nick, hostsuffix, reason);
    let more = shared.len() - 1;
    if more > 0 {
        line = format!(
            "{line} (was also in {more} more channel{})",
            if more == 1 { "" } else { "s" }
        );
    }
    vec![(shared[0].clone(), line)]
}

pub fn handle_message(state: &mut ClientState, msg: &RawMessage) -> Vec<String> {
    handle_message_with_highlight(state, msg, &[], &[], &[]).0
}

/// The full `nick!user@host` a `/hilight -mask`/`/ignore` entry matches
/// against, missing pieces become `*`.
fn full_mask(prefix: &Prefix) -> String {
    format!(
        "{}!{}@{}",
        prefix.name,
        prefix.user.as_deref().unwrap_or("*"),
        prefix.host.as_deref().unwrap_or("*")
    )
}

fn entry_applies(entry: &irust_config::IgnoreEntry, full: &str, nick: &str, level: &str) -> bool {
    let mask_matches = if entry.is_regex {
        crate::nicklist::matches_regex_cached(&entry.mask, full)
    } else if entry.mask.contains(['!', '@']) {
        crate::nicklist::matches_glob(&entry.mask, full)
    } else {
        crate::nicklist::matches_glob(&entry.mask, nick)
    };
    mask_matches && (entry.levels.is_empty() || entry.levels.iter().any(|l| l.eq_ignore_ascii_case(level)))
}

pub fn is_ignored(entries: &[irust_config::IgnoreEntry], prefix: &Prefix, level: &str) -> bool {
    let full = full_mask(prefix);
    let mut ignored = false;
    let mut excepted = false;
    for entry in entries {
        if entry_applies(entry, &full, &prefix.name, level) {
            if entry.is_except {
                excepted = true;
            } else {
                ignored = true;
            }
        }
    }
    ignored && !excepted
}

pub fn is_ignored_reply(
    entries: &[irust_config::IgnoreEntry],
    channel_nicks: &[String],
    text: &str,
    level: &str,
) -> bool {
    entries.iter().filter(|e| e.is_replies && !e.is_except).any(|entry| {
        channel_nicks
            .iter()
            .any(|nick| entry_applies(entry, nick, nick, level) && addresses_nick(text, nick))
    })
}

fn dcc_autoget_masks_match(masks: &str, nick: &str, full: &str) -> bool {
    masks.split_whitespace().any(|mask| {
        if mask.contains(['!', '@']) {
            crate::nicklist::matches_glob(mask, full)
        } else {
            crate::nicklist::matches_glob(mask, nick)
        }
    })
}

fn dcc_autoget_eligible(prefix: &Prefix, target: &str, filename: &str, size: u64, port: u16) -> bool {
    if !irust_config::settings::dcc_autoget() {
        return false;
    }
    if port < 1024 && !irust_config::settings::dcc_autoaccept_lowports() {
        return false;
    }
    let masks = irust_config::settings::dcc_autoget_masks();
    if !masks.is_empty() {
        if !dcc_autoget_masks_match(&masks, &prefix.name, &full_mask(prefix)) {
            return false;
        }
    } else if target.starts_with(['#', '&']) {
        // No masks configured at all -- auto-accept only ever applies
        // to a private/DM offer, never a channel-directed one, matching
        // `dcc-autoget.c` exactly.
        return false;
    }
    if filename.starts_with('.') && irust_config::settings::dcc_downloads_dir() == "~" {
        return false;
    }
    let max = irust_config::settings::dcc_autoget_max_size();
    let max: u64 = max.parse().unwrap_or(0);
    if max > 0 && size > max {
        return false;
    }
    true
}

/// `true` if `text` opens by addressing `nick` -- `"<nick>: ..."` or
/// `"<nick>, ..."`, case-insensitive, irssi's own `nick_match_msg`
/// convention for "who is this message talking to."
fn addresses_nick(text: &str, nick: &str) -> bool {
    let Some(rest) = text.get(..nick.len().min(text.len())) else {
        return false;
    };
    if !rest.eq_ignore_ascii_case(nick) {
        return false;
    }
    matches!(text[rest.len()..].chars().next(), Some(':') | Some(','))
}

pub fn contains_risky_emoji_sequence(text: &str) -> bool {
    const VARIATION_SELECTOR_16: char = '\u{FE0F}';
    const ZERO_WIDTH_JOINER: char = '\u{200D}';
    const REGIONAL_INDICATOR: std::ops::RangeInclusive<char> = '\u{1F1E6}'..='\u{1F1FF}';

    let mut prev_was_regional_indicator = false;
    for c in text.chars() {
        if c == VARIATION_SELECTOR_16 || c == ZERO_WIDTH_JOINER {
            return true;
        }
        let is_regional_indicator = REGIONAL_INDICATOR.contains(&c);
        if is_regional_indicator && prev_was_regional_indicator {
            return true;
        }
        prev_was_regional_indicator = is_regional_indicator;
    }
    false
}

fn autoban_reason_for(text: &str, entries: &[irust_config::AutobanEntry]) -> Option<String> {
    let lower_text = text.to_lowercase();
    entries.iter().find_map(|entry| {
        let hit = if entry.is_regex {
            crate::nicklist::matches_regex_cached(&entry.pattern, text)
        } else {
            lower_text.contains(&entry.pattern.to_lowercase())
        };
        hit.then(|| entry.reason.clone().unwrap_or_else(|| "spam".to_string()))
    })
}

pub fn handle_message_with_highlight(
    state: &mut ClientState,
    msg: &RawMessage,
    hilight_entries: &[irust_config::HilightEntry],
    ignore_entries: &[irust_config::IgnoreEntry],
    autoban_entries: &[irust_config::AutobanEntry],
) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut highlighted = false;

    // Flood-detection autoignores act like extra `/ignore` entries until
    // they expire; messages are counted before they are handled.
    let now = crate::buffer::now_epoch();
    crate::flood::live_entries(&mut state.auto_ignores, now);
    let combined: Vec<irust_config::IgnoreEntry>;
    let ignore_entries: &[irust_config::IgnoreEntry] = if state.auto_ignores.is_empty() {
        ignore_entries
    } else {
        combined = ignore_entries.iter().chain(state.auto_ignores.iter()).cloned().collect();
        &combined
    };
    if matches!(msg.command.as_str(), "PRIVMSG" | "NOTICE") {
        note_flood(state, msg, ignore_entries, now);
        // A sender autoignored for flooding is dropped like an `/ignore`d
        // one (`main.rs` only knows the configured list): not shown, not
        // logged, no CTCP auto-reply.
        if let Some(prefix) = &msg.prefix {
            if is_ignored(&state.auto_ignores, prefix, flood_level(msg)) {
                return (out, false);
            }
        }
    }

    // irssi's `motd_got`: set on the MOTD's end before anything renders it.
    if matches!(msg.command.as_str(), "376" | "422") {
        state.motd_got = true;
    }
    if msg.command == "381" {
        let opermode = irust_config::settings_net::str_of("opermode");
        if !opermode.is_empty() {
            state.pending_opermode = Some(opermode);
        }
    }
    match msg.command.as_str() {
        "PING" => {
            let token = msg.params.first().cloned().unwrap_or_default();
            out.push(format!("PONG :{token}"));
        }
        "001" => {
            state.connected = true;
            state.nick_registration_retries = 0;
            state.motd_got = false;
            state.rejoin_channels.clear();
            if let Some(confirmed_nick) = msg.params.first() {
                if confirmed_nick != &state.nick {
                    state.nick = confirmed_nick.clone();
                }
            }
            // irssi sends the configured `usermode` right after
            // registration completes, before the autojoin (irc-servers.c
            // `event_connected`), as a plain MODE on our own nick.
            let usermode = irust_config::settings_net::str_of("usermode");
            if !usermode.is_empty() {
                out.push(format!("MODE {} {}", state.nick, usermode));
            }
            let joins: Vec<(String, Option<String>)> = state
                .channels
                .values()
                .filter(|c| !c.left_by_user)
                .map(|c| (c.display_name.clone(), c.key.clone()))
                .collect();
            for (name, key) in joins {
                match key {
                    Some(key) => out.push(format!("JOIN {name} {key}")),
                    None => out.push(format!("JOIN {name}")),
                }
            }
            out.push(format!("WHOIS {}", state.nick));
            state.self_whois_pending = true;
        }
        "004" => {
            if let Some(version) = msg.params.get(2) {
                state.server_version = Some(version.clone());
            }
        }
        "396" => {
            if let Some(new_host) = msg.params.get(1) {
                state.own_userhost = match (&state.own_userhost, new_host.contains('@')) {
                    (_, true) => Some(new_host.clone()),
                    (Some(existing), false) => {
                        let user = existing.split('@').next().unwrap_or(existing);
                        Some(format!("{user}@{new_host}"))
                    }
                    (None, false) => None,
                };
            }
        }
        "JOIN" => {
            if let Some(prefix) = &msg.prefix {
                let chan_name = msg.params.first().cloned().unwrap_or_default();
                irust_config::debug_log::line(
                    "JOIN",
                    "-",
                    &format!(
                        "prefix={:?} chan={chan_name:?} state.nick={:?} bouncer_login={:?} is_self={}",
                        prefix.name,
                        state.nick,
                        state.bouncer_login,
                        state.is_self(&prefix.name)
                    ),
                );
                if state.is_self(&prefix.name) {
                    // irssi `sig_remove_rejoin`: a successful join ends the retry.
                    state.rejoin_channels.retain(|r| !r.channel.eq_ignore_ascii_case(&chan_name));
                    if state.own_userhost.is_none() {
                        if let (Some(user), Some(host)) = (&prefix.user, &prefix.host) {
                            state.own_userhost = Some(format!("{user}@{host}"));
                        }
                    }
                    let key = state.ensure_channel(&chan_name);
                    if let Some(chan) = state.channels.get_mut(&key) {
                        // fresh join (or a rejoin) -- the upcoming
                        // RPL_NAMREPLY is authoritative, don't carry over
                        // stale entries
                        chan.nicklist.clear();
                        chan.joined = true;
                        chan.left_by_user = false;
                        chan.buffer.push_system_join(irust_config::theme_formats::join_self(
                            &prefix.name,
                            &host_suffix(prefix),
                            &chan_name,
                        ));
                        // Starts the clock the "366" handler below reads
                        // to print irssi's own "Join to #chan was synced
                        // in N secs" line once RPL_ENDOFNAMES arrives.
                        chan.join_started_at = Some(std::time::Instant::now());
                    }
                    state
                        .pending_join_notices
                        .push_back((key.clone(), std::time::Instant::now()));
                    while state.pending_join_notices.len() > 20 {
                        state.pending_join_notices.pop_front();
                    }
                    if irust_config::settings_net::bool_of("channel_sync") {
                        out.push(format!("MODE {chan_name}"));
                    }
                    state.active_buffer = BufferId::Channel(key);
                } else {
                    state.last_join = Some(prefix.name.clone());
                    let chan_key = chan_name.to_lowercase();
                    let is_split_rejoin = state.record_split_join_if_recent(&chan_key, &prefix.name);
                    if let Some(chan) = state.channels.get_mut(&chan_key) {
                        chan.nicklist.insert_plain(&prefix.name);
                        chan.nicklist
                            .set_userhost(&prefix.name, prefix.user.as_deref(), prefix.host.as_deref());
                        crate::nick_completion::last_msg_add(
                            &mut chan.last_msgs,
                            &prefix.name,
                            false,
                            irust_config::settings_history_log::get_int("completion_keep_publics"),
                        );
                        if !is_split_rejoin && !is_ignored(ignore_entries, prefix, "joins") {
                            let mut line = irust_config::theme_formats::join(
                                &prefix.name,
                                &host_suffix(prefix),
                                &chan_name,
                            );
                            if irust_config::lookandfeel::get_bool("show_extended_join") {
                                if let (Some(account), Some(realname)) =
                                    (msg.params.get(1), msg.params.get(2))
                                {
                                    if account != "*" {
                                        line.push_str(&format!(" as {account}"));
                                    }
                                    if !realname.is_empty() {
                                        line.push_str(&format!(" ({realname})"));
                                    }
                                }
                            }
                            chan.buffer.push_system_join(line);
                        }
                    }
                }
            }
        }
        "PART" => {
            if let Some(prefix) = &msg.prefix {
                let chan_name = msg.params.first().cloned().unwrap_or_default();
                let is_us = state.is_self(&prefix.name);
                match state.channels.get_mut(&chan_name.to_lowercase()) {
                    Some(chan) => {
                        chan.nicklist.remove(&prefix.name);
                        crate::nick_completion::last_msg_remove(&mut chan.last_msgs, &prefix.name);
                        if is_us {
                            chan.joined = false;
                            chan.left_by_user = true;
                            let reason = msg.params.get(1).cloned().unwrap_or_default();
                            chan.buffer.push_system_leave(irust_config::theme_formats::part_self(
                                &prefix.name,
                                &host_suffix(prefix),
                                &chan_name,
                                &reason,
                            ));
                        } else if !is_ignored(ignore_entries, prefix, "parts") {
                            let reason = msg.params.get(1).cloned().unwrap_or_default();
                            chan.buffer.push_system_leave(irust_config::theme_formats::part(
                                &prefix.name,
                                &host_suffix(prefix),
                                &chan_name,
                                &reason,
                            ));
                        }
                    }
                    None if is_us => {
                        let reason = msg.params.get(1).cloned().unwrap_or_default();
                        state.status.push_system_leave(irust_config::theme_formats::part_self(
                            &prefix.name,
                            &host_suffix(prefix),
                            &chan_name,
                            &reason,
                        ));
                    }
                    None => {}
                }
            }
        }
        "KICK" => {
            if let Some(prefix) = &msg.prefix {
                let chan_name = msg.params.first().cloned().unwrap_or_default();
                let kicked_nick = msg.params.get(1).cloned().unwrap_or_default();
                let reason = msg.params.get(2).cloned().unwrap_or_default();
                if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                    chan.nicklist.remove(&kicked_nick);
                    crate::nick_completion::last_msg_remove(&mut chan.last_msgs, &kicked_nick);
                    if kicked_nick == state.nick {
                        chan.joined = false;
                        chan.buffer.push_system_leave(irust_config::theme_formats::kick_self(
                            &chan_name,
                            &prefix.name,
                            &reason,
                        ));
                    } else {
                        chan.buffer.push_system_leave(irust_config::theme_formats::kick(
                            &kicked_nick,
                            &chan_name,
                            &prefix.name,
                            &reason,
                        ));
                    }
                }
            }
        }
        "QUIT" => {
            if let Some(prefix) = &msg.prefix {
                let reason = msg.params.first().cloned().unwrap_or_default();
                if netsplit::is_split_reason(&reason) && irust_config::settings_net::bool_of("hide_netsplit_quits") {
                    state.record_split_quit(&prefix.name, &reason);
                } else {
                    let quit_ignored = is_ignored(ignore_entries, prefix, "quits");
                    let mut shared: Vec<String> = state
                        .channels
                        .iter()
                        .filter(|(_, chan)| chan.nicklist.contains(&prefix.name))
                        .map(|(key, _)| key.clone())
                        .collect();
                    // Sorted so `/set show_quit_once`'s collapsed single
                    // line always lands in the same (deterministic, not
                    // HashMap-iteration-order) channel run to run.
                    shared.sort();
                    for key in &shared {
                        if let Some(chan) = state.channels.get_mut(key) {
                            chan.nicklist.remove(&prefix.name);
                            crate::nick_completion::last_msg_remove(&mut chan.last_msgs, &prefix.name);
                        }
                    }
                    if !quit_ignored {
                        let collapse =
                            irust_config::settings::SHOW_QUIT_ONCE.load(std::sync::atomic::Ordering::Relaxed);
                        for (key, line) in
                            quit_display_lines(&prefix.name, &host_suffix(prefix), &reason, &shared, collapse)
                        {
                            if let Some(chan) = state.channels.get_mut(&key) {
                                chan.buffer.push_system_leave(line);
                            }
                        }
                    }
                }
            }
        }
        "NICK" => {
            if let Some(prefix) = &msg.prefix {
                let new_nick = msg.params.first().cloned().unwrap_or_default();
                let is_us = state.is_self(&prefix.name);
                let nick_ignored = !is_us && is_ignored(ignore_entries, prefix, "nicks");
                let mut active_channel_got_it = false;
                // `/set show_own_nickchange_once`: one "You're now known
                // as" line where we're looking, not one per channel.
                let own_nick_once = is_us && irust_config::lookandfeel::get_bool("show_own_nickchange_once");
                for (key, chan) in state.channels.iter_mut() {
                    // For our own rename, `joined` is the right relevance
                    // check (nicklist membership can lag behind a fresh
                    // JOIN until RPL_NAMREPLY arrives). For someone
                    // else's, nicklist membership is the correct proxy
                    // for "was this nick visible to us in this channel."
                    let relevant = if is_us {
                        chan.joined
                    } else {
                        chan.nicklist.contains(&prefix.name)
                    };
                    if relevant {
                        chan.nicklist.rename(&prefix.name, &new_nick);
                        crate::nick_completion::last_msg_rename(&mut chan.last_msgs, &prefix.name, &new_nick);
                        if is_us {
                            if !own_nick_once {
                                chan.buffer
                                    .push_system(irust_config::theme_formats::nick_self(&new_nick));
                                if state.active_buffer == BufferId::Channel(key.clone()) {
                                    active_channel_got_it = true;
                                }
                            }
                        } else if !nick_ignored {
                            chan.buffer
                                .push_system(irust_config::theme_formats::nick(&prefix.name, &new_nick));
                        }
                    }
                }
                if is_us && !active_channel_got_it {
                    state
                        .current_buffer_mut()
                        .push_system(irust_config::theme_formats::nick_self(&new_nick));
                }
                if !is_us && irust_config::lookandfeel::get_bool("query_track_nick_changes") {
                    let old_key = prefix.name.to_lowercase();
                    if let Some(mut buf) = state.query_buffers.remove(&old_key) {
                        buf.name = new_nick.clone();
                        buf.push_system(irust_config::theme_formats::nick(&prefix.name, &new_nick));
                        let new_key = new_nick.to_lowercase();
                        if state.active_buffer == BufferId::Query(old_key.clone()) {
                            state.active_buffer = BufferId::Query(new_key.clone());
                        }
                        state.query_buffers.insert(new_key, buf);
                    }
                }
                state.nick_colors.rename(&prefix.name, &new_nick);
                if is_us {
                    state.nick = new_nick;
                }
            }
        }
        "CHGHOST" => {
            if let Some(prefix) = &msg.prefix {
                let new_user = msg.params.first().cloned().unwrap_or_default();
                let new_host = msg.params.get(1).cloned().unwrap_or_default();
                for chan in state.channels.values_mut() {
                    if chan.nicklist.contains(&prefix.name) {
                        chan.nicklist
                            .set_userhost(&prefix.name, Some(&new_user), Some(&new_host));
                        chan.buffer.push_system(irust_config::theme_formats::chghost(
                            &prefix.name,
                            &new_user,
                            &new_host,
                        ));
                    }
                }
            }
        }
        "ACCOUNT" => {
            if let Some(prefix) = &msg.prefix {
                let account = msg.params.first().cloned().unwrap_or_default();
                // irssi only announces these with show_account_notify (its
                // default is off); irust always did, so its default is on.
                let announce = irust_config::lookandfeel::get_bool("show_account_notify");
                for chan in state.channels.values_mut() {
                    if announce && chan.nicklist.contains(&prefix.name) {
                        if account == "*" {
                            chan.buffer
                                .push_system(irust_config::theme_formats::account_logout(&prefix.name));
                        } else {
                            chan.buffer
                                .push_system(irust_config::theme_formats::account_login(&prefix.name, &account));
                        }
                    }
                }
            }
        }
        "SETNAME" => {
            if let Some(prefix) = &msg.prefix {
                let new_name = msg.params.first().cloned().unwrap_or_default();
                for chan in state.channels.values_mut() {
                    if chan.nicklist.contains(&prefix.name) {
                        chan.buffer
                            .push_system(irust_config::theme_formats::setname(&prefix.name, &new_name));
                    }
                }
            }
        }
        "MODE" => {
            let target = msg.params.first().cloned().unwrap_or_default();
            if target.starts_with(['#', '&']) {
                if let Some(chan) = state.channels.get_mut(&target.to_lowercase()) {
                    if let Some(modestring) = msg.params.get(1) {
                        // `$M` -- see `apply_channel_modestring`'s own
                        // doc comment for the full irssi-verified
                        // breakdown of what does/doesn't end up in here.
                        chan.modestring =
                            apply_channel_modestring(&chan.modestring, modestring, &msg.params[2..], &chan.nicklist);
                        apply_mode_line(&mut chan.nicklist, modestring, &msg.params[2..]);
                        let ban_changes = extract_ban_changes(modestring, &msg.params[2..], &chan.nicklist);
                        for (adding, mask) in ban_changes {
                            if adding {
                                if !chan.bans.iter().any(|b| b.eq_ignore_ascii_case(&mask)) {
                                    chan.bans.push(mask);
                                }
                            } else {
                                chan.bans.retain(|b| !b.eq_ignore_ascii_case(&mask));
                            }
                        }
                        let args = msg.params[2..].join(" ");
                        let full_mode = if args.is_empty() {
                            modestring.clone()
                        } else {
                            format!("{modestring} {args}")
                        };
                        let setter = msg.prefix.as_ref().map(|p| p.name.as_str()).unwrap_or("server");
                        chan.buffer
                            .push_system(irust_config::theme_formats::mode_channel(&target, &full_mode, setter));
                    }
                }
            } else if target == state.nick {
                if let Some(modestring) = msg.params.get(1) {
                    state.own_modes = apply_usermode_line(&state.own_modes, modestring);
                    state
                        .status
                        .push_system(irust_config::theme_formats::mode_user(modestring, &state.nick));
                }
            }
        }
        "221" => {
            // RPL_UMODEIS: params = [<nick>, <modestring>] -- a reply to
            // a MODE query, so it's a full replacement, not a delta.
            if let Some(modestring) = msg.params.get(1) {
                state.own_modes = apply_usermode_line("", modestring);
            }
        }
        "353" => {
            // RPL_NAMREPLY: params = [<nick>, <symbol>, <channel>, <names>]
            let chan_name = msg.params.get(2).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                if let Some(names) = msg.params.last() {
                    for token in names.split(' ').filter(|t| !t.is_empty()) {
                        chan.nicklist.add_from_names_token(token);
                    }
                }
            }
        }
        "324" => {
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            let modestring = msg.params.get(2).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                let mode_args = msg.params.get(3..).unwrap_or(&[]);
                chan.modestring = apply_channel_modestring(&chan.modestring, &modestring, mode_args, &chan.nicklist);
                let args = mode_args.join(" ");
                let full_mode = if args.is_empty() {
                    modestring
                } else {
                    format!("{modestring} {args}")
                };
                chan.buffer
                    .push_system_topic(format!("-- mode/{chan_name} [{full_mode}]"));
            }
        }
        "375" | "372" if irust_config::settings_net::bool_of("skip_motd") && !state.motd_got => {}
        "903" => state.status.push_system("-- SASL authentication succeeded".to_string()),
        "904" | "905" | "906" => {
            let reason = msg.params.get(1..).map(|p| p.join(" ")).unwrap_or_default();
            state
                .status
                .push_system(format!("-- Cannot authenticate via SASL ({reason})"));
            // irssi `sasl_disconnect_on_failure`: drop the connection instead of
            // carrying on unauthenticated (sasl.c `sig_sasl_over`).
            if irust_config::settings_net::bool_of("sasl_disconnect_on_failure") {
                out.push("QUIT".to_string());
            }
        }
        "328" => {
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            let url = msg.params.get(2).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                chan.buffer
                    .push_system_topic(format!("-- Home page for {chan_name}: {url}"));
            }
        }
        "329" => {
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            let raw_time = msg.params.get(2).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                if let Some(date) = format_epoch_secs(&raw_time) {
                    chan.buffer
                        .push_system_topic(format!("-- Channel {chan_name} created {date}"));
                }
            }
        }
        "366" => {
            // RPL_ENDOFNAMES -- nicklist is populated, but away-notify
            // only reports *changes* in away status from here on, not
            // who's already away right now. WHO the channel once to
            // seed everyone's initial state (see the "352" handler).
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            if !chan_name.is_empty() {
                let max_who = irust_config::settings_net::int_of("channel_max_who_sync") as usize;
                let too_big = state
                    .channels
                    .get(&chan_name.to_lowercase())
                    .is_some_and(|chan| chan.nicklist.len() > max_who);
                if !too_big && irust_config::settings_net::bool_of("channel_sync") {
                    out.push(format!("WHO {chan_name}"));
                }
                if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                    if let Some(started) = chan.join_started_at.take() {
                        let (ops, halfops, voices, normal) = chan.nicklist.status_counts();
                        let total = chan.nicklist.len();
                        chan.buffer.push_system_topic(format!(
                            "-- irust: {chan_name}: Total of {total} nicks [{ops} ops, {halfops} halfops, {voices} voices, {normal} normal]"
                        ));
                        let secs = started.elapsed().as_secs();
                        chan.buffer
                            .push_system_topic(format!("-- irust: Join to {chan_name} was synced in {secs} secs"));
                    }
                }
            }
        }
        "367" => {
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            let mask = msg.params.get(2).cloned().unwrap_or_default();
            let setter = msg.params.get(3).cloned().unwrap_or_default();
            let raw_time = msg.params.get(4).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                if !mask.is_empty() && !chan.bans.iter().any(|b| b == &mask) {
                    chan.bans.push(mask.clone());
                }
                let index = chan.bans.iter().position(|b| b == &mask).map_or(0, |i| i + 1);
                let line = if setter.is_empty() {
                    format!("-- {index} - {chan_name}: ban {mask}")
                } else {
                    let secs = elapsed_secs_since(&raw_time).unwrap_or(0);
                    format!("-- {index} - {chan_name}: ban {mask} (by {setter}, {secs} secs ago)")
                };
                chan.buffer.push_system(line);
            }
        }
        "368" => {
            // RPL_ENDOFBANLIST -- nothing further to capture, the list
            // built from 367 above is already what `/unban` resolves
            // against.
        }
        "346" => {
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            let mask = msg.params.get(2).cloned().unwrap_or_default();
            let setter = msg.params.get(3).cloned().unwrap_or_default();
            let raw_time = msg.params.get(4).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                let line = if setter.is_empty() {
                    format!("-- {chan_name}: invite {mask}")
                } else {
                    let secs = elapsed_secs_since(&raw_time).unwrap_or(0);
                    format!("-- {chan_name}: invite {mask} (by {setter}, {secs} secs ago)")
                };
                chan.buffer.push_system(line);
            }
        }
        "347" => {} // RPL_ENDOFINVITELIST -- nothing further to capture.
        "348" => {
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            let mask = msg.params.get(2).cloned().unwrap_or_default();
            let setter = msg.params.get(3).cloned().unwrap_or_default();
            let raw_time = msg.params.get(4).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                let line = if setter.is_empty() {
                    format!("-- {chan_name}: ban exception {mask}")
                } else {
                    let secs = elapsed_secs_since(&raw_time).unwrap_or(0);
                    format!("-- {chan_name}: ban exception {mask} (by {setter}, {secs} secs ago)")
                };
                chan.buffer.push_system(line);
            }
        }
        "349" => {} // RPL_ENDOFEXCEPTLIST -- nothing further to capture.
        "352" => {
            let who_nick = msg.params.get(5).cloned().unwrap_or_default();
            let flags = msg.params.get(6).cloned().unwrap_or_default();
            let away = flags.starts_with('G');
            for chan in state.channels.values_mut() {
                chan.nicklist.set_away(&who_nick, away);
            }
            // A user-typed `/who` also prints each row (irssi's `who`
            // format: `chan nick flags hops user@host (realname)`); the
            // silent post-join seeding `WHO` never does.
            let who_chan = msg.params.get(1).cloned().unwrap_or_default();
            let wanted = state.manual_who_pending.iter().any(|t| {
                t.eq_ignore_ascii_case(&who_chan) || !t.starts_with(['#', '&', '!', '+'])
            });
            if wanted {
                let user = msg.params.get(2).cloned().unwrap_or_default();
                let host = msg.params.get(3).cloned().unwrap_or_default();
                let tail = msg.params.get(7).cloned().unwrap_or_default();
                let (hops, realname) = tail.split_once(' ').unwrap_or((tail.as_str(), ""));
                let chan_col: String = who_chan.chars().take(10).collect();
                state.current_buffer_mut().push_system(format!(
                    "{chan_col:>10} {who_nick:<9} {flags:<3} {hops:<2} {user}@{host} ({realname})"
                ));
            }
        }
        "INVITE" => {
            if let Some(prefix) = &msg.prefix {
                let invitee = msg.params.first().cloned().unwrap_or_default();
                let channel = msg.params.get(1).cloned().unwrap_or_default();
                if state.is_self(&invitee) {
                    state.last_invite = Some(channel.clone());
                    // irssi `channel-events.c`: only for a channel we're not in.
                    if irust_config::settings_net::bool_of("join_auto_chans_on_invite")
                        && !state.channels.get(&channel.to_lowercase()).is_some_and(|c| c.joined)
                    {
                        state.pending_invite_autojoin.push(channel.clone());
                    }
                    state
                        .current_buffer_mut()
                        .push_system(irust_config::theme_formats::invite(&prefix.name, &channel));
                } else if let Some(chan) = state.channels.get_mut(&channel.to_lowercase()) {
                    chan.buffer.push_system(irust_config::theme_formats::invite_notify(
                        &prefix.name,
                        &invitee,
                        &channel,
                    ));
                }
            }
        }
        "AWAY" => {
            // away-notify: `:nick AWAY :reason` means now away, `:nick
            // AWAY` with no trailing param means back. Not scoped to one
            // channel -- update everywhere this nick is visible.
            if let Some(prefix) = &msg.prefix {
                let away = !msg.params.is_empty();
                let announce = irust_config::lookandfeel::get_bool("away_notify_public");
                let reason = msg.params.first().cloned().unwrap_or_default();
                for chan in state.channels.values_mut() {
                    let present = chan.nicklist.contains(&prefix.name);
                    chan.nicklist.set_away(&prefix.name, away);
                    if present && announce {
                        let line = if away {
                            format!("{} is now away: {reason}", prefix.name)
                        } else {
                            format!("{} is no longer away", prefix.name)
                        };
                        chan.buffer.push_system(line);
                    }
                }
            }
        }
        "TOPIC" => {
            if let Some(prefix) = &msg.prefix {
                let chan_name = msg.params.first().cloned().unwrap_or_default();
                if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                    let topic = msg.params.get(1).cloned().unwrap_or_default();
                    chan.buffer
                        .push_system_topic(irust_config::theme_formats::topic(&prefix.name, &chan_name, &topic));
                    chan.topic = if topic.is_empty() { None } else { Some(topic) };
                }
            }
        }
        "332" => {
            // RPL_TOPIC: params = [<our nick>, <channel>, <topic>]
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                let topic = msg.params.get(2).cloned().unwrap_or_default();
                chan.buffer.push_system_topic(format!("-- topic: {topic}"));
                chan.topic = if topic.is_empty() { None } else { Some(topic) };
            }
        }
        "331" => {
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                chan.topic = None;
                chan.buffer
                    .push_system_topic(format!("-- No topic set for {chan_name}"));
            }
        }
        "WALLOPS" => {
            let text = msg.params.first().cloned().unwrap_or_default();
            let source = msg.prefix.as_ref().map(|p| p.name.as_str()).unwrap_or("");
            highlighted = true;
            state.status.push_system_wallops(format!("!! {source} {text}"));
        }
        "NOTICE" => {
            if let Some(prefix) = &msg.prefix {
                let (status_prefix, target) = strip_statusmsg_prefix(msg.params.first().cloned().unwrap_or_default());
                let is_channel_target = target.starts_with(['#', '&']);
                let text = msg.params.get(1).cloned().unwrap_or_default();
                let text = match status_prefix {
                    Some(c) => format!("[{c}] {text}"),
                    None => text,
                };
                if is_channel_target {
                    let color_index = state.nick_colors.resolve(&prefix.name);
                    let matched_hilight = crate::highlight::find_matching_hilight(
                        &text,
                        &full_mask(prefix),
                        Some(&target),
                        hilight_entries,
                    );
                    let highlight = mentions_us(state, &text, &target) || matched_hilight.is_some();
                    if highlight {
                        highlighted = true;
                    }
                    if let Some(chan) = state.channels.get_mut(&target.to_lowercase()) {
                        let rank = chan.nicklist.prefix_for(&prefix.name);
                        if highlight {
                            chan.buffer.highlight_actcolor = matched_hilight.and_then(|e| e.actcolor.clone());
                        }
                        chan.buffer.push_chat_at_colored(
                            prefix.name.clone(),
                            color_index,
                            rank,
                            text,
                            highlight,
                            matched_hilight.and_then(|e| e.color.clone()),
                            server_time_hms(msg),
                            server_time_epoch(msg),
                        );
                    }
                } else if let Some((ctcp_cmd, ctcp_arg)) = parse_ctcp(&text) {
                    let ctcp_arg = ctcp_arg.trim();
                    let shown = if ctcp_arg.is_empty() {
                        format!("-- CTCP {ctcp_cmd} reply from {}", prefix.name)
                    } else {
                        format!("-- CTCP {ctcp_cmd} reply from {}: {ctcp_arg}", prefix.name)
                    };
                    state.status.push_system_notice(shown);
                } else if is_service_nick(&prefix.name) {
                    route_service_notice(state, &prefix.name, &text);
                    highlighted = true;
                } else if target == "*" {
                    highlighted = true;
                    state.status.push_system_wallops(format!("!! {} {text}", prefix.name));
                } else if prefix.name.contains('.') {
                    state.status.push_system_server(format!("!{} {text}", prefix.name));
                } else if let Some(chan_key) = irust_config::settings_net::bool_of("notice_channel_context")
                    .then(|| notice_channel_context_target(state, &text))
                    .flatten()
                {
                    let color_index = state.nick_colors.resolve(&prefix.name);
                    let matched_hilight = crate::highlight::find_matching_hilight(
                        &text,
                        &full_mask(prefix),
                        Some(&chan_key),
                        hilight_entries,
                    );
                    let highlight = mentions_us(state, &text, &chan_key) || matched_hilight.is_some();
                    if highlight {
                        highlighted = true;
                    }
                    if let Some(chan) = state.channels.get_mut(&chan_key) {
                        let rank = chan.nicklist.prefix_for(&prefix.name);
                        if highlight {
                            chan.buffer.highlight_actcolor = matched_hilight.and_then(|e| e.actcolor.clone());
                        }
                        chan.buffer.push_chat_at_colored(
                            prefix.name.clone(),
                            color_index,
                            rank,
                            text,
                            highlight,
                            matched_hilight.and_then(|e| e.color.clone()),
                            server_time_hms(msg),
                            server_time_epoch(msg),
                        );
                    }
                } else {
                    let line = format!("-- {}: {text}", prefix.name);
                    // irssi opens a query for it only when NOTICES is in
                    // `autocreate_query_level` (off by default here).
                    if irust_config::lookandfeel::get_level("autocreate_query_level") & irust_config::levels::NOTICES != 0 {
                        state.get_or_create_query_buffer(&prefix.name, &prefix.name).push_system_notice(line);
                    } else {
                        state.status.push_system_notice(line);
                    }
                }
            }
        }
        "PRIVMSG" => {
            if let Some(prefix) = &msg.prefix {
                let (status_prefix, target) = strip_statusmsg_prefix(msg.params.first().cloned().unwrap_or_default());
                let text = msg.params.get(1).cloned().unwrap_or_default();

                // A non-ACTION CTCP request (VERSION/PING/TIME/...) is a
                // protocol-level exchange, not chat -- auto-reply via
                // NOTICE and don't touch any buffer at all.
                if let Some((ctcp_cmd, ctcp_arg)) = parse_ctcp(&text) {
                    if !ctcp_cmd.eq_ignore_ascii_case("ACTION") && state.is_self(&prefix.name) {
                        return (out, highlighted);
                    }
                    // A `DCC CHAT` offer -- not answered automatically
                    // (unlike VERSION/PING/TIME above): recorded as
                    // pending so `/dcc chat <nick>` accepts it, and
                    // surfaced so the user actually knows to run that.
                    if ctcp_cmd.eq_ignore_ascii_case("DCC") {
                        if let Some((ip, port)) = crate::dcc::parse_offer(ctcp_arg) {
                            state.pending_dcc.insert(prefix.name.to_lowercase(), (ip, port));
                            state.status.push_system(format!(
                                "-- {} wants to start a DCC CHAT -- /dcc chat {} to accept",
                                prefix.name, prefix.name
                            ));
                        } else if let Some(offer) = crate::dcc_transfer::parse_send_offer(ctcp_arg) {
                            let reverse_offer_match = offer.token.is_some()
                                && state.pending_dcc_send.values().any(|p| {
                                    !p.incoming
                                        && p.peer_nick.eq_ignore_ascii_case(&prefix.name)
                                        && p.filename == offer.filename
                                        && p.token == offer.token
                                });
                            if reverse_offer_match {
                                if let Some(pending) = state.pending_dcc_send.values_mut().find(|p| {
                                    !p.incoming
                                        && p.peer_nick.eq_ignore_ascii_case(&prefix.name)
                                        && p.filename == offer.filename
                                        && p.token == offer.token
                                }) {
                                    pending.ip = offer.ip;
                                    pending.port = offer.port;
                                }
                                state.status.push_system(format!(
                                    "-- {} is ready to receive {} -- connecting...",
                                    prefix.name, offer.filename
                                ));
                            } else {
                                let id = state.next_dcc_id();
                                let size = offer.size;
                                let port = offer.port;
                                let filename = offer.filename.clone();
                                state.pending_dcc_send.insert(
                                    id,
                                    crate::dcc_transfer::PendingSendOffer {
                                        id,
                                        peer_nick: prefix.name.clone(),
                                        filename: offer.filename,
                                        ip: offer.ip,
                                        port: offer.port,
                                        size: offer.size,
                                        token: offer.token,
                                        incoming: true,
                                    },
                                );
                                state.status.push_system(format!(
                                    "-- {} wants to send you {filename} ({size} bytes) -- /dcc get {id} to accept",
                                    prefix.name
                                ));
                                if dcc_autoget_eligible(prefix, &target, &filename, size, port) {
                                    state.pending_auto_dcc_accept.push(id);
                                }
                            }
                        } else if let Some(resume) = crate::dcc_transfer::parse_resume(ctcp_arg) {
                            state.pending_dcc_resume_requests.push((prefix.name.clone(), resume));
                        } else if let Some(accept) = crate::dcc_transfer::parse_accept(ctcp_arg) {
                            state.pending_dcc_accept_replies.push((prefix.name.clone(), accept));
                        }
                        return (out, highlighted);
                    }
                    if !ctcp_cmd.eq_ignore_ascii_case("ACTION") {
                        if let Some(reply) = ctcp_reply(ctcp_cmd, ctcp_arg) {
                            out.push(format!("NOTICE {} :{reply}", prefix.name));
                        }
                        return (out, highlighted);
                    }
                }

                if state.is_self(&prefix.name) {
                    if !state.echo_message_active {
                        return (out, highlighted);
                    }
                    let color_index = state.nick_colors.resolve(&prefix.name);
                    let action_text = strip_ctcp_action(&text);
                    if let Some(chan) = state.channels.get_mut(&target.to_lowercase()) {
                        let rank = chan.nicklist.prefix_for(&prefix.name);
                        match action_text {
                            Some(a) => chan.buffer.push_action_at(
                                prefix.name.clone(),
                                color_index,
                                rank,
                                emphasize(mark_statusmsg(a, status_prefix), Some(&chan.nicklist)),
                                false,
                                server_time_hms(msg),
                                server_time_epoch(msg),
                            ),
                            None => chan.buffer.push_chat_at(
                                prefix.name.clone(),
                                color_index,
                                rank,
                                emphasize(mark_statusmsg(text, status_prefix), Some(&chan.nicklist)),
                                false,
                                server_time_hms(msg),
                                server_time_epoch(msg),
                            ),
                        }
                    } else {
                        let is_svc = is_service_nick(&target);
                        let buffer = if is_svc {
                            crate::service_notices::route_service_buffer(state, &target)
                        } else {
                            state.get_or_create_query_buffer(&target.to_lowercase(), &target)
                        };
                        match action_text {
                            Some(a) => buffer.push_action_at(
                                prefix.name.clone(),
                                color_index,
                                None,
                                a,
                                false,
                                server_time_hms(msg),
                                server_time_epoch(msg),
                            ),
                            None => buffer.push_chat_at(
                                prefix.name.clone(),
                                color_index,
                                None,
                                text,
                                false,
                                server_time_hms(msg),
                                server_time_epoch(msg),
                            ),
                        }
                    }
                    return (out, highlighted);
                }

                let color_index = state.nick_colors.resolve(&prefix.name);
                let action_text = strip_ctcp_action(&text);
                // A direct message always counts as a mention regardless
                // of whether our nick literally appears in the text.
                let is_direct = target.eq_ignore_ascii_case(&state.nick);
                let msg_text = action_text.as_deref().unwrap_or(&text);
                let channel_ctx = if is_direct { None } else { Some(target.as_str()) };
                if is_direct {
                    state.last_privmsg_from = Some(prefix.name.clone());
                } else {
                    state.last_public_from = Some(prefix.name.clone());
                }
                // `/set hilight_level`: which kinds of message a /hilight rule
                // may match at all (a DM is MSGS, anything in a channel PUBLIC).
                let msg_level = if is_direct { irust_config::levels::MSGS } else { irust_config::levels::PUBLIC };
                let rules_apply = irust_config::lookandfeel::get_level("hilight_level") & msg_level != 0;
                let matched_hilight = crate::highlight::find_matching_hilight(
                    msg_text,
                    &full_mask(prefix),
                    channel_ctx,
                    if rules_apply { hilight_entries } else { &[] },
                );
                let highlight =
                    is_direct || mentions_us(state, msg_text, &target) || matched_hilight.is_some();
                if highlight {
                    highlighted = true;
                }
                let highlight_color = matched_hilight.and_then(|e| e.color.clone());
                // Computed before the mutable `state.channels` borrow
                // just below starts -- `state.is_self` needs `&state`,
                // which can't coexist with `chan`'s `&mut
                // state.channels` borrow.
                let autoban_eligible_sender = irust_config::settings::AUTOBAN_ENABLED
                    .load(std::sync::atomic::Ordering::Relaxed)
                    && !state.is_self(&prefix.name)
                    && !is_service_nick(&prefix.name);
                if let Some(chan) = state.channels.get_mut(&target.to_lowercase()) {
                    // Backfills user/host for anyone who was already in
                    // the channel before we joined (JOIN alone only
                    // covers arrivals after that point).
                    chan.nicklist
                        .set_userhost(&prefix.name, prefix.user.as_deref(), prefix.host.as_deref());
                    // Never bans ourselves or a recognized service.
                    if autoban_eligible_sender {
                        if let Some(reason) = autoban_reason_for(msg_text, autoban_entries) {
                            state.pending_autoban.push(PendingAutoban {
                                channel: target.clone(),
                                nick: prefix.name.clone(),
                                mask: chan.nicklist.ban_mask_for(&prefix.name),
                                reason,
                            });
                        }
                    }
                    let rank = chan.nicklist.prefix_for(&prefix.name);
                    if action_text.is_none() && !prefix.name.eq_ignore_ascii_case(&state.nick) {
                        // irssi's `sig_message_public`: remember who spoke, and whether
                        // they addressed us, for tab-completion ordering.
                        let addressed_us =
                            crate::nick_completion::msg_addresses_nick(msg_text, &state.nick, |w| chan.nicklist.contains(w));
                        crate::nick_completion::last_msg_add(
                            &mut chan.last_msgs,
                            &prefix.name,
                            addressed_us,
                            irust_config::settings_history_log::get_int("completion_keep_publics"),
                        );
                    }
                    if highlight {
                        chan.buffer.highlight_actcolor = matched_hilight.and_then(|e| e.actcolor.clone());
                    }
                    match action_text {
                        Some(a) => chan.buffer.push_action_at_colored(
                            prefix.name.clone(),
                            color_index,
                            rank,
                            emphasize(mark_statusmsg(a, status_prefix), Some(&chan.nicklist)),
                            highlight,
                            highlight_color,
                            server_time_hms(msg),
                            server_time_epoch(msg),
                        ),
                        None => chan.buffer.push_chat_at_colored(
                            prefix.name.clone(),
                            color_index,
                            rank,
                            emphasize(mark_statusmsg(text, status_prefix), Some(&chan.nicklist)),
                            highlight,
                            highlight_color,
                            server_time_hms(msg),
                            server_time_epoch(msg),
                        ),
                    }
                } else if is_direct {
                    crate::nick_completion::last_msg_add(
                        &mut state.private_last_msgs,
                        &prefix.name,
                        true,
                        irust_config::settings_history_log::get_int("completion_keep_privates"),
                    );
                    let is_svc = is_service_nick(&prefix.name);
                    let already_had_query = state.query_buffers.contains_key(&prefix.name.to_lowercase());
                    let may_open_query = irust_config::lookandfeel::get_level("autocreate_query_level")
                        & irust_config::levels::MSGS
                        != 0
                        || already_had_query;
                    if !is_svc && may_open_query && !already_had_query && irust_config::lookandfeel::get_bool("window_auto_change") {
                        state.active_buffer = BufferId::Query(prefix.name.to_lowercase());
                    }
                    let buffer = if is_svc {
                        crate::service_notices::route_service_buffer(state, &prefix.name)
                    } else if may_open_query {
                        state.get_or_create_query_buffer(&prefix.name, &prefix.name)
                    } else {
                        state.current_buffer_mut()
                    };
                    // A query has no channel roster to look a rank up
                    // in -- IRC prefix modes are per-channel, so there's
                    // nothing meaningful to show here anyway.
                    if highlight {
                        buffer.highlight_actcolor = matched_hilight.and_then(|e| e.actcolor.clone());
                    }
                    match action_text {
                        Some(a) => buffer.push_action_at_colored(
                            prefix.name.clone(),
                            color_index,
                            None,
                            if is_svc { a } else { emphasize(a, None) },
                            highlight,
                            highlight_color,
                            server_time_hms(msg),
                            server_time_epoch(msg),
                        ),
                        None => buffer.push_chat_at_colored(
                            prefix.name.clone(),
                            color_index,
                            None,
                            if is_svc { text } else { emphasize(text, None) },
                            highlight,
                            highlight_color,
                            server_time_hms(msg),
                            server_time_epoch(msg),
                        ),
                    }
                }
            }
        }
        "PONG" => {}
        "CAP" => {}
        "315" => {
            let mask = msg.params.get(1).cloned().unwrap_or_default().to_lowercase();
            if let Some(i) = state.manual_who_pending.iter().position(|t| *t == mask) {
                state.manual_who_pending.remove(i);
                state.current_buffer_mut().push_system("End of /WHO list");
            }
        }
        "333" => {
            let chan_name = msg.params.get(1).cloned().unwrap_or_default();
            let setter = msg.params.get(2).cloned().unwrap_or_default();
            let raw_time = msg.params.get(3).cloned().unwrap_or_default();
            if let Some(chan) = state.channels.get_mut(&chan_name.to_lowercase()) {
                if let Some(date) = format_epoch_secs(&raw_time) {
                    let (nick, hostmask) = setter.split_once('!').unwrap_or((&setter, ""));
                    chan.buffer
                        .push_system_topic(format!("-- Topic set by {nick} [{hostmask}] [{date}]"));
                }
            }
        }
        "305" => {
            state.away = false;
            state.away_reason = None;
            state
                .status
                .push_system("-- You are no longer marked as being away".to_string());
            // irssi's `awaylog show`: what arrived while away, in the window
            // you are looking at.
            if let Some((count, lines)) = crate::log_format::awaylog_close() {
                let buf = state.current_buffer_mut();
                if count == 0 {
                    buf.push_system("-- No new messages in awaylog".to_string());
                } else {
                    buf.push_system(format!("-- {count} new messages in awaylog:"));
                    for line in lines {
                        buf.push_system(line);
                    }
                }
            }
        }
        "306" => {
            state.away = true;
            crate::log_format::awaylog_open();
            state
                .status
                .push_system("-- You have been marked as being away".to_string());
        }
        "005" => {
            if let Some(table) = msg
                .params
                .iter()
                .find_map(|p| p.strip_prefix("PREFIX=").and_then(PrefixTable::parse))
            {
                state.prefix_table = table.clone();
                for chan in state.channels.values_mut() {
                    chan.nicklist.set_prefix_table(table.clone());
                }
            }
            if let Some(table) = msg
                .params
                .iter()
                .find_map(|p| p.strip_prefix("CHANMODES=").and_then(ChanModesTable::parse))
            {
                state.chanmodes_table = table.clone();
                for chan in state.channels.values_mut() {
                    chan.nicklist.set_chanmodes_table(table.clone());
                }
            }
            if let Some(token) = msg.params.iter().find_map(|p| p.strip_prefix("MONITOR=")) {
                state.monitor_limit = Some(token.parse().unwrap_or(usize::MAX));
            } else if msg.params.iter().any(|p| p == "MONITOR") {
                state.monitor_limit = Some(usize::MAX);
            }
            if let Some(token) = msg.params.iter().find_map(|p| p.strip_prefix("IDCHAN=")) {
                state.idchan = parse_idchan(token);
            }
        }
        // irssi `event_target_unavailable` (channel-rejoin.c): a channel we are not
        // in that answers 437 goes on the rejoin list instead of printing an error.
        "437" if irust_config::settings_net::bool_of("channels_rejoin_unavailable")
            && msg.params.get(1).is_some_and(|c| c.starts_with(['#', '&', '!', '+']))
            && !state.channels.get(&msg.params[1].to_lowercase()).is_some_and(|c| c.joined) =>
        {
            let channel = msg.params[1].clone();
            let key = state.channels.get(&channel.to_lowercase()).and_then(|c| c.key.clone());
            match state.rejoin_channels.iter_mut().find(|r| r.channel.eq_ignore_ascii_case(&channel)) {
                Some(r) => {
                    r.joining = false;
                    r.key = key;
                }
                None => state.rejoin_channels.push(crate::rejoin::Rejoin { channel, key, joining: false }),
            }
        }
        "433" | "436" if !state.connected => {
            // irssi's `event_nick_in_use`: the first collision, while we are
            // still on the nick we asked for, tries `alternate_nick`; only
            // after that does it fall back to appending '_'.
            let alternate = irust_config::settings_net::str_of("alternate_nick");
            if state.nick_registration_retries == 0
                && !alternate.is_empty()
                && !alternate.eq_ignore_ascii_case(&state.nick)
            {
                state.nick = alternate;
                out.push(format!("NICK {}", state.nick));
            } else if state.nick_registration_retries < MAX_NICK_REGISTRATION_RETRIES {
                state.nick_registration_retries += 1;
                state.nick.push('_');
                out.push(format!("NICK {}", state.nick));
            } else {
                state.status.push_system(format!(
                    "-- Nickname unavailable, gave up after {} variants -- use /nick <name> to pick one manually",
                    state.nick_registration_retries + 1
                ));
            }
        }
        "TAGMSG" => {}
        "BATCH" => {}
        _ => {
            let is_numeric = !msg.command.is_empty() && msg.command.bytes().all(|b| b.is_ascii_digit());
            let mut params = msg.params.clone();
            if is_numeric && params.first().is_some_and(|p| p.eq_ignore_ascii_case(&state.nick)) {
                params.remove(0);
            }

            if is_numeric && state.self_whois_pending {
                if params.first().is_some_and(|p| p.eq_ignore_ascii_case(&state.nick)) {
                    if msg.command == "301" {
                        state.away = true;
                        state.away_reason = params.get(1).cloned();
                    }
                    if matches!(msg.command.as_str(), "318" | "401") {
                        state.self_whois_pending = false;
                    }
                    return (out, highlighted);
                }
            }

            if is_numeric {
                if let Some(subject) = params.first().cloned() {
                    if matches!(msg.command.as_str(), "378" | "338") {
                        if let Some(pending) = state.pending_ip.as_mut() {
                            if pending.host.is_none() && pending.nick.eq_ignore_ascii_case(&subject) {
                                let idx = if msg.command == "378" {
                                    1
                                } else {
                                    params.len().saturating_sub(2)
                                };
                                if let Some(host) = params.get(idx).and_then(|text| text.split_whitespace().last()) {
                                    pending.host = Some(host.to_string());
                                }
                            }
                        }
                    }
                    let is_real_terminator = match msg.command.as_str() {
                        "369" | "406" => true,
                        "318" => !state.auto_whowas_pending,
                        _ => false,
                    };
                    if is_real_terminator
                        && state
                            .pending_ip
                            .as_ref()
                            .is_some_and(|p| p.nick.eq_ignore_ascii_case(&subject))
                    {
                        state.pending_ip = None;
                    }
                }
            }

            let whois_target = is_numeric
                .then(|| state.pending_whois.clone())
                .flatten()
                .filter(|(nick, _)| params.first().is_some_and(|p| p.eq_ignore_ascii_case(nick)));

            match whois_target {
                Some((_, buffer_id)) => {
                    let display_nick = params.first().cloned().unwrap_or_default();

                    if msg.command == "401" && irust_config::settings_net::bool_of("auto_whowas") {
                        out.push(format!("WHOWAS {display_nick} 1"));
                        state.auto_whowas_pending = true;
                        return (out, highlighted);
                    }
                    if msg.command == "318" && state.auto_whowas_pending {
                        return (out, highlighted);
                    }
                    if matches!(msg.command.as_str(), "318" | "369" | "406") {
                        state.pending_whois = None;
                        state.auto_whowas_pending = false;
                        // `pending_ip`'s own clearing on this same
                        // terminator already happened above, decoupled
                        // from `pending_whois` on purpose -- see that
                        // block's own doc comment.
                    }
                    let suppress_for_ip = state
                        .pending_ip
                        .as_ref()
                        .is_some_and(|p| p.nick.eq_ignore_ascii_case(&display_nick))
                        && IP_SUPPRESSED_NUMERICS.contains(&msg.command.as_str());
                    if !suppress_for_ip {
                        // Same field layout irssi's own WHOIS formatting
                        // works from: everything after our nick *and*
                        // the queried nick.
                        let fields = &params[1.min(params.len())..];
                        // RPL_WHOISCHANNELS: strip a safe channel's
                        // random ID before display, matching real
                        // irssi's own `whois_hide_safe_channel_id`.
                        let stripped_channels;
                        let fields = if msg.command == "319"
                            && !state.idchan.is_empty()
                            && irust_config::lookandfeel::get_bool("whois_hide_safe_channel_id")
                        {
                            stripped_channels =
                                [hide_safe_channel_ids(&fields.join(" "), &state.idchan, &state.prefix_table)];
                            &stripped_channels[..]
                        } else {
                            fields
                        };
                        let lines = format_whois_lines(&msg.command, &display_nick, fields);
                        match state.buffer_mut(&buffer_id) {
                            Some(buffer) => {
                                for line in lines {
                                    buffer.push_system_whois(line);
                                }
                            }
                            None => {
                                for line in lines {
                                    state.status.push_system_whois(line);
                                }
                            }
                        }
                    }
                }
                None if msg.command == "301" => {
                    if let (Some(nick), Some(awaymsg)) = (params.first(), params.get(1)) {
                        // `/set show_away_once` (default on, irssi's own
                        // real default): skip re-showing the exact same
                        // away message for the same nick.
                        if state.should_show_away(nick, awaymsg) {
                            let line = format!("-- {nick} is away: {awaymsg}");
                            match state.query_buffers.get_mut(&nick.to_lowercase()) {
                                Some(buf) => buf.push_system(line),
                                None => state.status.push_system(line),
                            }
                        }
                    }
                }
                None => {
                    let text = params.join(" ");
                    let line = match &msg.prefix {
                        Some(prefix) => format!("-- {} {text}", prefix.name),
                        None => format!("-- {text}"),
                    };
                    state.status.push_system(line);
                }
            }
        }
    }

    (out, highlighted)
}

fn format_whois_field(label: &str, value: &str) -> String {
    irust_config::theme_formats::whois_line(&format!("{label:<8}"), value)
}

/// Parses `IDCHAN`'s value (after the `IDCHAN=` prefix is already
/// stripped), e.g. `!:5` or `!:5,&:4` -- comma-separated
/// `<prefix-char>:<id-length>` pairs. Unparsable entries are skipped
/// rather than aborting the whole token, same spirit as `PrefixTable`'s
/// own tolerant parsing.
fn parse_idchan(value: &str) -> Vec<(char, usize)> {
    value
        .split(',')
        .filter_map(|entry| {
            let (prefix, len) = entry.split_once(':')?;
            let prefix = prefix.chars().next()?;
            let len: usize = len.parse().ok()?;
            Some((prefix, len))
        })
        .collect()
}

fn hide_safe_channel_ids(chans: &str, idchan: &[(char, usize)], prefix_table: &PrefixTable) -> String {
    if idchan.is_empty() {
        return chans.to_string();
    }
    let mut out = String::with_capacity(chans.len());
    let mut skip = 0usize;
    let mut at_chan_start = true;
    for c in chans.chars() {
        if skip > 0 {
            skip -= 1;
            continue;
        }
        if c == ' ' {
            at_chan_start = true;
        } else {
            if at_chan_start {
                if let Some(&(_, length)) = idchan.iter().find(|&&(prefix, _)| prefix == c) {
                    skip = length;
                }
            }
            at_chan_start = at_chan_start && prefix_table.is_symbol(c);
        }
        out.push(c);
    }
    out
}

/// Wraps `text` in mIRC bold control codes (`\x02`) -- irssi's plain
/// `nick` abstract, used for the whois nick.
fn mirc_bold(text: &str) -> String {
    format!("\u{2}{text}\u{2}")
}

fn mirc_hilight(text: &str) -> String {
    format!("\u{2}\u{3}8{text}\u{3}\u{2}")
}

fn mirc_gray_brackets(inner: &str) -> String {
    format!("\u{3}14[\u{3}{inner}\u{3}14]\u{3}")
}

const IP_SUPPRESSED_NUMERICS: &[&str] = &["312", "319", "671"];

fn format_whois_lines(command: &str, nick: &str, fields: &[String]) -> Vec<String> {
    let joined = fields.join(" ");
    match command {
        // RPL_WHOISUSER: <user> <host> * :<realname>
        "311" => {
            let user = fields.first().map(String::as_str).unwrap_or("");
            let host = fields.get(1).map(String::as_str).unwrap_or("");
            let realname = fields.get(3).map(String::as_str).unwrap_or("");
            vec![
                irust_config::theme_formats::whois_header(
                    &mirc_bold(nick),
                    &mirc_gray_brackets(&format!("{user}@{host}")),
                ),
                format_whois_field("ircname", realname),
            ]
        }
        // RPL_WHOISSERVER: <server> :<description>
        "312" => {
            let server = fields.first().map(String::as_str).unwrap_or("");
            let desc = fields.get(1..).map(|f| f.join(" ")).unwrap_or_default();
            vec![format_whois_field(
                "server",
                &format!("{server} {}", mirc_gray_brackets(&desc)),
            )]
        }
        // RPL_WHOISOPERATOR: :is an IRC operator (or ircd-specific text)
        // -- irssi strips a leading "is an "/"is a " (bug #99), and
        // falls back to a generic "IRC Operator" if that empties it out.
        "313" => {
            let mut text = joined.as_str();
            for prefix in ["is an ", "is a "] {
                if let Some(rest) = text.strip_prefix(prefix) {
                    text = rest;
                    break;
                }
            }
            let text = if text.is_empty() { "IRC Operator" } else { text };
            vec![format_whois_field("", &mirc_hilight(text))]
        }
        // RPL_WHOISIDLE: <idle-secs> [<signon-epoch>] :seconds idle...
        "317" => {
            let secs: i64 = fields.first().and_then(|s| s.parse().ok()).unwrap_or(0);
            let (days, hours, mins, secs) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60, secs % 60);
            vec![format_whois_field(
                "idle",
                &format!("{days} days {hours} hours {mins} mins {secs} secs"),
            )]
        }
        // RPL_WHOISCHANNELS: :#chan1 #chan2 ...
        "319" => vec![format_whois_field("channels", &joined)],
        // RPL_AWAY (only reached here while a whois is pending -- see
        // the caller): :<away message>
        "301" => vec![format_whois_field("away", &joined)],
        // RPL_WHOISACCOUNT / "whois account": <authname> :is logged in as
        // -- irssi's `event_whois_auth` prints only the account name
        // (`whois_extra`), dropping the trailing "is logged in as".
        "330" => vec![format_whois_field("account", fields.first().map(String::as_str).unwrap_or(""))],
        "378" => {
            let host = if fields.len() >= 3 && fields[0] == "real" && fields[1] == "hostname" {
                Some(fields[2].clone())
            } else {
                joined.split_once("from ").map(|(_, h)| h.to_string())
            };
            match host {
                Some(h) => vec![format_whois_field("hostname", h.strip_prefix("*@").unwrap_or(&h))],
                None => vec![format_whois_field("", &joined)],
            }
        }
        // RPL_WHOISACTUALLY: `<user@host> <ip> :Actual user@host, actual IP`
        // (ircu) or `<ip> :actually using host` (ratbox/solanum) -- irssi's
        // `event_whois_realhost338`: "hostname" label with the address
        // field(s), the trailing text dropped.
        "338" => match fields.len() {
            n if n >= 3 => vec![format_whois_field("hostname", &format!("{} {}", fields[0], fields[1]))],
            2 => vec![format_whois_field("hostname", &fields[0])],
            _ => vec![format_whois_field("", &joined)],
        },
        // RPL_ENDOFWHOIS: :End of /WHOIS list.
        "318" => vec![irust_config::theme_formats::whois_end()],
        // ERR_NOSUCHNICK: :No such nick/channel -- the caller intercepts
        // this before it ever reaches here (auto_whowas retries as a
        // WHOWAS instead), so this arm is effectively dead in practice;
        // kept for direct unit tests of this function and as a sane
        // fallback if that interception is ever bypassed.
        "401" => vec![irust_config::theme_formats::whois_no_such_nick(nick)],
        // RPL_WHOWAS -- the auto_whowas fallback's own version of 311,
        // same field layout, "was" label instead of "ircname" (irssi's
        // own `whowas` format).
        "314" => {
            let user = fields.first().map(String::as_str).unwrap_or("");
            let host = fields.get(1).map(String::as_str).unwrap_or("");
            let realname = fields.get(3).map(String::as_str).unwrap_or("");
            vec![
                irust_config::theme_formats::whois_header(
                    &mirc_bold(nick),
                    &mirc_gray_brackets(&format!("{user}@{host}")),
                ),
                format_whois_field("was", realname),
            ]
        }
        // RPL_ENDOFWHOWAS: :End of WHOWAS
        "369" => vec![irust_config::theme_formats::whowas_end()],
        // ERR_WASNOSUCHNICK: the nick never existed either.
        "406" => vec![irust_config::theme_formats::whowas_no_such_nick(nick)],
        "671" => vec![format_whois_field("", joined.split(" [").next().unwrap_or(&joined))],
        // Everything irssi itself doesn't recognize either (a TLS/cert
        // line, an ircd-specific extension, ...) -- irssi's own
        // `whois_special` fallback: blank label, raw text.
        _ => vec![format_whois_field("", &joined)],
    }
}

/// Applies a `+iw-x`-style user-mode delta to `current`'s letter set,
/// returning the new set (order not meaningful, just membership).
fn apply_usermode_line(current: &str, modestring: &str) -> String {
    let mut modes: Vec<char> = current.chars().collect();
    let mut adding = true;
    for ch in modestring.chars() {
        match ch {
            '+' => adding = true,
            '-' => adding = false,
            letter if adding => {
                if !modes.contains(&letter) {
                    modes.push(letter);
                }
            }
            letter => modes.retain(|&c| c != letter),
        }
    }
    modes.into_iter().collect()
}

fn strip_statusmsg_prefix(target: String) -> (Option<char>, String) {
    if let Some(c) = target.chars().next() {
        if (c == '@' || c == '+') && target[1..].starts_with(['#', '&']) {
            return (Some(c), target[1..].to_string());
        }
    }
    (None, target)
}

/// Tags a PRIVMSG's displayed text with the STATUSMSG char it was sent
/// with (`strip_statusmsg_prefix`'s own `Some(c)`), so an ops-only/
/// voice-only message doesn't look identical to a normal one in the
/// channel buffer -- a no-op (`None`) for a plain PRIVMSG.
fn mark_statusmsg(text: String, status_prefix: Option<char>) -> String {
    match status_prefix {
        Some(c) => format!("[{c}] {text}"),
        None => text,
    }
}

/// `/set emphasis`: wraps `*bold*`/`_underlined_` words in mIRC codes for
/// display (after highlight matching, which sees the raw text).
fn emphasize(text: String, nicks: Option<&crate::nicklist::Nicklist>) -> String {
    crate::emphasis::expand_emphasis(text, &|word| nicks.is_some_and(|n| n.contains(word)))
}

/// Whether public `text` in `channel` counts as addressed to us, per
/// `/set hilight_nick_matches` and `hilight_nick_matches_everywhere`.
fn mentions_us(state: &ClientState, text: &str, channel: &str) -> bool {
    let chan = state.channels.get(&channel.to_lowercase());
    crate::nickmatch::for_me(text, &state.nick, &|word| chan.is_some_and(|c| c.nicklist.contains(word)))
}

fn server_time_hms(msg: &RawMessage) -> Option<String> {
    let raw = msg.tags.get("time")?.as_deref()?;
    let dt = chrono::DateTime::parse_from_rfc3339(raw).ok()?;
    Some(dt.with_timezone(&chrono::Local).format(&irust_config::lookandfeel::timestamp_format()).to_string())
}

pub fn is_replayed_message(msg: &RawMessage) -> bool {
    const REPLAY_AGE_SECS: i64 = 60;
    match server_time_epoch(msg) {
        Some(t) => chrono::Utc::now().timestamp() - t > REPLAY_AGE_SECS,
        None => false,
    }
}

fn server_time_epoch(msg: &RawMessage) -> Option<i64> {
    let raw = msg.tags.get("time")?.as_deref()?;
    let dt = chrono::DateTime::parse_from_rfc3339(raw).ok()?;
    Some(dt.timestamp())
}

fn format_epoch_secs(raw: &str) -> Option<String> {
    let secs: i64 = raw.parse().ok()?;
    let dt = chrono::DateTime::from_timestamp(secs, 0)?;
    Some(
        dt.with_timezone(&chrono::Local)
            .format("%a %b %e %H:%M:%S %Y")
            .to_string(),
    )
}

fn elapsed_secs_since(raw: &str) -> Option<i64> {
    let then: i64 = raw.parse().ok()?;
    Some((chrono::Utc::now().timestamp() - then).max(0))
}

/// `\x01ACTION <text>\x01` -- a `/me` sent as a CTCP-wrapped PRIVMSG.
fn strip_ctcp_action(text: &str) -> Option<String> {
    let inner = text.strip_prefix('\u{1}')?.strip_suffix('\u{1}')?;
    let action_text = inner.strip_prefix("ACTION").unwrap_or(inner);
    Some(action_text.strip_prefix(' ').unwrap_or(action_text).to_string())
}

/// Splits a CTCP-wrapped PRIVMSG (`\x01CMD arg\x01`) into its command
/// and argument. `None` if `text` isn't CTCP-wrapped at all.
/// The irssi level a `PRIVMSG`/`NOTICE` counts under for flood detection.
fn flood_level(msg: &RawMessage) -> &'static str {
    let target = msg.params.first().map(String::as_str).unwrap_or("");
    let text = msg.params.get(1).map(String::as_str).unwrap_or("");
    if msg.command == "NOTICE" {
        "notices"
    } else if parse_ctcp(text).is_some_and(|(cmd, _)| !cmd.eq_ignore_ascii_case("ACTION")) {
        "ctcps"
    } else if target.starts_with(['#', '&', '+', '!']) {
        "publics"
    } else {
        "msgs"
    }
}

/// irssi's `flood_privmsg` / `flood_notice` / `flood_ctcp`: count one
/// incoming message against its sender and, on a flood at an
/// `autoignore_level` level, ignore `nick!user@host` for `autoignore_time`.
fn note_flood(state: &mut ClientState, msg: &RawMessage, ignore_entries: &[irust_config::IgnoreEntry], now: i64) {
    use irust_config::settings_history_log as cfg;
    let (timecheck, max_msgs) = (cfg::get_int("flood_timecheck"), cfg::get_int("flood_max_msgs"));
    if timecheck <= 0 || max_msgs <= 0 {
        return;
    }
    let Some(prefix) = &msg.prefix else { return };
    let (Some(user), Some(host)) = (&prefix.user, &prefix.host) else { return };
    if state.is_self(&prefix.name) {
        return;
    }
    let target = msg.params.first().map(String::as_str).unwrap_or("");
    let level = flood_level(msg);
    if is_ignored(ignore_entries, prefix, level) {
        return;
    }
    if state.flood.note(&prefix.name, level, target, now, timecheck, max_msgs) {
        crate::flood::autoignore(&mut state.auto_ignores, &format!("{}!{user}@{host}", prefix.name), level, now);
    }
}

fn parse_ctcp(text: &str) -> Option<(&str, &str)> {
    let inner = text.strip_prefix('\u{1}')?.strip_suffix('\u{1}')?;
    Some(match inner.split_once(' ') {
        Some((cmd, arg)) => (cmd, arg),
        None => (inner, ""),
    })
}

fn notice_channel_context_target(state: &ClientState, text: &str) -> Option<String> {
    let rest = text.strip_prefix('[')?;
    let end = rest.find([' ', ',', ']'])?;
    if rest.as_bytes().get(end) != Some(&b']') {
        return None;
    }
    let name = &rest[..end];
    let key = name.to_lowercase();
    state.channels.contains_key(&key).then_some(key)
}

/// The auto-reply body (already CTCP-wrapped) for a recognized request,
/// or `None` for anything we don't answer automatically.
fn ctcp_reply(cmd: &str, arg: &str) -> Option<String> {
    match cmd.to_uppercase().as_str() {
        "VERSION" => crate::ctcp_reply::reply_for("VERSION", &irust_config::settings_net::str_of("ctcp_version_reply")),
        // Answered only if `ctcp_userinfo_reply` is set: irssi's default ("$Y", the
        // real name) is deliberately empty here, see the note further down.
        "USERINFO" => crate::ctcp_reply::reply_for("USERINFO", &irust_config::settings_net::str_of("ctcp_userinfo_reply")),
        // Echo the token back verbatim -- that round-trip is the whole
        // point of a CTCP PING (the sender measures the delay itself).
        "PING" => Some(format!("\u{1}PING {arg}\u{1}")),
        "TIME" => Some(format!("\u{1}TIME {}\u{1}", chrono::Local::now().to_rfc2822())),
        "CLIENTINFO" => Some("\u{1}CLIENTINFO VERSION PING TIME\u{1}".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{BufferLine, SystemKind};
    use irust_proto::parse;

    fn state() -> ClientState {
        ClientState::new("abcX", "#irust-test", 11)
    }

    use crate::test_support::{net_guard, net_set};

    #[test]
    fn opermode_is_not_queued_when_the_setting_is_empty() {
        let _guard = net_guard();
        let mut st = state();
        handle_message(&mut st, &parse(":irc.server 381 abcX :You are now an IRC operator").unwrap());
        assert_eq!(st.pending_opermode, None);
    }

    #[test]
    fn opermode_is_queued_on_381_when_the_setting_is_set() {
        let _guard = net_guard();
        net_set("opermode", "+p");
        let mut st = state();
        handle_message(&mut st, &parse(":irc.server 381 abcX :You are now an IRC operator").unwrap());
        assert_eq!(st.pending_opermode, Some("+p".to_string()));
    }

    #[test]
    fn hide_safe_channel_ids_strips_the_random_id_after_a_matching_prefix() {
        let idchan = vec![('!', 5)];
        let prefixes = PrefixTable::standard();
        assert_eq!(
            hide_safe_channel_ids("!12345safe #normal", &idchan, &prefixes),
            "!safe #normal"
        );
        // A status prefix in front of the safe channel is kept, and the
        // ID right after the `!` is still the part that's stripped.
        assert_eq!(hide_safe_channel_ids("@!12345safe", &idchan, &prefixes), "@!safe");
        // No IDCHAN learned yet (empty) -- passed through unchanged.
        assert_eq!(hide_safe_channel_ids("!12345safe", &[], &prefixes), "!12345safe");
        // `!` only counts right at a channel boundary, not mid-name.
        assert_eq!(hide_safe_channel_ids("#not!anid", &idchan, &prefixes), "#not!anid");
    }

    #[test]
    fn autoban_reason_for_matches_a_plain_substring_case_insensitively() {
        let entries = vec![irust_config::AutobanEntry {
            pattern: "call me on my number".to_string(),
            ..Default::default()
        }];
        assert_eq!(
            autoban_reason_for("Hi! CALL ME ON MY NUMBER here...", &entries),
            Some("spam".to_string())
        );
    }

    #[test]
    fn autoban_reason_for_uses_the_entrys_own_reason_when_set() {
        let entries = vec![irust_config::AutobanEntry {
            pattern: "webcam".to_string(),
            reason: Some("webcam spam".to_string()),
            ..Default::default()
        }];
        assert_eq!(
            autoban_reason_for("come watch me on my webcam", &entries),
            Some("webcam spam".to_string())
        );
    }

    #[test]
    fn autoban_reason_for_matches_a_regex_entry() {
        let entries = vec![irust_config::AutobanEntry {
            pattern: r"chat /w me".to_string(),
            is_regex: true,
            ..Default::default()
        }];
        assert_eq!(
            autoban_reason_for("come chat /w me :-)", &entries),
            Some("spam".to_string())
        );
    }

    #[test]
    fn autoban_reason_for_is_none_when_nothing_matches() {
        let entries = vec![irust_config::AutobanEntry {
            pattern: "buy crypto now".to_string(),
            ..Default::default()
        }];
        assert_eq!(autoban_reason_for("just chatting about the weather", &entries), None);
    }

    #[test]
    fn autoban_reason_for_with_no_entries_never_matches() {
        assert_eq!(autoban_reason_for("anything at all", &[]), None);
    }

    #[test]
    fn autoban_never_fires_while_the_master_setting_is_off_by_default() {
        assert!(!irust_config::settings::AUTOBAN_ENABLED.load(std::sync::atomic::Ordering::Relaxed));
        let mut st = state();
        st.ensure_channel("#irust-test");
        let entries = vec![irust_config::AutobanEntry {
            pattern: "call me on my number".to_string(),
            ..Default::default()
        }];
        let msg = parse(":derp!x@example.com PRIVMSG #irust-test :Hi! call me on my number here").unwrap();
        handle_message_with_highlight(&mut st, &msg, &[], &[], &entries);
        assert!(st.pending_autoban.is_empty());
    }

    #[test]
    fn detects_the_exact_flag_and_vs16_message_that_broke_abcxs_border() {
        assert!(contains_risky_emoji_sequence("Test 😂⚠️🇮🇹 😁🇬🇧t/👍😘😜"));
    }

    #[test]
    fn detects_a_bare_variation_selector_16_sequence() {
        assert!(contains_risky_emoji_sequence("⚠️"));
    }

    #[test]
    fn detects_a_zero_width_joiner_sequence() {
        // family: man + ZWJ + woman + ZWJ + girl
        assert!(contains_risky_emoji_sequence(
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"
        ));
    }

    #[test]
    fn detects_an_adjacent_regional_indicator_flag_pair() {
        assert!(contains_risky_emoji_sequence("🇮🇹"));
    }

    #[test]
    fn non_adjacent_regional_indicators_are_not_risky() {
        // two lone regional-indicator letters, never adjacent to each
        // other -- only an ADJACENT pair forms a flag glyph.
        assert!(!contains_risky_emoji_sequence("\u{1F1EE} text \u{1F1F9}"));
    }

    #[test]
    fn plain_text_and_ordinary_non_compound_emoji_are_not_risky() {
        assert!(!contains_risky_emoji_sequence("normal chat 😂 message"));
        assert!(!contains_risky_emoji_sequence(""));
    }

    #[test]
    fn incoming_invite_shows_a_clear_line_in_the_active_buffer() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":ChatWorld!cw@example.net INVITE abcX :#ops").unwrap();
        handle_message(&mut st, &msg);

        // `state()`'s active buffer is the seeded "#irust-test" channel,
        // not status -- confirms this doesn't hardcode status.
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- ChatWorld invites you to #ops");
    }

    #[test]
    fn an_invite_to_us_sets_last_invite_but_an_invite_notify_for_someone_else_does_not() {
        let _net = net_guard();
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":ChatWorld!cw@example.net INVITE abcX :#ops").unwrap(),
        );
        assert_eq!(st.last_invite.as_deref(), Some("#ops"));

        st.last_invite = None;
        handle_message(
            &mut st,
            &parse(":ChatWorld!cw@example.net INVITE Other :#irust-test").unwrap(),
        );
        assert_eq!(
            st.last_invite, None,
            "an invite-notify for someone else isn't an invite TO us"
        );
    }

    #[test]
    fn a_join_from_someone_else_sets_last_join_but_our_own_join_does_not() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        assert_eq!(st.last_join.as_deref(), Some("Other"));

        st.last_join = None;
        handle_message(&mut st, &parse(":abcX!u@h JOIN #second").unwrap());
        assert_eq!(st.last_join, None, "our own join must not count");
    }

    #[test]
    fn direct_and_channel_privmsgs_set_the_respective_last_sender_field() {
        let mut st = state();
        handle_message(&mut st, &parse(":Friend!u@h PRIVMSG abcX :hey there").unwrap());
        assert_eq!(st.last_privmsg_from.as_deref(), Some("Friend"));
        assert_eq!(
            st.last_public_from, None,
            "a direct message must not count as a channel sender"
        );

        handle_message(&mut st, &parse(":Other!u@h PRIVMSG #irust-test :hi all").unwrap());
        assert_eq!(st.last_public_from.as_deref(), Some("Other"));
        assert_eq!(
            st.last_privmsg_from.as_deref(),
            Some("Friend"),
            "unrelated to the channel message just seen"
        );
    }

    /// `$A`'s own source -- the authoritative "305" (`RPL_UNAWAY`) clears
    /// it even if it was never set through this client's own `/away`
    /// (an externally-triggered away going away again).
    #[test]
    fn numeric_305_clears_the_away_reason() {
        let mut st = state();
        st.away_reason = Some("brb".to_string());
        handle_message(
            &mut st,
            &parse(":irc.example.com 305 abcX :You are no longer marked as being away").unwrap(),
        );
        assert_eq!(st.away_reason, None);
    }

    /// The silent self-`/whois` probe's own `"301"` reply carries the
    /// real reason text -- covers a ZNC-persisted away this client never
    /// issued itself (same motivating case as `away` above it).
    #[test]
    fn silent_self_whois_301_reply_captures_the_away_reason() {
        let _net = net_guard();
        let mut st = state();
        handle_message(&mut st, &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        handle_message(&mut st, &parse(":irc.example.com 301 abcX abcX :gone fishing").unwrap());
        assert_eq!(st.away_reason.as_deref(), Some("gone fishing"));
    }

    #[test]
    fn self_part_after_the_buffer_is_already_closed_falls_back_to_status() {
        let mut st = state();
        // Simulates what `/wc` already did by the time this echo
        // arrives: the channel is gone from `channels` entirely.
        st.channels.remove("#irust-test");
        let msg = parse(":abcX PART #irust-test").unwrap();
        handle_message(&mut st, &msg);

        let BufferLine::System { text, .. } = &st.status.lines[st.status.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- abcX has left #irust-test []");
    }

    #[test]
    fn nick_in_use_before_registration_retries_with_a_trailing_underscore() {
        let _net = net_guard();
        let mut st = state();
        let out = handle_message(
            &mut st,
            &parse(":irc.example.com 433 * abcX :Nickname is already in use").unwrap(),
        );
        assert_eq!(out, vec!["NICK abcX_".to_string()]);
        assert_eq!(st.nick, "abcX_");
        assert_eq!(st.nick_registration_retries, 1);
    }

    #[test]
    fn nick_collision_retries_are_capped_and_then_give_up_visibly() {
        let _net = net_guard();
        let mut st = state();
        for _ in 0..MAX_NICK_REGISTRATION_RETRIES {
            handle_message(
                &mut st,
                &parse(":irc.example.com 433 * abcX :Nickname is already in use").unwrap(),
            );
        }
        let before = st.status.lines.len();
        let out = handle_message(
            &mut st,
            &parse(":irc.example.com 433 * abcX :Nickname is already in use").unwrap(),
        );
        assert!(out.is_empty(), "should stop sending NICK once the retry cap is hit");
        let BufferLine::System { text, .. } = &st.status.lines[before] else {
            unreachable!()
        };
        assert!(text.contains("use /nick"), "unexpected line: {text}");
    }

    #[test]
    fn nick_in_use_after_registration_is_not_auto_retried() {
        let _net = net_guard();
        let mut st = state();
        st.connected = true;
        let out = handle_message(
            &mut st,
            &parse(":irc.example.com 433 abcX newnick :Nickname is already in use").unwrap(),
        );
        assert!(out.is_empty());
        assert_eq!(st.nick, "abcX");
        assert_eq!(st.nick_registration_retries, 0);
    }

    #[test]
    fn cap_and_end_of_who_produce_no_status_noise() {
        let mut st = state();
        let before_status = st.status.lines.len();
        handle_message(&mut st, &parse(":irc.znc.in CAP sh NEW :away-notify").unwrap());
        handle_message(
            &mut st,
            &parse(":irc.example.com 315 abcX #irust-test :End of /WHO list.").unwrap(),
        );
        assert_eq!(
            st.status.lines.len(),
            before_status,
            "CAP/315 should not push any status line"
        );
    }

    #[test]
    fn topicwhotime_shows_who_set_the_topic_and_when() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 333 abcX #irust-test sh 1787269346").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert!(text.starts_with("-- Topic set by sh [] ["), "unexpected line: {text}");
    }

    #[test]
    fn topicwhotime_splits_a_full_hostmask_setter() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 333 abcX #irust-test nick!user@host.example 1787269346").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert!(
            text.starts_with("-- Topic set by nick [user@host.example] ["),
            "unexpected line: {text}"
        );
    }

    #[test]
    fn usermode_setting_controls_the_mode_sent_after_welcome() {
        let _net = net_guard();
        net_set("usermode", "+iw");
        let out = handle_message(&mut state(), &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        assert_eq!(out[0], "MODE abcX +iw");
        net_set("usermode", "");
        let out = handle_message(&mut state(), &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        assert!(!out.iter().any(|l| l.starts_with("MODE ")), "empty usermode sends nothing");
    }

    #[test]
    fn alternate_nick_is_tried_first_then_underscores() {
        let _net = net_guard();
        net_set("alternate_nick", "abcX-away");
        let mut st = state();
        let in_use = |nick: &str| parse(&format!(":irc.example.com 433 * {nick} :Nickname is already in use")).unwrap();
        let out = handle_message(&mut st, &in_use("abcX"));
        assert_eq!(out, vec!["NICK abcX-away".to_string()]);
        // The alternate is taken too: back to the normal '_' fallback.
        let out = handle_message(&mut st, &in_use("abcX-away"));
        assert_eq!(out, vec!["NICK abcX-away_".to_string()]);
    }

    #[test]
    fn alternate_nick_equal_to_the_current_nick_is_ignored() {
        let _net = net_guard();
        net_set("alternate_nick", "abcX");
        let mut st = state();
        let out = handle_message(
            &mut st,
            &parse(":irc.example.com 433 * abcX :Nickname is already in use").unwrap(),
        );
        assert_eq!(out, vec!["NICK abcX_".to_string()]);
    }

    #[test]
    fn sasl_failure_only_quits_when_sasl_disconnect_on_failure_is_on() {
        let _net = net_guard();
        let fail = parse(":irc.example.com 904 abcX :SASL authentication failed").unwrap();
        assert_eq!(handle_message(&mut state(), &fail), vec!["QUIT".to_string()], "on by default, matching irssi");
        net_set("sasl_disconnect_on_failure", "off");
        assert!(handle_message(&mut state(), &fail).is_empty());
    }

    #[test]
    fn skip_motd_hides_the_body_until_the_end_marker_then_shows_a_later_motd() {
        let _net = net_guard();
        net_set("skip_motd", "on");
        let mut st = state();
        let before = st.status.lines.len();
        for line in [
            ":irc.example.com 375 abcX :- irc.example.com Message of the day -",
            ":irc.example.com 372 abcX :- hello",
        ] {
            handle_message(&mut st, &parse(line).unwrap());
        }
        assert_eq!(st.status.lines.len(), before, "MOTD body is skipped");
        handle_message(&mut st, &parse(":irc.example.com 376 abcX :End of /MOTD command.").unwrap());
        assert!(st.motd_got);
        let after_end = st.status.lines.len();
        assert!(after_end > before, "the end marker itself still prints");
        // A manual /motd later in the session is shown in full.
        handle_message(&mut st, &parse(":irc.example.com 372 abcX :- again").unwrap());
        assert!(st.status.lines.len() > after_end);
        // A fresh registration hides it again.
        handle_message(&mut st, &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        assert!(!st.motd_got);
    }

    #[test]
    fn motd_is_shown_normally_when_skip_motd_is_off() {
        let _net = net_guard();
        let mut st = state();
        let before = st.status.lines.len();
        handle_message(&mut st, &parse(":irc.example.com 372 abcX :- hello").unwrap());
        assert!(st.status.lines.len() > before);
    }

    #[test]
    fn channel_sync_off_skips_the_join_time_who() {
        let _net = net_guard();
        let names = parse(":irc.example.com 366 abcX #irust-test :End of /NAMES list").unwrap();
        assert_eq!(handle_message(&mut state(), &names), vec!["WHO #irust-test".to_string()]);
        net_set("channel_sync", "off");
        assert!(handle_message(&mut state(), &names).is_empty());
    }

    #[test]
    fn channel_max_who_sync_caps_the_join_time_who() {
        let _net = net_guard();
        net_set("channel_max_who_sync", "1");
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.insert_plain("a");
        chan.nicklist.insert_plain("b");
        let names = parse(":irc.example.com 366 abcX #irust-test :End of /NAMES list").unwrap();
        assert!(handle_message(&mut st, &names).is_empty(), "2 nicks > limit 1");
    }

    #[test]
    fn invite_queues_an_autojoin_candidate_unless_the_setting_is_off_or_already_joined() {
        let _net = net_guard();
        let invite = |chan: &str| parse(&format!(":Op!u@h INVITE abcX :{chan}")).unwrap();
        let mut st = state();
        handle_message(&mut st, &invite("#other"));
        assert_eq!(st.pending_invite_autojoin, vec!["#other".to_string()]);
        st.pending_invite_autojoin.clear();
        st.channels.get_mut("#irust-test").unwrap().joined = true;
        handle_message(&mut st, &invite("#irust-test"));
        assert!(st.pending_invite_autojoin.is_empty(), "already in it");
        net_set("join_auto_chans_on_invite", "off");
        handle_message(&mut st, &invite("#other"));
        assert!(st.pending_invite_autojoin.is_empty());
    }

    #[test]
    fn a_437_puts_the_channel_on_the_rejoin_list_and_prints_nothing() {
        let _net = net_guard();
        let mut st = state();
        st.channels.get_mut("#irust-test").unwrap().joined = false;
        let before = st.status.lines.len();
        let out = handle_message(
            &mut st,
            &parse(":irc.example.com 437 abcX #irust-test :Nick/channel is temporarily unavailable").unwrap(),
        );
        assert!(out.is_empty());
        assert_eq!(st.status.lines.len(), before, "silent, like irssi's signal_stop");
        assert_eq!(st.rejoin_channels.len(), 1);
        assert_eq!(st.rejoin_channels[0].channel, "#irust-test");
        // A second 437 for the same channel re-arms it, no duplicate.
        st.rejoin_channels[0].joining = true;
        handle_message(&mut st, &parse(":irc.example.com 437 abcX #irust-test :again").unwrap());
        assert_eq!(st.rejoin_channels.len(), 1);
        assert!(!st.rejoin_channels[0].joining);
        // Our own successful JOIN ends it.
        handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
        assert!(st.rejoin_channels.is_empty());
    }

    #[test]
    fn channels_rejoin_unavailable_off_leaves_437_to_the_normal_error_path() {
        let _net = net_guard();
        net_set("channels_rejoin_unavailable", "off");
        let mut st = state();
        handle_message(&mut st, &parse(":irc.example.com 437 abcX #irust-test :unavailable").unwrap());
        assert!(st.rejoin_channels.is_empty());
    }

    #[test]
    fn sasl_success_shows_clean_confirmation() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 903 abcX :SASL authentication successful").unwrap(),
        );
        let BufferLine::System { text, .. } = st.status.lines.back().unwrap() else {
            unreachable!()
        };
        assert_eq!(text, "-- SASL authentication succeeded");
    }

    #[test]
    fn sasl_failure_shows_the_reason() {
        let _net = net_guard();
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 904 abcX :SASL authentication failed").unwrap(),
        );
        let BufferLine::System { text, .. } = st.status.lines.back().unwrap() else {
            unreachable!()
        };
        assert_eq!(text, "-- Cannot authenticate via SASL (SASL authentication failed)");
    }

    #[test]
    fn banlist_with_no_setter_shows_indexed_short_form() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 367 abcX #irust-test *!*@bad.host").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- 1 - #irust-test: ban *!*@bad.host");
        assert_eq!(
            st.channel("#irust-test").unwrap().bans,
            vec!["*!*@bad.host".to_string()]
        );
    }

    /// Same numeric, WITH setter info -- irssi's `"banlist_long"` format
    /// ("... (by <nick>, <N> secs ago)"), computing elapsed seconds
    /// fresh at display time rather than showing a stored absolute
    /// date. Also confirms the index counts up correctly for a second
    /// ban in the same query.
    #[test]
    fn banlist_with_setter_shows_elapsed_seconds_and_increments_index() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 367 abcX #irust-test *!*@first.host").unwrap(),
        );
        let set_time = chrono::Utc::now().timestamp() - 100;
        handle_message(
            &mut st,
            &parse(&format!(
                ":irc.example.com 367 abcX #irust-test *!*@bad.host StaffNick {set_time}"
            ))
            .unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert!(
            text.starts_with("-- 2 - #irust-test: ban *!*@bad.host (by StaffNick, "),
            "unexpected line: {text}"
        );
        assert!(text.ends_with(" secs ago)"), "unexpected line: {text}");
        let secs: i64 = text
            .rsplit_once("(by StaffNick, ")
            .and_then(|(_, rest)| rest.strip_suffix(" secs ago)"))
            .and_then(|n| n.parse().ok())
            .unwrap();
        assert!((99..=101).contains(&secs), "expected ~100 secs elapsed, got {secs}");
    }

    #[test]
    fn away_notice_outside_a_whois_falls_back_to_status() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 301 abcX Other :gone fishing").unwrap(),
        );
        let BufferLine::System { text, .. } = st.status.lines.back().unwrap() else {
            unreachable!()
        };
        assert_eq!(text, "-- Other is away: gone fishing");
    }

    /// Same numeric, but a query for that nick is already open -- routes
    /// there instead, matching irssi's own window targeting.
    #[test]
    fn away_notice_routes_to_an_already_open_query() {
        let mut st = state();
        st.get_or_create_query_buffer("other", "Other");
        handle_message(
            &mut st,
            &parse(":irc.example.com 301 abcX Other :gone fishing").unwrap(),
        );
        let BufferLine::System { text, .. } = st.query_buffers["other"].lines.back().unwrap() else {
            unreachable!()
        };
        assert_eq!(text, "-- Other is away: gone fishing");
    }

    #[test]
    fn invitelist_shows_the_mask_with_no_index() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 346 abcX #irust-test *!*@trusted.host").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- #irust-test: invite *!*@trusted.host");
    }

    /// `RPL_EXCEPTLIST` (348) -- same idea, the `/exemptlist` starter
    /// alias's reply.
    #[test]
    fn exceptlist_shows_the_mask_with_no_index() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 348 abcX #irust-test *!*@exempt.host").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- #irust-test: ban exception *!*@exempt.host");
    }

    #[test]
    fn channel_url_shows_home_page_line() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 328 abcX #irust-test :https://example.net/").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- Home page for #irust-test: https://example.net/");
    }

    #[test]
    fn channelmodeis_shows_current_modes_with_no_setter() {
        let mut st = state();
        handle_message(&mut st, &parse(":irc.example.com 324 abcX #irust-test +nt").unwrap());
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- mode/#irust-test [+nt]");
    }

    /// Same numeric, with a mode that takes a visible argument (e.g.
    /// `+l` -- a limit) -- the trailing arg must be appended, matching
    /// how a live `MODE` change already renders `full_mode`.
    #[test]
    fn channelmodeis_appends_trailing_mode_args() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 324 abcX #irust-test +ntl 50").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- mode/#irust-test [+ntl 50]");
    }

    #[test]
    fn creationtime_shows_when_the_channel_was_created() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 329 abcX #irust-test 1356992791").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert!(
            text.starts_with("-- Channel #irust-test created "),
            "unexpected line: {text}"
        );
    }

    #[test]
    fn end_of_names_prints_total_nicks_and_sync_time_after_a_real_join() {
        let _net = net_guard();
        let mut st = state();
        st.channels.remove("#irust-test");
        let out = handle_message(&mut st, &parse(":abcX!abcX@host JOIN #irust-test").unwrap());
        assert!(
            out.iter().any(|o| o == "MODE #irust-test"),
            "JOIN should also request channel modes (cnircd only sends 329 as a reply to this), got: {out:?}"
        );
        handle_message(
            &mut st,
            &parse(":irc.example.com 353 abcX = #irust-test :@Op +Voiced Plain").unwrap(),
        );
        let before = st.channels["#irust-test"].buffer.lines.len();
        handle_message(
            &mut st,
            &parse(":irc.example.com 366 abcX #irust-test :End of /NAMES list").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let new_lines: Vec<&str> = chan
            .lines
            .iter()
            .skip(before)
            .map(|l| {
                let BufferLine::System { text, .. } = l else {
                    unreachable!()
                };
                text.as_str()
            })
            .collect();
        assert!(
            new_lines
                .iter()
                .any(|t| t.contains("irust: #irust-test: Total of 3 nicks")
                    && t.contains("1 ops")
                    && t.contains("1 voices")
                    && t.contains("1 normal")),
            "missing/wrong nick-count line, got: {new_lines:?}"
        );
        assert!(
            new_lines
                .iter()
                .any(|t| t.starts_with("-- irust: Join to #irust-test was synced in")),
            "missing sync-time line, got: {new_lines:?}"
        );
    }

    #[test]
    fn end_of_names_without_a_pending_join_prints_no_sync_lines() {
        let _net = net_guard();
        let mut st = state();
        let before = st.channels["#irust-test"].buffer.lines.len();
        handle_message(
            &mut st,
            &parse(":irc.example.com 366 abcX #irust-test :End of /NAMES list").unwrap(),
        );
        let chan = &st.channels["#irust-test"].buffer;
        let extra: Vec<&BufferLine> = chan.lines.iter().skip(before).collect();
        assert_eq!(chan.lines.len(), before, "expected no new lines, got: {extra:?}");
    }

    #[test]
    fn channel_mode_change_announces_itself() {
        let mut st = state();
        let msg = parse(":K9!k9@example.net MODE #irust-test +v carol").unwrap();
        handle_message(&mut st, &msg);

        let chan = &st.channels["#irust-test"].buffer;
        let BufferLine::System { text, .. } = &chan.lines[chan.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- mode/#irust-test [+v carol] by K9");
    }

    #[test]
    fn live_ban_mode_updates_the_cached_ban_list() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":Op!u@h MODE #irust-test +b *!*@spammer.example").unwrap(),
        );
        assert_eq!(st.channels["#irust-test"].bans, vec!["*!*@spammer.example".to_string()]);

        handle_message(
            &mut st,
            &parse(":Op!u@h MODE #irust-test -b *!*@spammer.example").unwrap(),
        );
        assert!(st.channels["#irust-test"].bans.is_empty());
    }

    #[test]
    fn a_live_channel_mode_updates_the_modestring() {
        let mut st = state();
        handle_message(&mut st, &parse(":Op!u@h MODE #irust-test +ntk secretkey").unwrap());
        assert_eq!(st.channels["#irust-test"].modestring, "knt secretkey");

        handle_message(
            &mut st,
            &parse(":Op!u@h MODE #irust-test +b *!*@spammer.example").unwrap(),
        );
        assert_eq!(
            st.channels["#irust-test"].modestring, "knt secretkey",
            "a list mode must not appear in $M"
        );
    }

    #[test]
    fn numeric_324_updates_the_modestring_the_same_way_a_live_mode_does() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 324 abcX #irust-test +ntkl secretkey 50").unwrap(),
        );
        assert_eq!(st.channels["#irust-test"].modestring, "klnt secretkey 50");
    }

    #[test]
    fn live_ban_mode_does_not_duplicate_an_already_cached_mask() {
        let mut st = state();
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .bans
            .push("*!*@spammer.example".to_string());
        handle_message(
            &mut st,
            &parse(":Op!u@h MODE #irust-test +b *!*@spammer.example").unwrap(),
        );
        assert_eq!(st.channels["#irust-test"].bans.len(), 1);
    }

    #[test]
    fn private_notice_distinguishes_server_hostname_from_a_real_nick() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":Istanbul.TR.EU.ChatNet.Gg NOTICE abcX :*** Checking Ident").unwrap(),
        );
        let BufferLine::System { kind, .. } = &st.status.lines[st.status.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(*kind, SystemKind::Server);

        handle_message(&mut st, &parse(":dana!~dana@example.net NOTICE abcX :hey").unwrap());
        let BufferLine::System { kind, .. } = &st.status.lines[st.status.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(*kind, SystemKind::Notice);
    }

    #[test]
    fn a_service_privmsg_reply_does_not_auto_open_a_query() {
        let mut st = state();
        assert!(!st.query_buffers.contains_key("k9"));
        handle_message(&mut st, &parse(":K9!k9@example.net PRIVMSG abcX :Yes master.").unwrap());
        assert!(
            !st.query_buffers.contains_key("k9"),
            "a service reply must not auto-create a query buffer"
        );
    }

    #[test]
    fn a_real_persons_direct_privmsg_still_auto_opens_a_query() {
        let mut st = state();
        handle_message(&mut st, &parse(":dana!~dana@example.net PRIVMSG abcX :hey").unwrap());
        assert!(st.query_buffers.contains_key("dana"));
    }

    #[test]
    fn ip_lookup_captures_the_real_address_from_378_not_the_cloak_in_311() {
        use crate::state::PendingIp;

        let mut st = state();
        // `/ip`'s real handler (`main.rs`) always sets both together --
        // `pending_whois` so the normal WHOIS reply still displays,
        // `pending_ip` as the side-channel for the address capture.
        st.pending_whois = Some(("dana".to_string(), BufferId::Channel("#irust-test".to_string())));
        st.pending_ip = Some(PendingIp {
            nick: "dana".to_string(),
            echo_buffer: BufferId::Channel("#irust-test".to_string()),
            host: None,
            start_len: 0,
        });

        // 311's host field is the cloak -- must not be captured.
        let whois311 = parse(":irc.example.com 311 abcX dana ~dana CN-203.0.ihv.qkj * :dana").unwrap();
        handle_message(&mut st, &whois311);
        assert_eq!(st.pending_ip.as_ref().unwrap().host, None);

        let whois378 = parse(":irc.example.com 378 abcX dana :is connecting from *@192.0.2.16 192.0.2.16").unwrap();
        handle_message(&mut st, &whois378);
        assert_eq!(st.pending_ip.as_ref().unwrap().host.as_deref(), Some("192.0.2.16"));
    }

    #[test]
    fn an_active_ip_lookup_suppresses_channels_server_and_secure_lines_but_not_378_or_318() {
        use crate::state::PendingIp;

        let mut st = state();
        st.pending_whois = Some(("dana".to_string(), BufferId::Channel("#irust-test".to_string())));
        st.pending_ip = Some(PendingIp {
            nick: "dana".to_string(),
            echo_buffer: BufferId::Channel("#irust-test".to_string()),
            host: None,
            start_len: 0,
        });

        handle_message(&mut st, &parse(":irc.example.com 319 abcX dana :+#chatnet").unwrap());
        handle_message(
            &mut st,
            &parse(":irc.example.com 312 abcX dana irc.example.com :ChatNet").unwrap(),
        );
        handle_message(
            &mut st,
            &parse(":irc.example.com 671 abcX dana :is using a secure connection").unwrap(),
        );
        handle_message(
            &mut st,
            &parse(":irc.example.com 378 abcX dana :is connecting from *@1.2.3.4 1.2.3.4").unwrap(),
        );
        handle_message(
            &mut st,
            &parse(":irc.example.com 318 abcX dana :End of /WHOIS list.").unwrap(),
        );

        let lines: Vec<String> = st.channels["#irust-test"]
            .buffer
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(
            lines,
            vec![
                "-- hostname : 1.2.3.4 1.2.3.4".to_string(),
                "-- End of WHOIS".to_string(),
            ]
        );
    }

    #[test]
    fn ip_lookup_falls_back_to_338_from_an_auto_whowas_when_the_nick_is_offline() {
        let _net = net_guard();
        use crate::state::PendingIp;

        let mut st = state();
        st.pending_whois = Some(("dana".to_string(), BufferId::Channel("#irust-test".to_string())));
        st.pending_ip = Some(PendingIp {
            nick: "dana".to_string(),
            echo_buffer: BufferId::Channel("#irust-test".to_string()),
            host: None,
            start_len: 0,
        });

        let (out, _) = handle_message_with_highlight(
            &mut st,
            &parse(":irc.example.com 401 abcX dana :No such nick").unwrap(),
            &[],
            &[],
            &[],
        );
        assert_eq!(out, vec!["WHOWAS dana 1".to_string()]);
        assert_eq!(st.pending_ip.as_ref().unwrap().host, None);

        handle_message(
            &mut st,
            &parse(":irc.example.com 318 abcX dana :End of /WHOIS list.").unwrap(),
        );
        assert_eq!(
            st.pending_ip.as_ref().unwrap().host,
            None,
            "still waiting, not derailed"
        );

        handle_message(
            &mut st,
            &parse(":irc.example.com 338 abcX dana 203.0.113.21 :actually using host").unwrap(),
        );
        assert_eq!(st.pending_ip.as_ref().unwrap().host.as_deref(), Some("203.0.113.21"));
    }

    #[test]
    fn a_second_whois_on_a_different_nick_does_not_leave_pending_ip_stuck() {
        let _net = net_guard();
        use crate::state::PendingIp;

        let mut st = state();
        st.pending_whois = Some(("nick1".to_string(), BufferId::Channel("#irust-test".to_string())));
        st.pending_ip = Some(PendingIp {
            nick: "nick1".to_string(),
            echo_buffer: BufferId::Channel("#irust-test".to_string()),
            host: None,
            start_len: 0,
        });

        // nick1 is offline -- auto-WHOWAS retry kicks in, same as the
        // sibling test above.
        handle_message(
            &mut st,
            &parse(":irc.example.com 401 abcX nick1 :No such nick").unwrap(),
        );
        assert!(st.auto_whowas_pending);

        // A plain `/whois nick2` typed before nick1's WHOWAS reply
        // arrives -- overwrites `pending_whois`, same as the real
        // handler in `main.rs` does.
        st.pending_whois = Some(("nick2".to_string(), BufferId::Channel("#irust-test".to_string())));
        st.auto_whowas_pending = false;

        // nick1's real WHOWAS terminator finally arrives -- must still
        // clear `pending_ip`, even though `pending_whois` no longer
        // matches nick1 at all.
        handle_message(
            &mut st,
            &parse(":irc.example.com 369 abcX nick1 :End of WHOWAS").unwrap(),
        );
        assert!(st.pending_ip.is_none(), "pending_ip must not get stuck on nick1");

        // A later, unrelated plain /whois on nick1 must show its normal
        // fields again, not have them suppressed as if `/ip` were still
        // in flight.
        st.pending_whois = Some(("nick1".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(&mut st, &parse(":irc.example.com 319 abcX nick1 :#chatnet").unwrap());
        let lines: Vec<String> = st.channels["#irust-test"]
            .buffer
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(
            lines.iter().any(|l| l.contains("#chatnet")),
            "319 must not be suppressed once pending_ip is really gone, got: {lines:?}"
        );
    }

    #[test]
    fn self_mode_change_updates_own_modes() {
        let mut st = state();
        let msg = parse(":abcX MODE abcX :+iw").unwrap();
        handle_message(&mut st, &msg);
        let modes: std::collections::HashSet<char> = st.own_modes.chars().collect();
        assert_eq!(modes, ['i', 'w'].into_iter().collect());
    }

    #[test]
    fn self_mode_change_shows_a_visible_line() {
        let mut st = state();
        handle_message(&mut st, &parse(":abcX MODE abcX :+Zi").unwrap());
        let BufferLine::System { text, .. } = st.status.lines.back().unwrap() else {
            unreachable!()
        };
        assert_eq!(text, "-- Mode change [+Zi] for user abcX");
    }

    #[test]
    fn self_mode_change_removes_a_letter() {
        let mut st = state();
        st.own_modes = "iw".to_string();
        let msg = parse(":abcX MODE abcX :-w").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(st.own_modes, "i");
    }

    #[test]
    fn rpl_umodeis_replaces_own_modes_wholesale() {
        let mut st = state();
        st.own_modes = "stale".to_string();
        let msg = parse(":irc.example.com 221 abcX +iw").unwrap();
        handle_message(&mut st, &msg);
        let modes: std::collections::HashSet<char> = st.own_modes.chars().collect();
        assert_eq!(modes, ['i', 'w'].into_iter().collect());
    }

    #[test]
    fn a_channel_mode_change_does_not_touch_own_modes() {
        let mut st = state();
        let msg = parse(":Op!u@h MODE #irust-test +o abcX").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(st.own_modes, "");
    }

    fn system(text: &str) -> BufferLine {
        BufferLine::System {
            text: text.to_string(),
            timestamp: String::new(),
            time: None,
            kind: crate::buffer::SystemKind::Other,
        }
    }

    fn system_join(text: &str) -> BufferLine {
        BufferLine::System {
            text: text.to_string(),
            timestamp: String::new(),
            time: None,
            kind: crate::buffer::SystemKind::Join,
        }
    }

    fn system_leave(text: &str) -> BufferLine {
        BufferLine::System {
            text: text.to_string(),
            timestamp: String::new(),
            time: None,
            kind: crate::buffer::SystemKind::Leave,
        }
    }

    fn system_topic(text: &str) -> BufferLine {
        BufferLine::System {
            text: text.to_string(),
            timestamp: String::new(),
            time: None,
            kind: crate::buffer::SystemKind::Topic,
        }
    }

    fn last_line(st: &ClientState, chan: &str) -> BufferLine {
        st.channel(chan).unwrap().buffer.lines.back().unwrap().clone()
    }

    #[test]
    fn notopic_clears_state_and_shows_a_line() {
        let mut st = state();
        st.channels.get_mut("#irust-test").unwrap().topic = Some("stale".to_string());
        handle_message(
            &mut st,
            &parse(":irc.example.com 331 abcX #irust-test :No topic is set").unwrap(),
        );
        assert_eq!(st.channel("#irust-test").unwrap().topic, None);
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_topic("-- No topic set for #irust-test")
        );
    }

    #[test]
    fn topic_command_shows_who_changed_it_and_which_channel() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h TOPIC #irust-test :new topic here").unwrap());
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_topic("-- Other changed the topic of #irust-test to: new topic here")
        );
    }

    #[test]
    fn welcome_marks_connected_and_auto_joins() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":irc.example.com 001 abcX :Welcome").unwrap();
        let out = handle_message(&mut st, &msg);
        assert!(st.connected);
        assert_eq!(
            out,
            vec!["MODE abcX +i".to_string(), "JOIN #irust-test".to_string(), "WHOIS abcX".to_string()]
        );
        assert!(st.self_whois_pending);
    }

    #[test]
    fn welcome_rejoins_a_keyed_channel_with_its_remembered_key() {
        let _net = net_guard();
        let mut st = state();
        st.record_join_key("#irust-test", "secretkey".to_string());
        st.ensure_channel("#irust-test");
        assert_eq!(st.channel("#irust-test").unwrap().key.as_deref(), Some("secretkey"));

        let out = handle_message(&mut st, &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        assert_eq!(
            out,
            vec!["MODE abcX +i".to_string(), "JOIN #irust-test secretkey".to_string(), "WHOIS abcX".to_string()]
        );
    }

    #[test]
    fn numeric_004_captures_the_server_version() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.net 004 abcX irc.example.net solanum-1.0-dev DGIRSWabcdefgiorswxyz bciklmnopstvz")
                .unwrap(),
        );
        assert_eq!(st.server_version.as_deref(), Some("solanum-1.0-dev"));
    }

    #[test]
    fn self_join_captures_our_own_userhost_the_first_time_only() {
        let mut st = state();
        assert_eq!(st.own_userhost, None);

        handle_message(&mut st, &parse(":abcX!realuser@real.host JOIN #irust-test").unwrap());
        assert_eq!(st.own_userhost.as_deref(), Some("realuser@real.host"));

        handle_message(&mut st, &parse(":abcX!otheruser@other.host JOIN #another").unwrap());
        assert_eq!(st.own_userhost.as_deref(), Some("realuser@real.host"));
    }

    /// A channel JOIN from someone else must never be mistaken for our
    /// own userhost, regardless of ordering.
    #[test]
    fn a_join_from_someone_else_never_sets_our_own_userhost() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        assert_eq!(st.own_userhost, None);
    }

    #[test]
    fn numeric_396_updates_the_host_half_of_an_already_known_userhost() {
        let mut st = state();
        st.own_userhost = Some("realuser@old.cloak".to_string());
        handle_message(
            &mut st,
            &parse(":irc.example.net 396 abcX new.cloak :is now your displayed host").unwrap(),
        );
        assert_eq!(st.own_userhost.as_deref(), Some("realuser@new.cloak"));
    }

    #[test]
    fn numeric_396_with_a_full_userhost_replaces_the_whole_value() {
        let mut st = state();
        st.own_userhost = Some("realuser@old.cloak".to_string());
        handle_message(
            &mut st,
            &parse(":irc.example.net 396 abcX newuser@new.cloak :is now your displayed host").unwrap(),
        );
        assert_eq!(st.own_userhost.as_deref(), Some("newuser@new.cloak"));
    }

    #[test]
    fn numeric_396_before_any_userhost_is_known_and_with_no_at_sign_is_a_no_op() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.net 396 abcX new.cloak :is now your displayed host").unwrap(),
        );
        assert_eq!(st.own_userhost, None, "nothing to combine the bare host with yet");
    }

    #[test]
    fn silent_self_whois_syncs_away_state_from_a_301_reply() {
        let _net = net_guard();
        let mut st = state();
        handle_message(&mut st, &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        assert!(st.self_whois_pending);
        assert!(!st.away, "starts false until the probe's reply says otherwise");

        handle_message(&mut st, &parse(":irc.example.com 301 abcX abcX :gone fishing").unwrap());
        assert!(st.away);
        assert!(st.self_whois_pending, "301 isn't the terminator, still waiting for it");

        handle_message(
            &mut st,
            &parse(":irc.example.com 318 abcX abcX :End of /WHOIS list.").unwrap(),
        );
        assert!(!st.self_whois_pending, "318 ends the probe");
        assert!(st.away, "stays true after the probe finishes");
    }

    #[test]
    fn silent_self_whois_never_displays_anything() {
        let _net = net_guard();
        let mut st = state();
        handle_message(&mut st, &parse(":irc.example.com 001 abcX :Welcome").unwrap());

        handle_message(
            &mut st,
            &parse(":irc.example.com 311 abcX abcX ~abcX host * :abcX").unwrap(),
        );
        handle_message(&mut st, &parse(":irc.example.com 319 abcX abcX :#irust-test").unwrap());
        handle_message(
            &mut st,
            &parse(":irc.example.com 318 abcX abcX :End of /WHOIS list.").unwrap(),
        );

        let lines: Vec<String> = st
            .status
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(lines.is_empty(), "silent probe must not push anything, got: {lines:?}");
    }

    #[test]
    fn a_real_self_whois_after_the_probe_finishes_still_displays() {
        let _net = net_guard();
        let mut st = state();
        handle_message(&mut st, &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        handle_message(
            &mut st,
            &parse(":irc.example.com 318 abcX abcX :End of /WHOIS list.").unwrap(),
        );
        assert!(!st.self_whois_pending);

        st.set_pending_whois("abcx".to_string(), BufferId::Status);
        handle_message(
            &mut st,
            &parse(":irc.example.com 311 abcX abcX ~abcX host * :abcX").unwrap(),
        );

        let lines: Vec<String> = st
            .status
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert!(
            !lines.is_empty(),
            "a real /whois must still display once the silent probe is done"
        );
    }

    #[test]
    fn ping_gets_ponged() {
        let mut st = state();
        let msg = parse("PING :abc123").unwrap();
        let out = handle_message(&mut st, &msg);
        assert_eq!(out, vec!["PONG :abc123".to_string()]);
    }

    #[test]
    fn own_join_sets_joined_flag_switches_active_buffer_and_logs() {
        let mut st = state();
        let msg = parse(":abcX!u@h JOIN #irust-test").unwrap();
        handle_message(&mut st, &msg);
        assert!(st.channel("#irust-test").unwrap().joined);
        assert_eq!(st.active_buffer, crate::BufferId::Channel("#irust-test".to_string()));
        assert_eq!(last_line(&st, "#irust-test"), system_join("-- abcX [u@h] has joined #irust-test"));
    }

    #[test]
    fn end_of_names_triggers_a_who_to_seed_away_status() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":irc.example.com 366 abcX #irust-test :End of /NAMES list").unwrap();
        let out = handle_message(&mut st, &msg);
        assert_eq!(out, vec!["WHO #irust-test".to_string()]);
    }

    fn active_texts(st: &mut ClientState) -> Vec<String> {
        st.current_buffer_mut().lines
            .iter()
            .filter_map(|l| match l {
                crate::buffer::BufferLine::System { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn manual_who_prints_rows_and_end_marker_in_irssi_format() {
        let mut st = state();
        st.manual_who_pending.push("#irust-test".to_string());
        let before = active_texts(&mut st).len();
        handle_message(&mut st, &parse(":irc.example.com 352 abcX #irust-test ~u host.example irc.example.com Other H@ :0 Real Name").unwrap());
        handle_message(&mut st, &parse(":irc.example.com 315 abcX #irust-test :End of /WHO list.").unwrap());
        let lines = active_texts(&mut st);
        assert_eq!(lines[before], "#irust-tes Other     H@  0  ~u@host.example (Real Name)");
        assert_eq!(lines[before + 1], "End of /WHO list");
        assert!(st.manual_who_pending.is_empty());
    }

    #[test]
    fn automatic_who_stays_silent() {
        let mut st = state();
        let before = active_texts(&mut st).len();
        handle_message(&mut st, &parse(":irc.example.com 352 abcX #irust-test ~u host irc.example.com Other H :0 Real Name").unwrap());
        handle_message(&mut st, &parse(":irc.example.com 315 abcX #irust-test :End of /WHO list.").unwrap());
        assert_eq!(active_texts(&mut st).len(), before);
    }

    #[test]
    fn channel_messages_and_joins_feed_the_completion_last_speakers_list() {
        let mut st = state();
        st.channels.get_mut("#irust-test").unwrap().nicklist.insert_plain("Other");
        handle_message(&mut st, &parse(":Other!u@h PRIVMSG #irust-test :hello all").unwrap());
        handle_message(&mut st, &parse(":Newbie!u@h JOIN #irust-test").unwrap());
        let list = &st.channel("#irust-test").unwrap().last_msgs;
        assert_eq!(list.iter().map(|r| r.nick.as_str()).collect::<Vec<_>>(), ["Newbie", "Other"]);
        assert!(list.iter().all(|r| r.own == 0), "neither addressed us");
    }

    #[test]
    fn a_message_addressed_to_us_is_marked_own_and_a_departing_nick_is_forgotten() {
        let mut st = state();
        st.channels.get_mut("#irust-test").unwrap().nicklist.insert_plain("Other");
        handle_message(&mut st, &parse(":Other!u@h PRIVMSG #irust-test :abcX: ping").unwrap());
        assert!(st.channel("#irust-test").unwrap().last_msgs[0].own > 0);
        handle_message(&mut st, &parse(":Other!u@h PART #irust-test").unwrap());
        assert!(st.channel("#irust-test").unwrap().last_msgs.is_empty());
    }

    #[test]
    fn private_messages_feed_the_msg_completion_list() {
        let mut st = state();
        handle_message(&mut st, &parse(":Pal!u@h PRIVMSG abcX :psst").unwrap());
        assert_eq!(st.private_last_msgs.first().map(|r| r.nick.as_str()), Some("Pal"));
    }

    #[test]
    fn a_public_flood_autoignores_the_sender_when_autoignore_level_covers_publics() {
        let mut settings = irust_config::settings::Settings::default();
        settings.set("autoignore_level", "publics").unwrap();
        let mut st = state();
        let line = |n: usize| format!(":Spam!u@h PRIVMSG #irust-test :buy now {n}");
        // flood_max_msgs (4) messages are fine, the 5th floods...
        for n in 0..5 {
            handle_message(&mut st, &parse(&line(n)).unwrap());
        }
        let after_flood = st.channel("#irust-test").unwrap().buffer.lines.len();
        // ...and from then on the sender is ignored.
        handle_message(&mut st, &parse(&line(99)).unwrap());
        let after_more = st.channel("#irust-test").unwrap().buffer.lines.len();
        settings.set("autoignore_level", "").unwrap();
        assert_eq!(st.auto_ignores.len(), 1);
        assert_eq!(st.auto_ignores[0].mask, "Spam!u@h");
        assert_eq!(after_more, after_flood, "the sixth message was dropped");
    }

    #[test]
    fn flood_detection_without_autoignore_level_ignores_nobody() {
        let mut st = state();
        for n in 0..8 {
            handle_message(&mut st, &parse(&format!(":Chatty!u@h PRIVMSG #irust-test :line {n}")).unwrap());
        }
        assert!(st.auto_ignores.is_empty(), "autoignore_level is empty by default");
    }

    #[test]
    fn who_reply_marks_a_gone_member_away() {
        let mut st = state();
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .nicklist
            .insert_plain("Other");
        let msg = parse(":irc.example.com 352 abcX #irust-test ~u host irc.example.com Other G :0 Real Name").unwrap();
        handle_message(&mut st, &msg);
        let entry = st
            .channel("#irust-test")
            .unwrap()
            .nicklist
            .sorted()
            .into_iter()
            .find(|e| e.nick == "Other")
            .unwrap();
        assert!(entry.away);
    }

    #[test]
    fn who_reply_marks_the_same_nick_away_in_every_shared_channel() {
        let mut st = state();
        st.channels.get_mut("#irust-test").unwrap().nicklist.insert_plain("TGS");
        handle_message(&mut st, &parse(":abcX!abcX@host JOIN #routing").unwrap());
        st.channels.get_mut("#routing").unwrap().nicklist.insert_plain("TGS");

        // WHO was only issued/answered for #routing -- #irust-test never
        // got its own WHO reply for this nick.
        let msg = parse(":irc.example.com 352 abcX #routing ~u host irc.example.com TGS G :0 Real Name").unwrap();
        handle_message(&mut st, &msg);

        for chan_name in ["#routing", "#irust-test"] {
            let entry = st
                .channel(chan_name)
                .unwrap()
                .nicklist
                .sorted()
                .into_iter()
                .find(|e| e.nick == "TGS")
                .unwrap();
            assert!(entry.away, "{chan_name} should show TGS as away too");
        }
    }

    #[test]
    fn who_reply_marks_a_here_member_not_away() {
        let mut st = state();
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .nicklist
            .insert_plain("Other");
        let msg = parse(":irc.example.com 352 abcX #irust-test ~u host irc.example.com Other H :0 Real Name").unwrap();
        handle_message(&mut st, &msg);
        let entry = st
            .channel("#irust-test")
            .unwrap()
            .nicklist
            .sorted()
            .into_iter()
            .find(|e| e.nick == "Other")
            .unwrap();
        assert!(!entry.away);
    }

    #[test]
    fn rpl_nowaway_and_unaway_update_away_state_live() {
        let mut st = state();
        assert!(!st.away);

        handle_message(
            &mut st,
            &parse(":irc.example.com 306 abcX :You have been marked as away").unwrap(),
        );
        assert!(st.away);
        assert_eq!(
            st.status.lines.back().unwrap(),
            &system("-- You have been marked as being away")
        );

        handle_message(
            &mut st,
            &parse(":irc.example.com 305 abcX :You are no longer marked as away").unwrap(),
        );
        assert!(!st.away);
        assert_eq!(
            st.status.lines.back().unwrap(),
            &system("-- You are no longer marked as being away")
        );
    }

    #[test]
    fn away_notify_marks_and_unmarks_a_nick_everywhere_they_are_visible() {
        let mut st = state();
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .nicklist
            .insert_plain("Other");

        let gone = parse(":Other!u@h AWAY :be right back").unwrap();
        handle_message(&mut st, &gone);
        let away = st
            .channel("#irust-test")
            .unwrap()
            .nicklist
            .sorted()
            .into_iter()
            .find(|e| e.nick == "Other")
            .unwrap()
            .away;
        assert!(away);

        let back = parse(":Other!u@h AWAY").unwrap();
        handle_message(&mut st, &back);
        let away = st
            .channel("#irust-test")
            .unwrap()
            .nicklist
            .sorted()
            .into_iter()
            .find(|e| e.nick == "Other")
            .unwrap()
            .away;
        assert!(!away);
    }

    #[test]
    fn own_join_to_a_new_channel_creates_it_and_switches_to_it() {
        let mut st = state();
        handle_message(&mut st, &parse(":abcX!u@h JOIN #second").unwrap());
        assert!(st.channel("#second").is_some());
        assert_eq!(st.active_buffer, crate::BufferId::Channel("#second".to_string()));
    }

    #[test]
    fn others_join_logs_without_setting_joined_flag() {
        let mut st = state();
        let msg = parse(":Other!u@h JOIN #irust-test").unwrap();
        handle_message(&mut st, &msg);
        assert!(!st.channel("#irust-test").unwrap().joined);
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_join("-- Other [u@h] has joined #irust-test")
        );
    }

    #[test]
    fn ignored_join_still_updates_nicklist_but_suppresses_the_line() {
        let mut st = state();
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "Other".to_string(),
            levels: vec!["joins".to_string()],
            is_regex: false,
            ..Default::default()
        }];
        let msg = parse(":Other!u@h JOIN #irust-test").unwrap();
        handle_message_with_highlight(&mut st, &msg, &[], &ignores, &[]);
        assert!(
            st.channel("#irust-test").unwrap().nicklist.contains("Other"),
            "nicklist must still update"
        );
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.lines.len(),
            0,
            "the join line must be suppressed"
        );
    }

    #[test]
    fn ignored_part_still_updates_nicklist_but_suppresses_the_line() {
        let mut st = state();
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "Other".to_string(),
            levels: vec!["parts".to_string()],
            is_regex: false,
            ..Default::default()
        }];
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        let before = st.channel("#irust-test").unwrap().buffer.lines.len();
        let msg = parse(":Other!u@h PART #irust-test :bye").unwrap();
        handle_message_with_highlight(&mut st, &msg, &[], &ignores, &[]);
        assert!(
            !st.channel("#irust-test").unwrap().nicklist.contains("Other"),
            "nicklist must still update"
        );
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.lines.len(),
            before,
            "the part line must be suppressed"
        );
    }

    #[test]
    fn ignored_quit_still_updates_nicklist_but_suppresses_the_line() {
        let mut st = state();
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "Other".to_string(),
            levels: vec!["quits".to_string()],
            is_regex: false,
            ..Default::default()
        }];
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        let before = st.channel("#irust-test").unwrap().buffer.lines.len();
        let msg = parse(":Other!u@h QUIT :goodbye").unwrap();
        handle_message_with_highlight(&mut st, &msg, &[], &ignores, &[]);
        assert!(
            !st.channel("#irust-test").unwrap().nicklist.contains("Other"),
            "nicklist must still update"
        );
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.lines.len(),
            before,
            "the quit line must be suppressed"
        );
    }

    #[test]
    fn ignored_nick_change_still_renames_in_nicklist_but_suppresses_the_line() {
        let mut st = state();
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "Other".to_string(),
            levels: vec!["nicks".to_string()],
            is_regex: false,
            ..Default::default()
        }];
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        let before = st.channel("#irust-test").unwrap().buffer.lines.len();
        let msg = parse(":Other!u@h NICK :OtherRenamed").unwrap();
        handle_message_with_highlight(&mut st, &msg, &[], &ignores, &[]);
        assert!(
            st.channel("#irust-test").unwrap().nicklist.contains("OtherRenamed"),
            "nicklist must still rename"
        );
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.lines.len(),
            before,
            "the nick-change line must be suppressed"
        );
    }

    #[test]
    fn our_own_nick_change_is_never_suppressed_by_an_ignore() {
        let mut st = state();
        // A pathological ignore matching our own nick must never
        // suppress our own actions -- same convention `/dcc`/PART's own
        // "you left" line already follow.
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "abcX".to_string(),
            levels: vec!["nicks".to_string()],
            is_regex: false,
            ..Default::default()
        }];
        let msg = parse(":abcX!u@h NICK :abcXrenamed").unwrap();
        handle_message_with_highlight(&mut st, &msg, &[], &ignores, &[]);
        assert_eq!(st.nick, "abcXrenamed");
    }

    #[test]
    fn join_to_a_channel_we_dont_know_about_is_ignored() {
        let mut st = state();
        let msg = parse(":Other!u@h JOIN #somewhere-else").unwrap();
        handle_message(&mut st, &msg);
        assert!(st.channel("#somewhere-else").is_none());
    }

    #[test]
    fn our_own_privmsg_echoed_back_is_not_pushed_again() {
        let mut st = state();
        st.ensure_channel("#irust-test");
        let before = st.channel("#irust-test").unwrap().buffer.lines.len();
        let msg = parse(":abcX!u@h PRIVMSG #irust-test :already shown locally").unwrap();
        handle_message(&mut st, &msg);
        let after = st.channel("#irust-test").unwrap().buffer.lines.len();
        assert_eq!(before, after, "self-echoed PRIVMSG must not add a line");
    }

    #[test]
    fn privmsg_to_our_channel_is_logged_with_a_resolved_color() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :hello there").unwrap();
        handle_message(&mut st, &msg);
        let expected_color = st.nick_colors.resolve("Other");
        assert_eq!(
            last_line(&st, "#irust-test"),
            BufferLine::Chat {
                nick: "Other".to_string(),
                color_index: expected_color,
                rank: None,
                text: "hello there".to_string(),
                highlight: false,
                highlight_color: None,
                timestamp: String::new(),
                time: None,
            }
        );
    }

    #[test]
    fn handle_message_with_highlight_reports_true_for_a_mention() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :hey abcX").unwrap();
        let (_, highlighted) = handle_message_with_highlight(&mut st, &msg, &[], &[], &[]);
        assert!(highlighted);
    }

    #[test]
    fn handle_message_with_highlight_reports_false_for_unrelated_chatter() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :just chatting").unwrap();
        let (_, highlighted) = handle_message_with_highlight(&mut st, &msg, &[], &[], &[]);
        assert!(!highlighted);
    }

    #[test]
    fn handle_message_with_highlight_reports_true_for_a_service_notice() {
        let mut st = state();
        let msg = parse(":K9!service@k9.example.net NOTICE abcX :Welcome back.").unwrap();
        let (_, highlighted) = handle_message_with_highlight(&mut st, &msg, &[], &[], &[]);
        assert!(highlighted);
    }

    #[test]
    fn handle_message_with_highlight_reports_true_for_a_hilight_pattern_match() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :we have an outage").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            ..Default::default()
        }];
        let (_, highlighted) = handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert!(highlighted);
    }

    #[test]
    fn hilight_level_decides_which_messages_the_hilight_rules_may_match() {
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            ..Default::default()
        }];
        let hit = |settings: &[(&str, &str)], line: &str| {
            irust_config::lookandfeel::with_test_overrides(settings, || {
                let mut st = state();
                handle_message_with_highlight(&mut st, &parse(line).unwrap(), &entries, &[], &[]).1
            })
        };
        // a DM always highlights, so use a channel message the rule alone would catch
        let public = ":Other!u@h PRIVMSG #irust-test :we have an outage";
        assert!(hit(&[], public));
        assert!(!hit(&[("hilight_level", "MSGS")], public), "PUBLIC removed: the rule can't match");
        assert!(hit(&[("hilight_level", "PUBLIC")], public));
    }

    #[test]
    fn a_hilight_with_a_color_carries_it_onto_the_rendered_line() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :we have an outage").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            color: Some("203".to_string()),
            ..Default::default()
        }];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        match last_line(&st, "#irust-test") {
            BufferLine::Chat {
                highlight,
                highlight_color,
                ..
            } => {
                assert!(highlight);
                assert_eq!(highlight_color.as_deref(), Some("203"));
            }
            other => panic!("expected a Chat line, got {other:?}"),
        }
    }

    #[test]
    fn a_hilight_with_no_color_leaves_the_line_uncolored() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :we have an outage").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            ..Default::default()
        }];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        match last_line(&st, "#irust-test") {
            BufferLine::Chat {
                highlight,
                highlight_color,
                ..
            } => {
                assert!(highlight);
                assert_eq!(highlight_color, None);
            }
            other => panic!("expected a Chat line, got {other:?}"),
        }
    }

    #[test]
    fn an_own_nick_mention_never_carries_a_hilight_color() {
        // Own-nick mentions don't come from any `HilightEntry`, so there's
        // never a color to pull from even with unrelated colored entries
        // defined.
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :hey abcX").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            color: Some("203".to_string()),
            ..Default::default()
        }];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        match last_line(&st, "#irust-test") {
            BufferLine::Chat {
                highlight,
                highlight_color,
                ..
            } => {
                assert!(highlight);
                assert_eq!(highlight_color, None);
            }
            other => panic!("expected a Chat line, got {other:?}"),
        }
    }

    #[test]
    fn a_hilight_with_an_actcolor_sets_the_channel_buffers_pending_activity_color() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :we have an outage").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            actcolor: Some("magenta".to_string()),
            ..Default::default()
        }];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.highlight_actcolor.as_deref(),
            Some("magenta")
        );
    }

    #[test]
    fn a_hilight_with_no_actcolor_leaves_the_channel_buffers_activity_color_unset() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :we have an outage").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            ..Default::default()
        }];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert_eq!(st.channel("#irust-test").unwrap().buffer.highlight_actcolor, None);
    }

    #[test]
    fn an_own_nick_mention_never_sets_a_hilight_actcolor() {
        // Same reasoning as `an_own_nick_mention_never_carries_a_hilight_color`
        // above -- a plain mention comes from no `HilightEntry` at all,
        // so there's no `-actcolor` to pull even with unrelated entries
        // that have one defined.
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :hey abcX").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            actcolor: Some("magenta".to_string()),
            ..Default::default()
        }];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert_eq!(st.channel("#irust-test").unwrap().buffer.highlight_actcolor, None);
    }

    #[test]
    fn a_channel_notice_hilight_actcolor_sets_the_same_buffer_field_as_a_privmsg() {
        let mut st = state();
        let msg = parse(":K9!k9@example.net NOTICE #irust-test :we have an outage").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            actcolor: Some("magenta".to_string()),
            ..Default::default()
        }];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.highlight_actcolor.as_deref(),
            Some("magenta")
        );
    }

    #[test]
    fn a_direct_privmsg_hilight_actcolor_sets_the_query_buffers_activity_color() {
        let mut st = state();
        let msg = parse(":Friend!u@h PRIVMSG abcX :we have an outage").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            actcolor: Some("magenta".to_string()),
            ..Default::default()
        }];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert_eq!(
            st.query_buffers["friend"].highlight_actcolor.as_deref(),
            Some("magenta")
        );
    }

    #[test]
    fn a_higher_priority_hilight_actcolor_wins_over_an_earlier_matching_entry() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :we have an outage").unwrap();
        let entries = vec![
            irust_config::HilightEntry {
                pattern: "outage".to_string(),
                actcolor: Some("cyan".to_string()),
                ..Default::default()
            },
            irust_config::HilightEntry {
                pattern: "outage".to_string(),
                actcolor: Some("magenta".to_string()),
                priority: 10,
                ..Default::default()
            },
        ];
        handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.highlight_actcolor.as_deref(),
            Some("magenta")
        );
    }

    #[test]
    fn handle_message_with_highlight_ignores_unrelated_hilight_patterns() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :just chatting").unwrap();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            ..Default::default()
        }];
        let (_, highlighted) = handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert!(!highlighted);
    }

    #[test]
    fn handle_message_with_highlight_respects_a_hilight_channels_restriction() {
        let mut st = state();
        let entries = vec![irust_config::HilightEntry {
            pattern: "outage".to_string(),
            channels: vec!["#other-channel".to_string()],
            ..Default::default()
        }];
        let msg = parse(":Other!u@h PRIVMSG #irust-test :we have an outage").unwrap();
        let (_, highlighted) = handle_message_with_highlight(&mut st, &msg, &entries, &[], &[]);
        assert!(!highlighted, "hilight is scoped to a different channel");
    }

    #[test]
    fn channel_message_mentioning_our_nick_is_highlighted() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :hey abcX check this out").unwrap();
        handle_message(&mut st, &msg);
        match last_line(&st, "#irust-test") {
            BufferLine::Chat { highlight, .. } => assert!(highlight),
            _ => panic!("expected chat line"),
        }
    }

    fn is_highlighted_with(settings: &[(&str, &str)], line: &str) -> bool {
        irust_config::lookandfeel::with_test_overrides(settings, || {
            let mut st = state();
            handle_message(&mut st, &parse(line).unwrap());
            match last_line(&st, "#irust-test") {
                BufferLine::Chat { highlight, .. } => highlight,
                _ => panic!("expected chat line"),
            }
        })
    }

    #[test]
    fn hilight_nick_matches_off_stops_own_nick_mentions_highlighting() {
        let line = ":Other!u@h PRIVMSG #irust-test :abcX: are you there";
        assert!(is_highlighted_with(&[], line));
        assert!(!is_highlighted_with(&[("hilight_nick_matches", "off")], line));
    }

    #[test]
    fn without_everywhere_only_a_message_addressed_to_us_highlights() {
        let settings = [("hilight_nick_matches_everywhere", "off")];
        assert!(is_highlighted_with(&settings, ":Other!u@h PRIVMSG #irust-test :abcX: are you there"));
        assert!(!is_highlighted_with(&settings, ":Other!u@h PRIVMSG #irust-test :hey abcX check this out"));
        // the default (irust's long-standing behaviour) matches anywhere
        assert!(is_highlighted_with(&[], ":Other!u@h PRIVMSG #irust-test :hey abcX check this out"));
    }

    #[test]
    fn emphasis_wraps_marked_words_in_stored_chat_text_only_when_enabled() {
        let text_for = |settings: &[(&str, &str)]| {
            irust_config::lookandfeel::with_test_overrides(settings, || {
                let mut st = state();
                handle_message(&mut st, &parse(":Other!u@h PRIVMSG #irust-test :this is *great* news").unwrap());
                match last_line(&st, "#irust-test") {
                    BufferLine::Chat { text, .. } => text,
                    _ => panic!("expected chat line"),
                }
            })
        };
        assert_eq!(text_for(&[]), "this is *great* news");
        assert_eq!(text_for(&[("emphasis", "on")]), "this is \u{2}*great*\u{2} news");
    }

    #[test]
    fn show_own_nickchange_once_prints_a_single_line_where_you_are_looking() {
        let count_lines = |settings: &[(&str, &str)]| {
            irust_config::lookandfeel::with_test_overrides(settings, || {
                let mut st = state();
                handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
                handle_message(&mut st, &parse(":abcX!u@h JOIN #other").unwrap());
                handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
                let before: usize = st.channels.values().map(|c| c.buffer.lines.len()).sum::<usize>();
                handle_message(&mut st, &parse(":abcX!u@h NICK :abcY").unwrap());
                let after: usize = st.channels.values().map(|c| c.buffer.lines.len()).sum::<usize>();
                after - before
            })
        };
        assert_eq!(count_lines(&[]), 2, "default: one line in each of the two channels");
        assert_eq!(count_lines(&[("show_own_nickchange_once", "on")]), 1, "once, in the active channel");
    }

    #[test]
    fn autocreate_query_level_decides_whether_a_dm_or_notice_opens_a_query() {
        let dm = ":Stranger!u@h PRIVMSG abcX :hello";
        let notice = ":Stranger!u@h NOTICE abcX :psst";
        let queries = |settings: &[(&str, &str)], line: &str| {
            irust_config::lookandfeel::with_test_overrides(settings, || {
                let mut st = state();
                handle_message(&mut st, &parse(line).unwrap());
                st.query_buffers.contains_key("stranger")
            })
        };
        assert!(queries(&[], dm), "default: a DM opens a query");
        assert!(!queries(&[("autocreate_query_level", "DCCMSGS")], dm), "MSGS removed: no query");
        assert!(!queries(&[], notice), "default: a private notice stays in status");
        assert!(queries(&[("autocreate_query_level", "MSGS NOTICES")], notice), "NOTICES opens one");
    }

    #[test]
    fn window_auto_change_decides_whether_an_auto_created_query_steals_focus() {
        let dm = ":Stranger!u@h PRIVMSG abcX :hello";
        // Default (off): the query opens, but the channel stays active.
        let mut st = state();
        handle_message(&mut st, &parse(dm).unwrap());
        assert_eq!(st.active_buffer, BufferId::Channel("#irust-test".to_string()));

        // On: the first DM both opens AND switches to the query.
        irust_config::lookandfeel::with_test_overrides(&[("window_auto_change", "true")], || {
            let mut st = state();
            handle_message(&mut st, &parse(dm).unwrap());
            assert_eq!(st.active_buffer, BufferId::Query("stranger".to_string()));

            // A second DM to the same already-open query does not
            // re-steal focus if the user switched elsewhere meanwhile.
            st.active_buffer = BufferId::Channel("#irust-test".to_string());
            handle_message(&mut st, &parse(dm).unwrap());
            assert_eq!(st.active_buffer, BufferId::Channel("#irust-test".to_string()), "already open, not a fresh auto-create");
        });
    }

    #[test]
    fn direct_privmsg_is_always_highlighted_even_without_our_nick_in_the_text() {
        let mut st = state();
        let msg = parse(":Friend!u@h PRIVMSG abcX :hey there").unwrap();
        handle_message(&mut st, &msg);
        match st.query_buffers["friend"].lines.back().unwrap() {
            BufferLine::Chat { highlight, .. } => assert!(*highlight),
            _ => panic!("expected chat line"),
        }
    }

    #[test]
    fn ctcp_version_gets_an_automatic_notice_reply_and_no_buffer_line() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}VERSION\u{1}").unwrap();
        let out = handle_message(&mut st, &msg);
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("NOTICE Other :\u{1}VERSION irust "));
        assert!(out[0].ends_with('\u{1}'));
        assert!(
            st.query_buffers.is_empty(),
            "CTCP exchange must not create a query buffer"
        );
    }

    #[test]
    fn ctcp_ping_echoes_the_token_back() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}PING 123456789\u{1}").unwrap();
        let out = handle_message(&mut st, &msg);
        assert_eq!(out, vec!["NOTICE Other :\u{1}PING 123456789\u{1}".to_string()]);
    }

    #[test]
    fn ctcp_version_reply_is_shown_cleanly_not_as_raw_control_bytes() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":Other!u@h NOTICE abcX :\u{1}VERSION WeeChat 4.1.2\u{1}").unwrap();
        let out = handle_message(&mut st, &msg);
        assert!(out.is_empty(), "a reply to OUR ctcp needs no auto-reply of its own");
        let BufferLine::System { text, .. } = &st.status.lines[st.status.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- CTCP VERSION reply from Other: WeeChat 4.1.2");
    }

    #[test]
    fn ctcp_reply_with_no_argument_still_shows_cleanly() {
        let mut st = state();
        let msg = parse(":Other!u@h NOTICE abcX :\u{1}CLIENTINFO\u{1}").unwrap();
        handle_message(&mut st, &msg);
        let BufferLine::System { text, .. } = &st.status.lines[st.status.lines.len() - 1] else {
            unreachable!()
        };
        assert_eq!(text, "-- CTCP CLIENTINFO reply from Other");
    }

    #[test]
    fn ctcp_clientinfo_lists_the_commands_actually_answered() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}CLIENTINFO\u{1}").unwrap();
        let out = handle_message(&mut st, &msg);
        assert_eq!(
            out,
            vec!["NOTICE Other :\u{1}CLIENTINFO VERSION PING TIME\u{1}".to_string()]
        );
    }

    #[test]
    fn ctcp_time_gets_a_reply() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}TIME\u{1}").unwrap();
        let out = handle_message(&mut st, &msg);
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("NOTICE Other :\u{1}TIME "));
    }

    #[test]
    fn unrecognized_ctcp_gets_no_reply() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}THISISNOTAREALCTCP\u{1}").unwrap();
        let out = handle_message(&mut st, &msg);
        assert!(out.is_empty());
    }

    #[test]
    fn ctcp_version_and_userinfo_replies_follow_their_settings() {
        let _net = net_guard();
        let mut st = state();
        let ask = |st: &mut ClientState, cmd: &str| {
            handle_message(st, &parse(&format!(":Other!u@h PRIVMSG abcX :\x01{cmd}\x01")).unwrap())
        };
        assert_eq!(
            ask(&mut st, "VERSION"),
            vec![format!(
                "NOTICE Other :\x01VERSION irust v{} - running on {} {}\x01",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            )]
        );
        net_set("ctcp_version_reply", "custom $J");
        assert_eq!(ask(&mut st, "VERSION"), vec![format!("NOTICE Other :\x01VERSION custom {}\x01", env!("CARGO_PKG_VERSION"))]);
        net_set("ctcp_version_reply", "");
        assert!(ask(&mut st, "VERSION").is_empty(), "empty reply = silence, like irssi");
        net_set("ctcp_userinfo_reply", "just me");
        assert_eq!(ask(&mut st, "USERINFO"), vec!["NOTICE Other :\u{1}USERINFO just me\u{1}".to_string()]);
    }

    #[test]
    fn ctcp_finger_userinfo_source_are_deliberately_unanswered() {
        let _net = net_guard();
        let mut st = state();
        for cmd in ["FINGER", "USERINFO", "SOURCE"] {
            let line = format!(":Other!u@h PRIVMSG abcX :\u{1}{cmd}\u{1}");
            let msg = parse(&line).unwrap();
            assert!(
                handle_message(&mut st, &msg).is_empty(),
                "{cmd} should get no auto-reply"
            );
        }
    }

    #[test]
    fn dcc_chat_offer_is_recorded_pending_and_announced_with_no_auto_reply() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}DCC CHAT chat 3232235777 5000\u{1}").unwrap();
        let out = handle_message(&mut st, &msg);
        assert!(out.is_empty(), "a DCC offer must not get an automatic NOTICE reply");
        assert_eq!(st.pending_dcc.get("other"), Some(&("192.168.1.1".to_string(), 5000)));
        assert!(st.status.lines.back().is_some_and(|l| matches!(
            l,
            BufferLine::System { text, .. } if text.contains("Other wants to start a DCC CHAT")
        )));
    }

    #[test]
    fn self_echoed_dcc_offers_are_not_recorded_as_incoming() {
        let mut st = state();
        st.echo_message_active = true;

        let chat_echo = parse(":abcX!u@h PRIVMSG dcctestB :\u{1}DCC CHAT chat 3232235777 5000\u{1}").unwrap();
        assert!(handle_message(&mut st, &chat_echo).is_empty());
        assert!(st.pending_dcc.is_empty(), "must not record an offer from ourselves");

        let send_echo = parse(":abcX!u@h PRIVMSG dcctestB :\u{1}DCC SEND file.txt 3232235777 5000 1024\u{1}").unwrap();
        assert!(handle_message(&mut st, &send_echo).is_empty());
        assert!(
            st.pending_dcc_send.is_empty(),
            "must not record an offer from ourselves"
        );
    }

    #[test]
    fn dcc_send_offer_is_recorded_pending_and_announced_with_no_auto_reply() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}DCC SEND file.txt 3232235777 5000 1024\u{1}").unwrap();
        let out = handle_message(&mut st, &msg);
        assert!(out.is_empty(), "a DCC offer must not get an automatic NOTICE reply");
        assert_eq!(st.pending_dcc_send.len(), 1);
        let offer = st.pending_dcc_send.values().next().unwrap();
        assert_eq!(offer.peer_nick, "Other");
        assert_eq!(offer.filename, "file.txt");
        assert_eq!(offer.ip, "192.168.1.1");
        assert_eq!(offer.port, 5000);
        assert_eq!(offer.size, 1024);
        assert!(offer.incoming);
        assert!(st.status.lines.back().is_some_and(|l| matches!(
            l,
            BufferLine::System { text, .. }
                if text.contains("Other wants to send you file.txt") && text.contains(&format!("/dcc get {}", offer.id))
        )));
    }

    // The `dcc_autoget` family of settings is process-wide (same hazard
    // `irust_config::settings`'s own test-module lock documents) --
    // only the tests below that actually `/set` one of them need to
    // serialize against each other.
    static DCC_AUTOGET_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock_dcc_autoget() -> std::sync::MutexGuard<'static, ()> {
        DCC_AUTOGET_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn other_prefix() -> Prefix {
        Prefix {
            name: "Other".to_string(),
            user: Some("u".to_string()),
            host: Some("h".to_string()),
        }
    }

    #[test]
    fn dcc_autoget_masks_match_bare_nick_glob() {
        assert!(dcc_autoget_masks_match("Other AnotherNick", "Other", "Other!u@h"));
        assert!(!dcc_autoget_masks_match(
            "SomeoneElse AnotherNick",
            "Other",
            "Other!u@h"
        ));
    }

    #[test]
    fn dcc_autoget_masks_match_hostmask_glob() {
        assert!(dcc_autoget_masks_match(
            "*!*@trusted.example.com",
            "Other",
            "Other!u@trusted.example.com"
        ));
        assert!(!dcc_autoget_masks_match(
            "*!*@trusted.example.com",
            "Other",
            "Other!u@evil.example.com"
        ));
    }

    #[test]
    fn dcc_autoget_eligible_false_when_dcc_autoget_off() {
        let _guard = lock_dcc_autoget();
        // Default is off -- no `/set` at all needed for this one.
        assert!(!dcc_autoget_eligible(&other_prefix(), "abcX", "file.txt", 1024, 5000));
    }

    #[test]
    fn dcc_autoget_eligible_false_for_lowport_without_lowports_setting() {
        let _net = net_guard();
        let _guard = lock_dcc_autoget();
        let mut s = irust_config::settings::Settings::default();
        s.set("dcc_autoget", "on").unwrap();
        assert!(!dcc_autoget_eligible(&other_prefix(), "abcX", "file.txt", 1024, 80));
        s.set("dcc_autoaccept_lowports", "on").unwrap();
        assert!(dcc_autoget_eligible(&other_prefix(), "abcX", "file.txt", 1024, 80));
        s.set("dcc_autoget", "off").unwrap();
        s.set("dcc_autoaccept_lowports", "off").unwrap();
    }

    #[test]
    fn dcc_autoget_eligible_false_for_channel_target_with_no_masks_configured() {
        let _net = net_guard();
        let _guard = lock_dcc_autoget();
        let mut s = irust_config::settings::Settings::default();
        s.set("dcc_autoget", "on").unwrap();
        assert!(!dcc_autoget_eligible(
            &other_prefix(),
            "#irust-test",
            "file.txt",
            1024,
            5000
        ));
        s.set("dcc_autoget", "off").unwrap();
    }

    #[test]
    fn dcc_autoget_eligible_true_for_channel_target_when_masks_match() {
        let _net = net_guard();
        let _guard = lock_dcc_autoget();
        let mut s = irust_config::settings::Settings::default();
        s.set("dcc_autoget", "on").unwrap();
        s.set("dcc_autoget_masks", "Other").unwrap();
        assert!(dcc_autoget_eligible(
            &other_prefix(),
            "#irust-test",
            "file.txt",
            1024,
            5000
        ));
        s.set("dcc_autoget", "off").unwrap();
        s.set("dcc_autoget_masks", "").unwrap();
    }

    #[test]
    fn dcc_autoget_eligible_false_over_max_size() {
        let _net = net_guard();
        let _guard = lock_dcc_autoget();
        let mut s = irust_config::settings::Settings::default();
        s.set("dcc_autoget", "on").unwrap();
        s.set("dcc_autoget_max_size", "1000").unwrap();
        assert!(!dcc_autoget_eligible(&other_prefix(), "abcX", "file.txt", 1024, 5000));
        assert!(dcc_autoget_eligible(&other_prefix(), "abcX", "file.txt", 999, 5000));
        s.set("dcc_autoget", "off").unwrap();
        s.set("dcc_autoget_max_size", "0").unwrap();
    }

    #[test]
    fn dcc_autoget_eligible_false_for_dotfile_when_downloads_dir_is_tilde() {
        let _net = net_guard();
        let _guard = lock_dcc_autoget();
        let original_dir = irust_config::settings::dcc_downloads_dir();
        let mut s = irust_config::settings::Settings::default();
        s.set("dcc_autoget", "on").unwrap();
        s.set("dcc_downloads_dir", "~").unwrap();
        assert!(!dcc_autoget_eligible(&other_prefix(), "abcX", ".hidden", 1024, 5000));
        assert!(dcc_autoget_eligible(&other_prefix(), "abcX", "visible.txt", 1024, 5000));
        s.set("dcc_autoget", "off").unwrap();
        s.set("dcc_downloads_dir", &original_dir).unwrap();
    }

    #[test]
    fn handle_message_with_highlight_queues_auto_accept_id_when_eligible() {
        let _net = net_guard();
        let _guard = lock_dcc_autoget();
        let mut s = irust_config::settings::Settings::default();
        s.set("dcc_autoget", "on").unwrap();

        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}DCC SEND autoget.txt 3232235777 5000 1024\u{1}").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(st.pending_dcc_send.len(), 1);
        let id = *st.pending_dcc_send.keys().next().unwrap();
        assert_eq!(st.pending_auto_dcc_accept, vec![id]);

        s.set("dcc_autoget", "off").unwrap();
    }

    #[test]
    fn handle_message_with_highlight_still_pushes_manual_notice_even_when_auto_accepting() {
        let _net = net_guard();
        let _guard = lock_dcc_autoget();
        let mut s = irust_config::settings::Settings::default();
        s.set("dcc_autoget", "on").unwrap();

        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}DCC SEND autoget2.txt 3232235777 5000 1024\u{1}").unwrap();
        handle_message(&mut st, &msg);
        let offer = st.pending_dcc_send.values().next().unwrap();
        assert!(st.status.lines.back().is_some_and(|l| matches!(
            l,
            BufferLine::System { text, .. }
                if text.contains("Other wants to send you autoget2.txt") && text.contains(&format!("/dcc get {}", offer.id))
        )));
        assert!(
            !st.pending_auto_dcc_accept.is_empty(),
            "must still be queued for auto-accept too"
        );

        s.set("dcc_autoget", "off").unwrap();
    }

    #[test]
    fn dcc_passive_send_offer_is_recorded_with_port_zero_and_token() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG abcX :\u{1}DCC SEND file.txt 3232235777 0 1024 tok42\u{1}").unwrap();
        handle_message(&mut st, &msg);
        let offer = st.pending_dcc_send.values().next().unwrap();
        assert_eq!(offer.port, 0);
        assert_eq!(offer.token.as_deref(), Some("tok42"));
        assert!(offer.incoming);
    }

    #[test]
    fn dcc_resume_and_accept_requests_are_queued_with_the_sender_for_main_rs_to_drain() {
        let mut st = state();
        let resume = parse(":Other!u@h PRIVMSG abcX :\u{1}DCC RESUME file.txt 5000 4096\u{1}").unwrap();
        let out = handle_message(&mut st, &resume);
        assert!(out.is_empty());
        assert_eq!(st.pending_dcc_resume_requests.len(), 1);
        assert_eq!(st.pending_dcc_resume_requests[0].0, "Other");
        assert_eq!(st.pending_dcc_resume_requests[0].1.position, 4096);

        let accept = parse(":Other!u@h PRIVMSG abcX :\u{1}DCC ACCEPT file.txt 5000 4096\u{1}").unwrap();
        let out = handle_message(&mut st, &accept);
        assert!(out.is_empty());
        assert_eq!(st.pending_dcc_accept_replies.len(), 1);
        assert_eq!(st.pending_dcc_accept_replies[0].0, "Other");
        assert_eq!(st.pending_dcc_accept_replies[0].1.position, 4096);
    }

    #[test]
    fn dcc_reverse_offer_back_is_told_apart_from_a_fresh_incoming_offer() {
        let mut st = state();
        // We offered `movie.mkv` to `Other` passively (incoming: false,
        // port 0, a token) -- simulate that outstanding state directly.
        let id = st.next_dcc_id();
        st.pending_dcc_send.insert(
            id,
            crate::dcc_transfer::PendingSendOffer {
                id,
                peer_nick: "Other".to_string(),
                filename: "movie.mkv".to_string(),
                ip: "203.0.113.5".to_string(),
                port: 0,
                size: 999,
                token: Some("abc123".to_string()),
                incoming: false,
            },
        );

        let reverse =
            parse(":Other!u@h PRIVMSG abcX :\u{1}DCC SEND movie.mkv 3232235777 6000 999 abc123\u{1}").unwrap();
        handle_message(&mut st, &reverse);
        assert_eq!(st.pending_dcc_send.len(), 1);
        let updated = &st.pending_dcc_send[&id];
        assert_eq!(updated.port, 6000);
        assert_eq!(updated.ip, "192.168.1.1");

        // A genuinely fresh offer (different token) from the same peer
        // IS recorded as new, not mistaken for another reverse-offer.
        let fresh = parse(":Other!u@h PRIVMSG abcX :\u{1}DCC SEND other.txt 3232235777 7000 10 zzz\u{1}").unwrap();
        handle_message(&mut st, &fresh);
        assert_eq!(st.pending_dcc_send.len(), 2);
    }

    #[test]
    fn ctcp_action_renders_as_an_action_line_in_the_channel() {
        let mut st = state();
        let msg = parse(":Other!u@h PRIVMSG #irust-test :\u{1}ACTION waves\u{1}").unwrap();
        handle_message(&mut st, &msg);
        let expected_color = st.nick_colors.resolve("Other");
        assert_eq!(
            last_line(&st, "#irust-test"),
            BufferLine::Action {
                nick: "Other".to_string(),
                color_index: expected_color,
                rank: None,
                text: "waves".to_string(),
                highlight: false,
                highlight_color: None,
                timestamp: String::new(),
                time: None,
            }
        );
    }

    #[test]
    fn ctcp_action_to_us_directly_routes_into_a_query_buffer() {
        let mut st = state();
        let msg = parse(":Friend!u@h PRIVMSG abcX :\u{1}ACTION grins\u{1}").unwrap();
        handle_message(&mut st, &msg);
        let expected_color = st.nick_colors.resolve("Friend");
        assert_eq!(
            st.query_buffers["friend"].lines.back().unwrap(),
            &BufferLine::Action {
                nick: "Friend".to_string(),
                color_index: expected_color,
                rank: None,
                text: "grins".to_string(),
                highlight: true,
                highlight_color: None,
                timestamp: String::new(),
                time: None,
            }
        );
    }

    #[test]
    fn direct_privmsg_to_us_routes_into_a_query_buffer() {
        let mut st = state();
        let msg = parse(":Friend!u@h PRIVMSG abcX :hey there").unwrap();
        handle_message(&mut st, &msg);
        assert!(st.query_buffers.contains_key("friend"));
        assert_eq!(st.query_buffers["friend"].lines.len(), 1);
    }

    #[test]
    fn same_sender_keeps_the_same_color_across_messages() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h PRIVMSG #irust-test :first").unwrap());
        handle_message(&mut st, &parse(":Other!u@h PRIVMSG #irust-test :second").unwrap());
        let chan = st.channel("#irust-test").unwrap();
        let colors: Vec<usize> = chan
            .buffer
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::Chat { color_index, .. } => *color_index,
                _ => panic!("expected chat lines"),
            })
            .collect();
        assert_eq!(colors[0], colors[1]);
    }

    #[test]
    fn names_reply_populates_nicklist_with_ranks() {
        let mut st = state();
        let msg = parse(":irc.example.com 353 abcX = #irust-test :@Op +Voiced Plain").unwrap();
        handle_message(&mut st, &msg);
        let sorted = st.channel("#irust-test").unwrap().nicklist.sorted();
        let names: Vec<&str> = sorted.iter().map(|e| e.nick.as_str()).collect();
        assert_eq!(names, vec!["Op", "Voiced", "Plain"]);
    }

    #[test]
    fn others_join_adds_to_nicklist_others_part_removes() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        assert_eq!(st.channel("#irust-test").unwrap().nicklist.len(), 1);
        handle_message(&mut st, &parse(":Other!u@h PART #irust-test :bye").unwrap());
        assert!(st.channel("#irust-test").unwrap().nicklist.is_empty());
    }

    #[test]
    fn own_part_marks_not_joined_but_keeps_the_buffer() {
        let mut st = state();
        handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":abcX!u@h PART #irust-test :later").unwrap());
        let chan = st.channel("#irust-test").unwrap();
        assert!(!chan.joined);
        assert_eq!(last_line(&st, "#irust-test"), system_leave("-- abcX [u@h] has left #irust-test [later]"));
    }

    #[test]
    fn others_part_shows_has_left_with_bracketed_reason() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":Other!u@h PART #irust-test :bye").unwrap());
        assert!(st.channel("#irust-test").unwrap().nicklist.is_empty());
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_leave("-- Other [u@h] has left #irust-test [bye]")
        );
    }

    #[test]
    fn others_part_with_no_message_still_shows_an_empty_bracket() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":Other!u@h PART #irust-test").unwrap());
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_leave("-- Other [u@h] has left #irust-test []")
        );
    }

    #[test]
    fn quit_removes_only_from_channels_the_nick_was_actually_in() {
        let mut st = state();
        st.ensure_channel("#other");
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":Other!u@h QUIT :goodbye").unwrap());
        assert!(st.channel("#irust-test").unwrap().nicklist.is_empty());
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_leave("-- Other [u@h] has quit [goodbye]")
        );
        assert!(
            st.channel("#other").unwrap().buffer.lines.is_empty(),
            "must not spam a channel the nick was never in"
        );
    }

    #[test]
    fn quit_display_lines_not_collapsed_returns_one_line_per_channel() {
        let shared = vec!["#a".to_string(), "#b".to_string()];
        let lines = quit_display_lines("Other", " [u@h]", "bye", &shared, false);
        assert_eq!(
            lines,
            vec![
                ("#a".to_string(), "-- Other [u@h] has quit [bye]".to_string()),
                ("#b".to_string(), "-- Other [u@h] has quit [bye]".to_string()),
            ]
        );
    }

    #[test]
    fn quit_display_lines_collapsed_returns_one_line_noting_the_rest() {
        let shared = vec!["#a".to_string(), "#b".to_string(), "#c".to_string()];
        let lines = quit_display_lines("Other", " [u@h]", "bye", &shared, true);
        assert_eq!(
            lines,
            vec![(
                "#a".to_string(),
                "-- Other [u@h] has quit [bye] (was also in 2 more channels)".to_string()
            )]
        );
    }

    #[test]
    fn quit_display_lines_collapsed_with_only_one_channel_has_no_extra_note() {
        let shared = vec!["#a".to_string()];
        let lines = quit_display_lines("Other", "", "bye", &shared, true);
        assert_eq!(lines, vec![("#a".to_string(), "-- Other has quit [bye]".to_string())]);
    }

    #[test]
    fn quit_display_lines_with_no_shared_channels_is_empty_either_way() {
        assert!(quit_display_lines("Other", "", "bye", &[], false).is_empty());
        assert!(quit_display_lines("Other", "", "bye", &[], true).is_empty());
    }

    /// `/set show_away_once` (default on): a repeated identical `301`
    /// for the same nick is suppressed the second time.
    #[test]
    fn repeated_identical_away_notice_is_shown_only_once() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 301 abcX Other :gone fishing").unwrap(),
        );
        let before = st.status.lines.len();
        handle_message(
            &mut st,
            &parse(":irc.example.com 301 abcX Other :gone fishing").unwrap(),
        );
        assert_eq!(
            st.status.lines.len(),
            before,
            "the exact same away message again must not push a new line"
        );
    }

    /// Same setting, but the away message actually changed -- must
    /// still show.
    #[test]
    fn away_notice_shows_again_once_the_message_changes() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":irc.example.com 301 abcX Other :gone fishing").unwrap(),
        );
        handle_message(&mut st, &parse(":irc.example.com 301 abcX Other :back in 5").unwrap());
        let BufferLine::System { text, .. } = st.status.lines.back().unwrap() else {
            unreachable!()
        };
        assert_eq!(text, "-- Other is away: back in 5");
    }

    #[test]
    fn split_shaped_quit_is_buffered_not_shown_immediately() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        let before = st.channel("#irust-test").unwrap().buffer.lines.len();
        handle_message(
            &mut st,
            &parse(":Other!u@h QUIT :hub.example.net leaf.example.net").unwrap(),
        );
        assert!(
            st.channel("#irust-test").unwrap().nicklist.is_empty(),
            "must still remove from nicklist right away"
        );
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.lines.len(),
            before,
            "must not push a visible line before the flush delay"
        );
    }

    #[test]
    fn non_split_quit_still_shows_immediately() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":Other!u@h QUIT :Client Quit").unwrap());
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_leave("-- Other [u@h] has quit [Client Quit]")
        );
    }

    #[test]
    fn split_quits_flush_as_one_collapsed_line_per_channel() {
        let mut st = state();
        handle_message(&mut st, &parse(":Alice!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":Bob!u@h JOIN #irust-test").unwrap());
        handle_message(
            &mut st,
            &parse(":Alice!u@h QUIT :hub.example.net leaf.example.net").unwrap(),
        );
        handle_message(
            &mut st,
            &parse(":Bob!u@h QUIT :hub.example.net leaf.example.net").unwrap(),
        );
        std::thread::sleep(netsplit::NETSPLIT_FLUSH_DELAY + std::time::Duration::from_millis(200));
        st.flush_pending_netsplits();
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_topic("-- Netsplit hub.example.net <-> leaf.example.net quits: Alice, Bob")
        );
    }

    /// A rejoin within the recognition window shows as "netsplit over",
    /// not a plain "has joined" line -- and, same as the quit side,
    /// nicklist state (`insert_plain`/`set_userhost`) still applies
    /// right away, only the visible line is deferred/collapsed.
    #[test]
    fn split_rejoin_flushes_as_netsplit_over_not_a_plain_join() {
        let mut st = state();
        handle_message(&mut st, &parse(":Alice!u@h JOIN #irust-test").unwrap());
        handle_message(
            &mut st,
            &parse(":Alice!u@h QUIT :hub.example.net leaf.example.net").unwrap(),
        );
        handle_message(&mut st, &parse(":Alice!u@h JOIN #irust-test").unwrap());
        assert!(
            st.channel("#irust-test").unwrap().nicklist.contains("Alice"),
            "nicklist should already show Alice back, before the flush"
        );
        std::thread::sleep(netsplit::NETSPLIT_FLUSH_DELAY + std::time::Duration::from_millis(200));
        st.flush_pending_netsplits();
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_topic("-- Netsplit over, joins: Alice")
        );
    }

    #[test]
    fn others_kick_removes_from_nicklist_and_logs() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":Op!u@h KICK #irust-test Other :spamming").unwrap());
        assert!(st.channel("#irust-test").unwrap().nicklist.is_empty());
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_leave("-- Other was kicked from #irust-test by Op [spamming]")
        );
    }

    #[test]
    fn own_kick_marks_not_joined_and_logs() {
        let mut st = state();
        handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":Op!u@h KICK #irust-test abcX :bad behavior").unwrap());
        assert!(!st.channel("#irust-test").unwrap().joined);
        assert_eq!(
            last_line(&st, "#irust-test"),
            system_leave("-- you were kicked from #irust-test by Op [bad behavior]")
        );
    }

    #[test]
    fn tagmsg_is_silently_ignored_not_dumped_to_status() {
        let mut st = state();
        let before = st.status.lines.len();
        handle_message(&mut st, &parse(":jvaztap!u@h TAGMSG #libera").unwrap());
        assert_eq!(st.status.lines.len(), before, "TAGMSG must not add any line");
    }

    /// Same reasoning as `tagmsg_is_silently_ignored_not_dumped_to_status`
    /// above -- a bare `BATCH` start/end marker is IRCv3 `batch` cap
    /// bookkeeping, not content, and must not fall into the generic
    /// unhandled-command fallback either.
    #[test]
    fn batch_start_and_end_markers_are_silently_ignored() {
        let mut st = state();
        let before = st.status.lines.len();
        handle_message(&mut st, &parse(":irc.example.com BATCH +234ABC netjoin").unwrap());
        handle_message(&mut st, &parse(":irc.example.com BATCH -234ABC").unwrap());
        assert_eq!(st.status.lines.len(), before, "BATCH start/end must not add any line");
    }

    #[test]
    fn nick_change_renames_in_nicklist_and_updates_own_nick() {
        let mut st = state();
        handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":abcX!u@h NICK :abcX2").unwrap());
        assert_eq!(st.nick, "abcX2");
        assert_eq!(last_line(&st, "#irust-test"), system("-- You're now known as abcX2"));
    }

    #[test]
    fn nick_change_shows_in_status_when_thats_the_active_buffer() {
        let mut st = state();
        handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
        st.active_buffer = BufferId::Status;
        handle_message(&mut st, &parse(":abcX!u@h NICK :abcX2").unwrap());

        assert_eq!(
            st.status.lines.back().unwrap().clone(),
            system("-- You're now known as abcX2")
        );
        // The channel buffer still gets it too -- this isn't a
        // relocation, it's an addition.
        assert_eq!(last_line(&st, "#irust-test"), system("-- You're now known as abcX2"));
    }

    /// The active-buffer fallback must not double-print when the active
    /// buffer already IS a joined channel -- that channel already gets
    /// the line from the main loop.
    #[test]
    fn nick_change_does_not_double_print_when_active_buffer_is_the_channel() {
        let mut st = state();
        handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
        // state()'s active buffer is already "#irust-test" by default.
        handle_message(&mut st, &parse(":abcX!u@h NICK :abcX2").unwrap());

        let chan = st.channel("#irust-test").unwrap();
        let count = chan
            .buffer
            .lines
            .iter()
            .filter(|l| **l == system("-- You're now known as abcX2"))
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn nick_change_keeps_the_same_chat_color() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h PRIVMSG #irust-test :hi").unwrap());
        let before = match last_line(&st, "#irust-test") {
            BufferLine::Chat { color_index, .. } => color_index,
            _ => panic!("expected chat line"),
        };
        handle_message(&mut st, &parse(":Other!u@h NICK :Renamed").unwrap());
        handle_message(&mut st, &parse(":Renamed!u@h PRIVMSG #irust-test :hi again").unwrap());
        let after = match last_line(&st, "#irust-test") {
            BufferLine::Chat { color_index, .. } => color_index,
            _ => panic!("expected chat line"),
        };
        assert_eq!(before, after);
    }

    #[test]
    fn mode_op_updates_nicklist_rank() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        handle_message(&mut st, &parse(":Op!u@h MODE #irust-test +o Other").unwrap());
        let sorted = st.channel("#irust-test").unwrap().nicklist.sorted();
        assert_eq!(sorted[0].nick, "Other");
        assert_eq!(
            st.channel("#irust-test").unwrap().nicklist.prefix_char(sorted[0].rank),
            Some('@')
        );
    }

    #[test]
    fn isupport_prefix_is_learned_and_applied_to_existing_channels() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        handle_message(
            &mut st,
            &parse(":irc.example.com 005 abcX PREFIX=(ov)@+ :are supported by this server").unwrap(),
        );
        handle_message(&mut st, &parse(":Op!u@h MODE #irust-test +o Other").unwrap());
        let sorted = st.channel("#irust-test").unwrap().nicklist.sorted();
        assert_eq!(
            st.channel("#irust-test").unwrap().nicklist.prefix_char(sorted[0].rank),
            Some('@')
        );

        handle_message(&mut st, &parse(":Op!u@h MODE #irust-test +h Other").unwrap());
        let sorted = st.channel("#irust-test").unwrap().nicklist.sorted();
        assert_eq!(
            st.channel("#irust-test").unwrap().nicklist.prefix_char(sorted[0].rank),
            Some('@'),
            "h is not in this server's PREFIX, so +h must change nothing"
        );
    }

    #[test]
    fn isupport_chanmodes_is_learned_and_applied_to_existing_channels() {
        let mut st = state();
        handle_message(&mut st, &parse(":Other!u@h JOIN #irust-test").unwrap());
        handle_message(
            &mut st,
            &parse(":irc.example.com 005 abcX CHANMODES=eb,k,l,imnpst :are supported by this server").unwrap(),
        );
        // Only ONE arg for both letters -- if `I` wrongly ate it (the
        // default table's behavior), `o` would find nothing left and
        // never actually op anyone.
        handle_message(&mut st, &parse(":Op!u@h MODE #irust-test +Io Other").unwrap());
        let sorted = st.channel("#irust-test").unwrap().nicklist.sorted();
        assert_eq!(
            st.channel("#irust-test").unwrap().nicklist.prefix_char(sorted[0].rank),
            Some('@'),
            "I takes no arg on this server's CHANMODES, so the one arg must still land on o"
        );
    }

    /// `MONITOR=<n>` learns the numeric limit; a bare `MONITOR` token
    /// (no `=`) still means "supported, no advertised limit" per spec,
    /// not "unsupported" -- distinct from the token being absent
    /// entirely, which must leave `monitor_limit` `None` (the `/notify`
    /// handling in `main.rs` falls back to `ISON` polling on that).
    #[test]
    fn isupport_monitor_is_learned() {
        let mut st = state();
        assert_eq!(st.monitor_limit, None);

        handle_message(
            &mut st,
            &parse(":irc.example.com 005 abcX MONITOR=100 :are supported by this server").unwrap(),
        );
        assert_eq!(st.monitor_limit, Some(100));

        let mut st_bare = state();
        handle_message(
            &mut st_bare,
            &parse(":irc.example.com 005 abcX MONITOR :are supported by this server").unwrap(),
        );
        assert_eq!(st_bare.monitor_limit, Some(usize::MAX));

        let mut st_absent = state();
        handle_message(
            &mut st_absent,
            &parse(":irc.example.com 005 abcX CHANTYPES=# :are supported by this server").unwrap(),
        );
        assert_eq!(st_absent.monitor_limit, None);
    }

    /// `005` used to fall through to the generic numeric handler and
    /// show up as raw status-line noise -- must stay silent, same
    /// category as CAP/315/333.
    #[test]
    fn isupport_005_produces_no_visible_status_line() {
        let mut st = state();
        let before = st.status.lines.len();
        handle_message(
            &mut st,
            &parse(
                ":irc.example.com 005 abcX PREFIX=(qaohv)~&@%+ CHANMODES=beI,k,l,imnpst CHANTYPES=# \
                 :are supported by this server",
            )
            .unwrap(),
        );
        assert_eq!(
            st.status.lines.len(),
            before,
            "005 must stay silent, not fall through to the generic numeric handler"
        );
    }

    #[test]
    fn own_rejoin_clears_stale_nicklist_before_names_repopulates() {
        let mut st = state();
        handle_message(&mut st, &parse(":Stale!u@h JOIN #irust-test").unwrap());
        assert_eq!(st.channel("#irust-test").unwrap().nicklist.len(), 1);
        handle_message(&mut st, &parse(":abcX!u@h JOIN #irust-test").unwrap());
        assert!(
            st.channel("#irust-test").unwrap().nicklist.is_empty(),
            "own join must reset before NAMES"
        );
    }

    #[test]
    fn service_notice_direct_to_us_lands_in_the_active_buffer_with_no_query_open() {
        // Matches route_service_notices.pl: no query exists yet, so the
        // reply shows up wherever the user is actually looking (here, the
        // active channel), not in an auto-created query.
        let mut st = state();
        let msg = parse(":K9!service@k9.example.net NOTICE abcX :Welcome back.").unwrap();
        handle_message(&mut st, &msg);
        assert!(!st.query_buffers.contains_key("k9"));
        assert_eq!(st.channel("#irust-test").unwrap().buffer.lines.len(), 1);
    }

    #[test]
    fn service_notice_direct_to_us_reuses_an_already_open_query() {
        let mut st = state();
        st.get_or_create_query_buffer("k9", "K9");
        let msg = parse(":K9!service@k9.example.net NOTICE abcX :Welcome back.").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(st.query_buffers["k9"].lines.len(), 1);
        assert!(
            st.channel("#irust-test").unwrap().buffer.lines.is_empty(),
            "must not also land in the channel buffer"
        );
    }

    #[test]
    fn service_privmsg_direct_to_us_also_prefers_a_recent_join_over_an_open_query() {
        let mut st = state();
        st.get_or_create_query_buffer("k9", "K9");
        handle_message(&mut st, &parse(":abcX!abcX@host JOIN #newchan").unwrap());
        let before = st.channel("#newchan").unwrap().buffer.lines.len();

        let msg = parse(":K9!service@k9.example.net PRIVMSG abcX :Welcome to #newchan!").unwrap();
        handle_message(&mut st, &msg);

        assert_eq!(
            st.channel("#newchan").unwrap().buffer.lines.len(),
            before + 1,
            "PRIVMSG reply must land in the just-joined channel like a NOTICE would"
        );
        assert!(
            st.query_buffers["k9"].lines.is_empty(),
            "must not leak into the unrelated open K9 query"
        );
    }

    #[test]
    fn notice_from_a_non_service_nick_is_left_alone_in_v0_1() {
        let mut st = state();
        let msg = parse(":RandomUser!u@h NOTICE abcX :hey").unwrap();
        handle_message(&mut st, &msg);
        assert!(st.query_buffers.is_empty());
    }

    #[test]
    fn notice_channel_context_routes_a_bracketed_private_notice_into_that_channel() {
        let _guard = net_guard();
        let mut st = state();
        let before = st.channel("#irust-test").unwrap().buffer.lines.len();
        let msg = parse(":RandomUser!u@h NOTICE abcX :[#irust-test] kicked someone").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.lines.len(),
            before + 1,
            "a private NOTICE prefixed with a real channel name belongs in that channel"
        );
        assert!(st.query_buffers.is_empty(), "must not open a query for it");
    }

    #[test]
    fn notice_channel_context_off_falls_back_to_the_old_behaviour() {
        let _guard = net_guard();
        net_set("notice_channel_context", "off");
        let mut st = state();
        let before = st.channel("#irust-test").unwrap().buffer.lines.len();
        let msg = parse(":RandomUser!u@h NOTICE abcX :[#irust-test] kicked someone").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(
            st.channel("#irust-test").unwrap().buffer.lines.len(),
            before,
            "off means no redirect, same as before this setting existed"
        );
        assert_eq!(st.status.lines.len(), 1);
    }

    #[test]
    fn service_notice_ignores_notice_channel_context_entirely() {
        let msg = parse(":K9!service@k9.example.net NOTICE abcX :[#irust-test] some service text").unwrap();

        let _guard = net_guard();
        net_set("notice_channel_context", "on");
        let mut st_on = state();
        handle_message(&mut st_on, &msg);

        net_set("notice_channel_context", "off");
        let mut st_off = state();
        handle_message(&mut st_off, &msg);

        assert_eq!(
            st_on.channel("#irust-test").unwrap().buffer.lines.len(),
            st_off.channel("#irust-test").unwrap().buffer.lines.len(),
            "a service notice's own routing must not be touched by notice_channel_context"
        );
        assert!(st_on.query_buffers.is_empty() && st_off.query_buffers.is_empty());
    }

    #[test]
    fn service_notice_to_a_channel_is_not_routed_into_a_query() {
        let mut st = state();
        let msg = parse(":K9!service@k9.example.net NOTICE #irust-test :channel notice").unwrap();
        handle_message(&mut st, &msg);
        assert!(st.query_buffers.is_empty());
    }

    #[test]
    fn channel_notice_is_displayed_and_colored_like_any_other_sender() {
        let mut st = state();
        let msg = parse(":K9!service@k9.example.net NOTICE #irust-test :channel notice").unwrap();
        handle_message(&mut st, &msg);
        let expected_color = st.nick_colors.resolve("K9");
        assert_eq!(
            last_line(&st, "#irust-test"),
            BufferLine::Chat {
                nick: "K9".to_string(),
                color_index: expected_color,
                rank: None,
                text: "channel notice".to_string(),
                highlight: false,
                highlight_color: None,
                timestamp: String::new(),
                time: None,
            }
        );
    }

    #[test]
    fn statusmsg_privmsg_still_routes_into_the_real_channel() {
        let mut st = state();
        let msg = parse(":K9!service@k9.example.net PRIVMSG @#irust-test :ops only heads up").unwrap();
        handle_message(&mut st, &msg);
        assert!(st.query_buffers.is_empty(), "must not fall through to a query");
        let BufferLine::Chat { text, .. } = last_line(&st, "#irust-test") else {
            unreachable!()
        };
        assert_eq!(text, "[@] ops only heads up");
    }

    #[test]
    fn statusmsg_notice_still_routes_into_the_real_channel() {
        let mut st = state();
        let msg = parse(":K9!service@k9.example.net NOTICE +#irust-test :voiced heads up").unwrap();
        handle_message(&mut st, &msg);
        assert!(st.query_buffers.is_empty());
        let BufferLine::Chat { text, .. } = last_line(&st, "#irust-test") else {
            unreachable!()
        };
        assert_eq!(text, "[+] voiced heads up");
    }

    #[test]
    fn server_time_tag_overrides_the_displayed_timestamp() {
        let mut st = state();
        let msg = parse("@time=2026-09-01T03:30:00.000Z :K9!service@k9.example.net PRIVMSG #irust-test :hi").unwrap();
        handle_message(&mut st, &msg);
        let BufferLine::Chat { timestamp, .. } = last_line(&st, "#irust-test") else {
            unreachable!()
        };
        assert!(
            timestamp.ends_with(":30:00"),
            "got {timestamp}, expected a :30:00 tag-derived time"
        );
    }

    #[test]
    fn message_with_no_time_tag_still_falls_back_to_now() {
        let mut st = state();
        let msg = parse(":K9!service@k9.example.net PRIVMSG #irust-test :hi").unwrap();
        handle_message(&mut st, &msg);
        let BufferLine::Chat { timestamp, .. } = last_line(&st, "#irust-test") else {
            unreachable!()
        };
        assert!(!timestamp.is_empty());
    }

    #[test]
    fn server_time_tag_sets_the_lines_real_epoch_too() {
        let mut st = state();
        let msg = parse("@time=2026-09-01T03:30:00.000Z :K9!service@k9.example.net PRIVMSG #irust-test :hi").unwrap();
        handle_message(&mut st, &msg);
        let line = last_line(&st, "#irust-test");
        let expected = chrono::DateTime::parse_from_rfc3339("2026-09-01T03:30:00.000Z")
            .unwrap()
            .timestamp();
        assert_eq!(line.time(), Some(expected));
    }

    #[test]
    fn self_echoed_chat_and_action_also_get_the_real_server_time_epoch() {
        let mut st = state();
        st.echo_message_active = true;
        let expected = chrono::DateTime::parse_from_rfc3339("2026-09-01T03:30:00.000Z")
            .unwrap()
            .timestamp();

        handle_message(
            &mut st,
            &parse("@time=2026-09-01T03:30:00.000Z :abcX!u@h PRIVMSG #irust-test :hello").unwrap(),
        );
        assert_eq!(last_line(&st, "#irust-test").time(), Some(expected));

        handle_message(
            &mut st,
            &parse("@time=2026-09-01T03:30:00.000Z :abcX!u@h PRIVMSG #irust-test :\u{1}ACTION waves\u{1}").unwrap(),
        );
        assert_eq!(last_line(&st, "#irust-test").time(), Some(expected));
    }

    #[test]
    fn message_with_no_time_tag_still_gets_a_real_epoch_close_to_now() {
        let mut st = state();
        let before = crate::buffer::now_epoch();
        handle_message(
            &mut st,
            &parse(":K9!service@k9.example.net PRIVMSG #irust-test :hi").unwrap(),
        );
        let after = crate::buffer::now_epoch();
        let time = last_line(&st, "#irust-test")
            .time()
            .expect("a live line always has a real time");
        assert!((before..=after).contains(&time));
    }

    #[test]
    fn chghost_updates_the_nicklist_host_cache_for_every_shared_channel() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.insert_plain("Someone");
        chan.nicklist
            .set_userhost("Someone", Some("~old"), Some("old.host.example"));

        let msg = parse(":Someone!~old@old.host.example CHGHOST newuser new.host.example").unwrap();
        handle_message(&mut st, &msg);

        assert_eq!(
            st.channel("#irust-test").unwrap().nicklist.host_for("Someone"),
            Some("new.host.example")
        );
        assert_eq!(
            last_line(&st, "#irust-test"),
            system("-- Someone [newuser@new.host.example] has changed host")
        );
    }

    #[test]
    fn chghost_for_a_nick_not_in_any_shared_channel_is_a_no_op() {
        let mut st = state();
        let msg = parse(":Stranger!~u@old.host CHGHOST newuser new.host").unwrap();
        // Just must not panic -- nothing to assert on, no channel has
        // this nick.
        handle_message(&mut st, &msg);
    }

    #[test]
    fn account_notify_shows_a_login_line() {
        let mut st = state();
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .nicklist
            .insert_plain("Someone");
        let msg = parse(":Someone!~u@host ACCOUNT SomeAccount").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(
            last_line(&st, "#irust-test"),
            system("-- Someone is now authenticated as SomeAccount")
        );
    }

    #[test]
    fn show_account_notify_off_silences_the_login_and_logout_lines() {
        irust_config::lookandfeel::with_test_overrides(&[("show_account_notify", "off")], || {
            let mut st = state();
            st.channels.get_mut("#irust-test").unwrap().nicklist.insert_plain("Someone");
            let before = st.channels["#irust-test"].buffer.lines.len();
            handle_message(&mut st, &parse(":Someone!~u@host ACCOUNT SomeAccount").unwrap());
            handle_message(&mut st, &parse(":Someone!~u@host ACCOUNT *").unwrap());
            assert_eq!(st.channels["#irust-test"].buffer.lines.len(), before);
        });
    }

    #[test]
    fn account_notify_star_shows_a_logout_line() {
        let mut st = state();
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .nicklist
            .insert_plain("Someone");
        let msg = parse(":Someone!~u@host ACCOUNT *").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(
            last_line(&st, "#irust-test"),
            system("-- Someone is no longer authenticated")
        );
    }

    #[test]
    fn setname_shows_a_real_name_change_line() {
        let mut st = state();
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .nicklist
            .insert_plain("Someone");
        let msg = parse(":Someone!~u@host SETNAME :New Real Name").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(
            last_line(&st, "#irust-test"),
            system("-- Someone changed their real name to New Real Name")
        );
    }

    #[test]
    fn invite_notify_for_someone_else_shows_an_observer_line_in_the_channel() {
        let _net = net_guard();
        let mut st = state();
        let msg = parse(":ChatWorld!cw@example.net INVITE SomeoneElse #irust-test").unwrap();
        handle_message(&mut st, &msg);
        assert_eq!(
            last_line(&st, "#irust-test"),
            system("-- ChatWorld invited SomeoneElse to #irust-test")
        );
    }

    #[test]
    fn plain_privmsg_to_a_nick_starting_with_at_is_not_mistaken_for_statusmsg() {
        let (prefix, target) = strip_statusmsg_prefix("@notachannel".to_string());
        assert_eq!((prefix, target.as_str()), (None, "@notachannel"));
    }

    #[test]
    fn different_services_notifying_the_same_channel_get_different_colors() {
        let mut st = state();
        handle_message(
            &mut st,
            &parse(":K9!service@k9.example.net NOTICE #irust-test :from k9").unwrap(),
        );
        handle_message(
            &mut st,
            &parse(":OS!service@os.example.net NOTICE #irust-test :from os").unwrap(),
        );
        let k9_color = st.nick_colors.resolve("K9");
        let os_color = st.nick_colors.resolve("OS");
        assert_ne!(k9_color, os_color);
    }

    #[test]
    fn whois_reply_routes_to_the_buffer_active_when_the_command_was_issued() {
        let mut st = state();
        // The channel was active when `/whois` was launched -- even
        // though the user has since switched to the status window, the
        // reply must still land back in the channel.
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        st.active_buffer = BufferId::Status;

        handle_message(
            &mut st,
            &parse(":irc.example.com 311 abcX someone ~user host * :Real Name").unwrap(),
        );
        assert!(st.channels["#irust-test"]
            .buffer
            .lines
            .iter()
            .any(|l| matches!(l, BufferLine::System { text, .. } if text.contains("someone"))));
        assert!(st.status.lines.is_empty(), "must not also land in status");
        assert!(st.pending_whois.is_some(), "not the terminating numeric yet");

        handle_message(
            &mut st,
            &parse(":irc.example.com 318 abcX someone :End of /WHOIS list.").unwrap(),
        );
        assert!(st.pending_whois.is_none(), "318 clears the pending whois");
    }

    #[test]
    fn whois_330_shows_only_the_account_name_like_irssi() {
        assert_eq!(
            format_whois_lines("330", "Morticia^", &["Morticia^".to_string(), "is logged in as".to_string()]),
            vec!["-- account  : Morticia^".to_string()]
        );
    }

    #[test]
    fn whois_378_shows_a_hostname_line_like_irssi() {
        assert_eq!(
            format_whois_lines("378", "Gabiyan", &["is connecting from *@86-87-194-18.fixed.kpn.net 86.87.194.18".to_string()]),
            vec!["-- hostname : 86-87-194-18.fixed.kpn.net 86.87.194.18".to_string()]
        );
        // old `real hostname <host>` shape irssi also accepts
        assert_eq!(
            format_whois_lines("378", "x", &["real".to_string(), "hostname".to_string(), "1.2.3.4".to_string()]),
            vec!["-- hostname : 1.2.3.4".to_string()]
        );
        // no "from " -> irssi's whois_special fallback, unchanged
        assert_eq!(
            format_whois_lines("378", "x", &["something else".to_string()]),
            vec!["--          : something else".to_string()]
        );
    }

    #[test]
    fn whois_338_shows_a_hostname_line_like_irssi() {
        // ratbox/solanum (the WHOWAS shape cnircd sends)
        assert_eq!(
            format_whois_lines("338", "dana", &["203.0.113.21".to_string(), "actually using host".to_string()]),
            vec!["-- hostname : 203.0.113.21".to_string()]
        );
        // ircu
        assert_eq!(
            format_whois_lines(
                "338",
                "dana",
                &["u@h.example".to_string(), "1.2.3.4".to_string(), "Actual user@host, actual IP".to_string()]
            ),
            vec!["-- hostname : u@h.example 1.2.3.4".to_string()]
        );
    }

    #[test]
    fn whois_311_produces_a_nick_host_line_and_a_labeled_ircname_line() {
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 311 abcX someone ~user host.example.com * :Real Name").unwrap(),
        );
        let lines: Vec<String> = st.channels["#irust-test"]
            .buffer
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(
            lines,
            vec![
                format!(
                    "-- {} {}",
                    mirc_bold("someone"),
                    mirc_gray_brackets("~user@host.example.com")
                ),
                "-- ircname  : Real Name".to_string(),
            ]
        );
    }

    #[test]
    fn whois_313_strips_the_is_an_prefix_and_uses_a_blank_label() {
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 313 abcX someone :is an IRC Administrator").unwrap(),
        );
        let text = match &st.channels["#irust-test"].buffer.lines[0] {
            BufferLine::System { text, .. } => text.clone(),
            _ => panic!("expected a system line"),
        };
        assert_eq!(text, format!("--          : {}", mirc_hilight("IRC Administrator")));
    }

    #[test]
    fn whois_lines_use_the_nicks_case_from_the_wire_not_the_lowercased_pending_key() {
        let mut st = state();
        // `/whois k9` types it lowercase; `pending_whois` tracks it
        // lowercased for matching, but the server always echoes back
        // "K9" -- that's what must end up on screen.
        st.pending_whois = Some(("k9".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 311 abcX K9 K9 k9.example.net * :K9 Channel Services").unwrap(),
        );
        let text = match &st.channels["#irust-test"].buffer.lines[0] {
            BufferLine::System { text, .. } => text.clone(),
            _ => panic!("expected a system line"),
        };
        assert!(text.contains("K9"), "must show the wire's case, got: {text}");
        assert!(!text.contains("k9 ["), "must not show the lowercased pending_whois key");
    }

    #[test]
    fn whois_319_gets_a_channels_label() {
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 319 abcX someone :#brazil +#chatnet @#doghouse").unwrap(),
        );
        let text = match &st.channels["#irust-test"].buffer.lines[0] {
            BufferLine::System { text, .. } => text.clone(),
            _ => panic!("expected a system line"),
        };
        assert_eq!(text, "-- channels : #brazil +#chatnet @#doghouse");
    }

    #[test]
    fn whois_318_ends_with_a_plain_end_of_whois_line_and_clears_pending() {
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 318 abcX someone :End of /WHOIS list.").unwrap(),
        );
        let text = match &st.channels["#irust-test"].buffer.lines[0] {
            BufferLine::System { text, .. } => text.clone(),
            _ => panic!("expected a system line"),
        };
        assert_eq!(text, "-- End of WHOIS");
        assert!(st.pending_whois.is_none());
    }

    #[test]
    fn auto_whowas_off_does_not_retry_a_401_as_whowas() {
        let _net = net_guard();
        net_set("auto_whowas", "off");
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        let out = handle_message(
            &mut st,
            &parse(":irc.example.com 401 abcX someone :No such nick/channel").unwrap(),
        );
        assert!(!out.iter().any(|l| l.starts_with("WHOWAS")));
    }

    #[test]
    fn whois_401_auto_retries_as_whowas_and_keeps_pending_whois_alive() {
        let _net = net_guard();
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        let out = handle_message(
            &mut st,
            &parse(":irc.example.com 401 abcX someone :No such nick/channel").unwrap(),
        );
        assert_eq!(out, vec!["WHOWAS someone 1".to_string()]);
        assert!(st.pending_whois.is_some(), "still waiting on the whowas reply");
        assert!(
            st.channels["#irust-test"].buffer.lines.is_empty(),
            "irssi shows nothing for the 401 itself when auto_whowas kicks in"
        );
    }

    #[test]
    fn a_318_arriving_during_an_auto_whowas_retry_is_swallowed_not_treated_as_the_end() {
        let _net = net_guard();
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 401 abcX someone :No such nick").unwrap(),
        );
        assert!(st.auto_whowas_pending);

        // The leftover terminator for the original (failed) WHOIS --
        // must not clear `pending_whois`, and must not show anything.
        handle_message(
            &mut st,
            &parse(":irc.example.com 318 abcX someone :End of /WHOIS list.").unwrap(),
        );
        assert!(
            st.pending_whois.is_some(),
            "318 during an auto-whowas retry must not end it early"
        );
        assert!(
            st.channels["#irust-test"].buffer.lines.is_empty(),
            "must not display the stray 318"
        );

        handle_message(
            &mut st,
            &parse(":irc.example.com 314 abcX someone ~user host.example.com * :Real Name").unwrap(),
        );
        assert!(
            st.channels["#irust-test"].buffer.lines.iter().any(|l| matches!(
                l,
                BufferLine::System { text, .. } if text.contains("Real Name")
            )),
            "the 314 must still land in the right buffer"
        );

        handle_message(
            &mut st,
            &parse(":irc.example.com 369 abcX someone :End of WHOWAS").unwrap(),
        );
        assert!(st.pending_whois.is_none(), "369 is the real end");
        assert!(!st.auto_whowas_pending);
    }

    #[test]
    fn whowas_314_and_369_route_to_the_same_buffer_and_369_clears_pending() {
        let _net = net_guard();
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 401 abcX someone :No such nick").unwrap(),
        );
        handle_message(
            &mut st,
            &parse(":irc.example.com 314 abcX someone ~user host.example.com * :Real Name").unwrap(),
        );
        assert!(st.pending_whois.is_some());
        let lines: Vec<String> = st.channels["#irust-test"]
            .buffer
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.clone(),
                _ => String::new(),
            })
            .collect();
        assert_eq!(
            lines,
            vec![
                format!(
                    "-- {} {}",
                    mirc_bold("someone"),
                    mirc_gray_brackets("~user@host.example.com")
                ),
                "-- was      : Real Name".to_string(),
            ]
        );

        handle_message(
            &mut st,
            &parse(":irc.example.com 369 abcX someone :End of WHOWAS").unwrap(),
        );
        assert!(st.pending_whois.is_none());
    }

    #[test]
    fn whowas_406_reports_never_existed_and_clears_pending() {
        let _net = net_guard();
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 401 abcX someone :No such nick").unwrap(),
        );
        handle_message(
            &mut st,
            &parse(":irc.example.com 406 abcX someone :There was no such nickname").unwrap(),
        );
        let text = match &st.channels["#irust-test"].buffer.lines[0] {
            BufferLine::System { text, .. } => text.clone(),
            _ => panic!("expected a system line"),
        };
        assert_eq!(text, "-- There was no such nickname someone");
        assert!(st.pending_whois.is_none());
    }

    #[test]
    fn an_unrecognized_whois_numeric_falls_back_to_a_blank_label_special_line() {
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 999 abcX someone :some future extension").unwrap(),
        );
        let text = match &st.channels["#irust-test"].buffer.lines[0] {
            BufferLine::System { text, .. } => text.clone(),
            _ => panic!("expected a system line"),
        };
        assert_eq!(text, "--          : some future extension");
    }

    #[test]
    fn whois_671_trims_the_bracketed_cipher_detail() {
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 671 abcX someone :is using a secure connection [TLSv1.3, TLS_AES_256_GCM_SHA384]")
                .unwrap(),
        );
        let text = match &st.channels["#irust-test"].buffer.lines[0] {
            BufferLine::System { text, .. } => text.clone(),
            _ => panic!("expected a system line"),
        };
        assert_eq!(text, "--          : is using a secure connection");
    }

    #[test]
    fn whois_671_with_no_bracket_is_left_untrimmed() {
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        handle_message(
            &mut st,
            &parse(":irc.example.com 671 abcX someone :is using a secure connection").unwrap(),
        );
        let text = match &st.channels["#irust-test"].buffer.lines[0] {
            BufferLine::System { text, .. } => text.clone(),
            _ => panic!("expected a system line"),
        };
        assert_eq!(text, "--          : is using a secure connection");
    }

    #[test]
    fn a_numeric_reply_for_a_different_nick_than_the_pending_whois_falls_through_to_status() {
        let _net = net_guard();
        let mut st = state();
        st.pending_whois = Some(("someone".to_string(), BufferId::Channel("#irust-test".to_string())));
        // Some unrelated numeric that happens to arrive while a whois is
        // pending -- must not be misrouted just because a whois is in
        // flight.
        handle_message(&mut st, &parse(":irc.example.com 372 abcX :- motd line").unwrap());
        assert!(!st.status.lines.is_empty());
        assert!(st.channels["#irust-test"].buffer.lines.is_empty());
    }

    #[test]
    fn old_server_time_counts_as_replay_but_fresh_or_missing_does_not() {
        let old = parse("@time=2020-01-01T00:00:00.000Z :a!b@c PRIVMSG #x :hi").unwrap();
        assert!(is_replayed_message(&old));
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S.000Z").to_string();
        let fresh = parse(&format!("@time={now} :a!b@c PRIVMSG #x :hi")).unwrap();
        assert!(!is_replayed_message(&fresh));
        let untagged = parse(":a!b@c PRIVMSG #x :hi").unwrap();
        assert!(!is_replayed_message(&untagged));
    }

    /// A channel we `/part`ed on purpose must NOT come back at the next
    /// reconnect (found live: #ChatWorld/#K9 returned every day).
    #[test]
    fn welcome_does_not_rejoin_a_channel_we_parted() {
        let _net = net_guard();
        let mut st = state();
        st.ensure_channel("#keep");
        st.ensure_channel("#gone");
        handle_message(&mut st, &parse(":abcX!u@h JOIN #keep").unwrap());
        handle_message(&mut st, &parse(":abcX!u@h JOIN #gone").unwrap());
        handle_message(&mut st, &parse(":abcX!u@h PART #gone :bye").unwrap());
        // connection drops: every channel goes joined=false
        for c in st.channels.values_mut() {
            c.joined = false;
        }
        let out = handle_message(&mut st, &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        assert!(out.contains(&"JOIN #keep".to_string()), "{out:?}");
        assert!(!out.iter().any(|l| l.contains("#gone")), "{out:?}");
        // a later manual rejoin makes it eligible again
        handle_message(&mut st, &parse(":abcX!u@h JOIN #gone").unwrap());
        let out = handle_message(&mut st, &parse(":irc.example.com 001 abcX :Welcome").unwrap());
        assert!(out.contains(&"JOIN #gone".to_string()), "{out:?}");
    }
}
