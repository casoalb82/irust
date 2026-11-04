use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::buffer::Buffer;
use crate::dcc_transfer;
use crate::nickcolor::NickColors;
use crate::nicklist::{ChanModesTable, Nicklist, PrefixTable};

/// Anything in a buffer's display name that isn't alphanumeric/`-`/`_`/
/// `#` becomes `_`, so it's always safe to use in a filename.
fn sanitize_buffer_name(buffer_name: &str) -> String {
    buffer_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '#') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn is_autolog_ignored(ignore_targets: &str, target: &str) -> bool {
    ignore_targets.split(',').any(|t| t.trim().eq_ignore_ascii_case(target))
}

/// `autolog` / `autolog_only_saved_channels`: may a freshly created
/// buffer for `target` start logging by itself? Channels are matched by
/// the `#&+!` prefix; queries and status are never "saved channels".
fn autolog_allowed(dir: &Path, target: &str) -> bool {
    use irust_config::settings_history_log as cfg;
    if !cfg::get_bool("autolog") {
        return false;
    }
    if target.starts_with(['#', '&', '+', '!']) && cfg::get_bool("autolog_only_saved_channels") {
        let tag = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        return irust_config::channel_setup::load_channel_setup(tag, target).is_some();
    }
    true
}

fn log_file_path(dir: &Path, buffer_name: &str, date: chrono::NaiveDate) -> PathBuf {
    // `autolog_path`: when set, irssi's template ($tag, $0, strftime) picks
    // the file; empty keeps irust's own `<logs dir>/<name>-<date>.log`.
    let template = irust_config::settings_history_log::get_str("autolog_path");
    if !template.is_empty() {
        let tag = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        return crate::log_format::expand_autolog_path(&template, tag, &sanitize_buffer_name(buffer_name), date);
    }
    dir.join(format!(
        "{}-{}.log",
        sanitize_buffer_name(buffer_name),
        date.format("%Y-%m-%d")
    ))
}

pub struct ChannelState {
    pub display_name: String,
    pub buffer: Buffer,
    pub nicklist: Nicklist,
    pub joined: bool,
    /// The channel's ban list as last reported by RPL_BANLIST (367) --
    /// cleared right before a fresh `/ban` (no mask) query goes out, so a
    /// repeat query doesn't just keep appending stale entries. What
    /// `/unban -first`/`-last`/`<id>` resolve their target against.
    pub bans: Vec<String>,
    pub topic: Option<String>,
    pub key: Option<String>,
    pub join_started_at: Option<std::time::Instant>,
    pub modestring: String,
    /// Nicks that recently spoke or joined, newest first -- irssi's
    /// per-channel `lastmsgs`, the ordering source for nick tab-completion.
    pub last_msgs: Vec<crate::nick_completion::LastMsg>,
}

impl ChannelState {
    fn new(display_name: &str) -> Self {
        Self {
            display_name: display_name.to_string(),
            buffer: Buffer::new(display_name.to_string()),
            nicklist: Nicklist::default(),
            joined: false,
            bans: Vec::new(),
            topic: None,
            key: None,
            join_started_at: None,
            modestring: String::new(),
            last_msgs: Vec::new(),
        }
    }
}

pub fn channel_completion_candidates(channels: &HashMap<String, ChannelState>, prefix: &str) -> Vec<String> {
    let prefix = prefix.to_lowercase();
    let mut names: Vec<String> = channels
        .values()
        .map(|c| c.display_name.clone())
        .filter(|n| n.to_lowercase().starts_with(&prefix))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Which buffer the UI is currently showing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BufferId {
    /// The single always-present status window (server MOTD/numerics,
    /// untargeted NOTICEs, anything else that isn't part of a
    /// channel/query conversation) -- matches irssi's status window.
    Status,
    /// Lowercased key into `ClientState::channels`.
    Channel(String),
    /// Lowercased key into `ClientState::query_buffers`.
    Query(String),
    Empty(u64),
}

/// One non-focused pane in a `/split` view -- see
/// `ClientState::split_panes`'s own doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitPane {
    pub buffer: BufferId,
    pub scroll_offset: usize,
}

/// See `ClientState::split_orientation`'s own doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitOrientation {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone)]
pub struct PendingAutoban {
    pub channel: String,
    pub nick: String,
    pub mask: Option<String>,
    pub reason: String,
}

/// See `ClientState::pending_ip`'s own doc comment.
#[derive(Debug, Clone)]
pub struct PendingIp {
    pub nick: String,
    pub echo_buffer: BufferId,
    pub host: Option<String>,
    pub start_len: usize,
}

/// One row of `ClientState::buffer_summaries()` -- everything a
/// buffer-list UI needs to render one entry.
#[derive(Clone)]
pub struct BufferSummary {
    pub id: BufferId,
    pub display_name: String,
    pub real_name: String,
    pub has_unread: bool,
    /// See `Buffer::has_unread_message`'s own doc comment -- the
    /// "someone actually said something" tier, distinct from a plain
    /// system-only `has_unread`.
    pub has_unread_message: bool,
    /// See `Buffer::has_unread_join`'s own doc comment.
    pub has_unread_join: bool,
    pub has_unread_highlight: bool,
    /// See `Buffer::highlight_actcolor`'s own doc comment -- `None`
    /// means "fall back to the fixed default" (either no highlight, or
    /// the entry that fired had no `-actcolor`), not "no highlight."
    pub highlight_actcolor: Option<String>,
    pub own_rank: Option<char>,
}

pub struct ClientState {
    pub nick: String,
    /// The status window's own scrollback -- always exists, never closable.
    pub status: Buffer,
    /// Keyed by lowercased channel name.
    pub channels: HashMap<String, ChannelState>,
    /// Keyed by lowercased nick.
    pub query_buffers: HashMap<String, Buffer>,
    pub nick_colors: NickColors,
    pub nicklist_colors: bool,
    pub text_colors: bool,
    pub active_buffer: BufferId,
    pub active_scroll_offset: usize,
    pub split_panes: Vec<SplitPane>,
    pub split_orientation: SplitOrientation,
    pub connected: bool,
    pub nick_registration_retries: u8,
    /// `true` once the end of this connection's MOTD (376/422) has been
    /// seen -- irssi's `server->motd_got`. `skip_motd` hides the MOTD
    /// body only until then, so a later `/motd` still prints. Reset by
    /// `001` on every (re)connect.
    pub motd_got: bool,
    /// Channels we were invited to (lowercased key = the invite's channel,
    /// original spelling kept) that irssi would auto-join: not currently
    /// joined and `join_auto_chans_on_invite` on. `main.rs` drains this and
    /// joins those with an autojoin `/channel` entry for the network -- the
    /// setup lookup needs the network tag, which this crate doesn't have.
    pub pending_invite_autojoin: Vec<String>,
    /// The `opermode` setting's mode string, queued once by `client.rs`
    /// on `381` (`RPL_YOUREOPER`) when it's non-empty -- `main.rs`
    /// drains this and sends `MODE <ournick> <modestring>`, same reason
    /// as `pending_invite_autojoin`: this crate has no socket to write
    /// to directly.
    pub pending_opermode: Option<String>,
    /// Channels waiting to be retried after a `437` (see `rejoin.rs`);
    /// `main.rs` runs `rejoin::tick` on this every `REJOIN_INTERVAL`.
    /// `/rmrejoins` empties it.
    pub rejoin_channels: Vec<crate::rejoin::Rejoin>,
    /// Our own user modes (e.g. "iw"), tracked from self-targeting MODE
    /// lines and `RPL_UMODEIS` -- shown in the buflist bar's nick label,
    /// matching irssi's `{sbmode $usermode}`.
    pub own_modes: String,
    pub own_userhost: Option<String>,
    pub server_version: Option<String>,
    /// Set/cleared by `/away`, or by the silent self-`/whois` on
    /// connect syncing it from whatever ZNC's own persistent
    /// connection already had -- matches irssi's `{sbaway $A}`
    /// indicator.
    pub away: bool,
    pub away_reason: Option<String>,
    pub last_privmsg_from: Option<String>,
    pub last_sent_msg: Option<String>,
    /// `$;` -- the last nick to send a public message to a channel
    /// we're on (irssi's `last_public_from`, "message public" signal).
    /// Set in `client.rs`'s `PRIVMSG` handling, the channel branch (same
    /// not-us guarantee as `last_privmsg_from` above).
    pub last_public_from: Option<String>,
    pub last_join: Option<String>,
    pub last_invite: Option<String>,
    /// Targets (lowercased) of user-typed `/who` queries still awaiting
    /// their `315`. Only while a target is queued do `352` rows print
    /// (irssi shows a manual `/who` but keeps its own post-join seeding
    /// `WHO` silent, via server redirects). Pushed in `main.rs`'s
    /// `ServerCommand` handling, popped in `client.rs`'s `315`.
    pub manual_who_pending: Vec<String>,
    /// Nicks that recently messaged us (or that we messaged), newest
    /// first -- irssi's per-server `lastmsgs`, what `/msg <Tab>` cycles.
    pub private_last_msgs: Vec<crate::nick_completion::LastMsg>,
    /// Per-sender message counters behind `flood_max_msgs`/`flood_timecheck`.
    pub flood: crate::flood::FloodTracker,
    /// Ignores added automatically by flood detection (`autoignore_level`),
    /// consulted alongside the configured `/ignore` list until they expire.
    pub auto_ignores: Vec<irust_config::IgnoreEntry>,
    pub self_whois_pending: bool,
    /// Round-trip time to the server in milliseconds, from `main.rs`'s
    /// periodic `PING`/`PONG` measurement -- matches irssi's `{lag}`.
    /// `None` until the first measurement completes.
    pub lag_ms: Option<u64>,
    pub pending_whois: Option<(String, BufferId)>,
    pub pending_join_notices: std::collections::VecDeque<(String, std::time::Instant)>,
    pub auto_whowas_pending: bool,
    pub pending_ip: Option<PendingIp>,
    /// An incoming, not-yet-accepted `DCC CHAT` offer per sender
    /// (lowercased nick -> their advertised IP/port from the CTCP) --
    /// `/dcc chat <nick>` accepts it if present, otherwise sends a fresh
    /// offer instead. Cleared once accepted (or replaced by a newer
    /// offer from the same nick).
    pub pending_dcc: HashMap<String, (String, u16)>,
    pub pending_dcc_send: HashMap<u64, dcc_transfer::PendingSendOffer>,
    /// Mints the id every `PendingSendOffer` and active transfer is
    /// keyed by -- see `next_dcc_id()`.
    pub next_dcc_id: u64,
    pub pending_dcc_resume_requests: Vec<(String, dcc_transfer::DccResumeRequest)>,
    /// `DCC ACCEPT` replies received off the wire, drained the same way
    /// -- resolves whichever `DccResumeWaiters` entry (in `main.rs`) is
    /// blocked on the matching port/token.
    pub pending_dcc_accept_replies: Vec<(String, dcc_transfer::DccAcceptReply)>,
    pub pending_auto_dcc_accept: Vec<u64>,
    pub pending_autoban: Vec<PendingAutoban>,
    pub bouncer_login: Option<String>,
    pending_join_keys: HashMap<String, String>,
    logs_dir: Option<PathBuf>,
    pub prefix_table: PrefixTable,
    /// The server's real `CHANMODES=` scheme, mirroring `prefix_table`
    /// in every way -- see its own doc comment.
    pub chanmodes_table: ChanModesTable,
    pub echo_message_active: bool,
    pub setname_active: bool,
    pub monitor_limit: Option<usize>,
    pub idchan: Vec<(char, usize)>,
    recent_split_quits: HashMap<String, (String, std::time::Instant)>,
    pending_split_quits: HashMap<(String, String), (std::time::Instant, Vec<String>)>,
    /// Same idea, for the "Netsplit over ...: joins: ..." collapsed
    /// line.
    pending_split_joins: HashMap<(String, String), (std::time::Instant, Vec<String>)>,
    last_away_shown: HashMap<String, String>,
    window_item_groups: Vec<Vec<BufferId>>,
    pub empty_windows: HashMap<u64, Buffer>,
    /// Monotonic id source for `empty_windows` -- never reused even
    /// after the window closes, same "no id collision, ever" principle
    /// `next_dcc_id` already follows.
    next_empty_window_id: u64,
    pub immortal: std::collections::HashSet<BufferId>,
    pub sticky: std::collections::HashSet<BufferId>,
}

impl ClientState {
    /// `palette_len` comes from whatever palette the UI layer defines
    /// (`irust-tui::color::len()`) -- this crate stays UI-agnostic and
    /// only ever deals in indices into that palette.
    pub fn new(nick: impl Into<String>, initial_channel: impl Into<String>, palette_len: usize) -> Self {
        Self::with_color_overrides(nick, initial_channel, palette_len, HashMap::new())
    }

    pub fn with_color_overrides(
        nick: impl Into<String>,
        initial_channel: impl Into<String>,
        palette_len: usize,
        color_overrides: HashMap<String, usize>,
    ) -> Self {
        let nick = nick.into();
        let initial_channel = initial_channel.into();
        let key = initial_channel.to_lowercase();
        let mut channels = HashMap::new();
        channels.insert(key.clone(), ChannelState::new(&initial_channel));
        Self {
            nick,
            status: Buffer::new("status".to_string()),
            channels,
            query_buffers: HashMap::new(),
            nick_colors: NickColors::with_overrides(palette_len, color_overrides),
            nicklist_colors: false,
            text_colors: irust_config::lookandfeel::get_bool("colors"),
            active_buffer: BufferId::Channel(key),
            active_scroll_offset: 0,
            split_panes: Vec::new(),
            split_orientation: SplitOrientation::default(),
            connected: false,
            nick_registration_retries: 0,
            motd_got: false,
            pending_invite_autojoin: Vec::new(),
            pending_opermode: None,
            rejoin_channels: Vec::new(),
            own_modes: String::new(),
            own_userhost: None,
            server_version: None,
            away: false,
            away_reason: None,
            last_privmsg_from: None,
            last_sent_msg: None,
            last_public_from: None,
            last_join: None,
            last_invite: None,
            manual_who_pending: Vec::new(),
            private_last_msgs: Vec::new(),
            flood: Default::default(),
            auto_ignores: Vec::new(),
            self_whois_pending: false,
            lag_ms: None,
            pending_whois: None,
            pending_join_notices: std::collections::VecDeque::new(),
            auto_whowas_pending: false,
            pending_ip: None,
            pending_dcc: HashMap::new(),
            pending_dcc_send: HashMap::new(),
            next_dcc_id: 0,
            pending_dcc_resume_requests: Vec::new(),
            pending_dcc_accept_replies: Vec::new(),
            pending_auto_dcc_accept: Vec::new(),
            pending_autoban: Vec::new(),
            bouncer_login: None,
            pending_join_keys: HashMap::new(),
            logs_dir: None,
            prefix_table: PrefixTable::default(),
            chanmodes_table: ChanModesTable::default(),
            echo_message_active: false,
            setname_active: false,
            monitor_limit: None,
            idchan: Vec::new(),
            recent_split_quits: HashMap::new(),
            pending_split_quits: HashMap::new(),
            pending_split_joins: HashMap::new(),
            last_away_shown: HashMap::new(),
            window_item_groups: Vec::new(),
            empty_windows: HashMap::new(),
            next_empty_window_id: 1,
            immortal: std::collections::HashSet::new(),
            sticky: std::collections::HashSet::new(),
        }
    }

    pub fn without_initial_channel(
        nick: impl Into<String>,
        palette_len: usize,
        color_overrides: HashMap<String, usize>,
    ) -> Self {
        Self {
            nick: nick.into(),
            status: Buffer::new("status".to_string()),
            channels: HashMap::new(),
            query_buffers: HashMap::new(),
            nick_colors: NickColors::with_overrides(palette_len, color_overrides),
            nicklist_colors: false,
            text_colors: irust_config::lookandfeel::get_bool("colors"),
            active_buffer: BufferId::Status,
            active_scroll_offset: 0,
            split_panes: Vec::new(),
            split_orientation: SplitOrientation::default(),
            connected: false,
            nick_registration_retries: 0,
            motd_got: false,
            pending_invite_autojoin: Vec::new(),
            pending_opermode: None,
            rejoin_channels: Vec::new(),
            own_modes: String::new(),
            own_userhost: None,
            server_version: None,
            away: false,
            away_reason: None,
            last_privmsg_from: None,
            last_sent_msg: None,
            last_public_from: None,
            last_join: None,
            last_invite: None,
            manual_who_pending: Vec::new(),
            private_last_msgs: Vec::new(),
            flood: Default::default(),
            auto_ignores: Vec::new(),
            self_whois_pending: false,
            lag_ms: None,
            pending_whois: None,
            pending_join_notices: std::collections::VecDeque::new(),
            auto_whowas_pending: false,
            pending_ip: None,
            pending_dcc: HashMap::new(),
            pending_dcc_send: HashMap::new(),
            next_dcc_id: 0,
            pending_dcc_resume_requests: Vec::new(),
            pending_dcc_accept_replies: Vec::new(),
            pending_auto_dcc_accept: Vec::new(),
            pending_autoban: Vec::new(),
            bouncer_login: None,
            pending_join_keys: HashMap::new(),
            logs_dir: None,
            prefix_table: PrefixTable::default(),
            chanmodes_table: ChanModesTable::default(),
            echo_message_active: false,
            setname_active: false,
            monitor_limit: None,
            idchan: Vec::new(),
            recent_split_quits: HashMap::new(),
            pending_split_quits: HashMap::new(),
            pending_split_joins: HashMap::new(),
            last_away_shown: HashMap::new(),
            window_item_groups: Vec::new(),
            empty_windows: HashMap::new(),
            next_empty_window_id: 1,
            immortal: std::collections::HashSet::new(),
            sticky: std::collections::HashSet::new(),
        }
    }

    pub fn next_dcc_id(&mut self) -> u64 {
        self.next_dcc_id += 1;
        self.next_dcc_id
    }

    pub fn is_self(&self, name: &str) -> bool {
        self.nick.eq_ignore_ascii_case(name)
            || self
                .bouncer_login
                .as_deref()
                .is_some_and(|login| login.eq_ignore_ascii_case(name))
    }

    pub fn enable_logging(&mut self, dir: PathBuf) {
        let today = crate::today();
        if autolog_allowed(&dir, &self.status.name) {
            self.status.set_log_path(log_file_path(&dir, &self.status.name, today));
        }
        for chan in self.channels.values_mut() {
            if autolog_allowed(&dir, &chan.buffer.name) {
                chan.buffer.set_log_path(log_file_path(&dir, &chan.buffer.name, today));
            }
        }
        for buf in self.query_buffers.values_mut() {
            if autolog_allowed(&dir, &buf.name) {
                buf.set_log_path(log_file_path(&dir, &buf.name, today));
            }
        }
        self.logs_dir = Some(dir);
    }

    pub fn log_path_for(&self, buffer_name: &str) -> Option<PathBuf> {
        let dir = self.logs_dir.clone()?;
        Some(log_file_path(&dir, buffer_name, crate::today()))
    }

    pub fn rotate_logs(&mut self, date: chrono::NaiveDate) {
        let Some(dir) = self.logs_dir.clone() else { return };
        if !self.status.logging_stopped() {
            self.status.set_log_path(log_file_path(&dir, &self.status.name, date));
        }
        for chan in self.channels.values_mut() {
            if !chan.buffer.logging_stopped() {
                chan.buffer.set_log_path(log_file_path(&dir, &chan.buffer.name, date));
            }
        }
        for buf in self.query_buffers.values_mut() {
            if !buf.logging_stopped() {
                buf.set_log_path(log_file_path(&dir, &buf.name, date));
            }
        }
    }

    pub fn push_daychange_to_all_buffers(&mut self, date: chrono::NaiveDate) {
        let text = format!("-- Day changed to {}", date.format("%d %b %Y"));
        self.status.push_system_daychange(text.clone());
        for chan in self.channels.values_mut() {
            chan.buffer.push_system_daychange(text.clone());
        }
        for buf in self.query_buffers.values_mut() {
            buf.push_system_daychange(text.clone());
        }
    }

    pub fn should_show_away(&mut self, nick: &str, message: &str) -> bool {
        if !irust_config::settings::SHOW_AWAY_ONCE.load(std::sync::atomic::Ordering::Relaxed) {
            return true;
        }
        let key = nick.to_lowercase();
        if self.last_away_shown.get(&key).map(String::as_str) == Some(message) {
            return false;
        }
        self.last_away_shown.insert(key, message.to_string());
        true
    }

    pub fn record_split_quit(&mut self, nick: &str, reason: &str) {
        let now = std::time::Instant::now();
        self.recent_split_quits
            .insert(nick.to_lowercase(), (reason.to_string(), now));
        // Two passes -- collect which channels actually had this nick
        // first (immutable), then mutate each one -- rather than
        // fighting the borrow checker over holding a `channels`
        // iterator mutably while also touching `pending_split_quits`.
        let affected: Vec<String> = self
            .channels
            .iter()
            .filter(|(_, chan)| chan.nicklist.contains(nick))
            .map(|(name, _)| name.clone())
            .collect();
        for chan_name in affected {
            if let Some(chan) = self.channels.get_mut(&chan_name) {
                chan.nicklist.remove(nick);
            }
            let entry = self
                .pending_split_quits
                .entry((chan_name, reason.to_string()))
                .or_insert_with(|| (now, Vec::new()));
            entry.1.push(nick.to_string());
        }
    }

    pub fn record_split_join_if_recent(&mut self, chan_name: &str, nick: &str) -> bool {
        let Some((reason, quit_at)) = self.recent_split_quits.get(&nick.to_lowercase()) else {
            return false;
        };
        if quit_at.elapsed() > crate::netsplit::NETSPLIT_REJOIN_WINDOW {
            return false;
        }
        let reason = reason.clone();
        let now = std::time::Instant::now();
        let entry = self
            .pending_split_joins
            .entry((chan_name.to_lowercase(), reason))
            .or_insert_with(|| (now, Vec::new()));
        entry.1.push(nick.to_string());
        true
    }

    pub fn flush_pending_netsplits(&mut self) -> bool {
        let mut flushed = false;
        let ready: Vec<(String, String)> = self
            .pending_split_quits
            .iter()
            .filter(|(_, (first_seen, _))| first_seen.elapsed() >= crate::netsplit::NETSPLIT_FLUSH_DELAY)
            .map(|(key, _)| key.clone())
            .collect();
        for key in ready {
            if let Some((_, nicks)) = self.pending_split_quits.remove(&key) {
                let (chan_name, reason) = key;
                let (server_a, server_b) = reason.split_once(' ').unwrap_or((&reason, ""));
                let line = format!(
                    "-- Netsplit {server_a} <-> {server_b} quits: {}",
                    crate::netsplit::format_nick_list(&nicks, crate::netsplit::Side::Quits)
                );
                if let Some(chan) = self.channels.get_mut(&chan_name) {
                    chan.buffer.push_system_topic(line);
                    flushed = true;
                }
            }
        }
        let ready: Vec<(String, String)> = self
            .pending_split_joins
            .iter()
            .filter(|(_, (first_seen, _))| first_seen.elapsed() >= crate::netsplit::NETSPLIT_FLUSH_DELAY)
            .map(|(key, _)| key.clone())
            .collect();
        for key in ready {
            if let Some((_, nicks)) = self.pending_split_joins.remove(&key) {
                let (chan_name, _reason) = key;
                let line = format!("-- Netsplit over, joins: {}", crate::netsplit::format_nick_list(&nicks, crate::netsplit::Side::Joins));
                if let Some(chan) = self.channels.get_mut(&chan_name) {
                    chan.buffer.push_system_topic(line);
                    flushed = true;
                }
            }
        }
        flushed
    }

    pub fn recent_netsplits(&self) -> Vec<crate::netsplit::NetsplitSummary> {
        let mut groups: HashMap<String, (usize, std::time::Instant)> = HashMap::new();
        for (reason, quit_at) in self.recent_split_quits.values() {
            if quit_at.elapsed() > crate::netsplit::NETSPLIT_REJOIN_WINDOW {
                continue;
            }
            groups
                .entry(reason.clone())
                .and_modify(|(count, earliest)| {
                    *count += 1;
                    if *quit_at < *earliest {
                        *earliest = *quit_at;
                    }
                })
                .or_insert((1, *quit_at));
        }
        let mut summaries: Vec<crate::netsplit::NetsplitSummary> = groups
            .into_iter()
            .map(|(reason, (nick_count, earliest))| {
                let (server_a, server_b) = reason.split_once(' ').unwrap_or((&reason, ""));
                crate::netsplit::NetsplitSummary {
                    server_a: server_a.to_string(),
                    server_b: server_b.to_string(),
                    nick_count,
                    since: earliest.elapsed(),
                }
            })
            .collect();
        summaries.sort_by_key(|s| std::cmp::Reverse(s.since));
        summaries
    }

    /// Looks up a channel's state by name (case-insensitive).
    pub fn channel(&self, name: &str) -> Option<&ChannelState> {
        self.channels.get(&name.to_lowercase())
    }

    pub fn ensure_channel(&mut self, display_name: &str) -> String {
        let key = display_name.to_lowercase();
        let logs_dir = self.logs_dir.clone();
        let autolog_ignore_targets = irust_config::settings::autolog_ignore_targets();
        // So a channel joined AFTER ISUPPORT arrives starts with the
        // real server table instead of the standard default -- see
        // `prefix_table`'s own doc comment.
        let prefix_table = self.prefix_table.clone();
        let chanmodes_table = self.chanmodes_table.clone();
        self.channels.entry(key.clone()).or_insert_with(|| {
            let mut chan = ChannelState::new(display_name);
            chan.nicklist.set_prefix_table(prefix_table);
            chan.nicklist.set_chanmodes_table(chanmodes_table);
            // `autolog_ignore_targets` (irssi parity) skips this whole
            // block for a listed target -- a manual `/log open` still
            // works afterward regardless, see `is_autolog_ignored`'s
            // own doc comment.
            if let Some(dir) = &logs_dir {
                if autolog_allowed(dir, display_name) && !is_autolog_ignored(&autolog_ignore_targets, display_name) {
                    let today = crate::today();
                    chan.buffer.set_log_path(log_file_path(dir, display_name, today));
                }
            }
            chan
        });
        // Consumed on every call, not just a brand-new channel's first
        // join -- rejoining an already-known channel with a fresh
        // `/join #chan <newkey>` (key changed, or just re-typed) should
        // update the remembered key too.
        if let Some(key_value) = self.pending_join_keys.remove(&key) {
            if let Some(chan) = self.channels.get_mut(&key) {
                chan.key = Some(key_value);
            }
        }
        key
    }

    /// `/window new` -- creates an itemless scratch window and switches
    /// to it immediately (matches irssi: a fresh `/window new` becomes
    /// the active window). Returns its `BufferId` so the caller can
    /// report it.
    pub fn create_empty_window(&mut self) -> BufferId {
        let id = self.next_empty_window_id;
        self.next_empty_window_id += 1;
        self.empty_windows.insert(id, Buffer::new(format!("(empty {id})")));
        let buffer_id = BufferId::Empty(id);
        self.active_buffer = buffer_id.clone();
        self.active_scroll_offset = 0;
        buffer_id
    }

    pub fn record_join_key(&mut self, channel: &str, key: String) {
        self.pending_join_keys.insert(channel.to_lowercase(), key);
    }

    /// `/ts`: irssi's `cmd_ts` -- "Topic for <channel>: <topic>" for every
    /// channel we are in, sorted by name for a stable listing.
    pub fn topic_summary_lines(&self) -> Vec<String> {
        let mut chans: Vec<&ChannelState> = self.channels.values().filter(|c| c.joined).collect();
        chans.sort_by_key(|c| c.display_name.to_lowercase());
        chans
            .into_iter()
            .map(|c| format!("Topic for {}: {}", c.display_name, c.topic.as_deref().unwrap_or("")))
            .collect()
    }

    pub fn note_own_message(&mut self, target: &str, text: &str) {
        use crate::nick_completion::last_msg_add;
        let key = target.to_lowercase();
        if target.starts_with(['#', '&', '+', '!']) {
            let keep = irust_config::settings_history_log::get_int("completion_keep_publics");
            let own_nick = self.nick.clone();
            let Some(chan) = self.channels.get_mut(&key) else { return };
            let Some((first, _)) = text.split_once(' ') else { return };
            if first.is_empty() {
                return;
            }
            let find = |chan: &ChannelState, word: &str| {
                chan.nicklist.sorted().into_iter().map(|e| e.nick).find(|n| n.eq_ignore_ascii_case(word))
            };
            let mut found = find(chan, first);
            if found.is_none() && first.chars().count() > 1 {
                // "bob:" / "bob," -- try without the trailing char.
                let mut trimmed = first.to_string();
                trimmed.pop();
                found = find(chan, &trimmed);
            }
            if let Some(nick) = found {
                if !nick.eq_ignore_ascii_case(&own_nick) {
                    last_msg_add(&mut chan.last_msgs, &nick, true, keep);
                }
            }
        } else if !self.query_buffers.contains_key(&key) {
            let keep = irust_config::settings_history_log::get_int("completion_keep_privates");
            last_msg_add(&mut self.private_last_msgs, target, true, keep);
        }
    }

    pub fn get_or_create_query_buffer(&mut self, key: &str, display_name: &str) -> &mut Buffer {
        let key = key.to_lowercase();
        let logs_dir = self.logs_dir.clone();
        let autolog_ignore_targets = irust_config::settings::autolog_ignore_targets();
        self.query_buffers.entry(key.clone()).or_insert_with(|| {
            let mut buf = Buffer::new(display_name.to_string());
            if let Some(dir) = &logs_dir {
                if autolog_allowed(dir, display_name) && !is_autolog_ignored(&autolog_ignore_targets, display_name) {
                    let today = crate::today();
                    buf.set_log_path(log_file_path(dir, display_name, today));
                }
            }
            buf
        });
        self.query_buffers.get_mut(&key).unwrap()
    }

    /// Clears the unread markers on whichever buffer is currently
    /// active. Meant to be called once per render frame -- you're
    /// looking at it right now, so by definition it's read.
    pub fn mark_current_buffer_read(&mut self) {
        match self.active_buffer.clone() {
            BufferId::Status => self.status.mark_read(),
            BufferId::Channel(key) => {
                if let Some(chan) = self.channels.get_mut(&key) {
                    chan.buffer.mark_read();
                }
            }
            BufferId::Query(key) => {
                if let Some(buf) = self.query_buffers.get_mut(&key) {
                    buf.mark_read();
                }
            }
            BufferId::Empty(id) => {
                if let Some(buf) = self.empty_windows.get_mut(&id) {
                    buf.mark_read();
                }
            }
        }
    }

    pub fn current_buffer(&self) -> &Buffer {
        match &self.active_buffer {
            BufferId::Status => &self.status,
            BufferId::Channel(key) => &self.channels[key].buffer,
            BufferId::Query(key) => &self.query_buffers[key],
            BufferId::Empty(id) => &self.empty_windows[id],
        }
    }

    pub fn current_buffer_mut(&mut self) -> &mut Buffer {
        match self.active_buffer.clone() {
            BufferId::Status => &mut self.status,
            BufferId::Channel(key) => &mut self.channels.get_mut(&key).unwrap().buffer,
            BufferId::Query(key) => self.query_buffers.get_mut(&key).unwrap(),
            BufferId::Empty(id) => self.empty_windows.get_mut(&id).unwrap(),
        }
    }

    pub fn reset_active_scroll(&mut self) {
        self.active_scroll_offset = 0;
    }

    pub fn set_pending_whois(&mut self, nick: String, buffer_id: BufferId) {
        self.pending_whois = Some((nick, buffer_id));
        self.auto_whowas_pending = false;
    }

    /// Looks up any buffer by id, not just the active one -- `None` if a
    /// channel/query it once pointed at has since been closed (`/wc`).
    pub fn buffer_mut(&mut self, id: &BufferId) -> Option<&mut Buffer> {
        match id {
            BufferId::Status => Some(&mut self.status),
            BufferId::Channel(key) => self.channels.get_mut(key).map(|c| &mut c.buffer),
            BufferId::Query(key) => self.query_buffers.get_mut(key),
            BufferId::Empty(id) => self.empty_windows.get_mut(id),
        }
    }

    /// Read-only sibling of [`buffer_mut`](Self::buffer_mut) -- split
    /// windows (`/split`) need to render a second, non-active buffer
    /// without mutating it.
    pub fn buffer(&self, id: &BufferId) -> Option<&Buffer> {
        match id {
            BufferId::Status => Some(&self.status),
            BufferId::Channel(key) => self.channels.get(key).map(|c| &c.buffer),
            BufferId::Query(key) => self.query_buffers.get(key),
            BufferId::Empty(id) => self.empty_windows.get(id),
        }
    }

    pub fn open_split(&mut self, target: BufferId) -> bool {
        if target == self.active_buffer
            || self.split_panes.iter().any(|p| p.buffer == target)
            || self.buffer(&target).is_none()
        {
            return false;
        }
        self.split_panes.push(SplitPane {
            buffer: target,
            scroll_offset: 0,
        });
        true
    }

    pub fn close_split(&mut self, target: Option<&BufferId>) -> bool {
        match target {
            Some(id) => {
                let before = self.split_panes.len();
                self.split_panes.retain(|p| &p.buffer != id);
                self.split_panes.len() != before
            }
            None => self.split_panes.pop().is_some(),
        }
    }

    /// `/unsplit -all` -- closes every split pane at once. `false` if
    /// there were none open.
    pub fn close_all_splits(&mut self) -> bool {
        let had_any = !self.split_panes.is_empty();
        self.split_panes.clear();
        had_any
    }

    pub fn focus_split(&mut self, target: &BufferId) -> bool {
        let Some(pos) = self.split_panes.iter().position(|p| &p.buffer == target) else {
            return false;
        };
        let incoming = self.split_panes.remove(pos);
        let outgoing = SplitPane {
            buffer: std::mem::replace(&mut self.active_buffer, incoming.buffer),
            scroll_offset: self.active_scroll_offset,
        };
        self.active_scroll_offset = incoming.scroll_offset;
        self.split_panes.push(outgoing);
        true
    }

    /// `/split_focus` with no argument -- moves focus to the next (or,
    /// if `!forward`, previous) pane in `split_panes` order, wrapping
    /// the outgoing focused pane around to the opposite end so a
    /// forward cycle followed by a backward one is a true no-op.
    /// `false` (no-op) with no split panes open.
    pub fn cycle_split_focus(&mut self, forward: bool) -> bool {
        if self.split_panes.is_empty() {
            return false;
        }
        if forward {
            let target = self.split_panes[0].buffer.clone();
            self.focus_split(&target)
        } else {
            let target = self.split_panes[self.split_panes.len() - 1].buffer.clone();
            let ok = self.focus_split(&target);
            if ok {
                if let Some(last) = self.split_panes.pop() {
                    self.split_panes.insert(0, last);
                }
            }
            ok
        }
    }

    /// All buffers that currently exist, in a stable display order:
    /// the status window first, then channels (alphabetically), then
    /// query buffers (alphabetically).
    pub fn buffer_ids(&self) -> Vec<BufferId> {
        let mut ids = vec![BufferId::Status];
        let mut chan_keys: Vec<&String> = self.channels.keys().collect();
        chan_keys.sort();
        ids.extend(chan_keys.into_iter().map(|k| BufferId::Channel(k.clone())));

        let mut query_keys: Vec<&String> = self.query_buffers.keys().collect();
        query_keys.sort();
        ids.extend(query_keys.into_iter().map(|k| BufferId::Query(k.clone())));

        let mut empty_ids: Vec<&u64> = self.empty_windows.keys().collect();
        empty_ids.sort();
        ids.extend(empty_ids.into_iter().map(|id| BufferId::Empty(*id)));

        self.collapse_window_items(ids)
    }

    /// The `/window item` group containing `id`, if any -- `None` means
    /// `id` is its own ungrouped window.
    fn group_containing(&self, id: &BufferId) -> Option<&Vec<BufferId>> {
        self.window_item_groups.iter().find(|g| g.contains(id))
    }

    fn remove_from_any_group(&mut self, id: &BufferId) {
        for g in &mut self.window_item_groups {
            g.retain(|m| m != id);
        }
        self.window_item_groups.retain(|g| g.len() > 1);
    }

    fn collapse_window_items(&self, ids: Vec<BufferId>) -> Vec<BufferId> {
        if self.window_item_groups.is_empty() {
            return ids;
        }
        let mut seen_groups: Vec<usize> = Vec::new();
        let mut out = Vec::with_capacity(ids.len());
        for id in &ids {
            match self.window_item_groups.iter().position(|g| g.contains(id)) {
                Some(gi) if seen_groups.contains(&gi) => continue,
                Some(gi) => {
                    seen_groups.push(gi);
                    let group = &self.window_item_groups[gi];
                    let representative = if group.contains(&self.active_buffer) {
                        self.active_buffer.clone()
                    } else {
                        group[0].clone()
                    };
                    out.push(representative);
                }
                None => out.push(id.clone()),
            }
        }
        out
    }

    pub fn window_item_add(&mut self, target: &BufferId) -> anyhow::Result<()> {
        if target == &self.active_buffer {
            anyhow::bail!("that's already the active window");
        }
        if self
            .group_containing(&self.active_buffer)
            .is_some_and(|g| g.contains(target))
        {
            anyhow::bail!("already grouped with the active window");
        }
        self.remove_from_any_group(target);
        match self
            .window_item_groups
            .iter_mut()
            .find(|g| g.contains(&self.active_buffer))
        {
            Some(g) => g.push(target.clone()),
            None => self
                .window_item_groups
                .push(vec![self.active_buffer.clone(), target.clone()]),
        }
        Ok(())
    }

    /// `/window item remove` -- pulls the CURRENTLY ACTIVE window back
    /// out of its group, making it a standalone window slot again.
    /// `Err` if it wasn't grouped with anything.
    pub fn window_item_remove(&mut self) -> anyhow::Result<()> {
        if self.group_containing(&self.active_buffer).is_none() {
            anyhow::bail!("the active window isn't grouped with anything");
        }
        let active = self.active_buffer.clone();
        self.remove_from_any_group(&active);
        Ok(())
    }

    pub fn window_item_move_to_slot(&mut self, slot: usize) -> anyhow::Result<()> {
        let active = self.active_buffer.clone();
        if matches!(active, BufferId::Status | BufferId::Empty(_)) {
            anyhow::bail!("this window has no item to move");
        }
        let ids = self.buffer_ids();
        let Some(target) = slot.checked_sub(1).and_then(|i| ids.get(i)).cloned() else {
            anyhow::bail!("no window number {slot}");
        };
        if target == active || self.group_containing(&target).is_some_and(|g| g.contains(&active)) {
            anyhow::bail!("that item is already in window {slot}");
        }
        self.remove_from_any_group(&active);
        match self.window_item_groups.iter_mut().find(|g| g.contains(&target)) {
            Some(g) => g.push(active),
            None => self.window_item_groups.push(vec![target, active]),
        }
        Ok(())
    }

    /// `/window item [prev|next]` -- switches `active_buffer` to the
    /// next/previous member of its own group, wrapping. `false` (a
    /// no-op) if the active window isn't grouped with anything.
    pub fn window_item_cycle(&mut self, forward: bool) -> bool {
        let Some(group) = self.group_containing(&self.active_buffer) else {
            return false;
        };
        let group = group.clone();
        let pos = group.iter().position(|id| id == &self.active_buffer).unwrap_or(0);
        let next = if forward {
            (pos + 1) % group.len()
        } else {
            (pos + group.len() - 1) % group.len()
        };
        self.active_buffer = group[next].clone();
        self.active_scroll_offset = 0;
        true
    }

    pub fn window_item_goto(&mut self, target: &BufferId) -> bool {
        let in_group = self
            .group_containing(&self.active_buffer)
            .is_some_and(|g| g.contains(target));
        if !in_group {
            return false;
        }
        self.active_buffer = target.clone();
        self.active_scroll_offset = 0;
        true
    }

    pub fn own_rank_in_channel(&self, key: &str) -> Option<char> {
        let c = self.channels.get(key)?;
        c.nicklist
            .sorted()
            .into_iter()
            .find(|e| e.nick.eq_ignore_ascii_case(&self.nick))
            .and_then(|e| c.nicklist.prefix_char(e.rank))
    }

    pub fn buffer_summaries(&self) -> Vec<BufferSummary> {
        self.buffer_ids()
            .into_iter()
            .filter_map(|id| match &id {
                BufferId::Status => Some(BufferSummary {
                    id: id.clone(),
                    display_name: self.status.shown_name().to_string(),
                    real_name: self.status.name.clone(),
                    has_unread: self.status.has_unread,
                    has_unread_message: self.status.has_unread_message,
                    has_unread_join: self.status.has_unread_join,
                    has_unread_highlight: self.status.has_unread_highlight,
                    highlight_actcolor: self.status.highlight_actcolor.clone(),
                    own_rank: None,
                }),
                BufferId::Channel(key) => {
                    let own_rank = self.own_rank_in_channel(key);
                    self.channels.get(key).map(|c| BufferSummary {
                        id: id.clone(),
                        display_name: c.buffer.shown_name().to_string(),
                        real_name: c.buffer.name.clone(),
                        has_unread: c.buffer.has_unread,
                        has_unread_message: c.buffer.has_unread_message,
                        has_unread_join: c.buffer.has_unread_join,
                        has_unread_highlight: c.buffer.has_unread_highlight,
                        highlight_actcolor: c.buffer.highlight_actcolor.clone(),
                        own_rank,
                    })
                }
                BufferId::Query(key) => self.query_buffers.get(key).map(|b| BufferSummary {
                    id: id.clone(),
                    display_name: b.shown_name().to_string(),
                    real_name: b.name.clone(),
                    has_unread: b.has_unread,
                    has_unread_message: b.has_unread_message,
                    has_unread_join: b.has_unread_join,
                    has_unread_highlight: b.has_unread_highlight,
                    highlight_actcolor: b.highlight_actcolor.clone(),
                    own_rank: None,
                }),
                BufferId::Empty(window_id) => self.empty_windows.get(window_id).map(|b| BufferSummary {
                    id: id.clone(),
                    display_name: b.shown_name().to_string(),
                    real_name: b.name.clone(),
                    has_unread: b.has_unread,
                    has_unread_message: b.has_unread_message,
                    has_unread_join: b.has_unread_join,
                    has_unread_highlight: b.has_unread_highlight,
                    highlight_actcolor: b.highlight_actcolor.clone(),
                    own_rank: None,
                }),
            })
            .collect()
    }

    pub fn close_buffer(&mut self, id: &BufferId) -> bool {
        if matches!(id, BufferId::Status) {
            return false;
        }
        if self.immortal.contains(id) {
            return false;
        }
        let was_active = &self.active_buffer == id;
        match id {
            BufferId::Status => unreachable!("handled above"),
            BufferId::Channel(key) => {
                self.channels.remove(key);
            }
            BufferId::Query(key) => {
                self.query_buffers.remove(key);
            }
            BufferId::Empty(window_id) => {
                self.empty_windows.remove(window_id);
            }
        }
        self.sticky.remove(id);
        // A closed buffer can't stay referenced in a `/window item`
        // group either -- same reasoning as `split_panes` below.
        self.remove_from_any_group(id);
        if was_active {
            if let Some(next) = self.buffer_ids().into_iter().next() {
                self.active_buffer = next;
            }
        }
        self.split_panes.retain(|p| &p.buffer != id);
        true
    }

    pub fn cycle_buffer(&mut self, forward: bool) {
        let ids = self.buffer_ids();
        let len = ids.len();
        if len <= 1 {
            return;
        }
        let current_pos = ids.iter().position(|id| id == &self.active_buffer).unwrap_or(0);
        let mut pos = current_pos;
        for _ in 0..len {
            pos = if forward {
                (pos + 1) % len
            } else {
                (pos + len - 1) % len
            };
            if pos == current_pos || !self.sticky.contains(&ids[pos]) {
                break;
            }
        }
        self.active_buffer = ids[pos].clone();
        self.active_scroll_offset = 0;
    }

    /// The buffer at `index` (1-based, same order and numbering
    /// `buffer_summaries()`/`/window list` already show), or `None` if
    /// `index` is 0 or past the last buffer. Shared lookup behind
    /// [`goto_buffer_index`](Self::goto_buffer_index) and `/wc <n>`.
    pub fn buffer_id_at_index(&self, index: usize) -> Option<BufferId> {
        let pos = index.checked_sub(1)?;
        self.buffer_ids().get(pos).cloned()
    }

    pub fn goto_buffer_index(&mut self, index: usize) -> bool {
        match self.buffer_id_at_index(index) {
            Some(id) => {
                if id != self.active_buffer {
                    self.active_buffer = id;
                    self.active_scroll_offset = 0;
                }
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_own_message_marks_an_addressed_nick_and_new_private_partners() {
        let mut st = ClientState::new("abcX", "#c", 11);
        st.channels.get_mut("#c").unwrap().nicklist.insert_plain("Alice");
        st.note_own_message("#c", "alice: hello there");
        st.note_own_message("#c", "nobody: hello");
        st.note_own_message("#c", "single");
        let list = &st.channels["#c"].last_msgs;
        assert_eq!(list.len(), 1, "only a real nick, only with text after it: {list:?}");
        assert_eq!(list[0].nick, "Alice");
        assert!(list[0].own > 0);

        st.note_own_message("Pal", "psst");
        assert_eq!(st.private_last_msgs[0].nick, "Pal");
        st.get_or_create_query_buffer("Friend", "Friend");
        st.note_own_message("Friend", "hi");
        assert_eq!(st.private_last_msgs.len(), 1, "an existing query is not re-added");
    }

    static AUTOLOG_IGNORE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock_autolog_ignore() -> std::sync::MutexGuard<'static, ()> {
        AUTOLOG_IGNORE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn state_with_two_queries() -> ClientState {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.query_buffers.insert("k9".to_string(), Buffer::new("K9"));
        st.query_buffers
            .insert("chatworld".to_string(), Buffer::new("ChatWorld"));
        st
    }

    #[test]
    fn window_item_add_groups_target_with_the_active_window() {
        let mut st = state_with_two_queries();
        let k9 = BufferId::Query("k9".to_string());
        assert!(st.window_item_add(&k9).is_ok());
        assert_eq!(
            st.buffer_ids(),
            vec![
                BufferId::Status,
                st.active_buffer.clone(),
                BufferId::Query("chatworld".to_string())
            ],
            "k9 collapses into the active channel's own slot"
        );
    }

    #[test]
    fn window_item_add_onto_the_active_buffer_itself_is_an_error() {
        let mut st = state_with_two_queries();
        let active = st.active_buffer.clone();
        assert!(st.window_item_add(&active).is_err());
    }

    #[test]
    fn window_item_add_twice_onto_the_same_group_is_an_error_the_second_time() {
        let mut st = state_with_two_queries();
        let k9 = BufferId::Query("k9".to_string());
        assert!(st.window_item_add(&k9).is_ok());
        assert!(st.window_item_add(&k9).is_err());
    }

    #[test]
    fn window_item_move_puts_the_active_item_into_another_window_slot() {
        let mut st = state_with_two_queries();
        let chan = st.active_buffer.clone();
        let k9 = BufferId::Query("k9".to_string());
        // slots: 1 status, 2 #irust-test (active), 3 chatworld, 4 k9
        assert_eq!(st.buffer_ids().len(), 4);
        assert!(st.window_item_move_to_slot(4).is_ok());
        assert_eq!(st.active_buffer, chan, "we follow the moved item");
        assert_eq!(st.buffer_ids().len(), 3, "the moved item now shares k9's slot");
        assert!(st.window_item_cycle(true), "the two are one window group");
        assert_eq!(st.active_buffer, k9);
    }

    #[test]
    fn window_item_move_rejects_bad_targets() {
        let mut st = state_with_two_queries();
        assert!(st.window_item_move_to_slot(0).is_err(), "windows are numbered from 1");
        assert!(st.window_item_move_to_slot(9).is_err(), "no such window");
        assert!(st.window_item_move_to_slot(2).is_err(), "already in its own window");
        st.window_item_move_to_slot(4).unwrap();
        assert!(st.window_item_move_to_slot(3).is_ok(), "moving on to another window is fine");
        st.active_buffer = BufferId::Status;
        assert!(st.window_item_move_to_slot(3).is_err(), "the status window has no item");
    }

    #[test]
    fn window_item_cycle_switches_within_the_group_and_wraps() {
        let mut st = state_with_two_queries();
        let chan = st.active_buffer.clone();
        let k9 = BufferId::Query("k9".to_string());
        st.window_item_add(&k9).unwrap();
        assert!(st.window_item_cycle(true));
        assert_eq!(st.active_buffer, k9);
        assert!(st.window_item_cycle(true), "wraps back to the first member");
        assert_eq!(st.active_buffer, chan);
        assert!(st.window_item_cycle(false), "backward wraps too");
        assert_eq!(st.active_buffer, k9);
    }

    #[test]
    fn window_item_cycle_on_an_ungrouped_window_is_a_no_op() {
        let mut st = state_with_two_queries();
        assert!(!st.window_item_cycle(true));
    }

    #[test]
    fn window_item_goto_jumps_within_the_group_but_not_outside_it() {
        let mut st = state_with_two_queries();
        let k9 = BufferId::Query("k9".to_string());
        let chatworld = BufferId::Query("chatworld".to_string());
        st.window_item_add(&k9).unwrap();
        assert!(st.window_item_goto(&k9));
        assert_eq!(st.active_buffer, k9);
        assert!(!st.window_item_goto(&chatworld), "not a member of this group");
    }

    #[test]
    fn window_item_remove_ungroups_the_active_window() {
        let mut st = state_with_two_queries();
        let k9 = BufferId::Query("k9".to_string());
        st.window_item_add(&k9).unwrap();
        assert!(st.window_item_remove().is_ok());
        assert_eq!(st.buffer_ids().len(), 4, "k9 is back to being its own window slot");
        assert!(st.window_item_remove().is_err(), "already ungrouped");
    }

    #[test]
    fn closing_a_grouped_buffer_removes_it_from_the_group_too() {
        let mut st = state_with_two_queries();
        let k9 = BufferId::Query("k9".to_string());
        st.window_item_add(&k9).unwrap();
        assert!(st.close_buffer(&k9));
        assert!(
            !st.window_item_cycle(true),
            "the group dissolved when it dropped to one member"
        );
    }

    #[test]
    fn split_opens_onto_a_real_buffer_and_leaves_active_untouched() {
        let mut st = state_with_two_queries();
        let target = BufferId::Query("k9".to_string());
        assert!(st.open_split(target.clone()));
        assert_eq!(
            st.split_panes,
            vec![SplitPane {
                buffer: target,
                scroll_offset: 0
            }]
        );
        assert_eq!(st.active_buffer, BufferId::Channel("#irust-test".to_string()));
    }

    #[test]
    fn split_onto_the_already_active_buffer_is_a_no_op() {
        let mut st = state_with_two_queries();
        let active = st.active_buffer.clone();
        assert!(!st.open_split(active));
        assert!(st.split_panes.is_empty());
    }

    #[test]
    fn split_onto_a_buffer_already_shown_in_another_pane_is_a_no_op() {
        let mut st = state_with_two_queries();
        assert!(st.open_split(BufferId::Query("k9".to_string())));
        assert!(!st.open_split(BufferId::Query("k9".to_string())));
        assert_eq!(st.split_panes.len(), 1);
    }

    #[test]
    fn split_onto_a_buffer_that_does_not_exist_is_a_no_op() {
        let mut st = state_with_two_queries();
        assert!(!st.open_split(BufferId::Query("nobody".to_string())));
        assert!(st.split_panes.is_empty());
    }

    #[test]
    fn a_second_split_adds_a_second_pane_not_replacing_the_first() {
        let mut st = state_with_two_queries();
        assert!(st.open_split(BufferId::Query("k9".to_string())));
        assert!(st.open_split(BufferId::Query("chatworld".to_string())));
        assert_eq!(st.split_panes.len(), 2);
    }

    #[test]
    fn unsplit_with_no_argument_closes_the_most_recently_opened_pane() {
        let mut st = state_with_two_queries();
        assert!(!st.close_split(None), "nothing to close yet");
        st.open_split(BufferId::Query("k9".to_string()));
        st.open_split(BufferId::Query("chatworld".to_string()));
        assert!(st.close_split(None));
        assert_eq!(
            st.split_panes,
            vec![SplitPane {
                buffer: BufferId::Query("k9".to_string()),
                scroll_offset: 0
            }]
        );
    }

    #[test]
    fn unsplit_with_an_argument_closes_that_specific_pane() {
        let mut st = state_with_two_queries();
        st.open_split(BufferId::Query("k9".to_string()));
        st.open_split(BufferId::Query("chatworld".to_string()));
        assert!(st.close_split(Some(&BufferId::Query("k9".to_string()))));
        assert_eq!(
            st.split_panes,
            vec![SplitPane {
                buffer: BufferId::Query("chatworld".to_string()),
                scroll_offset: 0
            }]
        );
    }

    #[test]
    fn close_all_splits_clears_every_pane() {
        let mut st = state_with_two_queries();
        st.open_split(BufferId::Query("k9".to_string()));
        st.open_split(BufferId::Query("chatworld".to_string()));
        assert!(st.close_all_splits());
        assert!(st.split_panes.is_empty());
        assert!(!st.close_all_splits(), "nothing left to close the second time");
    }

    #[test]
    fn focus_split_swaps_active_and_the_targeted_pane_preserving_each_scroll() {
        let mut st = state_with_two_queries();
        let original_active = st.active_buffer.clone();
        st.active_scroll_offset = 42;
        let split_target = BufferId::Query("k9".to_string());
        st.open_split(split_target.clone());
        st.split_panes[0].scroll_offset = 7;

        assert!(st.focus_split(&split_target));
        assert_eq!(st.active_buffer, split_target);
        assert_eq!(st.active_scroll_offset, 7, "inherits the pane's own saved scroll");
        assert_eq!(
            st.split_panes,
            vec![SplitPane {
                buffer: original_active,
                scroll_offset: 42
            }],
            "the outgoing focused pane keeps its own scroll position too"
        );
    }

    #[test]
    fn focus_split_on_a_buffer_not_in_any_split_pane_is_a_no_op() {
        let mut st = state_with_two_queries();
        assert!(!st.focus_split(&BufferId::Query("k9".to_string())));
    }

    #[test]
    fn cycle_split_focus_with_no_split_open_is_a_no_op() {
        let mut st = state_with_two_queries();
        assert!(!st.cycle_split_focus(true));
        assert!(!st.cycle_split_focus(false));
    }

    #[test]
    fn cycle_split_focus_forward_then_backward_is_a_no_op_overall() {
        let mut st = state_with_two_queries();
        let original_active = st.active_buffer.clone();
        st.open_split(BufferId::Query("k9".to_string()));
        st.open_split(BufferId::Query("chatworld".to_string()));
        let panes_before = st.split_panes.clone();

        assert!(st.cycle_split_focus(true));
        assert!(st.cycle_split_focus(false));
        assert_eq!(st.active_buffer, original_active);
        assert_eq!(st.split_panes, panes_before);
    }

    #[test]
    fn cycle_split_focus_forward_visits_every_pane_in_order() {
        let mut st = state_with_two_queries();
        let original_active = st.active_buffer.clone();
        st.open_split(BufferId::Query("k9".to_string()));
        st.open_split(BufferId::Query("chatworld".to_string()));

        st.cycle_split_focus(true);
        assert_eq!(st.active_buffer, BufferId::Query("k9".to_string()));
        st.cycle_split_focus(true);
        assert_eq!(st.active_buffer, BufferId::Query("chatworld".to_string()));
        st.cycle_split_focus(true);
        assert_eq!(
            st.active_buffer, original_active,
            "wraps back around to where it started"
        );
    }

    #[test]
    fn closing_the_buffer_shown_in_a_split_pane_removes_just_that_pane() {
        let mut st = state_with_two_queries();
        let split_target = BufferId::Query("k9".to_string());
        st.open_split(split_target.clone());
        st.open_split(BufferId::Query("chatworld".to_string()));
        assert!(st.close_buffer(&split_target));
        assert_eq!(
            st.split_panes,
            vec![SplitPane {
                buffer: BufferId::Query("chatworld".to_string()),
                scroll_offset: 0
            }],
            "a closed buffer can't stay pinned in a split pane, but other panes are untouched"
        );
    }

    #[test]
    fn channel_completion_matches_by_prefix_case_insensitively() {
        let mut st = ClientState::new("abcX", "#chatnet", 11);
        st.channels
            .insert("#chatworld".to_string(), ChannelState::new("#ChatWorld"));
        st.channels.insert("#brazil".to_string(), ChannelState::new("#brazil"));

        // "#chatnet" sorts before "#ChatWorld" case-insensitively --
        // "chat**n**et" < "chat**w**orld".
        let candidates = channel_completion_candidates(&st.channels, "#CH");
        assert_eq!(candidates, vec!["#chatnet".to_string(), "#ChatWorld".to_string()]);
    }

    #[test]
    fn channel_completion_ignores_non_matching_channels() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        assert!(channel_completion_candidates(&st.channels, "#brazil").is_empty());
    }

    #[test]
    fn channel_completion_includes_parted_channels_still_open() {
        // `channels` entries are never removed, only marked not-joined
        // on PART -- a parted channel with open scrollback should still
        // complete, matching what the buflist bar itself still shows.
        let mut st = ClientState::new("abcX", "#chatnet", 11);
        st.channels.get_mut("#chatnet").unwrap().joined = false;
        assert_eq!(
            channel_completion_candidates(&st.channels, "#chat"),
            vec!["#chatnet".to_string()]
        );
    }

    #[test]
    fn channel_completion_sorts_case_insensitively() {
        let mut st = ClientState::new("abcX", "#zzz", 11);
        st.channels.insert("#Alpha".to_string(), ChannelState::new("#Alpha"));
        st.channels.insert("#beta".to_string(), ChannelState::new("#beta"));

        let candidates = channel_completion_candidates(&st.channels, "#");
        assert_eq!(
            candidates,
            vec!["#Alpha".to_string(), "#beta".to_string(), "#zzz".to_string()]
        );
    }

    #[test]
    fn buffer_summaries_reports_our_own_rank_in_a_channel_we_op() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .nicklist
            .add_from_names_token("@abcX");
        let summaries = st.buffer_summaries();
        let chan = summaries
            .iter()
            .find(|s| s.id == BufferId::Channel("#irust-test".to_string()))
            .unwrap();
        assert_eq!(chan.own_rank, Some('@'));
    }

    #[test]
    fn buffer_summaries_reports_no_rank_when_we_have_none() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .nicklist
            .add_from_names_token("abcX");
        let summaries = st.buffer_summaries();
        let chan = summaries
            .iter()
            .find(|s| s.id == BufferId::Channel("#irust-test".to_string()))
            .unwrap();
        assert_eq!(chan.own_rank, None);
    }

    #[test]
    fn create_empty_window_switches_to_it_and_appears_in_buffer_ids() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        let id = st.create_empty_window();
        assert_eq!(st.active_buffer, id);
        assert!(st.buffer_ids().contains(&id));
        assert_eq!(st.current_buffer().shown_name(), "(empty 1)");
    }

    #[test]
    fn immortal_blocks_close_for_any_buffer_type() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        let chan = BufferId::Channel("#irust-test".to_string());
        st.immortal.insert(chan.clone());
        assert!(!st.close_buffer(&chan));
        assert!(st.channels.contains_key("#irust-test"));

        st.immortal.remove(&chan);
        assert!(st.close_buffer(&chan));
    }

    #[test]
    fn closing_an_empty_window_removes_it() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        let id = st.create_empty_window();
        assert!(st.close_buffer(&id));
        assert!(!st.buffer_ids().contains(&id));
    }

    #[test]
    fn cycle_buffer_skips_a_sticky_window() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.ensure_channel("#other");
        // Order: Status, #irust-test, #other. Make #irust-test sticky
        // while parked on Status, then cycle forward -- must land on
        // #other, not the sticky one.
        st.active_buffer = BufferId::Status;
        st.sticky.insert(BufferId::Channel("#irust-test".to_string()));
        st.cycle_buffer(true);
        assert_eq!(st.active_buffer, BufferId::Channel("#other".to_string()));
    }

    #[test]
    fn cycle_buffer_with_everything_sticky_settles_back_on_current() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.sticky.insert(BufferId::Channel("#irust-test".to_string()));
        st.active_buffer = BufferId::Status;
        // Only Status + one sticky channel exist -- cycling must not
        // hang or panic, just settle back on Status.
        st.cycle_buffer(true);
        assert_eq!(st.active_buffer, BufferId::Status);
    }

    #[test]
    fn close_buffer_removes_a_non_active_query() {
        let mut st = state_with_two_queries();
        let closed = st.close_buffer(&BufferId::Query("k9".to_string()));
        assert!(closed);
        assert!(!st.query_buffers.contains_key("k9"));
        assert_eq!(st.active_buffer, BufferId::Channel("#irust-test".to_string()));
    }

    #[test]
    fn close_buffer_falls_back_when_closing_the_active_one() {
        let mut st = state_with_two_queries();
        st.active_buffer = BufferId::Query("k9".to_string());
        let closed = st.close_buffer(&BufferId::Query("k9".to_string()));
        assert!(closed);
        // buffer_ids() order: status first, then channels, then queries
        // alphabetically.
        assert_eq!(st.active_buffer, BufferId::Status);
    }

    #[test]
    fn close_buffer_refuses_to_remove_the_status_window() {
        let mut st = state_with_two_queries();
        let closed = st.close_buffer(&BufferId::Status);
        assert!(!closed);
    }

    #[test]
    fn close_buffer_allows_closing_down_to_just_the_status_window() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        let closed = st.close_buffer(&BufferId::Channel("#irust-test".to_string()));
        assert!(closed);
        assert!(!st.channels.contains_key("#irust-test"));
        assert_eq!(st.active_buffer, BufferId::Status);
    }

    #[test]
    fn starts_on_the_initial_channel_buffer() {
        let st = ClientState::new("abcX", "#irust-test", 11);
        assert_eq!(st.active_buffer, BufferId::Channel("#irust-test".to_string()));
        assert_eq!(st.current_buffer().name, "#irust-test");
    }

    #[test]
    fn ensure_channel_creates_once_and_is_idempotent() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        let key1 = st.ensure_channel("#Second");
        let key2 = st.ensure_channel("#second");
        assert_eq!(key1, key2);
        assert_eq!(st.channels.len(), 2);
    }

    #[test]
    fn ensure_channel_picks_up_a_recorded_pending_join_key() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.record_join_key("#Second", "hunter2".to_string());
        st.ensure_channel("#second");
        assert_eq!(st.channel("#second").unwrap().key.as_deref(), Some("hunter2"));
    }

    #[test]
    fn ensure_channel_without_a_pending_key_leaves_it_none() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.ensure_channel("#nokeychan");
        assert_eq!(st.channel("#nokeychan").unwrap().key, None);
    }

    #[test]
    fn rejoining_with_a_new_key_overwrites_the_old_one() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.record_join_key("#second", "old".to_string());
        st.ensure_channel("#second");
        assert_eq!(st.channel("#second").unwrap().key.as_deref(), Some("old"));

        st.record_join_key("#second", "new".to_string());
        st.ensure_channel("#second");
        assert_eq!(st.channel("#second").unwrap().key.as_deref(), Some("new"));
    }

    #[test]
    fn cycle_forward_visits_status_then_channels_then_queries_alphabetically_then_wraps() {
        let mut st = state_with_two_queries();
        st.ensure_channel("#zzz-channel");
        assert_eq!(st.active_buffer, BufferId::Channel("#irust-test".to_string()));

        st.cycle_buffer(true);
        assert_eq!(st.active_buffer, BufferId::Channel("#zzz-channel".to_string()));

        st.cycle_buffer(true);
        assert_eq!(st.active_buffer, BufferId::Query("chatworld".to_string()));

        st.cycle_buffer(true);
        assert_eq!(st.active_buffer, BufferId::Query("k9".to_string()));

        st.cycle_buffer(true);
        assert_eq!(st.active_buffer, BufferId::Status, "wraps to the status window first");

        st.cycle_buffer(true);
        assert_eq!(
            st.active_buffer,
            BufferId::Channel("#irust-test".to_string()),
            "then back to where we started"
        );
    }

    #[test]
    fn cycle_backward_wraps_the_other_way() {
        let mut st = state_with_two_queries();
        // ids: [Status, Channel(#irust-test), Query(chatworld), Query(k9)]
        // -- backward from the initial channel goes to the status window
        // first, same as forward would reach it last.
        st.cycle_buffer(false);
        assert_eq!(st.active_buffer, BufferId::Status);

        st.cycle_buffer(false);
        assert_eq!(st.active_buffer, BufferId::Query("k9".to_string()));
    }

    #[test]
    fn cycling_with_only_the_status_window_and_one_channel_toggles_between_them() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.cycle_buffer(true);
        assert_eq!(st.active_buffer, BufferId::Status);
        st.cycle_buffer(true);
        assert_eq!(st.active_buffer, BufferId::Channel("#irust-test".to_string()));
    }

    #[test]
    fn enable_logging_writes_lines_to_a_file_for_the_initial_channel() {
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "initial"));
        let _ = std::fs::remove_dir_all(&dir);

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .buffer
            .push_system("hello log");

        let today = crate::today();
        let contents = std::fs::read_to_string(log_file_path(&dir, "#irust-test", today)).unwrap();
        assert!(contents.contains("hello log"));
        assert!(
            !contents.contains("-- -- "),
            "system text's own '-- ' prefix must not be doubled by the logger"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn logging_applies_to_channels_and_queries_created_after_enable_logging() {
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "later"));
        let _ = std::fs::remove_dir_all(&dir);

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());
        st.ensure_channel("#second");
        st.channels
            .get_mut("#second")
            .unwrap()
            .buffer
            .push_system("second channel line");

        st.get_or_create_query_buffer("k9", "K9").push_system("query line");

        let today = crate::today();
        let chan_log = std::fs::read_to_string(log_file_path(&dir, "#second", today)).unwrap();
        assert!(chan_log.contains("second channel line"));
        let query_log = std::fs::read_to_string(log_file_path(&dir, "K9", today)).unwrap();
        assert!(query_log.contains("query line"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn is_autolog_ignored_is_case_insensitive() {
        assert!(is_autolog_ignored("#Ignored,#other", "#ignored"));
        assert!(is_autolog_ignored("#ignored,#other", "#IGNORED"));
        assert!(!is_autolog_ignored("#ignored,#other", "#unrelated"));
        assert!(!is_autolog_ignored("", "#anything"));
    }

    #[test]
    fn ensure_channel_skips_auto_log_path_for_an_ignored_target() {
        let _net = crate::test_support::net_guard();
        let _guard = lock_autolog_ignore();
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "autolog-ignored"));
        let _ = std::fs::remove_dir_all(&dir);

        let mut s = irust_config::settings::Settings::default();
        s.set("autolog_ignore_targets", "#autolog-ignore-test-target-zzz")
            .unwrap();

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());
        st.ensure_channel("#autolog-ignore-test-target-zzz");
        st.channels
            .get_mut("#autolog-ignore-test-target-zzz")
            .unwrap()
            .buffer
            .push_system("must not be written to disk");

        let today = crate::today();
        assert!(
            std::fs::read_to_string(log_file_path(&dir, "#autolog-ignore-test-target-zzz", today)).is_err(),
            "an ignored target must never get a log file at all"
        );

        s.set("autolog_ignore_targets", "").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_channel_still_logs_a_non_ignored_target() {
        let _guard = lock_autolog_ignore();
        let dir = std::env::temp_dir().join(format!(
            "irust-test-log-{}-{}",
            std::process::id(),
            "autolog-not-ignored"
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let mut s = irust_config::settings::Settings::default();
        s.set("autolog_ignore_targets", "#autolog-ignore-test-target-zzz")
            .unwrap();

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());
        st.ensure_channel("#autolog-not-ignored-test-target-zzz");
        st.channels
            .get_mut("#autolog-not-ignored-test-target-zzz")
            .unwrap()
            .buffer
            .push_system("must still be logged");

        let today = crate::today();
        let log = std::fs::read_to_string(log_file_path(&dir, "#autolog-not-ignored-test-target-zzz", today)).unwrap();
        assert!(log.contains("must still be logged"));

        s.set("autolog_ignore_targets", "").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn get_or_create_query_buffer_skips_auto_log_path_for_an_ignored_target() {
        let _guard = lock_autolog_ignore();
        let dir = std::env::temp_dir().join(format!(
            "irust-test-log-{}-{}",
            std::process::id(),
            "autolog-ignored-query"
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let mut s = irust_config::settings::Settings::default();
        s.set("autolog_ignore_targets", "IgnoredQueryZzz").unwrap();

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());
        st.get_or_create_query_buffer("ignoredqueryzzz", "IgnoredQueryZzz")
            .push_system("must not be written to disk");

        let today = crate::today();
        assert!(
            std::fs::read_to_string(log_file_path(&dir, "IgnoredQueryZzz", today)).is_err(),
            "an ignored query target must never get a log file at all"
        );

        s.set("autolog_ignore_targets", "").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_logs_switches_every_open_buffer_to_the_new_days_file() {
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "rotate"));
        let _ = std::fs::remove_dir_all(&dir);

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        let today = crate::today();
        st.enable_logging(dir.clone());
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .buffer
            .push_system("before rotation");

        let tomorrow = today.succ_opt().unwrap();
        st.rotate_logs(tomorrow);
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .buffer
            .push_system("after rotation");
        st.status.push_system("status after rotation");

        let todays_log = std::fs::read_to_string(log_file_path(&dir, "#irust-test", today)).unwrap();
        assert!(todays_log.contains("before rotation"));
        assert!(
            !todays_log.contains("after rotation"),
            "a line written post-rotation must not land in the old file"
        );

        let tomorrows_log = std::fs::read_to_string(log_file_path(&dir, "#irust-test", tomorrow)).unwrap();
        assert!(tomorrows_log.contains("after rotation"));
        assert!(!tomorrows_log.contains("before rotation"));

        let tomorrows_status_log =
            std::fs::read_to_string(log_file_path(&dir, &st.status.name.clone(), tomorrow)).unwrap();
        assert!(
            tomorrows_status_log.contains("status after rotation"),
            "rotate_logs must also cover the status window"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_logs_skips_a_buffer_that_was_explicitly_stopped() {
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "rotate-stopped"));
        let _ = std::fs::remove_dir_all(&dir);

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());
        assert!(st.channels["#irust-test"].buffer.is_logging());
        st.channels.get_mut("#irust-test").unwrap().buffer.stop_logging();
        assert!(!st.channels["#irust-test"].buffer.is_logging());

        let tomorrow = crate::today().succ_opt().unwrap();
        st.rotate_logs(tomorrow);
        assert!(
            !st.channels["#irust-test"].buffer.is_logging(),
            "an explicitly-stopped buffer must stay off across a day-change rollover"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_start_resumes_into_todays_dated_file_and_clears_the_stopped_flag() {
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "resume"));
        let _ = std::fs::remove_dir_all(&dir);

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());
        st.channels.get_mut("#irust-test").unwrap().buffer.stop_logging();

        let path = st
            .log_path_for("#irust-test")
            .expect("logging was enabled, a path must resolve");
        st.channels.get_mut("#irust-test").unwrap().buffer.resume_logging(path);
        assert!(st.channels["#irust-test"].buffer.is_logging());
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .buffer
            .push_system("resumed line");

        let todays_log = std::fs::read_to_string(log_file_path(&dir, "#irust-test", crate::today())).unwrap();
        assert!(todays_log.contains("resumed line"));

        // Rotation must carry it forward again now that it's no longer stopped.
        let tomorrow = crate::today().succ_opt().unwrap();
        st.rotate_logs(tomorrow);
        assert!(st.channels["#irust-test"].buffer.is_logging());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_path_for_is_none_before_logging_was_ever_enabled() {
        let st = ClientState::new("abcX", "#irust-test", 11);
        assert!(st.log_path_for("#irust-test").is_none());
    }

    #[test]
    fn enable_logging_never_reads_an_existing_legacy_log_back_into_the_buffer() {
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "legacy-fallback"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("#irust-test.log"),
            "[01:00:00] <someone> pre-rotation message\n",
        )
        .unwrap();

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());

        assert!(
            st.channels["#irust-test"].buffer.lines.is_empty(),
            "must NOT load any history back from an existing on-disk log"
        );
        st.channels
            .get_mut("#irust-test")
            .unwrap()
            .buffer
            .push_system("new line goes to the dated file");

        // The legacy file is untouched (nothing new appended to it)...
        let legacy = std::fs::read_to_string(dir.join("#irust-test.log")).unwrap();
        assert!(!legacy.contains("new line goes to the dated file"));
        // ...new activity lands in today's dated file instead.
        let today = crate::today();
        let dated = std::fs::read_to_string(log_file_path(&dir, "#irust-test", today)).unwrap();
        assert!(dated.contains("new line goes to the dated file"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_activity_on_an_inactive_buffer_shows_up_as_unread() {
        let mut st = state_with_two_queries();
        // active_buffer is #irust-test, so pushing into the K9 query
        // shouldn't be "read" until we actually switch there.
        st.query_buffers
            .get_mut("k9")
            .unwrap()
            .push_chat("K9", 0, None, "hi", false);
        assert!(st.query_buffers["k9"].has_unread);

        let summaries = st.buffer_summaries();
        let k9 = summaries.iter().find(|s| s.display_name == "K9").unwrap();
        assert!(k9.has_unread);
        assert!(!k9.has_unread_highlight);
    }

    #[test]
    fn mark_current_buffer_read_clears_only_the_active_one() {
        let mut st = state_with_two_queries();
        st.query_buffers
            .get_mut("k9")
            .unwrap()
            .push_chat("K9", 0, None, "hi", true);
        st.query_buffers
            .get_mut("chatworld")
            .unwrap()
            .push_chat("ChatWorld", 0, None, "hi", false);

        st.active_buffer = BufferId::Query("k9".to_string());
        st.mark_current_buffer_read();

        assert!(!st.query_buffers["k9"].has_unread);
        assert!(!st.query_buffers["k9"].has_unread_highlight);
        assert!(
            st.query_buffers["chatworld"].has_unread,
            "untouched buffer stays unread"
        );
    }

    /// `buffer_summaries()` has 4 near-identical arms (Status/Channel/
    /// Query/Empty); only Query is exercised here since they all pull
    /// `highlight_actcolor` the same way (a plain `.clone()` off the
    /// underlying `Buffer`, same field `has_unread_highlight` already
    /// comes from) -- not worth a copy of this test per arm.
    #[test]
    fn buffer_summaries_carries_the_pending_highlight_actcolor_through() {
        let mut st = state_with_two_queries();
        st.query_buffers.get_mut("k9").unwrap().highlight_actcolor = Some("magenta".to_string());

        let summaries = st.buffer_summaries();
        let k9 = summaries.iter().find(|s| s.display_name == "K9").unwrap();
        assert_eq!(k9.highlight_actcolor.as_deref(), Some("magenta"));

        let chatworld = summaries.iter().find(|s| s.display_name == "ChatWorld").unwrap();
        assert_eq!(chatworld.highlight_actcolor, None);
    }

    #[test]
    fn enable_logging_leaves_the_initial_channel_and_status_empty_even_with_prior_logs_on_disk() {
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "reload-initial"));
        let _ = std::fs::remove_dir_all(&dir);

        // A previous run's log, sitting on disk before this ClientState
        // (or its logging) even exists.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("#irust-test.log"), "[01:00:00] <someone> earlier message\n").unwrap();
        std::fs::write(dir.join("status.log"), "[01:00:00] -- earlier status line\n").unwrap();

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());

        assert!(st.channels["#irust-test"].buffer.lines.is_empty());
        assert!(st.status.lines.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_channel_and_get_or_create_query_buffer_never_load_history_for_a_brand_new_entry() {
        let dir = std::env::temp_dir().join(format!("irust-test-log-{}-{}", std::process::id(), "reload-later"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("#second.log"), "[01:00:00] <K9> from a previous run\n").unwrap();
        std::fs::write(dir.join("K9.log"), "[01:00:00] <K9> hi again\n").unwrap();

        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.enable_logging(dir.clone());

        st.ensure_channel("#second");
        assert!(st.channels["#second"].buffer.lines.is_empty());

        st.get_or_create_query_buffer("k9", "K9");
        assert!(st.query_buffers["k9"].lines.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn current_buffer_follows_active_buffer() {
        let mut st = state_with_two_queries();
        st.active_buffer = BufferId::Query("k9".to_string());
        assert_eq!(st.current_buffer().name, "K9");
    }

    /// Matches irssi's own `sig_print_text` -- the marker lands in
    /// every open buffer (status, every channel, every query), not just
    /// whichever one happens to get the next message.
    #[test]
    fn daychange_marker_lands_in_every_open_buffer_at_once() {
        let mut st = state_with_two_queries();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();

        st.push_daychange_to_all_buffers(date);

        let is_daychange = |buf: &Buffer| {
            matches!(
                buf.lines.back(),
                Some(crate::buffer::BufferLine::System { kind: crate::buffer::SystemKind::DayChange, text, .. })
                    if text == "-- Day changed to 27 Aug 2026"
            )
        };
        assert!(is_daychange(&st.status), "status window should get the marker");
        assert!(
            is_daychange(&st.channels["#irust-test"].buffer),
            "the channel should get the marker"
        );
        assert!(
            is_daychange(&st.query_buffers["k9"]),
            "every query buffer should get the marker"
        );
        assert!(
            is_daychange(&st.query_buffers["chatworld"]),
            "every query buffer should get the marker"
        );
    }

    #[test]
    fn buffer_id_at_index_matches_window_list_numbering() {
        let st = state_with_two_queries();
        // Same 1-based order as `buffer_summaries()`/`/window list`:
        // status first, then the rest in `buffer_ids()` order.
        assert_eq!(st.buffer_id_at_index(1), Some(BufferId::Status));
        assert_eq!(st.buffer_id_at_index(0), None, "0 is not a window, same as /window 0");
        let past_the_end = st.buffer_ids().len() + 1;
        assert_eq!(st.buffer_id_at_index(past_the_end), None);
    }

    #[test]
    fn goto_buffer_index_and_buffer_id_at_index_agree() {
        let mut st = state_with_two_queries();
        let third = st.buffer_id_at_index(3);
        assert!(third.is_some(), "test fixture should have at least 3 buffers");
        assert!(st.goto_buffer_index(3));
        assert_eq!(Some(st.active_buffer.clone()), third);
    }

    #[test]
    fn recent_netsplits_is_empty_with_no_split_quits() {
        let st = ClientState::new("abcX", "#irust-test", 11);
        assert!(st.recent_netsplits().is_empty());
    }

    #[test]
    fn should_show_away_suppresses_the_exact_same_repeated_message() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        assert!(st.should_show_away("Other", "gone fishing"), "first time -- must show");
        assert!(
            !st.should_show_away("Other", "gone fishing"),
            "same message again -- must suppress"
        );
    }

    #[test]
    fn should_show_away_shows_again_once_the_message_changes() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        assert!(st.should_show_away("Other", "gone fishing"));
        assert!(!st.should_show_away("Other", "gone fishing"));
        assert!(
            st.should_show_away("Other", "back later"),
            "a changed message must show again"
        );
    }

    #[test]
    fn should_show_away_tracks_each_nick_independently() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        assert!(st.should_show_away("Alice", "gone fishing"));
        assert!(
            st.should_show_away("Bob", "gone fishing"),
            "a different nick with the same text must still show"
        );
    }

    #[test]
    fn recent_netsplits_groups_by_server_pair_and_counts_nicks() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.record_split_quit("Alice", "hub.example.net leaf.example.net");
        st.record_split_quit("Bob", "hub.example.net leaf.example.net");
        st.record_split_quit("Carol", "hub.example.net istanbul.example.net");

        let splits = st.recent_netsplits();
        assert_eq!(splits.len(), 2, "two distinct server pairs, one summary each");
        let leaf = splits.iter().find(|s| s.server_b == "leaf.example.net").unwrap();
        assert_eq!(leaf.server_a, "hub.example.net");
        assert_eq!(leaf.nick_count, 2, "Alice and Bob share the same split");
        let istanbul = splits.iter().find(|s| s.server_b == "istanbul.example.net").unwrap();
        assert_eq!(istanbul.nick_count, 1);
    }

    #[test]
    fn recent_netsplits_excludes_a_quit_past_the_rejoin_window() {
        let mut st = ClientState::new("abcX", "#irust-test", 11);
        st.record_split_quit("Alice", "hub.example.net leaf.example.net");
        // Directly back-date the recorded quit past the recognition
        // window, rather than actually sleeping 30 real minutes in a
        // unit test.
        if let Some(entry) = st.recent_split_quits.get_mut("alice") {
            entry.1 =
                std::time::Instant::now() - crate::netsplit::NETSPLIT_REJOIN_WINDOW - std::time::Duration::from_secs(1);
        }
        assert!(
            st.recent_netsplits().is_empty(),
            "an expired split-quit must not show as still ongoing"
        );
    }
}
