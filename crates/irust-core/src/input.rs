use crate::state::{BufferId, ClientState, SplitOrientation};

pub const IGNORE_LEVELS: &[&str] = &[
    "msgs", "notices", "ctcps", "joins", "parts", "quits", "nicks", "invites", "wallops",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatMode {
    GetOrSet,
    Delete,
    Reset,
}

/// `/log start|stop`'s verb -- see `InputAction::Log`'s own doc comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogToggle {
    Start,
    Stop,
    Open(String),
    Close,
}

/// `/scrollback goto`'s parsed target -- see `InputAction::ScrollbackGoto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbackGoto {
    Relative(i64),
    Line(usize),
    Timestamp {
        date: Option<ScrollbackGotoDate>,
        hour: u32,
        minute: u32,
        second: u32,
    },
}

/// See `ScrollbackGoto::Timestamp`'s own doc comment. `None` at the
/// call site (a bare `hh:mi[:ss]`) means "today, or yesterday if that
/// time hasn't happened yet" -- irssi's own default, encoded as the
/// absence of a variant here rather than a third one, since it needs no
/// data of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbackGotoDate {
    /// `-<days ago>` -- e.g. `-2` for the day before yesterday.
    DaysAgo(u32),
    /// `dd[.mm]` -- `month` is `None` if only the day was given (irssi
    /// infers the month: this one, unless `day` is already later than
    /// today's, meaning last month's day was meant instead).
    OnDay { day: u32, month: Option<u32> },
}

/// `/foreach`'s target set -- see `InputAction::Foreach`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeachScope {
    Window,
    Channel,
    Query,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelSetupAction {
    Add,
    Modify,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecodeSub {
    /// `/recode` (bare) -- lists every configured override.
    List,
    /// `/recode add [[<tag>/]<target>] <charset>` -- `target: None`
    /// means "default to the active buffer's target", resolved in
    /// `main.rs` (this crate has no notion of "the active buffer").
    Add { target: Option<String>, charset: String },
    /// `/recode remove [<target>]` -- same active-buffer default as
    /// `Add` when omitted.
    Remove { target: Option<String> },
}

/// The optional per-network fields of `/network add`/`/network modify`
/// beyond host/nick/autojoin/cert: everything a bouncer or a
/// SASL-protected network needs to be set up from inside the client
/// (`None` = not given; `modify` leaves those fields alone).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkExtras {
    /// `-password <pw>` -- the server/bouncer PASS (`user/network:pw`
    /// for ZNC).
    pub password: Option<String>,
    /// `-auto` / `-noauto` -- connect at startup.
    pub autoconnect: Option<bool>,
    /// `-fingerprint <sha256>` -- pin the server certificate.
    pub fingerprint: Option<String>,
    /// `-noverify` / `-verify` -- CA-chain verification on/off.
    pub tls_verify: Option<bool>,
    /// `-sasl <account> <password>`.
    pub sasl: Option<(String, String)>,
}

impl NetworkExtras {
    /// Consumes one `/network` extra flag (and its value tokens) if
    /// `tok` is one; `false` means `tok` is not an extras flag.
    fn take_flag<'a>(&mut self, tok: &str, tokens: &mut impl Iterator<Item = &'a str>) -> bool {
        match tok {
            "-password" | "-pass" => self.password = tokens.next().map(str::to_string),
            "-auto" | "-autoconnect" => self.autoconnect = Some(true),
            "-noauto" => self.autoconnect = Some(false),
            "-fingerprint" | "-fp" => self.fingerprint = tokens.next().map(str::to_string),
            "-noverify" => self.tls_verify = Some(false),
            "-verify" => self.tls_verify = Some(true),
            "-sasl" => {
                if let (Some(account), Some(password)) = (tokens.next(), tokens.next()) {
                    self.sasl = Some((account.to_string(), password.to_string()));
                }
            }
            _ => return false,
        }
        true
    }
}

/// The trailing `-flag`/`-flag <value>` tokens `/network add`,
/// `/network modify`, `/server add` and `/server modify` all accept the
/// same way, after whatever positional arguments each one takes of its
/// own. Shared so `/server`'s irssi-named subcommands reuse the exact
/// same parsing `/network` already had, not a second copy of it.
fn parse_network_flags<'a>(
    tokens: &mut impl Iterator<Item = &'a str>,
    tls: &mut Option<bool>,
    target: &mut Option<String>,
) -> (Option<String>, Option<String>, Option<String>, Option<String>, NetworkExtras) {
    let mut nick = None;
    let mut autojoin = None;
    let mut client_cert = None;
    let mut client_key = None;
    let mut extras = NetworkExtras::default();
    while let Some(tok) = tokens.next() {
        if extras.take_flag(tok, tokens) {
            continue;
        }
        match tok {
            "-notls" | "-plain" => *tls = Some(false),
            "-tls" | "-ssl" => *tls = Some(true),
            "-nick" => nick = tokens.next().map(str::to_string),
            "-autojoin" => autojoin = tokens.next().map(str::to_string),
            "-cert" => client_cert = tokens.next().map(str::to_string),
            "-key" => client_key = tokens.next().map(str::to_string),
            "-host" | "-target" => *target = tokens.next().map(str::to_string),
            _ if target.is_none() && !tok.starts_with('-') => *target = Some(tok.to_string()),
            _ => {}
        }
    }
    (nick, autojoin, client_cert, client_key, extras)
}


/// What typing Enter on a line of user input should result in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    /// Send `text` as a `PRIVMSG` to `target`, and locally echo it into
    /// `echo_buffer` (the server won't echo it back without the
    /// `echo-message` cap, which isn't requested in v0.1).
    SendMessage {
        target: String,
        text: String,
        echo_buffer: BufferId,
    },
    /// `/me <text>` -- a CTCP ACTION, always targeting the active buffer.
    Action {
        target: String,
        text: String,
        echo_buffer: BufferId,
    },
    Join {
        channel: String,
        key: Option<String>,
    },
    /// `/part [<channel>] [<message>]` -- `channel: None` means whichever
    /// channel is currently active. `message` is the part reason, sent
    /// as `PART <chan> :<message>` (irssi's own `cmd_part`, which
    /// otherwise falls back to the `part_message` setting -- irust has
    /// no such setting, so an omitted message just means no reason).
    Part {
        channel: Option<String>,
        message: Option<String>,
    },
    Nick(String),
    Topic {
        channel: Option<String>,
        text: Option<String>,
    },
    /// `channel: None` targets the active channel, matching irssi's own
    /// `KICK [<channel>] <nick> [<reason>]`.
    Kick {
        channel: Option<String>,
        nick: String,
        reason: Option<String>,
    },
    /// Backs `/mode`, and the `/op`/`/deop`/`/voice`/`/devoice`
    /// shortcuts, which all just construct one of these. Always targets
    /// the active channel.
    ChannelMode {
        modestring: String,
        args: Vec<String>,
    },
    UserMode {
        target: String,
        modestring: String,
        args: Vec<String>,
    },
    /// `/color <nick> [index]` -- `Some(index)` sets a persisted override
    /// for that nick's color; `None` queries its currently resolved one.
    Color {
        nick: String,
        index: Option<usize>,
    },
    CloseBuffer(Option<String>),
    CloseBufferByIndex(usize),
    CloseAllBuffers,
    /// `/wc queries` (or `-queries`, `q`/`-q`) -- closes every query
    /// buffer, channels left untouched. A one-shot way to clear a pile
    /// of open queries without also losing channel windows the way
    /// `/wc *` does.
    CloseAllQueries,
    Split {
        target: String,
        orientation: Option<SplitOrientation>,
    },
    /// `/split [-v|-h] <n>` -- same split, targeting the `n`-th buffer
    /// by position (1-based), mirroring `CloseBufferByIndex`.
    SplitByIndex {
        index: usize,
        orientation: Option<SplitOrientation>,
    },
    /// `/unsplit [name]` -- closes one split pane: the named one, or
    /// (no argument) the most recently opened one. See
    /// `ClientState::close_split`.
    Unsplit(Option<String>),
    /// `/unsplit <n>` -- same close, targeting the `n`-th buffer by
    /// position.
    UnsplitByIndex(usize),
    /// `/unsplit -all` -- closes every split pane at once. See
    /// `ClientState::close_all_splits`.
    UnsplitAll,
    /// `/split_focus <name>` -- focuses that specific pane (must
    /// already be a split pane, not the one already focused). See
    /// `ClientState::focus_split`.
    SplitFocus(String),
    /// `/split_focus <n>` -- same, targeting the `n`-th buffer by
    /// position.
    SplitFocusByIndex(usize),
    /// `/split_focus` (no argument) -- cycles focus to the next split
    /// pane; `/split_focus -prev` cycles to the previous one instead.
    /// `true` = forward. See `ClientState::cycle_split_focus`.
    SplitFocusCycle(bool),
    NickColors(bool),
    /// `/textcolors [on|off]` -- see `ClientState::text_colors`'s own
    /// doc comment. No argument flips the current state.
    TextColors(bool),
    /// `/away [reason]` sets away; `/away` alone (or `/back`, irssi's
    /// alias for the same thing) clears it.
    Away {
        reason: Option<String>,
    },
    /// `/whois <nick>` (also `/wi`) -- sends a raw `WHOIS`; the reply
    /// numerics land in the status window (`BufferId::Status`), same as
    /// any other unrouted server reply.
    Whois(String),
    Ip(String),
    Connect {
        target: String,
        tls: Option<bool>,
        ip_version: Option<u8>,
    },
    NetworkList,
    NetworkAdd {
        name: String,
        target: String,
        tls: bool,
        nick: Option<String>,
        autojoin: Option<String>,
        client_cert: Option<String>,
        client_key: Option<String>,
        extras: NetworkExtras,
    },
    NetworkRemove(String),
    NetworkModify {
        name: String,
        target: Option<String>,
        tls: Option<bool>,
        nick: Option<String>,
        autojoin: Option<String>,
        client_cert: Option<String>,
        client_key: Option<String>,
        extras: NetworkExtras,
    },
    /// `/disconnect [network]` -- drops one server connection.  `None`
    /// means whichever server the command was typed on.
    Disconnect(Option<String>),
    /// `/server <network>` -- switches which already-connected server the
    /// UI is looking at (and where new commands/messages go), without
    /// touching any connection. Distinct from `/connect`, which opens a
    /// new one.
    SwitchServer(String),
    /// `/cycle [channel] [message]` -- irssi's `CYCLE`: leaves and
    /// immediately rejoins a channel (the active one if `channel` is
    /// omitted), staying in the same buffer/window throughout. Useful
    /// for forcing a fresh rejoin (e.g. to reset your own channel
    /// state) without losing scrollback the way `/wc` would.
    Cycle {
        channel: Option<String>,
        message: Option<String>,
    },
    ClearView {
        all: bool,
    },
    /// `/notice <target> <message>` -- sends a `NOTICE`, echoed as a
    /// system line into `echo_buffer` (a `NOTICE` isn't a `PRIVMSG`, so
    /// it doesn't belong in `SendMessage`'s chat-line echo).
    Notice {
        target: String,
        text: String,
        echo_buffer: BufferId,
    },
    /// `/invite <nick> [channel]` -- `channel: None` means the active
    /// channel.
    Invite {
        nick: String,
        channel: Option<String>,
    },
    /// `/ctcp <target> <command> [data]` -- a manual outgoing CTCP
    /// request (`command` gets uppercased on the wire, matching
    /// irssi's own `cmd_ctcp`).
    Ctcp {
        target: String,
        command: String,
        data: Option<String>,
    },
    /// `/nctcp <target> <command> [data]` -- irssi parity: an unsolicited
    /// CTCP sent via `NOTICE` (a CTCP *reply* shape) rather than `/ctcp`'s
    /// `PRIVMSG`-based CTCP *request* -- same wire shape either way,
    /// `data` optional exactly like `Ctcp` above.
    Nctcp {
        target: String,
        command: String,
        data: Option<String>,
    },
    Wall {
        channel: Option<String>,
        text: String,
    },
    RawlogOpen(String),
    /// `/rawlog close`.
    RawlogClose,
    ScrollbackGoto(ScrollbackGoto),
    /// `/scrollback home` -- scrolls to the very oldest line.
    ScrollbackHome,
    /// `/scrollback end` -- scrolls back to the live tail (same effect
    /// as `reset_active_scroll`).
    ScrollbackEnd,
    /// `/scrollback status` -- irssi parity, simplified: reports the
    /// active window's own line count only, not irssi's fuller
    /// all-windows total + memory-usage breakdown (irust doesn't track
    /// per-buffer memory accounting).
    ScrollbackStatus,
    Foreach {
        scope: ForeachScope,
        command: String,
    },
    Query {
        nick: Option<String>,
        message: Option<String>,
    },
    Ban {
        channel: Option<String>,
        mask: Option<String>,
    },
    Unban {
        channel: Option<String>,
        masks: Vec<String>,
    },
    Quiet {
        channel: Option<String>,
        mask: String,
    },
    /// `/unquiet [<channel>] <nick-or-mask> [...]` -- channel mode `-q`,
    /// one or more space-separated tokens, each resolved the same way
    /// `/quiet`'s own mask does (no cached quiet-list to match
    /// `-first`/`-last`/index against, see `Quiet`'s own doc comment).
    Unquiet {
        channel: Option<String>,
        masks: Vec<String>,
    },
    /// `/kickban [<channel>] <nick> [<reason>]` -- kicks, then bans
    /// (`mask` pre-resolved at parse time the same way `/ban` does).
    Kickban {
        channel: Option<String>,
        nick: String,
        mask: String,
        reason: Option<String>,
    },
    /// `/knockout [<minutes>] [<channel>] <nick> <reason>` -- bans,
    /// kicks, then schedules an automatic `/unban` of the same mask
    /// after the timeout (irssi's own default `knockout_time` is 5
    /// minutes, used here when `minutes` is omitted -- irust has no
    /// `/set` yet to change the default).
    Knockout {
        channel: Option<String>,
        minutes: Option<u64>,
        nick: String,
        mask: String,
        reason: Option<String>,
    },
    ServerCommand {
        command: String,
        rest: String,
    },
    /// `/squit <server> <reason>` -- `reason` becomes one trailing
    /// `:`-prefixed param (can contain spaces), unlike
    /// `ServerCommand`'s space-separated passthrough.
    Squit {
        server: String,
        reason: String,
    },
    /// `/wallops <message>`.
    Wallops {
        message: String,
    },
    SwitchBuffer(String),
    WindowList,
    SwitchBufferByIndex(usize),
    /// `/window name <name>` -- a cosmetic display-name override for
    /// the active buffer (irssi's own `WINDOW NAME`). Never touches the
    /// real channel/query name any wire command targets.
    WindowName(String),
    WindowItemAdd(String),
    /// `/window item remove` -- pulls the active window back out of
    /// its group. Takes no argument (matches irssi's own real form).
    WindowItemRemove,
    /// `/window item [prev|next]` -- switches within the active
    /// window's own group; bare `/window item` (no verb) means `next`,
    /// same "no argument = forward" convention `/split_focus` already
    /// uses. `true` = next, `false` = prev.
    WindowItemCycle(bool),
    /// `/window item goto <name>` -- jumps directly to `name` WITHIN
    /// the active window's own group.
    WindowItemGoto(String),
    /// `/window item move <n>` -- moves the active item into window
    /// number `n` (irssi's numeric form; the `<name>` form is
    /// `WindowItemAdd`, which already pulls that item into this window).
    WindowItemMove(usize),
    /// `/window new [hidden|split|-right split]` -- irssi parity:
    /// creates a genuinely empty scratch window (no channel/query
    /// behind it, see `ClientState::empty_windows`) and switches to it.
    WindowNew,
    /// `/window immortal [on|off|toggle]` -- irssi parity: blocks the
    /// active buffer from being closed (`/wc` or auto-close-on-part
    /// alike) until turned back off. No argument reports the current
    /// state, matching irssi's own no-arg-echoes-current behavior.
    WindowImmortal(Option<String>),
    WindowStick,
    Alias {
        name: Option<String>,
        command: Option<String>,
    },
    /// `/unalias <name>`.
    Unalias(String),
    Set {
        key: Option<String>,
        value: Option<String>,
    },
    Format {
        name: Option<String>,
        template: Option<String>,
        mode: FormatMode,
    },
    Recode(RecodeSub),
    Exec(String),
    Echo(String),
    /// Echoes a warning locally, then still sends `command rest` to the
    /// server. `/sconnect` asks a
    /// server to link to another one, worth a visible heads-up, but
    /// (unlike `/die`) it isn't gated behind `-yes` since a bad link is
    /// reversible with `/squit`.
    EchoThenServerCommand { echo: String, command: String, rest: String },
    Toggle {
        key: String,
        value: Option<String>,
    },
    Beep,
    Cat(String),
    /// `/cd <directory>` -- irssi parity: changes irust's own process
    /// working directory (affects relative paths for `/exec`, `/cat`,
    /// logs, ...).
    Cd(String),
    StatusbarInfo(String),
    Eval(String),
    Wait(u64),
    /// `/uptime` -- how long the irust client process itself has been
    /// running (irssi's own client-side `UPTIME`, not a server query).
    /// Purely local; no wire traffic.
    Uptime,
    ClientVersion,
    SetName(String),
    Completion {
        key: Option<String>,
        value: Option<String>,
        delete: bool,
    },
    Ignore {
        mask: Option<String>,
        levels: Vec<String>,
        is_regex: bool,
        is_except: bool,
        is_replies: bool,
        time_secs: Option<u64>,
    },
    /// `/unignore <id>|<mask>` -- `<id>` is the 1-based position in the
    /// list (irssi's own indexing), matching the mask string removes it
    /// directly.
    Unignore(String),
    Hilight {
        pattern: Option<String>,
        is_regex: bool,
        is_mask: bool,
        channels: Vec<String>,
        color: Option<String>,
        actcolor: Option<String>,
        priority: Option<i32>,
    },
    /// `/dehilight <id>|<word>` -- same indexing convention as
    /// `Unignore`.
    Dehilight(String),
    Autoban {
        pattern: Option<String>,
        is_regex: bool,
        reason: Option<String>,
    },
    /// `/unautoban <id|pattern>` -- same convention as `/dehilight`:
    /// accepts either the 1-based position from `/autoban`'s listing or
    /// the literal pattern text.
    Unautoban(String),
    /// `/statusbar` or `/statusbar list` -- shows the current bottom-bar
    /// item order/side. irssi's own pluggable-statusbar concept, scoped
    /// down to the one bottom bar irust actually has (no multi-bar
    /// groups).
    StatusbarList,
    StatusbarAdd {
        item: String,
        before: Option<String>,
        after: Option<String>,
        alignment: Option<String>,
    },
    /// `/statusbar remove <item>`.
    StatusbarRemove(String),
    /// `/statusbar reset` -- back to the default order.
    StatusbarReset,
    /// `/layout` or `/layout list` -- shows which networks have a saved
    /// layout and what's in it. Executed by the dispatcher (spans every
    /// connected `Servers` entry, not any one server's wire), same
    /// reasoning as `StatusbarList` above.
    LayoutList,
    /// `/layout save` -- snapshots every currently connected network's
    /// joined channels (with their `+k` keys) and open query windows,
    /// overwriting each network's own entry in the saved layout.
    LayoutSave,
    LayoutReset,
    Bind {
        key: Option<String>,
        action: Option<String>,
        delete: bool,
        reset: bool,
    },
    DccChat(String),
    DccSend {
        nick: String,
        path: String,
        passive: bool,
    },
    DccGet(u64),
    DccResume(u64),
    /// `/dcc list` -- prints every queued offer and active transfer for
    /// the current server into the active buffer. The only way to see
    /// them: there's no progress-bar/gauge widget in `irust-tui`, so
    /// this and the per-transfer buffer's own throttled system-line
    /// updates are the whole surface.
    DccList,
    DccClose(u64),
    Lastlog {
        pattern: String,
        is_regex: bool,
        case_sensitive: bool,
        count: Option<usize>,
    },
    Log {
        action: LogToggle,
        target: Option<String>,
    },
    LogByIndex {
        action: LogToggle,
        index: usize,
    },
    LogStatus,
    Netsplit,
    ChannelSetup {
        action: ChannelSetupAction,
        channel: String,
        network: String,
        autojoin: Option<bool>,
        botmasks: Option<Vec<String>>,
        botcmd: Option<String>,
        password: Option<String>,
    },
    /// `/channel remove <channel> <network>`.
    ChannelRemove {
        channel: String,
        network: String,
    },
    ChannelList,
    Notify {
        nick: Option<String>,
    },
    /// `/unnotify <nick>` -- removes a nick from that same watch list.
    Unnotify {
        nick: String,
    },
    Reconnect,
    RmReconns,
    RmRejoins,
    Cmds(Option<String>),
    /// `/redraw` -- forces a full terminal repaint (clears ratatui's
    /// diff state first), not just the next normal incremental draw
    /// `redraw_tx` already requests. Useful after a resize glitch or
    /// anything else that leaves stray content on screen.
    Redraw,
    PluginReload(Option<String>),
    /// `/pluginload <name>`, also reachable as `/load <name>` or
    /// `/script load <name>` -- loads a `.lua` file under the plugins dir
    /// (with or without a trailing `.lua`) that may not already be
    /// loaded; same underlying call as `PluginReload(Some(name))`, a
    /// separate command for irssi-naming parity.
    PluginLoad(String),
    /// `/pluginunload <name>`, also reachable as `/unload <name>` or
    /// `/script unload <name>` -- removes exactly that script's
    /// registered `on_message`/`on_send`/`on_command` handlers
    /// (`PluginHost::unload_script`) without reloading it.
    PluginUnload(String),
    /// `/pluginexec <lua code>`, also reachable as `/script exec <lua
    /// code>` -- irssi parity: runs an ad-hoc snippet inline via
    /// `PluginHost::load_str`, without a backing `.lua` file -- a
    /// throwaway, not a named loaded script (doesn't show up in
    /// `/pluginreload` with no args' reload sweep).
    PluginExec(String),
    /// `/pluginreset`, also reachable as `/script reset` -- irssi
    /// parity: unloads every currently-loaded plugin at once, without
    /// reloading any of them back from disk.
    PluginReset,
    Update,
    Upgrade,
    Restart,
    Ping,
    /// `/ts` -- one "Topic for <channel>: <topic>" line per joined
    /// channel (irssi's `cmd_ts`), printed into the active window.
    TopicSummary,
    Quit {
        reason: Option<String>,
    },
    /// A `/word` this crate doesn't know about -- not necessarily
    /// invalid, a plugin script may have registered a handler for it
    /// (see `irust-plugin::PluginHost::on_command`). The caller checks.
    Command {
        name: String,
        arg: String,
    },
    /// A malformed invocation of a command this crate *does* know about
    /// (e.g. `/msg` with no text, `/join` with no channel).
    Unknown(String),
}

fn strip_cmdchar<'a>(cmdchars: &str, line: &'a str) -> Option<&'a str> {
    let mut chars = line.chars();
    let first = chars.next()?;
    cmdchars.contains(first).then_some(chars.as_str())
}

fn auto_complete_nick(state: &ClientState, line: &str) -> Option<String> {
    if !irust_config::settings_history_log::get_bool("completion_auto") {
        return None;
    }
    let BufferId::Channel(key) = &state.active_buffer else { return None };
    let chan = state.channels.get(key)?;
    let nicks: Vec<String> = chan.nicklist.sorted().into_iter().map(|e| e.nick).collect();
    crate::nick_completion::auto_complete(
        line,
        &irust_config::settings::completion_char(),
        &nicks,
        &state.nick,
        &chan.last_msgs,
        &crate::nick_completion::Options::from_settings(),
    )
}

pub fn parse_input(state: &ClientState, line: &str) -> InputAction {
    let Some(rest) = strip_cmdchar(&irust_config::settings::cmdchars(), line) else {
        // Plain text typed while looking at the status window has
        // nowhere on the wire to go -- there's no channel/query behind
        // it, unlike every other buffer.
        let Some((target, echo_buffer)) = active_target(state) else {
            return InputAction::Unknown(line.to_string());
        };
        let text = auto_complete_nick(state, line).unwrap_or_else(|| line.to_string());
        return InputAction::SendMessage {
            target,
            text,
            echo_buffer,
        };
    };

    let mut parts = rest.splitn(2, ' ');
    let raw_cmd = parts.next().unwrap_or("");
    let cmd = raw_cmd.to_lowercase();
    let arg = parts.next().unwrap_or("").trim();

    match cmd.as_str() {
        "msg" => {
            let mut it = arg.splitn(2, ' ');
            let target = it.next().unwrap_or("").to_string();
            let text = it.next().unwrap_or("").trim().to_string();
            if target.is_empty() || text.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            // `*` (irssi's own shorthand): whichever buffer is active.
            if target == "*" {
                let Some((real_target, echo_buffer)) = active_target(state) else {
                    return InputAction::Unknown(line.to_string());
                };
                return InputAction::SendMessage { target: real_target, text, echo_buffer };
            }
            let echo_buffer = if target.starts_with(['#', '&']) {
                BufferId::Channel(target.to_lowercase())
            } else {
                BufferId::Query(target.to_lowercase())
            };
            InputAction::SendMessage {
                target,
                text,
                echo_buffer,
            }
        }
        "me" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let Some((target, echo_buffer)) = active_target(state) else {
                return InputAction::Unknown(line.to_string());
            };
            InputAction::Action {
                target,
                text: arg.to_string(),
                echo_buffer,
            }
        }
        "join" | "j" => {
            if arg.is_empty() {
                InputAction::Unknown(line.to_string())
            } else {
                let mut parts = arg.splitn(2, ' ');
                let name = parts.next().unwrap_or("");
                let key = parts.next().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                InputAction::Join { channel: normalize_channel(name), key }
            }
        }
        "part" => {
            let mut words = arg.splitn(2, ' ');
            let first = words.next().unwrap_or("");
            if first.starts_with(['#', '&']) {
                let message = words.next().map(str::to_string).filter(|s| !s.is_empty());
                InputAction::Part { channel: Some(first.to_string()), message }
            } else {
                let message = if arg.is_empty() { None } else { Some(arg.to_string()) };
                InputAction::Part { channel: None, message }
            }
        }
        "nick" => {
            if arg.is_empty() {
                InputAction::Unknown(line.to_string())
            } else {
                InputAction::Nick(arg.to_string())
            }
        }
        "topic" => {
            let mut top = arg.splitn(2, ' ');
            let first = top.next().unwrap_or("");
            let (channel, rest) = if first.starts_with(['#', '&']) {
                (Some(first.to_string()), top.next().unwrap_or(""))
            } else {
                (None, arg)
            };
            // `-delete` (irssi's own flag): explicitly clears the topic
            // (`TOPIC <chan> :`), distinct from no text at all (a plain
            // `TOPIC <chan>` query).
            let text = if rest.eq_ignore_ascii_case("-delete") {
                Some(String::new())
            } else if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            };
            InputAction::Topic { channel, text }
        }
        "kick" => {
            let mut top = arg.splitn(2, ' ');
            let first = top.next().unwrap_or("");
            let (channel, nick_and_reason) = if first.starts_with(['#', '&']) {
                (Some(first.to_string()), top.next().unwrap_or(""))
            } else {
                (None, arg)
            };
            let mut inner = nick_and_reason.splitn(2, ' ');
            let nick = inner.next().unwrap_or("").to_string();
            if nick.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let reason = inner.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            InputAction::Kick { channel, nick, reason }
        }
        "ban" => {
            let mut top = arg.splitn(2, ' ');
            let first = top.next().unwrap_or("");
            let (channel, rest) = if first.starts_with(['#', '&']) {
                (Some(first.to_string()), top.next().unwrap_or(""))
            } else {
                (None, arg)
            };
            // `/ban *`: ban everyone currently in the (active-only --
            // see `ban_everyone_masks`'s doc comment) channel. Built as
            // a batched `ChannelMode` (same shape `/op`/`/deop`/`/voice`/
            // `/devoice *` already use) rather than `Ban`, since `Ban`
            // only ever carries one mask.
            if channel.is_none() && rest == "*" {
                let masks = ban_everyone_masks(state);
                if masks.is_empty() {
                    return InputAction::Unknown(line.to_string());
                }
                return InputAction::ChannelMode {
                    modestring: format!("+{}", "b".repeat(masks.len())),
                    args: masks,
                };
            }
            let mask = if rest.is_empty() { None } else { Some(resolve_ban_target(state, &channel, rest)) };
            InputAction::Ban { channel, mask }
        }
        "unban" => {
            let mut top = arg.splitn(2, ' ');
            let first = top.next().unwrap_or("");
            let (channel, rest) = if first.starts_with(['#', '&']) {
                (Some(first.to_string()), top.next().unwrap_or(""))
            } else {
                (None, arg)
            };
            if rest.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let masks = resolve_unban_targets(state, &channel, rest);
            if masks.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Unban { channel, masks }
        }
        "quiet" => {
            let mut top = arg.splitn(2, ' ');
            let first = top.next().unwrap_or("");
            let (channel, rest) = if first.starts_with(['#', '&']) {
                (Some(first.to_string()), top.next().unwrap_or(""))
            } else {
                (None, arg)
            };
            if rest.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let mask = resolve_ban_target(state, &channel, rest);
            InputAction::Quiet { channel, mask }
        }
        "unquiet" => {
            let mut top = arg.splitn(2, ' ');
            let first = top.next().unwrap_or("");
            let (channel, rest) = if first.starts_with(['#', '&']) {
                (Some(first.to_string()), top.next().unwrap_or(""))
            } else {
                (None, arg)
            };
            if rest.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let masks: Vec<String> =
                rest.split_whitespace().map(|tok| resolve_ban_target(state, &channel, tok)).collect();
            InputAction::Unquiet { channel, masks }
        }
        "kickban" => {
            let mut top = arg.splitn(2, ' ');
            let first = top.next().unwrap_or("");
            let (channel, nick_and_reason) = if first.starts_with(['#', '&']) {
                (Some(first.to_string()), top.next().unwrap_or(""))
            } else {
                (None, arg)
            };
            let mut inner = nick_and_reason.splitn(2, ' ');
            let nick = inner.next().unwrap_or("").to_string();
            if nick.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let reason = inner.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let mask = resolve_ban_target(state, &channel, &nick);
            InputAction::Kickban { channel, nick, mask, reason }
        }
        "knockout" => {
            let mut top = arg.splitn(2, ' ');
            let first = top.next().unwrap_or("");
            let (channel, rest_after_chan) = if first.starts_with(['#', '&']) {
                (Some(first.to_string()), top.next().unwrap_or(""))
            } else {
                (None, arg)
            };
            let mut inner = rest_after_chan.splitn(2, ' ');
            let maybe_minutes = inner.next().unwrap_or("");
            let (minutes, nick_and_reason) = match maybe_minutes.parse::<u64>() {
                Ok(n) => (Some(n), inner.next().unwrap_or("")),
                Err(_) => (None, rest_after_chan),
            };
            let mut inner2 = nick_and_reason.splitn(2, ' ');
            let nick = inner2.next().unwrap_or("").to_string();
            if nick.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let reason = inner2.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let mask = resolve_ban_target(state, &channel, &nick);
            InputAction::Knockout { channel, minutes, nick, mask, reason }
        }
        "mode" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let mut it = arg.split(' ').filter(|s| !s.is_empty());
            let first = it.next().unwrap_or("").to_string();
            if !first.starts_with(['+', '-']) {
                let modestring = match it.next() {
                    Some(m) => m.to_string(),
                    None => return InputAction::Unknown(line.to_string()),
                };
                let args: Vec<String> = it.map(|s| s.to_string()).collect();
                return InputAction::UserMode { target: first, modestring, args };
            }
            let args: Vec<String> = it.map(|s| s.to_string()).collect();
            InputAction::ChannelMode { modestring: first, args }
        }
        "op" | "deop" | "voice" | "devoice" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let (sign, letter, want_op, want_voice) = match cmd.as_str() {
                "op" => ('+', 'o', Some(false), None),
                "deop" => ('-', 'o', Some(true), None),
                "voice" => ('+', 'v', Some(false), Some(false)),
                _ => ('-', 'v', None, Some(true)),
            };
            let nicklist = match &state.active_buffer {
                BufferId::Channel(key) => state.channels.get(key).map(|c| &c.nicklist),
                _ => None,
            };
            // A literal nick (no `*`/`?`) is always taken as-is, same as
            // irssi -- only tokens with wildcards get expanded against
            // the channel's current membership.
            let mut nicks: Vec<String> = Vec::new();
            // irssi's `-yes` option confirms a wildcard that matches more than
            // `max_wildcard_modes` nicks (0 = no limit).
            let confirmed = arg.split(' ').any(|t| t.eq_ignore_ascii_case("-yes"));
            let max_wildcard = irust_config::settings_net::int_of("max_wildcard_modes") as usize;
            for token in arg.split(' ').filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("-yes")) {
                if token.contains(['*', '?']) {
                    if let Some(nl) = nicklist {
                        let matched = crate::nicklist::wildcard_targets(nl, token, &state.nick, want_op, want_voice);
                        if max_wildcard > 0 && matched.len() > max_wildcard && !confirmed {
                            return InputAction::Echo(format!(
                                "-- Not a good idea: {token} matches {} nicks (max_wildcard_modes is {max_wildcard}); use -yes to do it anyway",
                                matched.len()
                            ));
                        }
                        nicks.extend(matched);
                    }
                } else {
                    nicks.push(token.to_string());
                }
            }
            if nicks.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::ChannelMode {
                modestring: format!("{sign}{}", letter.to_string().repeat(nicks.len())),
                args: nicks,
            }
        }
        "color" => {
            let mut it = arg.splitn(2, ' ');
            let nick = it.next().unwrap_or("").to_string();
            if nick.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let index_arg = it.next().map(str::trim).filter(|s| !s.is_empty());
            let index = match index_arg {
                None => None,
                Some(s) => match s.parse::<usize>() {
                    Ok(n) => Some(n),
                    Err(_) => return InputAction::Unknown(line.to_string()),
                },
            };
            InputAction::Color { nick, index }
        }
        "wc" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                InputAction::CloseBuffer(None)
            } else if trimmed == "*" || matches!(trimmed.to_lowercase().as_str(), "all" | "-all") {
                InputAction::CloseAllBuffers
            } else if matches!(trimmed.to_lowercase().as_str(), "queries" | "-queries" | "query" | "-query" | "q" | "-q") {
                InputAction::CloseAllQueries
            } else if let Ok(n) = trimmed.parse::<usize>() {
                // Same "0 is not a window" rule as `/window <n>` --
                // fall through to the by-name path instead (harmless:
                // no buffer is ever named "0", so it'll just report
                // "no such buffer").
                if n == 0 {
                    InputAction::CloseBuffer(Some(trimmed.to_string()))
                } else {
                    InputAction::CloseBufferByIndex(n)
                }
            } else {
                InputAction::CloseBuffer(Some(trimmed.to_string()))
            }
        }
        "split" => {
            let mut orientation = None;
            let mut target: Option<&str> = None;
            for tok in arg.split_whitespace() {
                match tok {
                    "-v" | "-vertical" => orientation = Some(SplitOrientation::Vertical),
                    "-h" | "-horizontal" => orientation = Some(SplitOrientation::Horizontal),
                    _ if target.is_none() => target = Some(tok),
                    // Extra tokens beyond the target are ignored, same
                    // leniency `/connect`'s own flag parsing already has.
                    _ => {}
                }
            }
            match target {
                None => InputAction::Unknown(line.to_string()),
                Some(t) => match t.parse::<usize>() {
                    Ok(n) if n != 0 => InputAction::SplitByIndex { index: n, orientation },
                    _ => InputAction::Split { target: t.to_string(), orientation },
                },
            }
        }
        "unsplit" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                InputAction::Unsplit(None)
            } else if trimmed == "*" || matches!(trimmed.to_lowercase().as_str(), "all" | "-all") {
                InputAction::UnsplitAll
            } else {
                match trimmed.parse::<usize>() {
                    Ok(n) if n != 0 => InputAction::UnsplitByIndex(n),
                    _ => InputAction::Unsplit(Some(trimmed.to_string())),
                }
            }
        }
        "split_focus" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                InputAction::SplitFocusCycle(true)
            } else if matches!(trimmed.to_lowercase().as_str(), "-prev" | "-previous") {
                InputAction::SplitFocusCycle(false)
            } else {
                match trimmed.parse::<usize>() {
                    Ok(n) if n != 0 => InputAction::SplitFocusByIndex(n),
                    _ => InputAction::SplitFocus(trimmed.to_string()),
                }
            }
        }
        "nickcolors" => {
            let enabled = match arg.to_lowercase().as_str() {
                "on" => true,
                "off" => false,
                "" => !state.nicklist_colors,
                _ => return InputAction::Unknown(line.to_string()),
            };
            InputAction::NickColors(enabled)
        }
        "textcolors" => {
            let enabled = match arg.to_lowercase().as_str() {
                "on" => true,
                "off" => false,
                "" => !state.text_colors,
                _ => return InputAction::Unknown(line.to_string()),
            };
            InputAction::TextColors(enabled)
        }
        "connect" => {
            if arg.is_empty() {
                InputAction::Unknown(line.to_string())
            } else {
                let mut tls: Option<bool> = None;
                let mut ip_version: Option<u8> = None;
                let mut target: Option<&str> = None;
                for tok in arg.split_whitespace() {
                    match tok {
                        "-notls" | "-plain" => tls = Some(false),
                        "-tls" | "-ssl" => tls = Some(true),
                        "-4" | "-ipv4" => ip_version = Some(4),
                        "-6" | "-ipv6" => ip_version = Some(6),
                        _ if target.is_none() => target = Some(tok),
                        // Extra tokens beyond the target are ignored, same
                        // leniency the old single-token parse already had.
                        _ => {}
                    }
                }
                match target {
                    Some(target) => InputAction::Connect { target: target.to_string(), tls, ip_version },
                    None => InputAction::Unknown(line.to_string()),
                }
            }
        }
        "disconnect" => {
            let network = arg.split(' ').next().filter(|s| !s.is_empty()).map(str::to_string);
            InputAction::Disconnect(network)
        }
        // irssi: an IRC-operator command, sent to the server as typed
        // (`command_self`); the server decides whether you may.
        "ts" => InputAction::TopicSummary,
        // A mistyped/tab-completed
        // `/die` would otherwise shut down the connected server right
        // now (a netsplit) from one keystroke. irssi's own `-yes`
        // idiom (see `/op` et al. above) gates it the same way
        // `max_wildcard_modes` already does.
        "die" => {
            if !arg.split(' ').any(|t| t.eq_ignore_ascii_case("-yes")) {
                return InputAction::Echo(
                    "-- /die shuts down the connected server right now (netsplit) -- use /die -yes to confirm"
                        .to_string(),
                );
            }
            let rest = arg
                .split(' ')
                .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("-yes"))
                .collect::<Vec<_>>()
                .join(" ");
            InputAction::ServerCommand { command: "DIE".to_string(), rest }
        }
        // irssi's `cmd_unquery`: close the query with `nick`, or the
        // active one. Only ever closes a query, never a channel window.
        "unquery" => {
            let nick = arg.split(' ').next().unwrap_or("");
            if nick.is_empty() {
                return match &state.active_buffer {
                    BufferId::Query(_) => InputAction::CloseBuffer(None),
                    _ => InputAction::Echo("-- not in a query window".to_string()),
                };
            }
            if state.query_buffers.contains_key(&nick.to_lowercase()) {
                InputAction::CloseBuffer(Some(nick.to_string()))
            } else {
                InputAction::Echo(format!("-- No query with {nick}"))
            }
        }
        // irssi's `cmd_ver`: CTCP VERSION to <nick|channel|*>; with no
        // argument, to the query partner (an error anywhere else).
        "ver" => {
            let given = arg.split(' ').next().unwrap_or("");
            let active_query = match &state.active_buffer {
                BufferId::Query(key) => state.query_buffers.get(key).map(|b| b.name.clone()),
                _ => None,
            };
            let target = match (given, active_query) {
                ("", Some(q)) => q,
                ("", None) => return InputAction::Unknown(line.to_string()),
                ("*", _) => match active_target(state) {
                    Some((t, _)) => t,
                    None => return InputAction::Unknown(line.to_string()),
                },
                (t, _) => t.to_string(),
            };
            InputAction::Ctcp { target, command: "VERSION".to_string(), data: None }
        }
        "flushbuffer" => InputAction::Echo("-- log buffers flushed (irust writes log lines immediately)".to_string()),
        "network" | "ircnet" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
                return InputAction::NetworkList;
            }
            let mut parts = trimmed.splitn(2, ' ');
            let verb = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("").trim();
            match verb.to_lowercase().as_str() {
                "remove" | "del" | "delete" => {
                    if rest.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::NetworkRemove(rest.to_lowercase())
                }
                "add" => {
                    let mut tokens = rest.split_whitespace();
                    let (Some(name), Some(target)) = (tokens.next(), tokens.next()) else {
                        return InputAction::Unknown(line.to_string());
                    };
                    let mut tls = Some(true);
                    let mut target_unused = None;
                    let (nick, autojoin, client_cert, client_key, extras) =
                        parse_network_flags(&mut tokens, &mut tls, &mut target_unused);
                    InputAction::NetworkAdd {
                        name: name.to_lowercase(),
                        target: target.to_string(),
                        tls: tls.unwrap_or(true),
                        nick,
                        autojoin,
                        client_cert,
                        client_key,
                        extras,
                    }
                }
                "modify" => {
                    let mut tokens = rest.split_whitespace();
                    let Some(name) = tokens.next() else {
                        return InputAction::Unknown(line.to_string());
                    };
                    let mut target = None;
                    let mut tls = None;
                    let (nick, autojoin, client_cert, client_key, extras) =
                        parse_network_flags(&mut tokens, &mut tls, &mut target);
                    InputAction::NetworkModify {
                        name: name.to_lowercase(),
                        target,
                        tls,
                        nick,
                        autojoin,
                        client_cert,
                        client_key,
                        extras,
                    }
                }
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        "server" => {
            let trimmed = arg.trim();
            let mut parts = trimmed.splitn(2, ' ');
            let verb = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("").trim();
            match verb.to_lowercase().as_str() {
                "" | "list" => InputAction::NetworkList,
                "remove" | "del" | "delete" => {
                    if rest.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::NetworkRemove(rest.to_lowercase())
                }
                "connect" => {
                    if rest.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::Connect { target: rest.split(' ').next().unwrap_or(rest).to_string(), tls: None, ip_version: None }
                }
                "add" => {
                    let mut tokens = rest.split_whitespace();
                    let (Some(name), Some(address)) = (tokens.next(), tokens.next()) else {
                        return InputAction::Unknown(line.to_string());
                    };
                    let mut target = address.to_string();
                    let mut peek = tokens.clone();
                    if let Some(maybe_port) = peek.next() {
                        if !maybe_port.starts_with('-') {
                            target = format!("{address}:{maybe_port}");
                            tokens = peek;
                        }
                    }
                    let mut tls = Some(true);
                    let mut target_unused = None;
                    let (nick, autojoin, client_cert, client_key, extras) =
                        parse_network_flags(&mut tokens, &mut tls, &mut target_unused);
                    InputAction::NetworkAdd {
                        name: name.to_lowercase(),
                        target,
                        tls: tls.unwrap_or(true),
                        nick,
                        autojoin,
                        client_cert,
                        client_key,
                        extras,
                    }
                }
                "modify" => {
                    let mut tokens = rest.split_whitespace();
                    let Some(name) = tokens.next() else {
                        return InputAction::Unknown(line.to_string());
                    };
                    let mut target = None;
                    let mut tls = None;
                    let (nick, autojoin, client_cert, client_key, extras) =
                        parse_network_flags(&mut tokens, &mut tls, &mut target);
                    InputAction::NetworkModify {
                        name: name.to_lowercase(),
                        target,
                        tls,
                        nick,
                        autojoin,
                        client_cert,
                        client_key,
                        extras,
                    }
                }
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        "cycle" => {
            let mut words = arg.splitn(2, ' ');
            let first = words.next().unwrap_or("");
            if first.starts_with(['#', '&']) {
                let message = words.next().map(str::to_string).filter(|s| !s.is_empty());
                InputAction::Cycle { channel: Some(first.to_string()), message }
            } else {
                let message = if arg.is_empty() { None } else { Some(arg.to_string()) };
                InputAction::Cycle { channel: None, message }
            }
        }
        "clear" => {
            let all = matches!(arg.trim().to_lowercase().as_str(), "-all" | "all");
            InputAction::ClearView { all }
        }
        "notice" => {
            let mut it = arg.splitn(2, ' ');
            let target = it.next().unwrap_or("").to_string();
            let text = it.next().unwrap_or("").trim().to_string();
            if target.is_empty() || text.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let echo_buffer = if target.starts_with(['#', '&']) {
                BufferId::Channel(target.to_lowercase())
            } else {
                BufferId::Query(target.to_lowercase())
            };
            InputAction::Notice { target, text, echo_buffer }
        }
        "invite" => {
            let mut it = arg.split(' ').filter(|s| !s.is_empty());
            let nick = it.next().unwrap_or("").to_string();
            if nick.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let channel = it.next().map(str::to_string);
            InputAction::Invite { nick, channel }
        }
        "ctcp" => {
            let mut it = arg.splitn(3, ' ');
            let target = it.next().unwrap_or("").to_string();
            let command = it.next().unwrap_or("").to_string();
            if target.is_empty() || command.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let data = it.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            InputAction::Ctcp { target, command: command.to_uppercase(), data }
        }
        "nctcp" => {
            let mut it = arg.splitn(3, ' ');
            let target = it.next().unwrap_or("").to_string();
            let command = it.next().unwrap_or("").to_string();
            if target.is_empty() || command.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let data = it.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            InputAction::Nctcp { target, command: command.to_uppercase(), data }
        }
        "action" => {
            let mut it = arg.splitn(2, ' ');
            let target = it.next().unwrap_or("").to_string();
            let text = it.next().map(str::trim).unwrap_or("").to_string();
            if target.is_empty() || text.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let echo_buffer = if target.starts_with(['#', '&']) {
                BufferId::Channel(target.to_lowercase())
            } else {
                BufferId::Query(target.to_lowercase())
            };
            InputAction::Action { target, text, echo_buffer }
        }
        "wall" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let (channel, text) = if trimmed.starts_with(['#', '&']) {
                let mut it = trimmed.splitn(2, ' ');
                let chan = it.next().unwrap_or("").to_string();
                let text = it.next().map(str::trim).unwrap_or("").to_string();
                (Some(chan), text)
            } else {
                (None, trimmed.to_string())
            };
            if text.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Wall { channel, text }
        }
        "rawlog" => {
            let trimmed = arg.trim();
            let mut parts = trimmed.split_whitespace();
            match parts.next() {
                Some("open") => {
                    let path = trimmed.strip_prefix("open").unwrap_or("").trim();
                    if path.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::RawlogOpen(path.to_string())
                }
                Some("close") => InputAction::RawlogClose,
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        "scrollback" => {
            let trimmed = arg.trim();
            let mut parts = trimmed.split_whitespace();
            match parts.next() {
                Some("home") => InputAction::ScrollbackHome,
                Some("end") => InputAction::ScrollbackEnd,
                Some("status") => InputAction::ScrollbackStatus,
                Some("goto") => {
                    let rest = trimmed.strip_prefix("goto").unwrap_or("").trim();
                    if let Some(n) = rest.strip_prefix('+').and_then(|s| s.trim().parse::<i64>().ok()) {
                        InputAction::ScrollbackGoto(ScrollbackGoto::Relative(n))
                    } else if let Some(n) = rest.strip_prefix('-').and_then(|s| s.trim().parse::<i64>().ok()) {
                        InputAction::ScrollbackGoto(ScrollbackGoto::Relative(-n))
                    } else if let Ok(n) = rest.parse::<usize>() {
                        InputAction::ScrollbackGoto(ScrollbackGoto::Line(n))
                    } else if let Some(target) = parse_scrollback_timestamp(rest) {
                        InputAction::ScrollbackGoto(target)
                    } else {
                        // Genuinely unparseable -- empty, or garbage
                        // that doesn't fit any of the 3 forms above.
                        InputAction::Unknown(line.to_string())
                    }
                }
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        "foreach" => {
            let trimmed = arg.trim();
            let mut parts = trimmed.splitn(2, ' ');
            let scope = match parts.next() {
                Some("window") => ForeachScope::Window,
                Some("channel") => ForeachScope::Channel,
                Some("query") => ForeachScope::Query,
                Some("server") => ForeachScope::Server,
                _ => return InputAction::Unknown(line.to_string()),
            };
            let command = parts.next().unwrap_or("").trim();
            if command.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Foreach { scope, command: command.to_string() }
        }
        "query" | "q" => {
            let mut it = arg.splitn(2, ' ');
            let nick = it.next().filter(|s| !s.is_empty()).map(str::to_string);
            let message = it.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            InputAction::Query { nick, message }
        }
        "whois" | "wi" => {
            let nick = if arg.is_empty() {
                // irssi's own default when no nick is given: whoever
                // you're querying, if the active window is a query --
                // otherwise yourself.
                match &state.active_buffer {
                    BufferId::Query(key) => state
                        .query_buffers
                        .get(key)
                        .map(|b| b.name.clone())
                        .unwrap_or_else(|| state.nick.clone()),
                    _ => state.nick.clone(),
                }
            } else {
                // Only the first word -- WHOIS takes a single nick (or a
                // comma list on some servers), not a trailing reason.
                arg.split(' ').next().unwrap_or(arg).to_string()
            };
            InputAction::Whois(nick)
        }
        "ip" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Ip(arg.split(' ').next().unwrap_or(arg).to_string())
        }
        "away" => InputAction::Away {
            reason: if arg.is_empty() { None } else { Some(arg.to_string()) },
        },
        // irssi's own default alias: BACK = "AWAY" -- always clears,
        // regardless of any trailing text.
        "back" => InputAction::Away { reason: None },
        "quit" => InputAction::Quit {
            reason: if arg.is_empty() {
                None
            } else {
                Some(arg.to_string())
            },
        },
        "quote" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::ServerCommand { command: arg.to_string(), rest: String::new() }
        }
        "oper" | "rehash" | "knock" | "links" | "lusers" | "map" | "motd" | "stats" | "trace"
        | "admin" | "time" | "version" | "userhost" | "ison" | "accept" | "silence" | "whowas"
        | "info" | "kline" | "unkline" | "dline" | "undline" | "xline" | "unxline"
        | "resv" | "unresv" | "gline" | "ungline" | "testline" | "operwall" | "grant"
        | "chghost" | "sethost" | "close" | "unreject" | "testmask"
        | "etrace" | "operspy" | "snote" | "list" | "hash"
        // `SQUERY`/`SERVLIST` -- irssi has no dedicated `cmd_squery`/
        // `cmd_servlist` handler either (checked irssi's own source),
        // its own default is the exact same raw passthrough irust now
        // does.
        | "squery" | "servlist" => {
            InputAction::ServerCommand { command: cmd.to_uppercase(), rest: arg.to_string() }
        }
        // `/sconnect <new server> [[<port>] <existing server>]` -- irssi's
        // `cmd_sconnect` (irc-commands.c) is just `CONNECT <args>`: an oper
        // command asking a server to link to another one. Named
        // differently from `/connect` (which connects OUR client).
        "sconnect" => {
            if arg.trim().is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            // No `-yes` gate (a bad
            // link is reversible with `/squit`), but still worth a
            // visible heads-up before asking a server to link another one.
            InputAction::EchoThenServerCommand {
                echo: format!("-- asking the server to CONNECT {}", arg.trim()),
                command: "CONNECT".to_string(),
                rest: arg.trim().to_string(),
            }
        }
        "kill" => {
            let mut it = arg.splitn(2, ' ');
            let nick = it.next().unwrap_or("").to_string();
            if nick.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let reason = it.next().unwrap_or("").trim();
            let rest = if reason.is_empty() { nick } else { format!("{nick} :{reason}") };
            InputAction::ServerCommand { command: "KILL".to_string(), rest }
        }
        // `/unsilence <mask>` -- irssi parity: irssi's own `cmd_unsilence`
        // just sends `SILENCE -<mask>` raw, same server command `/silence`
        // above already passes through -- reused directly rather than a
        // dedicated `InputAction`/handler.
        "unsilence" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::ServerCommand { command: "SILENCE".to_string(), rest: format!("-{trimmed}") }
        }
        "lastlog" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let mut parts = trimmed.split_whitespace();
            let mut is_regex = false;
            let mut case_sensitive = false;
            let mut next = parts.next().unwrap_or("");
            loop {
                match next {
                    "-regex" => {
                        is_regex = true;
                        next = parts.next().unwrap_or("");
                    }
                    "-case" => {
                        case_sensitive = true;
                        next = parts.next().unwrap_or("");
                    }
                    _ => break,
                }
            }
            let mut rest_tokens: Vec<&str> = Vec::new();
            if !next.is_empty() {
                rest_tokens.push(next);
            }
            rest_tokens.extend(parts);
            if rest_tokens.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            // Irssi's own trailing-number convention: the LAST token, if
            // it parses as a plain integer AND there's still at least
            // one token left for the pattern itself, is a result-count
            // cap rather than part of the pattern.
            let (count, pattern_tokens): (Option<usize>, &[&str]) = match rest_tokens.split_last() {
                Some((last, init)) if !init.is_empty() && last.parse::<usize>().is_ok() => {
                    (last.parse::<usize>().ok(), init)
                }
                _ => (None, &rest_tokens),
            };
            let pattern = pattern_tokens.join(" ");
            if is_regex && !crate::nicklist::is_valid_regex(&pattern) {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Lastlog { pattern, is_regex, case_sensitive, count }
        }
        "log" => {
            let trimmed = arg.trim();
            let mut parts = trimmed.split_whitespace();
            match parts.next() {
                Some("start") | Some("stop") => {
                    let action =
                        if trimmed.starts_with("start") { LogToggle::Start } else { LogToggle::Stop };
                    let target = parts.next();
                    if parts.next().is_some() {
                        return InputAction::Unknown(line.to_string());
                    }
                    match target {
                        None => InputAction::Log { action, target: None },
                        Some(t) => match t.parse::<usize>() {
                            Ok(n) if n != 0 => InputAction::LogByIndex { action, index: n },
                            _ => InputAction::Log { action, target: Some(t.to_string()) },
                        },
                    }
                }
                Some("open") => {
                    let path = trimmed.strip_prefix("open").unwrap_or("").trim();
                    if path.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::Log { action: LogToggle::Open(path.to_string()), target: None }
                }
                Some("close") => InputAction::Log { action: LogToggle::Close, target: None },
                Some(_) => InputAction::Unknown(line.to_string()),
                None => InputAction::LogStatus,
            }
        }
        "notify" => InputAction::Notify {
            nick: if arg.is_empty() { None } else { Some(arg.split(' ').next().unwrap_or(arg).to_string()) },
        },
        "unnotify" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Unnotify { nick: arg.split(' ').next().unwrap_or(arg).to_string() }
        }
        "reconnect" => InputAction::Reconnect,
        "rmreconns" => InputAction::RmReconns,
        "netsplit" => InputAction::Netsplit,
        "channel" => {
            let trimmed = arg.trim();
            let mut parts = trimmed.split_whitespace();
            match parts.next() {
                None | Some("list") => InputAction::ChannelList,
                Some("remove") => {
                    let (Some(channel), Some(network)) = (parts.next(), parts.next()) else {
                        return InputAction::Unknown(line.to_string());
                    };
                    if parts.next().is_some() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::ChannelRemove { channel: channel.to_string(), network: network.to_string() }
                }
                Some(verb @ ("add" | "modify")) => {
                    let action = if verb == "add" { ChannelSetupAction::Add } else { ChannelSetupAction::Modify };
                    let (Some(channel), Some(network)) = (parts.next(), parts.next()) else {
                        return InputAction::Unknown(line.to_string());
                    };
                    let mut autojoin = None;
                    let mut botmasks = None;
                    let mut botcmd = None;
                    let mut password = None;
                    while let Some(tok) = parts.next() {
                        match tok {
                            "-auto" => autojoin = Some(true),
                            "-noauto" => autojoin = Some(false),
                            "-bots" => {
                                let Some(masks) = parts.next() else {
                                    return InputAction::Unknown(line.to_string());
                                };
                                let parsed: Vec<String> =
                                    masks.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
                                if parsed.is_empty() {
                                    return InputAction::Unknown(line.to_string());
                                }
                                botmasks = Some(parsed);
                            }
                            "-botcmd" => {
                                // Takes the rest of the line -- must be
                                // the last flag, see this variant's own
                                // doc comment.
                                let rest: Vec<&str> = parts.by_ref().collect();
                                if rest.is_empty() {
                                    return InputAction::Unknown(line.to_string());
                                }
                                botcmd = Some(rest.join(" "));
                            }
                            _ if password.is_none() => password = Some(tok.to_string()),
                            _ => return InputAction::Unknown(line.to_string()),
                        }
                    }
                    InputAction::ChannelSetup {
                        action,
                        channel: channel.to_string(),
                        network: network.to_string(),
                        autojoin,
                        botmasks,
                        botcmd,
                        password,
                    }
                }
                Some(_) => InputAction::Unknown(line.to_string()),
            }
        }
        // `help` is irssi's/most other clients' own name for this --
        // plain familiarity for anyone arriving from elsewhere, not a
        // second command with different behavior.
        "cmds" | "help" => InputAction::Cmds(if arg.is_empty() { None } else { Some(arg.to_lowercase()) }),
        // irssi's `/resize` (`fe-text/term.c` `cmd_resize`) re-reads the
        // terminal size and relays out; ratatui re-queries the size on
        // every draw, so the full repaint `/redraw` forces is the same act.
        "redraw" | "resize" => InputAction::Redraw,
        "update" => InputAction::Update,
        "upgrade" => InputAction::Upgrade,
        "restart" => InputAction::Restart,
        "pluginreload" => {
            InputAction::PluginReload(if arg.is_empty() { None } else { Some(arg.to_string()) })
        }
        "pluginload" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::PluginLoad(arg.split(' ').next().unwrap_or(arg).to_string())
        }
        "pluginunload" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::PluginUnload(arg.split(' ').next().unwrap_or(arg).to_string())
        }
        "pluginexec" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::PluginExec(arg.to_string())
        }
        "pluginreset" => InputAction::PluginReset,
        "rmrejoins" => InputAction::RmRejoins,
        // irssi's own names for the same actions above -- `/load`,
        // `/unload`, `/reload` and `/script <sub>` are just other
        // spellings of `/pluginload`, `/pluginunload`, `/pluginreload`
        // and `/pluginexec`/`/pluginreset`, kept for anyone arriving
        // from irssi (same reasoning as `cmds`/`help` above).
        "load" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::PluginLoad(arg.split(' ').next().unwrap_or(arg).to_string())
        }
        "unload" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::PluginUnload(arg.split(' ').next().unwrap_or(arg).to_string())
        }
        "reload" => {
            InputAction::PluginReload(if arg.is_empty() { None } else { Some(arg.to_string()) })
        }
        "script" => {
            let trimmed = arg.trim();
            let mut parts = trimmed.splitn(2, ' ');
            match parts.next() {
                Some("load") => match parts.next() {
                    Some(name) if !name.is_empty() => InputAction::PluginLoad(name.to_string()),
                    _ => InputAction::Unknown(line.to_string()),
                },
                Some("unload") => match parts.next() {
                    Some(name) if !name.is_empty() => InputAction::PluginUnload(name.to_string()),
                    _ => InputAction::Unknown(line.to_string()),
                },
                Some("exec") => match parts.next() {
                    Some(code) if !code.is_empty() => InputAction::PluginExec(code.to_string()),
                    _ => InputAction::Unknown(line.to_string()),
                },
                Some("reset") => InputAction::PluginReset,
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        // `/ping <nick>` reuses `/ctcp`'s already-working runtime path
        // verbatim -- no dedicated handling needed beyond building the
        // same action `/ctcp <nick> PING` would. Bare `/ping` (no
        // target) is different: it manually fires the existing lag-
        // measuring PING/PONG round trip instead.
        "ping" => {
            if arg.is_empty() {
                InputAction::Ping
            } else {
                InputAction::Ctcp {
                    target: arg.split(' ').next().unwrap_or(arg).to_string(),
                    command: "PING".to_string(),
                    data: None,
                }
            }
        }
        // NAMES/WHO with no explicit channel default to the active one
        // (irssi's own cmd_names/cmd_who), not a bare server-wide query.
        "names" | "who" => {
            let target = if arg.is_empty() {
                match &state.active_buffer {
                    BufferId::Channel(key) => state.channels.get(key).map(|c| c.display_name.clone()),
                    _ => None,
                }
            } else {
                Some(arg.to_string())
            };
            match target {
                Some(t) => InputAction::ServerCommand { command: cmd.to_uppercase(), rest: t },
                None => InputAction::Unknown(line.to_string()),
            }
        }
        "squit" => {
            let mut it = arg.splitn(2, ' ');
            let server = it.next().unwrap_or("").to_string();
            if server.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let reason = it.next().unwrap_or("").trim().to_string();
            InputAction::Squit { server, reason }
        }
        "wallops" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Wallops { message: arg.to_string() }
        }
        "window" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            let mut it = trimmed.splitn(2, ' ');
            let first = it.next().unwrap_or("");
            if first.eq_ignore_ascii_case("list") {
                InputAction::WindowList
            } else if first.eq_ignore_ascii_case("name") {
                let name = it.next().unwrap_or("").trim().to_string();
                if name.is_empty() {
                    return InputAction::Unknown(line.to_string());
                }
                InputAction::WindowName(name)
            } else if first.eq_ignore_ascii_case("item") {
                let rest = it.next().unwrap_or("").trim();
                let mut sub = rest.splitn(2, ' ');
                match sub.next().unwrap_or("").to_lowercase().as_str() {
                    "" | "next" => InputAction::WindowItemCycle(true),
                    "prev" | "previous" => InputAction::WindowItemCycle(false),
                    "remove" => InputAction::WindowItemRemove,
                    "add" => {
                        let name = sub.next().unwrap_or("").trim().to_string();
                        if name.is_empty() {
                            return InputAction::Unknown(line.to_string());
                        }
                        InputAction::WindowItemAdd(name)
                    }
                    "goto" => {
                        let name = sub.next().unwrap_or("").trim().to_string();
                        if name.is_empty() {
                            return InputAction::Unknown(line.to_string());
                        }
                        InputAction::WindowItemGoto(name)
                    }
                    // irssi: a number moves the ACTIVE item to that window;
                    // a name moves that item to the CURRENT window.
                    "move" => {
                        let target = sub.next().unwrap_or("").trim().to_string();
                        match target.parse::<usize>() {
                            Ok(n) => InputAction::WindowItemMove(n),
                            Err(_) if !target.is_empty() => InputAction::WindowItemAdd(target),
                            Err(_) => InputAction::Unknown(line.to_string()),
                        }
                    }
                    _ => InputAction::Unknown(line.to_string()),
                }
            } else if first.eq_ignore_ascii_case("new") {
                InputAction::WindowNew
            } else if first.eq_ignore_ascii_case("immortal") {
                let value = it.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                InputAction::WindowImmortal(value)
            } else if first.eq_ignore_ascii_case("stick") {
                InputAction::WindowStick
            } else if first.eq_ignore_ascii_case("logfile") {
                let path = it.next().unwrap_or("").trim();
                if path.is_empty() {
                    return InputAction::Unknown(line.to_string());
                }
                InputAction::Log { action: LogToggle::Open(path.to_string()), target: None }
            } else if first.eq_ignore_ascii_case("log") {
                let rest = it.next().unwrap_or("").trim();
                let mut sub = rest.splitn(2, ' ');
                let verb = sub.next().unwrap_or("").to_lowercase();
                let path = sub.next().map(str::trim).filter(|s| !s.is_empty());
                let currently_on = state.current_buffer().is_logging();
                let turn_on = match verb.as_str() {
                    "on" => true,
                    "off" => false,
                    "toggle" => !currently_on,
                    _ => return InputAction::Unknown(line.to_string()),
                };
                match (turn_on, path) {
                    (true, Some(p)) => InputAction::Log { action: LogToggle::Open(p.to_string()), target: None },
                    (true, None) => InputAction::Log { action: LogToggle::Start, target: None },
                    (false, _) => InputAction::Log { action: LogToggle::Close, target: None },
                }
            } else if first.eq_ignore_ascii_case("hide") {
                let rest = it.next().unwrap_or("").trim();
                if rest.is_empty() {
                    InputAction::Unsplit(None)
                } else {
                    match rest.parse::<usize>() {
                        Ok(n) if n != 0 => InputAction::UnsplitByIndex(n),
                        _ => InputAction::Unsplit(Some(rest.to_string())),
                    }
                }
            } else if first.eq_ignore_ascii_case("show") {
                let rest = it.next().unwrap_or("").trim();
                let mut orientation = None;
                let mut target: Option<&str> = None;
                for tok in rest.split_whitespace() {
                    match tok {
                        "-right" => orientation = Some(SplitOrientation::Vertical),
                        _ if target.is_none() => target = Some(tok),
                        _ => {}
                    }
                }
                match target {
                    None => InputAction::Unknown(line.to_string()),
                    Some(t) => match t.parse::<usize>() {
                        Ok(n) if n != 0 => InputAction::SplitByIndex { index: n, orientation },
                        _ => InputAction::Split { target: t.to_string(), orientation },
                    },
                }
            } else if first.eq_ignore_ascii_case("server") {
                // `/window server <name>` -- switches the active window
                // to that network, same `SwitchServer` action the bare
                // `/server <name>` used before `/server` itself was
                // freed up for irssi's own meaning (saved-server-entry
                // management: add/connect/list/modify/remove).
                let name = it.next().unwrap_or("").trim();
                if name.is_empty() {
                    return InputAction::Unknown(line.to_string());
                }
                InputAction::SwitchServer(name.split(' ').next().unwrap_or(name).to_string())
            } else if let Ok(n) = first.parse::<usize>() {
                if n == 0 {
                    return InputAction::Unknown(line.to_string());
                }
                InputAction::SwitchBufferByIndex(n)
            } else {
                InputAction::SwitchBuffer(trimmed.to_string())
            }
        }
        "alias" => {
            let mut it = arg.splitn(2, ' ');
            let name = it.next().filter(|s| !s.is_empty()).map(|s| s.to_lowercase());
            let command = it.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            InputAction::Alias { name, command }
        }
        "unalias" => {
            let name = arg.trim();
            if name.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Unalias(name.to_lowercase())
        }
        "eval" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Eval(arg.to_string())
        }
        "wait" => {
            let msecs = arg
                .split_whitespace()
                .find(|tok| !tok.starts_with('-'))
                .and_then(|tok| tok.parse::<u64>().ok());
            match msecs {
                Some(ms) => InputAction::Wait(ms),
                None => InputAction::Unknown(line.to_string()),
            }
        }
        "echo" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Echo(arg.to_string())
        }
        "toggle" => {
            let mut it = arg.splitn(2, ' ');
            let key = it.next().filter(|s| !s.is_empty()).map(|s| s.to_lowercase());
            let Some(key) = key else {
                return InputAction::Unknown(line.to_string());
            };
            let value = it.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            InputAction::Toggle { key, value }
        }
        "beep" => InputAction::Beep,
        "cat" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Cat(arg.to_string())
        }
        "cd" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Cd(arg.to_string())
        }
        "set" => {
            let mut it = arg.splitn(2, ' ');
            let key = it.next().filter(|s| !s.is_empty()).map(|s| s.to_lowercase());
            let value = it.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            InputAction::Set { key, value }
        }
        "format" => {
            let mut it = arg.splitn(2, ' ');
            let first = it.next().unwrap_or("");
            let (mode, rest) = match first {
                "-delete" => (FormatMode::Delete, it.next().unwrap_or("")),
                "-reset" => (FormatMode::Reset, it.next().unwrap_or("")),
                _ => (FormatMode::GetOrSet, arg),
            };
            let mut it2 = rest.splitn(2, ' ');
            let name = it2.next().filter(|s| !s.is_empty()).map(|s| s.to_lowercase());
            let template = it2.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            if matches!(mode, FormatMode::Delete | FormatMode::Reset) && name.is_none() {
                return InputAction::Unknown(line.to_string());
            }
            let template = if matches!(mode, FormatMode::Delete | FormatMode::Reset) { None } else { template };
            InputAction::Format { name, template, mode }
        }
        "recode" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Recode(RecodeSub::List);
            }
            let mut it = trimmed.splitn(2, ' ');
            let sub = it.next().unwrap_or("").to_lowercase();
            let rest = it.next().map(str::trim).unwrap_or("");
            match sub.as_str() {
                "add" => {
                    let mut parts = rest.splitn(2, ' ');
                    let first = parts.next().unwrap_or("");
                    let second = parts.next().map(str::trim).filter(|s| !s.is_empty());
                    if first.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    let (target, charset) = match second {
                        Some(charset) => (Some(first.to_string()), charset.to_string()),
                        None => (None, first.to_string()),
                    };
                    InputAction::Recode(RecodeSub::Add { target, charset })
                }
                "remove" => {
                    let target = if rest.is_empty() { None } else { Some(rest.to_string()) };
                    InputAction::Recode(RecodeSub::Remove { target })
                }
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        "exec" => {
            if arg.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Exec(arg.to_string())
        }
        "uptime" => InputAction::Uptime,
        "clientversion" => InputAction::ClientVersion,
        "setname" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                InputAction::Unknown(line.to_string())
            } else {
                InputAction::SetName(trimmed.to_string())
            }
        }
        "completion" => {
            let trimmed = arg.trim();
            if let Some(rest) = trimmed.strip_prefix("-delete") {
                let key = rest.trim();
                if key.is_empty() {
                    return InputAction::Unknown(line.to_string());
                }
                return InputAction::Completion {
                    key: Some(key.to_string()),
                    value: None,
                    delete: true,
                };
            }
            let mut it = trimmed.splitn(2, ' ');
            let key = it.next().filter(|s| !s.is_empty()).map(|s| s.to_string());
            let value = it.next().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            InputAction::Completion { key, value, delete: false }
        }
        "ignore" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Ignore {
                    mask: None,
                    levels: Vec::new(),
                    is_regex: false,
                    is_except: false,
                    is_replies: false,
                    time_secs: None,
                };
            }
            let mut parts = trimmed.split_whitespace();
            let mut is_regex = false;
            let mut is_except = false;
            let mut is_replies = false;
            let mut time_secs = None;
            let mut next = parts.next().unwrap_or("");
            loop {
                match next {
                    "-regex" => {
                        is_regex = true;
                        next = parts.next().unwrap_or("");
                    }
                    "-except" => {
                        is_except = true;
                        next = parts.next().unwrap_or("");
                    }
                    "-replies" => {
                        is_replies = true;
                        next = parts.next().unwrap_or("");
                    }
                    "-time" => {
                        let duration = parts.next().unwrap_or("");
                        let Some(secs) = irust_config::ignores::parse_time_interval(duration) else {
                            return InputAction::Unknown(line.to_string());
                        };
                        time_secs = Some(secs);
                        next = parts.next().unwrap_or("");
                    }
                    _ => break,
                }
            }
            let mask = next.to_string();
            if mask.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            if is_regex && !crate::nicklist::is_valid_regex(&mask) {
                return InputAction::Unknown(line.to_string());
            }
            let mut levels = Vec::new();
            for word in parts {
                let lower = word.to_lowercase();
                if lower == "all" {
                    // Explicit "everything" -- same as giving no levels
                    // at all, just spelled out (irssi's own vocabulary).
                    levels.clear();
                    break;
                }
                if !IGNORE_LEVELS.contains(&lower.as_str()) {
                    return InputAction::Unknown(line.to_string());
                }
                levels.push(lower);
            }
            InputAction::Ignore { mask: Some(mask), levels, is_regex, is_except, is_replies, time_secs }
        }
        "unignore" => {
            let mask = arg.trim();
            if mask.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Unignore(mask.to_string())
        }
        "hilight" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Hilight {
                    pattern: None,
                    is_regex: false,
                    is_mask: false,
                    channels: Vec::new(),
                    color: None,
                    actcolor: None,
                    priority: None,
                };
            }
            let mut parts = trimmed.split_whitespace();
            let mut is_regex = false;
            let mut is_mask = false;
            let mut channels = Vec::new();
            let mut color = None;
            let mut actcolor = None;
            let mut priority = None;
            let mut next = parts.next().unwrap_or("");
            loop {
                match next {
                    "-regex" => {
                        is_regex = true;
                        next = parts.next().unwrap_or("");
                    }
                    "-mask" => {
                        is_mask = true;
                        next = parts.next().unwrap_or("");
                    }
                    "-channels" => {
                        let Some(list) = parts.next() else { return InputAction::Unknown(line.to_string()) };
                        channels = list.split(',').filter(|c| !c.is_empty()).map(str::to_string).collect();
                        next = parts.next().unwrap_or("");
                    }
                    "-color" => {
                        let Some(c) = parts.next() else { return InputAction::Unknown(line.to_string()) };
                        color = Some(c.to_string());
                        next = parts.next().unwrap_or("");
                    }
                    "-actcolor" => {
                        let Some(c) = parts.next() else { return InputAction::Unknown(line.to_string()) };
                        actcolor = Some(c.to_string());
                        next = parts.next().unwrap_or("");
                    }
                    "-priority" => {
                        let Some(p) = parts.next().and_then(|p| p.parse::<i32>().ok()) else {
                            return InputAction::Unknown(line.to_string());
                        };
                        priority = Some(p);
                        next = parts.next().unwrap_or("");
                    }
                    _ => break,
                }
            }
            let pattern = next.to_string();
            if pattern.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            if is_regex && !crate::nicklist::is_valid_regex(&pattern) {
                // Same reasoning as `/ignore -regex`: reject a bad
                // pattern up front rather than silently storing an
                // entry that will never match anything.
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Hilight { pattern: Some(pattern), is_regex, is_mask, channels, color, actcolor, priority }
        }
        "dehilight" => {
            let word = arg.trim();
            if word.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Dehilight(word.to_string())
        }
        "autoban" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Autoban { pattern: None, is_regex: false, reason: None };
            }
            let mut parts = trimmed.split_whitespace();
            let mut is_regex = false;
            let mut reason = None;
            let mut next = parts.next().unwrap_or("");
            loop {
                match next {
                    "-regex" => {
                        is_regex = true;
                        next = parts.next().unwrap_or("");
                    }
                    "-reason" => {
                        let Some(r) = parts.next() else { return InputAction::Unknown(line.to_string()) };
                        reason = Some(r.to_string());
                        next = parts.next().unwrap_or("");
                    }
                    _ => break,
                }
            }
            let pattern = next.to_string();
            if pattern.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            if is_regex && !crate::nicklist::is_valid_regex(&pattern) {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Autoban { pattern: Some(pattern), is_regex, reason }
        }
        "unautoban" => {
            let word = arg.trim();
            if word.is_empty() {
                return InputAction::Unknown(line.to_string());
            }
            InputAction::Unautoban(word.to_string())
        }
        "statusbar" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
                return InputAction::StatusbarList;
            }
            let mut parts = trimmed.splitn(2, ' ');
            let verb = parts.next().unwrap_or("");
            let rest = parts.next().unwrap_or("").trim();
            match verb.to_lowercase().as_str() {
                "reset" => InputAction::StatusbarReset,
                "info" => {
                    if rest.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::StatusbarInfo(rest.to_string())
                }
                "remove" | "del" | "delete" => {
                    if rest.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::StatusbarRemove(rest.to_string())
                }
                "add" => {
                    let mut tokens = rest.split_whitespace();
                    let mut before = None;
                    let mut after = None;
                    let mut alignment = None;
                    let mut item = None;
                    while let Some(tok) = tokens.next() {
                        match tok {
                            "-before" => before = tokens.next().map(str::to_string),
                            "-after" => after = tokens.next().map(str::to_string),
                            "-alignment" => alignment = tokens.next().map(str::to_string),
                            _ if item.is_none() => item = Some(tok.to_string()),
                            _ => {}
                        }
                    }
                    match item {
                        Some(item) => InputAction::StatusbarAdd { item, before, after, alignment },
                        None => InputAction::Unknown(line.to_string()),
                    }
                }
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        "layout" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
                return InputAction::LayoutList;
            }
            match trimmed.to_lowercase().as_str() {
                "save" => InputAction::LayoutSave,
                "reset" => InputAction::LayoutReset,
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        "bind" => {
            let trimmed = arg.trim();
            if trimmed.is_empty() {
                return InputAction::Bind { key: None, action: None, delete: false, reset: false };
            }
            if trimmed.eq_ignore_ascii_case("-reset") {
                return InputAction::Bind { key: None, action: None, delete: false, reset: true };
            }
            if let Some(rest) = trimmed.strip_prefix("-delete") {
                let key = match crate::keybind::normalize_key_name(rest.trim()) {
                    Some(k) => k,
                    None => return InputAction::Unknown(line.to_string()),
                };
                return InputAction::Bind { key: Some(key), action: None, delete: true, reset: false };
            }
            let mut it = trimmed.splitn(2, ' ');
            let key = it.next().unwrap_or("");
            let action = it.next().map(str::trim).unwrap_or("");
            let (Some(key), true) = (crate::keybind::normalize_key_name(key), crate::keybind::is_valid_action(action))
            else {
                return InputAction::Unknown(line.to_string());
            };
            InputAction::Bind { key: Some(key), action: Some(action.to_string()), delete: false, reset: false }
        }
        "dcc" => {
            let mut it = arg.splitn(2, ' ');
            let sub = it.next().unwrap_or("").to_lowercase();
            let rest = it.next().map(str::trim).unwrap_or("");
            match sub.as_str() {
                "chat" => {
                    if rest.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::DccChat(rest.to_string())
                }
                "send" => {
                    let (passive, rest) = match rest.strip_prefix("-passive") {
                        Some(after) => (true, after.trim_start()),
                        None => (false, rest),
                    };
                    let mut parts = rest.splitn(2, ' ');
                    let nick = parts.next().unwrap_or("");
                    let path = parts.next().map(str::trim).unwrap_or("");
                    if nick.is_empty() || path.is_empty() {
                        return InputAction::Unknown(line.to_string());
                    }
                    InputAction::DccSend { nick: nick.to_string(), path: path.to_string(), passive }
                }
                "get" => match rest.parse::<u64>() {
                    Ok(id) => InputAction::DccGet(id),
                    Err(_) => InputAction::Unknown(line.to_string()),
                },
                "resume" => match rest.parse::<u64>() {
                    Ok(id) => InputAction::DccResume(id),
                    Err(_) => InputAction::Unknown(line.to_string()),
                },
                "list" => InputAction::DccList,
                "close" => match rest.parse::<u64>() {
                    Ok(id) => InputAction::DccClose(id),
                    Err(_) => InputAction::Unknown(line.to_string()),
                },
                _ => InputAction::Unknown(line.to_string()),
            }
        }
        other => {
            if irust_config::settings_net::bool_of("window_number_commands") {
                if let Ok(n) = other.parse::<usize>() {
                    if n != 0 {
                        return InputAction::SwitchBufferByIndex(n);
                    }
                }
            }
            let service_target = if crate::service_notices::is_service_nick(other) {
                Some(raw_cmd.to_string())
            } else {
                crate::service_notices::resolve_service_alias(other).map(str::to_string)
            };
            if let Some(target) = service_target {
                if !arg.is_empty() {
                    // From the status window there's no channel to echo
                    // into -- fall back to the service's own query
                    // buffer instead (created on demand), same place a
                    // `/msg` to it would land.
                    let echo_buffer = active_target(state)
                        .map(|(_, buf)| buf)
                        .unwrap_or_else(|| BufferId::Query(target.to_lowercase()));
                    return InputAction::SendMessage {
                        target,
                        text: arg.to_string(),
                        echo_buffer,
                    };
                }
            }
            InputAction::Command {
                name: other.to_string(),
                arg: arg.to_string(),
            }
        }
    }
}

fn parse_scrollback_timestamp(rest: &str) -> Option<ScrollbackGoto> {
    let mut tokens = rest.split_whitespace();
    let first = tokens.next()?;
    let second = tokens.next();
    if tokens.next().is_some() {
        return None; // too many tokens for any real form
    }

    let (date, time_str) = match second {
        // Bare `hh:mi[:ss]` -- irssi's own "only time given" case.
        None => (None, first),
        Some(time_str) => {
            if let Some(days) = first.strip_prefix('-').and_then(|s| s.parse::<u32>().ok()) {
                (Some(ScrollbackGotoDate::DaysAgo(days)), time_str)
            } else {
                let mut date_parts = first.splitn(2, '.');
                let day: u32 = date_parts.next()?.parse().ok()?;
                if !(1..=31).contains(&day) {
                    return None;
                }
                let month = match date_parts.next() {
                    Some(m) => {
                        let month: u32 = m.parse().ok()?;
                        if !(1..=12).contains(&month) {
                            return None;
                        }
                        Some(month)
                    }
                    None => None,
                };
                (Some(ScrollbackGotoDate::OnDay { day, month }), time_str)
            }
        }
    };

    let mut hms = time_str.splitn(3, ':');
    let hour: u32 = hms.next()?.parse().ok()?;
    let minute: u32 = hms.next()?.parse().ok()?;
    let second: u32 = match hms.next() {
        Some(s) => s.parse().ok()?,
        None => 0,
    };
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    Some(ScrollbackGoto::Timestamp {
        date,
        hour,
        minute,
        second,
    })
}

pub fn resolve_scrollback_timestamp(
    date: Option<ScrollbackGotoDate>,
    hour: u32,
    minute: u32,
    second: u32,
    now: chrono::DateTime<chrono::Local>,
) -> Option<i64> {
    use chrono::Datelike;

    let today = now.date_naive();
    let bare_time_only = date.is_none();

    let base_date = match date {
        None => today,
        Some(ScrollbackGotoDate::DaysAgo(days)) => today.checked_sub_signed(chrono::Duration::days(days as i64))?,
        Some(ScrollbackGotoDate::OnDay { day, month }) => {
            let mut year = today.year();
            let mut month0 = today.month0(); // 0-based, matches irssi's own tm_mon
            match month {
                Some(m) => {
                    let target_month0 = m - 1;
                    if target_month0 > month0 {
                        year -= 1;
                    }
                    month0 = target_month0;
                }
                None => {
                    if day > today.day() {
                        if month0 > 0 {
                            month0 -= 1;
                        } else {
                            year -= 1;
                            month0 = 11;
                        }
                    }
                }
            }
            chrono::NaiveDate::from_ymd_opt(year, month0 + 1, day)?
        }
    };

    let naive = base_date.and_hms_opt(hour, minute, second)?;
    let mut resolved = naive.and_local_timezone(chrono::Local).single()?;

    if resolved > now && bare_time_only {
        resolved -= chrono::Duration::days(1);
    }
    if resolved > now {
        return None;
    }

    Some(resolved.timestamp())
}

/// `/join`/`/j` convenience: a bare name with no valid channel prefix
/// (`#`, `&`, `+`, `!`) is treated as shorthand for `#name`, matching
/// irssi/weechat behavior.
fn normalize_channel(arg: &str) -> String {
    if arg.starts_with(['#', '&', '+', '!']) {
        arg.to_string()
    } else {
        format!("#{arg}")
    }
}

const BAN_EVERYONE_EXEMPT_HOST_GLOBS: &[&str] = &["chatnet/bot*", "chatnet/staff*"];

fn ban_everyone_masks(state: &ClientState) -> Vec<String> {
    let BufferId::Channel(key) = &state.active_buffer else {
        return Vec::new();
    };
    let Some(chan) = state.channels.get(key) else {
        return Vec::new();
    };
    chan.nicklist
        .sorted()
        .into_iter()
        .filter(|e| !e.nick.eq_ignore_ascii_case(&state.nick))
        .filter(|e| {
            let Some(host) = chan.nicklist.host_for(&e.nick) else {
                return true;
            };
            !BAN_EVERYONE_EXEMPT_HOST_GLOBS
                .iter()
                .any(|pat| crate::nicklist::matches_glob(pat, host))
        })
        .filter_map(|e| chan.nicklist.ban_mask_for(&e.nick))
        .collect()
}

/// Resolves `/unban`'s (possibly multi-token) argument against the
/// target channel's known ban list -- see `InputAction::Unban`'s doc
/// comment for the exact per-token rules. Order-preserving, de-duplicated
/// (a `-first`/`<id>`/wildcard combination could otherwise pick the same
/// mask twice).
fn resolve_unban_targets(state: &ClientState, channel: &Option<String>, rest: &str) -> Vec<String> {
    let key = channel
        .clone()
        .map(|c| c.to_lowercase())
        .or_else(|| match &state.active_buffer {
            BufferId::Channel(k) => Some(k.clone()),
            _ => None,
        });
    let empty: Vec<String> = Vec::new();
    let known: &[String] = key
        .as_ref()
        .and_then(|k| state.channels.get(k))
        .map_or(&empty, |c| &c.bans);

    let mut result: Vec<String> = Vec::new();
    for token in rest.split_whitespace() {
        let mut matched_known = false;
        if token.eq_ignore_ascii_case("-first") {
            if let Some(m) = known.first() {
                result.push(m.clone());
                matched_known = true;
            }
        } else if token.eq_ignore_ascii_case("-last") {
            if let Some(m) = known.last() {
                result.push(m.clone());
                matched_known = true;
            }
        } else if let Ok(n) = token.parse::<usize>() {
            if n >= 1 {
                if let Some(m) = known.get(n - 1) {
                    result.push(m.clone());
                    matched_known = true;
                }
            }
        } else {
            for m in known {
                if crate::nicklist::matches_glob(token, m) {
                    result.push(m.clone());
                    matched_known = true;
                }
            }
        }
        if !matched_known {
            result.push(resolve_ban_target(state, channel, token));
        }
    }
    let mut seen = std::collections::HashSet::new();
    result.retain(|m| seen.insert(m.clone()));
    result
}

fn resolve_ban_target(state: &ClientState, channel: &Option<String>, token: &str) -> String {
    if token.contains(['!', '@']) {
        return token.to_string();
    }
    let key = channel
        .clone()
        .map(|c| c.to_lowercase())
        .or_else(|| match &state.active_buffer {
            BufferId::Channel(k) => Some(k.clone()),
            _ => None,
        });
    key.and_then(|k| state.channels.get(&k))
        .and_then(|c| c.nicklist.ban_mask_for(token))
        .unwrap_or_else(|| token.to_string())
}

/// Resolves the currently focused buffer to a wire target + echo buffer,
/// shared by plain-text sends and `/me`. `None` when the status window
/// is active -- there's no channel/query behind it to address.
pub(crate) fn active_target(state: &ClientState) -> Option<(String, BufferId)> {
    match &state.active_buffer {
        BufferId::Status => None,
        // An empty window (`/window new`) has no channel/query behind
        // it either -- same "nothing to address" outcome as Status.
        BufferId::Empty(_) => None,
        BufferId::Channel(key) => {
            let display = state
                .channels
                .get(key)
                .map(|c| c.display_name.clone())
                .unwrap_or_else(|| key.clone());
            Some((display, state.active_buffer.clone()))
        }
        BufferId::Query(key) => {
            let display = state
                .query_buffers
                .get(key)
                .map(|b| b.name.clone())
                .unwrap_or_else(|| key.clone());
            Some((display, state.active_buffer.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn state() -> ClientState {
        ClientState::new("abcX", "#irust-test", 11)
    }

    /// `completion_auto` is a process-wide global, so the tests that flip
    /// it take turns.
    static COMPLETION_AUTO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn completion_auto_expands_a_partial_nick_before_the_colon_only_when_on() {
        let _g = COMPLETION_AUTO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = state();
        st.channels.get_mut("#irust-test").unwrap().nicklist.insert_plain("alice");
        let sent = |st: &ClientState| match parse_input(st, "ali: hi there") {
            InputAction::SendMessage { text, .. } => text,
            other => panic!("expected a message, got {other:?}"),
        };
        assert_eq!(sent(&st), "ali: hi there", "off by default: sent as typed");

        let mut settings = irust_config::settings::Settings::default();
        settings.set("completion_auto", "on").unwrap();
        assert_eq!(sent(&st), "alice: hi there");
        settings.set("completion_auto", "off").unwrap();
        assert_eq!(sent(&st), "ali: hi there");
    }

    #[test]
    fn ts_lists_the_topics_of_joined_channels() {
        assert_eq!(parse_input(&state(), "/ts"), InputAction::TopicSummary);
    }

    #[test]
    fn die_needs_dash_yes_or_it_only_warns() {
        assert!(matches!(parse_input(&state(), "/die"), InputAction::Echo(_)), "no -yes: must not DIE yet");
        assert!(matches!(parse_input(&state(), "/die maintenance"), InputAction::Echo(_)), "no -yes: must not DIE yet");
        assert_eq!(
            parse_input(&state(), "/die -yes"),
            InputAction::ServerCommand { command: "DIE".to_string(), rest: String::new() }
        );
        assert_eq!(
            parse_input(&state(), "/die maintenance -yes"),
            InputAction::ServerCommand { command: "DIE".to_string(), rest: "maintenance".to_string() }
        );
    }

    #[test]
    fn ircnet_is_the_network_command() {
        assert_eq!(parse_input(&state(), "/ircnet"), parse_input(&state(), "/network"));
        assert_eq!(parse_input(&state(), "/ircnet list"), InputAction::NetworkList);
    }

    #[test]
    fn unquery_only_closes_queries() {
        let mut st = state();
        assert_eq!(
            parse_input(&st, "/unquery"),
            InputAction::Echo("-- not in a query window".to_string()),
            "from a channel window there is no query to close"
        );
        st.get_or_create_query_buffer("Pal", "Pal");
        assert_eq!(parse_input(&st, "/unquery pal"), InputAction::CloseBuffer(Some("pal".to_string())));
        assert_eq!(
            parse_input(&st, "/unquery #irust-test"),
            InputAction::Echo("-- No query with #irust-test".to_string()),
            "a channel name is never closed by /unquery"
        );
        st.active_buffer = BufferId::Query("pal".to_string());
        assert_eq!(parse_input(&st, "/unquery"), InputAction::CloseBuffer(None));
    }

    #[test]
    fn ver_sends_ctcp_version_to_the_target_the_query_partner_or_the_channel() {
        let mut st = state();
        let ver = |t: &str| InputAction::Ctcp { target: t.to_string(), command: "VERSION".to_string(), data: None };
        assert_eq!(parse_input(&st, "/ver alice"), ver("alice"));
        assert_eq!(parse_input(&st, "/ver *"), ver("#irust-test"));
        assert_eq!(parse_input(&st, "/ver"), InputAction::Unknown("/ver".to_string()), "no target outside a query");
        st.get_or_create_query_buffer("Pal", "Pal");
        st.active_buffer = BufferId::Query("pal".to_string());
        assert_eq!(parse_input(&st, "/ver"), ver("Pal"));
    }

    #[test]
    fn flushbuffer_reports_that_logs_are_unbuffered() {
        assert!(matches!(parse_input(&state(), "/flushbuffer"), InputAction::Echo(t) if t.contains("flushed")));
    }

    #[test]
    fn strip_cmdchar_uses_the_default_slash_cmdchar() {
        assert_eq!(strip_cmdchar("/", "/join #chan"), Some("join #chan"));
        assert_eq!(strip_cmdchar("/", "ciao"), None);
    }

    #[test]
    fn strip_cmdchar_recognizes_a_second_configured_cmdchar() {
        assert_eq!(strip_cmdchar("/.", "/join #chan"), Some("join #chan"));
        assert_eq!(strip_cmdchar("/.", ".join #chan"), Some("join #chan"));
    }

    #[test]
    fn strip_cmdchar_treats_the_unconfigured_char_as_plain_text() {
        assert_eq!(strip_cmdchar("/.", "!join #chan"), None);
    }

    #[test]
    fn plain_text_goes_to_the_active_channel() {
        let action = parse_input(&state(), "ciao a tutti");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "#irust-test".to_string(),
                text: "ciao a tutti".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn plain_text_while_viewing_a_query_goes_to_that_query() {
        let mut st = state();
        st.get_or_create_query_buffer("k9", "K9");
        st.active_buffer = BufferId::Query("k9".to_string());
        let action = parse_input(&st, "COMMANDS");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "K9".to_string(),
                text: "COMMANDS".to_string(),
                echo_buffer: BufferId::Query("k9".to_string()),
            }
        );
    }

    #[test]
    fn msg_to_a_nick_echoes_into_a_query_buffer() {
        let action = parse_input(&state(), "/msg K9 HELP");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "K9".to_string(),
                text: "HELP".to_string(),
                echo_buffer: BufferId::Query("k9".to_string()),
            }
        );
    }

    #[test]
    fn msg_to_a_channel_echoes_into_that_channel_buffer() {
        let action = parse_input(&state(), "/msg #other hello there");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "#other".to_string(),
                text: "hello there".to_string(),
                echo_buffer: BufferId::Channel("#other".to_string()),
            }
        );
    }

    #[test]
    fn msg_with_no_text_is_unknown() {
        let action = parse_input(&state(), "/msg K9");
        assert_eq!(action, InputAction::Unknown("/msg K9".to_string()));
    }

    #[test]
    fn me_targets_the_active_buffer() {
        let action = parse_input(&state(), "/me waves");
        assert_eq!(
            action,
            InputAction::Action {
                target: "#irust-test".to_string(),
                text: "waves".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn me_with_no_text_is_unknown() {
        let action = parse_input(&state(), "/me");
        assert_eq!(action, InputAction::Unknown("/me".to_string()));
    }

    #[test]
    fn join_with_a_channel() {
        let action = parse_input(&state(), "/join #newchan");
        assert_eq!(
            action,
            InputAction::Join {
                channel: "#newchan".to_string(),
                key: None
            }
        );
    }

    #[test]
    fn join_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/join");
        assert_eq!(action, InputAction::Unknown("/join".to_string()));
    }

    #[test]
    fn join_without_hash_prefix_gets_one_added() {
        let action = parse_input(&state(), "/join newchan");
        assert_eq!(
            action,
            InputAction::Join {
                channel: "#newchan".to_string(),
                key: None
            }
        );
    }

    #[test]
    fn j_is_an_alias_for_join_and_also_normalizes() {
        let action = parse_input(&state(), "/j newchan");
        assert_eq!(
            action,
            InputAction::Join {
                channel: "#newchan".to_string(),
                key: None
            }
        );

        let action = parse_input(&state(), "/j &localchan");
        assert_eq!(
            action,
            InputAction::Join {
                channel: "&localchan".to_string(),
                key: None
            }
        );
    }

    #[test]
    fn join_with_a_key_keeps_it_separate_from_the_channel() {
        let action = parse_input(&state(), "/join #newchan secretkey");
        assert_eq!(
            action,
            InputAction::Join {
                channel: "#newchan".to_string(),
                key: Some("secretkey".to_string())
            }
        );
    }

    #[test]
    fn part_with_no_argument_means_the_active_channel_no_message() {
        let action = parse_input(&state(), "/part");
        assert_eq!(
            action,
            InputAction::Part {
                channel: None,
                message: None
            }
        );
    }

    #[test]
    fn part_with_just_a_message_targets_the_active_channel() {
        let action = parse_input(&state(), "/part goodbye for now");
        assert_eq!(
            action,
            InputAction::Part {
                channel: None,
                message: Some("goodbye for now".to_string())
            }
        );
    }

    #[test]
    fn part_with_an_explicit_channel_no_message() {
        let action = parse_input(&state(), "/part #other");
        assert_eq!(
            action,
            InputAction::Part {
                channel: Some("#other".to_string()),
                message: None
            }
        );
    }

    #[test]
    fn part_with_an_explicit_channel_and_message() {
        let action = parse_input(&state(), "/part #other see ya");
        assert_eq!(
            action,
            InputAction::Part {
                channel: Some("#other".to_string()),
                message: Some("see ya".to_string()),
            }
        );
    }

    #[test]
    fn nick_with_a_new_name() {
        let action = parse_input(&state(), "/nick newname");
        assert_eq!(action, InputAction::Nick("newname".to_string()));
    }

    #[test]
    fn nick_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/nick");
        assert_eq!(action, InputAction::Unknown("/nick".to_string()));
    }

    #[test]
    fn topic_with_no_argument_queries() {
        let action = parse_input(&state(), "/topic");
        assert_eq!(
            action,
            InputAction::Topic {
                channel: None,
                text: None
            }
        );
    }

    #[test]
    fn topic_with_text_sets_it() {
        let action = parse_input(&state(), "/topic new topic text");
        assert_eq!(
            action,
            InputAction::Topic {
                channel: None,
                text: Some("new topic text".to_string())
            }
        );
    }

    #[test]
    fn topic_with_an_explicit_channel_and_text() {
        let action = parse_input(&state(), "/topic #other new topic text");
        assert_eq!(
            action,
            InputAction::Topic {
                channel: Some("#other".to_string()),
                text: Some("new topic text".to_string()),
            }
        );
    }

    #[test]
    fn topic_with_an_explicit_channel_and_no_text_queries_it() {
        let action = parse_input(&state(), "/topic #other");
        assert_eq!(
            action,
            InputAction::Topic {
                channel: Some("#other".to_string()),
                text: None
            }
        );
    }

    #[test]
    fn topic_dash_delete_clears_it_with_an_explicit_empty_string() {
        let action = parse_input(&state(), "/topic -delete");
        assert_eq!(
            action,
            InputAction::Topic {
                channel: None,
                text: Some(String::new())
            }
        );

        let action = parse_input(&state(), "/topic #other -delete");
        assert_eq!(
            action,
            InputAction::Topic {
                channel: Some("#other".to_string()),
                text: Some(String::new())
            }
        );
    }

    #[test]
    fn kick_with_a_reason() {
        let action = parse_input(&state(), "/kick Baduser being rude");
        assert_eq!(
            action,
            InputAction::Kick {
                channel: None,
                nick: "Baduser".to_string(),
                reason: Some("being rude".to_string()),
            }
        );
    }

    #[test]
    fn kick_with_no_reason() {
        let action = parse_input(&state(), "/kick Baduser");
        assert_eq!(
            action,
            InputAction::Kick {
                channel: None,
                nick: "Baduser".to_string(),
                reason: None
            }
        );
    }

    #[test]
    fn kick_with_an_explicit_channel_and_reason() {
        let action = parse_input(&state(), "/kick #other Baduser being rude");
        assert_eq!(
            action,
            InputAction::Kick {
                channel: Some("#other".to_string()),
                nick: "Baduser".to_string(),
                reason: Some("being rude".to_string()),
            }
        );
    }

    #[test]
    fn kick_with_an_explicit_channel_and_no_reason() {
        let action = parse_input(&state(), "/kick #other Baduser");
        assert_eq!(
            action,
            InputAction::Kick {
                channel: Some("#other".to_string()),
                nick: "Baduser".to_string(),
                reason: None
            }
        );
    }

    #[test]
    fn kick_with_only_a_channel_and_no_nick_is_unknown() {
        let action = parse_input(&state(), "/kick #other");
        assert_eq!(action, InputAction::Unknown("/kick #other".to_string()));
    }

    #[test]
    fn kick_with_no_nick_is_unknown() {
        let action = parse_input(&state(), "/kick");
        assert_eq!(action, InputAction::Unknown("/kick".to_string()));
    }

    #[test]
    fn ban_with_no_argument_queries_the_list() {
        let action = parse_input(&state(), "/ban");
        assert_eq!(
            action,
            InputAction::Ban {
                channel: None,
                mask: None
            }
        );
    }

    #[test]
    fn ban_with_a_literal_mask_passes_it_through_unchanged() {
        let action = parse_input(&state(), "/ban *!*@spammer.example");
        assert_eq!(
            action,
            InputAction::Ban {
                channel: None,
                mask: Some("*!*@spammer.example".to_string())
            }
        );
    }

    #[test]
    fn ban_with_a_bare_nick_resolves_via_the_nicklist() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.insert_plain("Baduser");
        chan.nicklist
            .set_userhost("Baduser", Some("~bad"), Some("dyn1.spammer.example"));

        let action = parse_input(&st, "/ban Baduser");
        assert_eq!(
            action,
            InputAction::Ban {
                channel: None,
                mask: Some("*!*bad@*.spammer.example".to_string())
            }
        );
    }

    #[test]
    fn ban_with_a_bare_nick_and_unknown_host_falls_back_to_the_literal_nick() {
        let action = parse_input(&state(), "/ban Baduser");
        assert_eq!(
            action,
            InputAction::Ban {
                channel: None,
                mask: Some("Baduser".to_string())
            }
        );
    }

    #[test]
    fn ban_with_an_explicit_channel() {
        let action = parse_input(&state(), "/ban #other *!*@spammer.example");
        assert_eq!(
            action,
            InputAction::Ban {
                channel: Some("#other".to_string()),
                mask: Some("*!*@spammer.example".to_string()),
            }
        );
    }

    #[test]
    fn ban_star_bans_everyone_except_self_batched_into_one_channelmode() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.insert_plain("abcX");
        chan.nicklist
            .set_userhost("abcX", Some("abcX"), Some("chatnet/staff/sh"));
        chan.nicklist.insert_plain("Alice");
        chan.nicklist
            .set_userhost("Alice", Some("alice"), Some("dyn1.example.net"));
        chan.nicklist.insert_plain("Bob");
        chan.nicklist.set_userhost("Bob", Some("bob"), Some("dyn2.example.net"));

        let action = parse_input(&st, "/ban *");
        let InputAction::ChannelMode { modestring, args } = action else {
            panic!("expected ChannelMode");
        };
        assert_eq!(modestring, "+bb");
        assert_eq!(args.len(), 2);
        assert!(args.contains(&"*!*alice@*.example.net".to_string()));
        assert!(args.contains(&"*!*bob@*.example.net".to_string()));
    }

    #[test]
    fn ban_star_exempts_bots_and_staff_cloaks() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.insert_plain("SomeBot");
        chan.nicklist
            .set_userhost("SomeBot", Some("bot"), Some("chatnet/bot/somebot"));
        chan.nicklist.insert_plain("Staffer");
        chan.nicklist
            .set_userhost("Staffer", Some("staffer"), Some("chatnet/staff/staffer"));
        chan.nicklist.insert_plain("Rando");
        chan.nicklist
            .set_userhost("Rando", Some("rando"), Some("dyn1.example.net"));

        let action = parse_input(&st, "/ban *");
        let InputAction::ChannelMode { args, .. } = action else {
            panic!("expected ChannelMode");
        };
        assert_eq!(args, vec!["*!*rando@*.example.net".to_string()]);
    }

    #[test]
    fn ban_star_with_nobody_banable_is_unknown() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.insert_plain("abcX");
        // No other members, and no known host for the user's own nick
        // (self is excluded anyway) -- nothing to ban.
        let action = parse_input(&st, "/ban *");
        assert_eq!(action, InputAction::Unknown("/ban *".to_string()));
    }

    #[test]
    fn unban_with_no_mask_is_unknown() {
        let action = parse_input(&state(), "/unban");
        assert_eq!(action, InputAction::Unknown("/unban".to_string()));
    }

    #[test]
    fn unban_with_a_literal_mask() {
        let action = parse_input(&state(), "/unban *!*@spammer.example");
        assert_eq!(
            action,
            InputAction::Unban {
                channel: None,
                masks: vec!["*!*@spammer.example".to_string()]
            }
        );
    }

    #[test]
    fn unban_first_and_last_pick_the_oldest_and_newest_known_ban() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.bans = vec!["*!*@one.example".to_string(), "*!*@two.example".to_string()];

        assert_eq!(
            parse_input(&st, "/unban -first"),
            InputAction::Unban {
                channel: None,
                masks: vec!["*!*@one.example".to_string()]
            }
        );
        assert_eq!(
            parse_input(&st, "/unban -last"),
            InputAction::Unban {
                channel: None,
                masks: vec!["*!*@two.example".to_string()]
            }
        );
    }

    #[test]
    fn unban_by_numeric_id_is_one_based() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.bans = vec!["*!*@one.example".to_string(), "*!*@two.example".to_string()];

        assert_eq!(
            parse_input(&st, "/unban 2"),
            InputAction::Unban {
                channel: None,
                masks: vec!["*!*@two.example".to_string()]
            }
        );
    }

    #[test]
    fn unban_star_removes_every_known_ban() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.bans = vec!["*!*@one.example".to_string(), "*!*@two.example".to_string()];

        assert_eq!(
            parse_input(&st, "/unban *"),
            InputAction::Unban {
                channel: None,
                masks: vec!["*!*@one.example".to_string(), "*!*@two.example".to_string()]
            }
        );
    }

    #[test]
    fn unban_out_of_range_id_falls_back_to_literal_token() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.bans = vec!["*!*@one.example".to_string()];

        // No 2nd entry -- "2" isn't a known index, so it's treated as a
        // best-effort literal mask/nick token instead (no such nick, so
        // it stays "2" verbatim).
        assert_eq!(
            parse_input(&st, "/unban 2"),
            InputAction::Unban {
                channel: None,
                masks: vec!["2".to_string()]
            }
        );
    }

    #[test]
    fn bind_with_no_args_lists() {
        let action = parse_input(&state(), "/bind");
        assert_eq!(
            action,
            InputAction::Bind {
                key: None,
                action: None,
                delete: false,
                reset: false
            }
        );
    }

    #[test]
    fn bind_reset_clears_everything() {
        let action = parse_input(&state(), "/bind -reset");
        assert_eq!(
            action,
            InputAction::Bind {
                key: None,
                action: None,
                delete: false,
                reset: true
            }
        );
    }

    #[test]
    fn bind_delete_normalizes_the_key() {
        let action = parse_input(&state(), "/bind -delete Ctrl-N");
        assert_eq!(
            action,
            InputAction::Bind {
                key: Some("ctrl-n".to_string()),
                action: None,
                delete: true,
                reset: false
            }
        );
    }

    #[test]
    fn bind_sets_a_normalized_key_to_a_valid_action() {
        let action = parse_input(&state(), "/bind Ctrl-A history_up");
        assert_eq!(
            action,
            InputAction::Bind {
                key: Some("ctrl-a".to_string()),
                action: Some("history_up".to_string()),
                delete: false,
                reset: false,
            }
        );
    }

    #[test]
    fn bind_with_an_unrecognized_key_is_unknown() {
        let action = parse_input(&state(), "/bind F1 history_up");
        assert_eq!(action, InputAction::Unknown("/bind F1 history_up".to_string()));
    }

    #[test]
    fn bind_with_an_invalid_action_is_unknown() {
        let action = parse_input(&state(), "/bind ctrl-a nonexistent_action");
        assert_eq!(
            action,
            InputAction::Unknown("/bind ctrl-a nonexistent_action".to_string())
        );
    }

    #[test]
    fn dcc_chat_with_a_nick() {
        let action = parse_input(&state(), "/dcc chat someone");
        assert_eq!(action, InputAction::DccChat("someone".to_string()));
    }

    #[test]
    fn dcc_with_no_nick_is_unknown() {
        let action = parse_input(&state(), "/dcc chat");
        assert_eq!(action, InputAction::Unknown("/dcc chat".to_string()));
    }

    #[test]
    fn dcc_send_parses_nick_and_path_active_by_default() {
        let action = parse_input(&state(), "/dcc send someone file.txt");
        assert_eq!(
            action,
            InputAction::DccSend {
                nick: "someone".to_string(),
                path: "file.txt".to_string(),
                passive: false
            }
        );
    }

    #[test]
    fn dcc_send_passive_flag_is_parsed() {
        let action = parse_input(&state(), "/dcc send -passive someone file.txt");
        assert_eq!(
            action,
            InputAction::DccSend {
                nick: "someone".to_string(),
                path: "file.txt".to_string(),
                passive: true
            }
        );
    }

    #[test]
    fn dcc_send_with_a_path_containing_spaces() {
        let action = parse_input(&state(), "/dcc send someone my file.txt");
        assert_eq!(
            action,
            InputAction::DccSend {
                nick: "someone".to_string(),
                path: "my file.txt".to_string(),
                passive: false
            }
        );
    }

    #[test]
    fn dcc_send_missing_path_is_unknown() {
        let action = parse_input(&state(), "/dcc send someone");
        assert_eq!(action, InputAction::Unknown("/dcc send someone".to_string()));
    }

    #[test]
    fn dcc_get_and_close_parse_a_numeric_id() {
        assert_eq!(parse_input(&state(), "/dcc get 3"), InputAction::DccGet(3));
        assert_eq!(parse_input(&state(), "/dcc close 3"), InputAction::DccClose(3));
        assert_eq!(
            parse_input(&state(), "/dcc get abc"),
            InputAction::Unknown("/dcc get abc".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/dcc close"),
            InputAction::Unknown("/dcc close".to_string())
        );
    }

    #[test]
    fn dcc_resume_parses_a_numeric_id() {
        assert_eq!(parse_input(&state(), "/dcc resume 3"), InputAction::DccResume(3));
        assert_eq!(
            parse_input(&state(), "/dcc resume abc"),
            InputAction::Unknown("/dcc resume abc".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/dcc resume"),
            InputAction::Unknown("/dcc resume".to_string())
        );
    }

    #[test]
    fn dcc_list_takes_no_args() {
        assert_eq!(parse_input(&state(), "/dcc list"), InputAction::DccList);
    }

    #[test]
    fn kickban_resolves_the_mask_and_kicks_and_bans_the_same_nick() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.insert_plain("Baduser");
        chan.nicklist
            .set_userhost("Baduser", Some("bad"), Some("chatnet/staff/evil"));

        let action = parse_input(&st, "/kickban Baduser being rude");
        assert_eq!(
            action,
            InputAction::Kickban {
                channel: None,
                nick: "Baduser".to_string(),
                mask: "*!*bad@chatnet/staff/evil".to_string(),
                reason: Some("being rude".to_string()),
            }
        );
    }

    #[test]
    fn kickban_with_no_nick_is_unknown() {
        let action = parse_input(&state(), "/kickban");
        assert_eq!(action, InputAction::Unknown("/kickban".to_string()));
    }

    #[test]
    fn quiet_with_a_literal_mask_passes_it_through_unchanged() {
        let action = parse_input(&state(), "/quiet *!*@spammer.example");
        assert_eq!(
            action,
            InputAction::Quiet {
                channel: None,
                mask: "*!*@spammer.example".to_string()
            }
        );
    }

    #[test]
    fn quiet_with_a_bare_nick_resolves_via_the_nicklist() {
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.insert_plain("Loudmouth");
        chan.nicklist
            .set_userhost("Loudmouth", Some("~lm"), Some("dyn2.spammer.example"));

        let action = parse_input(&st, "/quiet Loudmouth");
        assert_eq!(
            action,
            InputAction::Quiet {
                channel: None,
                mask: "*!*lm@*.spammer.example".to_string()
            }
        );
    }

    #[test]
    fn quiet_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/quiet");
        assert_eq!(action, InputAction::Unknown("/quiet".to_string()));
    }

    #[test]
    fn quiet_with_an_explicit_channel() {
        let action = parse_input(&state(), "/quiet #other *!*@spammer.example");
        assert_eq!(
            action,
            InputAction::Quiet {
                channel: Some("#other".to_string()),
                mask: "*!*@spammer.example".to_string(),
            }
        );
    }

    #[test]
    fn unquiet_with_multiple_masks() {
        let action = parse_input(&state(), "/unquiet *!*@spammer.example *!*@other.example");
        assert_eq!(
            action,
            InputAction::Unquiet {
                channel: None,
                masks: vec!["*!*@spammer.example".to_string(), "*!*@other.example".to_string()],
            }
        );
    }

    #[test]
    fn unquiet_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/unquiet");
        assert_eq!(action, InputAction::Unknown("/unquiet".to_string()));
    }

    #[test]
    fn knockout_with_explicit_minutes_and_a_reason() {
        let action = parse_input(&state(), "/knockout 10 Baduser spamming links");
        assert_eq!(
            action,
            InputAction::Knockout {
                channel: None,
                minutes: Some(10),
                nick: "Baduser".to_string(),
                mask: "Baduser".to_string(),
                reason: Some("spamming links".to_string()),
            }
        );
    }

    #[test]
    fn knockout_without_minutes_defaults_to_none_meaning_irssis_5min_default() {
        let action = parse_input(&state(), "/knockout Baduser spamming links");
        assert_eq!(
            action,
            InputAction::Knockout {
                channel: None,
                minutes: None,
                nick: "Baduser".to_string(),
                mask: "Baduser".to_string(),
                reason: Some("spamming links".to_string()),
            }
        );
    }

    #[test]
    fn knockout_with_no_nick_is_unknown() {
        let action = parse_input(&state(), "/knockout");
        assert_eq!(action, InputAction::Unknown("/knockout".to_string()));
    }

    #[test]
    fn op_deop_voice_devoice_shortcuts_build_a_channel_mode() {
        assert_eq!(
            parse_input(&state(), "/op SomeNick"),
            InputAction::ChannelMode {
                modestring: "+o".to_string(),
                args: vec!["SomeNick".to_string()],
            }
        );
        assert_eq!(
            parse_input(&state(), "/deop SomeNick"),
            InputAction::ChannelMode {
                modestring: "-o".to_string(),
                args: vec!["SomeNick".to_string()],
            }
        );
        assert_eq!(
            parse_input(&state(), "/voice SomeNick"),
            InputAction::ChannelMode {
                modestring: "+v".to_string(),
                args: vec!["SomeNick".to_string()],
            }
        );
        assert_eq!(
            parse_input(&state(), "/devoice SomeNick"),
            InputAction::ChannelMode {
                modestring: "-v".to_string(),
                args: vec!["SomeNick".to_string()],
            }
        );
    }

    #[test]
    fn op_with_no_nick_is_unknown() {
        let action = parse_input(&state(), "/op");
        assert_eq!(action, InputAction::Unknown("/op".to_string()));
    }

    #[test]
    fn op_with_multiple_literal_nicks_batches_into_one_modestring() {
        let action = parse_input(&state(), "/op bob sarah");
        assert_eq!(
            action,
            InputAction::ChannelMode {
                modestring: "+oo".to_string(),
                args: vec!["bob".to_string(), "sarah".to_string()]
            }
        );
    }

    #[test]
    fn op_star_expands_to_everyone_not_already_opped_excluding_self() {
        let _net = crate::test_support::net_guard();
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.add_from_names_token("@already_op");
        chan.nicklist.add_from_names_token("plain_member");
        chan.nicklist.insert_plain("abcX"); // our own nick, from state()

        let action = parse_input(&st, "/op *");
        assert_eq!(
            action,
            InputAction::ChannelMode {
                modestring: "+o".to_string(),
                args: vec!["plain_member".to_string()],
            }
        );
    }

    #[test]
    fn op_wildcard_over_max_wildcard_modes_needs_dash_yes() {
        let _net = crate::test_support::net_guard();
        crate::test_support::net_set("max_wildcard_modes", "2");
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        for n in ["a1", "a2", "a3"] {
            chan.nicklist.insert_plain(n);
        }
        match parse_input(&st, "/op a*") {
            InputAction::Echo(msg) => assert!(msg.contains("Not a good idea"), "{msg}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        match parse_input(&st, "/op -yes a*") {
            InputAction::ChannelMode { modestring, args } => {
                assert_eq!(modestring, "+ooo");
                assert_eq!(args.len(), 3);
            }
            other => panic!("expected the op, got {other:?}"),
        }
        // A literal nick is never counted against the limit.
        assert!(matches!(parse_input(&st, "/op a1 a2 a3"), InputAction::ChannelMode { .. }));
    }

    #[test]
    fn sconnect_sends_a_server_connect_and_needs_an_argument() {
        let st = state();
        assert_eq!(
            parse_input(&st, "/sconnect hub2.example.net 6667 hub1.example.net"),
            InputAction::EchoThenServerCommand {
                echo: "-- asking the server to CONNECT hub2.example.net 6667 hub1.example.net".to_string(),
                command: "CONNECT".to_string(),
                rest: "hub2.example.net 6667 hub1.example.net".to_string()
            }
        );
        assert!(matches!(parse_input(&st, "/sconnect"), InputAction::Unknown(_)));
    }

    #[test]
    fn devoice_star_only_matches_currently_voiced_members() {
        let _net = crate::test_support::net_guard();
        let mut st = state();
        let chan = st.channels.get_mut("#irust-test").unwrap();
        chan.nicklist.add_from_names_token("+voiced_one");
        chan.nicklist.add_from_names_token("plain_member");

        let action = parse_input(&st, "/devoice *");
        assert_eq!(
            action,
            InputAction::ChannelMode {
                modestring: "-v".to_string(),
                args: vec!["voiced_one".to_string()],
            }
        );
    }

    #[test]
    fn wildcard_matching_nobody_is_unknown() {
        let action = parse_input(&state(), "/op nonexistent*");
        assert_eq!(action, InputAction::Unknown("/op nonexistent*".to_string()));
    }

    #[test]
    fn generic_mode_command() {
        let action = parse_input(&state(), "/mode +b *!*@baddomain");
        assert_eq!(
            action,
            InputAction::ChannelMode {
                modestring: "+b".to_string(),
                args: vec!["*!*@baddomain".to_string()],
            }
        );
    }

    #[test]
    fn mode_with_a_leading_target_is_a_user_mode_not_a_channel_mode() {
        let action = parse_input(&state(), "/mode abcx +s cC");
        assert_eq!(
            action,
            InputAction::UserMode {
                target: "abcx".to_string(),
                modestring: "+s".to_string(),
                args: vec!["cC".to_string()],
            }
        );
    }

    #[test]
    fn mode_with_a_leading_target_and_no_modestring_is_unknown() {
        let action = parse_input(&state(), "/mode abcx");
        assert_eq!(action, InputAction::Unknown("/mode abcx".to_string()));
    }

    #[test]
    fn color_sets_an_index() {
        let action = parse_input(&state(), "/color K9 7");
        assert_eq!(
            action,
            InputAction::Color {
                nick: "K9".to_string(),
                index: Some(7),
            }
        );
    }

    #[test]
    fn color_with_no_index_queries() {
        let action = parse_input(&state(), "/color K9");
        assert_eq!(
            action,
            InputAction::Color {
                nick: "K9".to_string(),
                index: None,
            }
        );
    }

    #[test]
    fn color_with_no_nick_is_unknown() {
        let action = parse_input(&state(), "/color");
        assert_eq!(action, InputAction::Unknown("/color".to_string()));
    }

    #[test]
    fn color_with_a_non_numeric_index_is_unknown() {
        let action = parse_input(&state(), "/color K9 red");
        assert_eq!(action, InputAction::Unknown("/color K9 red".to_string()));
    }

    #[test]
    fn wc_closes_the_active_buffer() {
        let action = parse_input(&state(), "/wc");
        assert_eq!(action, InputAction::CloseBuffer(None));
    }

    #[test]
    fn wc_with_an_argument_targets_that_buffer() {
        let action = parse_input(&state(), "/wc #chatnet");
        assert_eq!(action, InputAction::CloseBuffer(Some("#chatnet".to_string())));
    }

    #[test]
    fn wc_with_a_numeric_argument_closes_by_index() {
        let action = parse_input(&state(), "/wc 3");
        assert_eq!(action, InputAction::CloseBufferByIndex(3));
    }

    #[test]
    fn wc_star_closes_every_buffer() {
        assert_eq!(parse_input(&state(), "/wc *"), InputAction::CloseAllBuffers);
    }

    #[test]
    fn wc_all_and_dash_all_are_synonyms_for_star() {
        assert_eq!(parse_input(&state(), "/wc all"), InputAction::CloseAllBuffers);
        assert_eq!(parse_input(&state(), "/wc -all"), InputAction::CloseAllBuffers);
        assert_eq!(parse_input(&state(), "/wc ALL"), InputAction::CloseAllBuffers);
    }

    #[test]
    fn wc_queries_closes_only_queries() {
        assert_eq!(parse_input(&state(), "/wc queries"), InputAction::CloseAllQueries);
        assert_eq!(parse_input(&state(), "/wc -queries"), InputAction::CloseAllQueries);
        assert_eq!(parse_input(&state(), "/wc query"), InputAction::CloseAllQueries);
        assert_eq!(parse_input(&state(), "/wc -query"), InputAction::CloseAllQueries);
        assert_eq!(parse_input(&state(), "/wc QUERIES"), InputAction::CloseAllQueries);
        assert_eq!(parse_input(&state(), "/wc q"), InputAction::CloseAllQueries);
        assert_eq!(parse_input(&state(), "/wc -q"), InputAction::CloseAllQueries);
    }

    #[test]
    fn wc_0_falls_back_to_the_by_name_path_not_an_index() {
        // Same "0 is not a window" rule as `/window 0` -- no buffer is
        // ever named "0" either, so this just ends up reporting "no
        // such buffer" at the handler, same as any other bad name.
        let action = parse_input(&state(), "/wc 0");
        assert_eq!(action, InputAction::CloseBuffer(Some("0".to_string())));
    }

    #[test]
    fn split_with_a_name_targets_that_buffer() {
        let action = parse_input(&state(), "/split #chatnet");
        assert_eq!(
            action,
            InputAction::Split {
                target: "#chatnet".to_string(),
                orientation: None
            }
        );
    }

    #[test]
    fn split_with_a_numeric_argument_targets_by_index() {
        let action = parse_input(&state(), "/split 3");
        assert_eq!(
            action,
            InputAction::SplitByIndex {
                index: 3,
                orientation: None
            }
        );
    }

    #[test]
    fn split_0_falls_back_to_the_by_name_path_not_an_index() {
        // Same "0 is not a window" convention as `/wc 0`/`/window 0`.
        let action = parse_input(&state(), "/split 0");
        assert_eq!(
            action,
            InputAction::Split {
                target: "0".to_string(),
                orientation: None
            }
        );
    }

    #[test]
    fn split_with_no_argument_is_unknown() {
        // Unlike `/wc`, there's no sane "split onto nothing" default --
        // an explicit target is required.
        assert_eq!(
            parse_input(&state(), "/split"),
            InputAction::Unknown("/split".to_string())
        );
    }

    #[test]
    fn split_dash_v_and_dash_h_set_orientation_regardless_of_flag_position() {
        assert_eq!(
            parse_input(&state(), "/split -v #chatnet"),
            InputAction::Split {
                target: "#chatnet".to_string(),
                orientation: Some(SplitOrientation::Vertical)
            }
        );
        assert_eq!(
            parse_input(&state(), "/split #chatnet -h"),
            InputAction::Split {
                target: "#chatnet".to_string(),
                orientation: Some(SplitOrientation::Horizontal)
            }
        );
        assert_eq!(
            parse_input(&state(), "/split -v 3"),
            InputAction::SplitByIndex {
                index: 3,
                orientation: Some(SplitOrientation::Vertical)
            }
        );
    }

    #[test]
    fn unsplit_with_no_argument_closes_the_most_recent_pane() {
        assert_eq!(parse_input(&state(), "/unsplit"), InputAction::Unsplit(None));
    }

    #[test]
    fn unsplit_with_a_name_targets_that_pane() {
        assert_eq!(
            parse_input(&state(), "/unsplit #chatnet"),
            InputAction::Unsplit(Some("#chatnet".to_string()))
        );
    }

    #[test]
    fn unsplit_with_a_numeric_argument_targets_by_index() {
        assert_eq!(parse_input(&state(), "/unsplit 3"), InputAction::UnsplitByIndex(3));
    }

    #[test]
    fn unsplit_star_and_dash_all_close_every_pane() {
        assert_eq!(parse_input(&state(), "/unsplit *"), InputAction::UnsplitAll);
        assert_eq!(parse_input(&state(), "/unsplit -all"), InputAction::UnsplitAll);
        assert_eq!(parse_input(&state(), "/unsplit all"), InputAction::UnsplitAll);
    }

    #[test]
    fn split_focus_with_no_argument_cycles_forward() {
        assert_eq!(
            parse_input(&state(), "/split_focus"),
            InputAction::SplitFocusCycle(true)
        );
    }

    #[test]
    fn split_focus_dash_prev_cycles_backward() {
        assert_eq!(
            parse_input(&state(), "/split_focus -prev"),
            InputAction::SplitFocusCycle(false)
        );
    }

    #[test]
    fn split_focus_with_a_name_or_index_jumps_directly() {
        assert_eq!(
            parse_input(&state(), "/split_focus #chatnet"),
            InputAction::SplitFocus("#chatnet".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/split_focus 2"),
            InputAction::SplitFocusByIndex(2)
        );
    }

    #[test]
    fn netsplit_takes_no_arguments() {
        assert_eq!(parse_input(&state(), "/netsplit"), InputAction::Netsplit);
    }

    #[test]
    fn channel_bare_and_list_both_mean_list() {
        assert_eq!(parse_input(&state(), "/channel"), InputAction::ChannelList);
        assert_eq!(parse_input(&state(), "/channel list"), InputAction::ChannelList);
    }

    #[test]
    fn channel_remove_needs_a_channel_and_network() {
        assert_eq!(
            parse_input(&state(), "/channel remove #chatnet chatnet"),
            InputAction::ChannelRemove {
                channel: "#chatnet".to_string(),
                network: "chatnet".to_string()
            }
        );
        assert_eq!(
            parse_input(&state(), "/channel remove #chatnet"),
            InputAction::Unknown("/channel remove #chatnet".to_string())
        );
    }

    #[test]
    fn channel_add_with_no_flags_leaves_everything_none() {
        assert_eq!(
            parse_input(&state(), "/channel add #chatnet chatnet"),
            InputAction::ChannelSetup {
                action: ChannelSetupAction::Add,
                channel: "#chatnet".to_string(),
                network: "chatnet".to_string(),
                autojoin: None,
                botmasks: None,
                botcmd: None,
                password: None,
            }
        );
    }

    #[test]
    fn channel_add_parses_auto_bots_password_and_botcmd() {
        let action = parse_input(
            &state(),
            "/channel add #chatnet chatnet -auto -bots K9,HelperBot secretkey -botcmd identify hunter2",
        );
        assert_eq!(
            action,
            InputAction::ChannelSetup {
                action: ChannelSetupAction::Add,
                channel: "#chatnet".to_string(),
                network: "chatnet".to_string(),
                autojoin: Some(true),
                botmasks: Some(vec!["K9".to_string(), "HelperBot".to_string()]),
                botcmd: Some("identify hunter2".to_string()),
                password: Some("secretkey".to_string()),
            }
        );
    }

    #[test]
    fn channel_modify_uses_the_modify_action() {
        let action = parse_input(&state(), "/channel modify #chatnet chatnet -noauto");
        assert_eq!(
            action,
            InputAction::ChannelSetup {
                action: ChannelSetupAction::Modify,
                channel: "#chatnet".to_string(),
                network: "chatnet".to_string(),
                autojoin: Some(false),
                botmasks: None,
                botcmd: None,
                password: None,
            }
        );
    }

    #[test]
    fn channel_add_with_no_channel_or_network_is_unknown() {
        assert_eq!(
            parse_input(&state(), "/channel add"),
            InputAction::Unknown("/channel add".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/channel add #chatnet"),
            InputAction::Unknown("/channel add #chatnet".to_string())
        );
    }

    #[test]
    fn channel_add_bots_flag_needs_a_value() {
        assert_eq!(
            parse_input(&state(), "/channel add #chatnet chatnet -bots"),
            InputAction::Unknown("/channel add #chatnet chatnet -bots".to_string())
        );
    }

    #[test]
    fn channel_add_a_second_bare_word_after_password_is_rejected() {
        assert_eq!(
            parse_input(&state(), "/channel add #chatnet chatnet secretkey extraword"),
            InputAction::Unknown("/channel add #chatnet chatnet secretkey extraword".to_string())
        );
    }

    #[test]
    fn channel_unknown_subcommand_is_unknown() {
        assert_eq!(
            parse_input(&state(), "/channel bogus"),
            InputAction::Unknown("/channel bogus".to_string())
        );
    }

    #[test]
    fn log_with_no_argument_reports_status() {
        assert_eq!(parse_input(&state(), "/log"), InputAction::LogStatus);
    }

    #[test]
    fn log_start_and_stop_with_no_target_apply_to_the_current_buffer() {
        assert_eq!(
            parse_input(&state(), "/log start"),
            InputAction::Log {
                action: LogToggle::Start,
                target: None
            }
        );
        assert_eq!(
            parse_input(&state(), "/log stop"),
            InputAction::Log {
                action: LogToggle::Stop,
                target: None
            }
        );
    }

    #[test]
    fn log_start_and_stop_with_a_name_or_index_target_that_window() {
        assert_eq!(
            parse_input(&state(), "/log stop #chatnet"),
            InputAction::Log {
                action: LogToggle::Stop,
                target: Some("#chatnet".to_string())
            }
        );
        assert_eq!(
            parse_input(&state(), "/log start 2"),
            InputAction::LogByIndex {
                action: LogToggle::Start,
                index: 2
            }
        );
    }

    #[test]
    fn log_with_an_unknown_verb_or_trailing_junk_is_unknown() {
        assert_eq!(
            parse_input(&state(), "/log pause"),
            InputAction::Unknown("/log pause".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/log stop #chatnet extra"),
            InputAction::Unknown("/log stop #chatnet extra".to_string())
        );
    }

    #[test]
    fn setname_with_a_name_sets_it() {
        assert_eq!(
            parse_input(&state(), "/setname Alex Smith"),
            InputAction::SetName("Alex Smith".to_string())
        );
    }

    #[test]
    fn setname_with_no_argument_is_unknown() {
        assert_eq!(
            parse_input(&state(), "/setname"),
            InputAction::Unknown("/setname".to_string())
        );
    }

    #[test]
    fn nickcolors_with_no_argument_flips_the_current_state() {
        let mut st = state();
        assert!(!st.nicklist_colors);
        assert_eq!(parse_input(&st, "/nickcolors"), InputAction::NickColors(true));
        st.nicklist_colors = true;
        assert_eq!(parse_input(&st, "/nickcolors"), InputAction::NickColors(false));
    }

    #[test]
    fn nickcolors_on_off_set_explicitly_regardless_of_current_state() {
        let st = state();
        assert_eq!(parse_input(&st, "/nickcolors off"), InputAction::NickColors(false));
        assert_eq!(parse_input(&st, "/nickcolors on"), InputAction::NickColors(true));
    }

    #[test]
    fn nickcolors_with_garbage_argument_is_unknown() {
        let action = parse_input(&state(), "/nickcolors purple");
        assert_eq!(action, InputAction::Unknown("/nickcolors purple".to_string()));
    }

    #[test]
    fn textcolors_defaults_off_and_no_argument_flips_it() {
        let mut st = state();
        assert!(!st.text_colors);
        assert_eq!(parse_input(&st, "/textcolors"), InputAction::TextColors(true));
        st.text_colors = true;
        assert_eq!(parse_input(&st, "/textcolors"), InputAction::TextColors(false));
    }

    #[test]
    fn textcolors_on_off_set_explicitly() {
        let st = state();
        assert_eq!(parse_input(&st, "/textcolors on"), InputAction::TextColors(true));
        assert_eq!(parse_input(&st, "/textcolors off"), InputAction::TextColors(false));
    }

    #[test]
    fn whois_and_its_wi_alias_both_target_the_nick() {
        let action = parse_input(&state(), "/whois abcX");
        assert_eq!(action, InputAction::Whois("abcX".to_string()));

        let action = parse_input(&state(), "/wi abcX");
        assert_eq!(action, InputAction::Whois("abcX".to_string()));
    }

    #[test]
    fn whois_with_no_argument_targets_our_own_nick_while_viewing_a_channel() {
        let action = parse_input(&state(), "/whois");
        assert_eq!(action, InputAction::Whois("abcX".to_string()));
    }

    #[test]
    fn whois_with_no_argument_targets_the_query_counterpart_while_viewing_one() {
        let mut st = state();
        st.query_buffers.insert("k9".to_string(), Buffer::new("K9"));
        st.active_buffer = BufferId::Query("k9".to_string());
        let action = parse_input(&st, "/whois");
        assert_eq!(action, InputAction::Whois("K9".to_string()));
    }

    #[test]
    fn connect_with_a_network_name() {
        let action = parse_input(&state(), "/connect libera");
        assert_eq!(
            action,
            InputAction::Connect {
                target: "libera".to_string(),
                tls: None,
                ip_version: None
            }
        );
    }

    #[test]
    fn connect_with_notls_flag() {
        let action = parse_input(&state(), "/connect -notls irc.example.org");
        assert_eq!(
            action,
            InputAction::Connect {
                target: "irc.example.org".to_string(),
                tls: Some(false),
                ip_version: None
            }
        );
    }

    #[test]
    fn connect_with_ipv6_flag_in_either_order() {
        let action = parse_input(&state(), "/connect -6 irc.example.org");
        assert_eq!(
            action,
            InputAction::Connect {
                target: "irc.example.org".to_string(),
                tls: None,
                ip_version: Some(6)
            }
        );
        let action = parse_input(&state(), "/connect irc.example.org -4");
        assert_eq!(
            action,
            InputAction::Connect {
                target: "irc.example.org".to_string(),
                tls: None,
                ip_version: Some(4)
            }
        );
    }

    #[test]
    fn network_with_no_argument_lists_everything() {
        assert_eq!(parse_input(&state(), "/network"), InputAction::NetworkList);
        assert_eq!(parse_input(&state(), "/network list"), InputAction::NetworkList);
    }

    #[test]
    fn network_add_minimal() {
        let action = parse_input(&state(), "/network add myserver irc.example.org:6667 -notls");
        assert_eq!(
            action,
            InputAction::NetworkAdd {
                name: "myserver".to_string(),
                target: "irc.example.org:6667".to_string(),
                tls: false,
                nick: None,
                autojoin: None,
                client_cert: None,
                client_key: None,
                extras: NetworkExtras::default(),
            }
        );
    }

    #[test]
    fn network_add_with_nick_and_autojoin() {
        let action = parse_input(
            &state(),
            "/network add myserver irc.example.org -nick Bob -autojoin #test",
        );
        assert_eq!(
            action,
            InputAction::NetworkAdd {
                name: "myserver".to_string(),
                target: "irc.example.org".to_string(),
                tls: true,
                nick: Some("Bob".to_string()),
                autojoin: Some("#test".to_string()),
                client_cert: None,
                client_key: None,
                extras: NetworkExtras::default(),
            }
        );
    }

    #[test]
    fn network_add_with_client_cert_and_key() {
        let action = parse_input(
            &state(),
            "/network add myserver irc.example.org -cert /path/client.crt -key /path/client.key",
        );
        assert_eq!(
            action,
            InputAction::NetworkAdd {
                name: "myserver".to_string(),
                target: "irc.example.org".to_string(),
                tls: true,
                nick: None,
                autojoin: None,
                client_cert: Some("/path/client.crt".to_string()),
                client_key: Some("/path/client.key".to_string()),
                extras: NetworkExtras::default(),
            }
        );
    }

    #[test]
    fn network_add_missing_target_is_unknown() {
        let action = parse_input(&state(), "/network add myserver");
        assert_eq!(action, InputAction::Unknown("/network add myserver".to_string()));
    }

    #[test]
    fn network_remove_by_name() {
        let action = parse_input(&state(), "/network remove myserver");
        assert_eq!(action, InputAction::NetworkRemove("myserver".to_string()));
    }

    #[test]
    fn network_remove_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/network remove");
        assert_eq!(action, InputAction::Unknown("/network remove".to_string()));
    }

    #[test]
    fn network_modify_only_sets_fields_actually_given() {
        let action = parse_input(&state(), "/network modify myserver -nick newnick");
        assert_eq!(
            action,
            InputAction::NetworkModify {
                name: "myserver".to_string(),
                target: None,
                tls: None,
                nick: Some("newnick".to_string()),
                autojoin: None,
                client_cert: None,
                client_key: None,
                extras: NetworkExtras::default(),
            }
        );
        let action = parse_input(&state(), "/network modify myserver irc.example.com:6667 -notls");
        assert_eq!(
            action,
            InputAction::NetworkModify {
                name: "myserver".to_string(),
                target: Some("irc.example.com:6667".to_string()),
                tls: Some(false),
                nick: None,
                autojoin: None,
                client_cert: None,
                client_key: None,
                extras: NetworkExtras::default(),
            }
        );
        assert_eq!(
            parse_input(&state(), "/network modify"),
            InputAction::Unknown("/network modify".to_string())
        );
    }

    #[test]
    fn wait_needs_a_millisecond_count() {
        assert_eq!(parse_input(&state(), "/wait 500"), InputAction::Wait(500));
        assert_eq!(
            parse_input(&state(), "/wait"),
            InputAction::Unknown("/wait".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/wait notanumber"),
            InputAction::Unknown("/wait notanumber".to_string())
        );
    }

    #[test]
    fn log_open_and_close() {
        assert_eq!(
            parse_input(&state(), "/log open /tmp/custom.log"),
            InputAction::Log {
                action: LogToggle::Open("/tmp/custom.log".to_string()),
                target: None
            }
        );
        assert_eq!(
            parse_input(&state(), "/log open"),
            InputAction::Unknown("/log open".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/log close"),
            InputAction::Log {
                action: LogToggle::Close,
                target: None
            }
        );
    }

    #[test]
    fn pluginexec_needs_code_pluginreset_takes_no_args() {
        assert_eq!(
            parse_input(&state(), "/pluginexec irust.echo('hi')"),
            InputAction::PluginExec("irust.echo('hi')".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/pluginexec"),
            InputAction::Unknown("/pluginexec".to_string())
        );
        assert_eq!(parse_input(&state(), "/pluginreset"), InputAction::PluginReset);
    }

    #[test]
    fn connect_with_only_flags_and_no_target_is_unknown() {
        let action = parse_input(&state(), "/connect -notls");
        assert_eq!(action, InputAction::Unknown("/connect -notls".to_string()));
    }

    #[test]
    fn connect_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/connect");
        assert_eq!(action, InputAction::Unknown("/connect".to_string()));
    }

    #[test]
    fn disconnect_with_no_argument_means_the_current_server() {
        let action = parse_input(&state(), "/disconnect");
        assert_eq!(action, InputAction::Disconnect(None));
    }

    #[test]
    fn disconnect_with_a_network_name() {
        let action = parse_input(&state(), "/disconnect libera");
        assert_eq!(action, InputAction::Disconnect(Some("libera".to_string())));
    }

    #[test]
    fn window_server_switches_to_a_named_connection() {
        // `/server <name>` moved here once `/server` itself was freed
        // up for irssi's own saved-entry meaning -- see the `server`
        // and `window` tests further below.
        let action = parse_input(&state(), "/window server libera");
        assert_eq!(action, InputAction::SwitchServer("libera".to_string()));
    }

    #[test]
    fn bare_server_lists_saved_entries() {
        let action = parse_input(&state(), "/server");
        assert_eq!(action, InputAction::NetworkList);
    }

    #[test]
    fn server_list_is_also_network_list() {
        let action = parse_input(&state(), "/server list");
        assert_eq!(action, InputAction::NetworkList);
    }

    #[test]
    fn server_add_minimal_with_port() {
        let action = parse_input(&state(), "/server add myserver irc.example.org 6667 -notls");
        assert_eq!(
            action,
            InputAction::NetworkAdd {
                name: "myserver".to_string(),
                target: "irc.example.org:6667".to_string(),
                tls: false,
                nick: None,
                autojoin: None,
                client_cert: None,
                client_key: None,
                extras: NetworkExtras::default(),
            }
        );
    }

    #[test]
    fn server_add_without_a_port() {
        let action = parse_input(&state(), "/server add myserver irc.example.org -nick Bob");
        assert_eq!(
            action,
            InputAction::NetworkAdd {
                name: "myserver".to_string(),
                target: "irc.example.org".to_string(),
                tls: true,
                nick: Some("Bob".to_string()),
                autojoin: None,
                client_cert: None,
                client_key: None,
                extras: NetworkExtras::default(),
            }
        );
    }

    #[test]
    fn server_connect_is_also_connect() {
        let action = parse_input(&state(), "/server connect myserver");
        assert_eq!(
            action,
            InputAction::Connect { target: "myserver".to_string(), tls: None, ip_version: None }
        );
    }

    #[test]
    fn server_modify_changes_target() {
        let action = parse_input(&state(), "/server modify myserver irc.other.org:6697");
        assert_eq!(
            action,
            InputAction::NetworkModify {
                name: "myserver".to_string(),
                target: Some("irc.other.org:6697".to_string()),
                tls: None,
                nick: None,
                autojoin: None,
                client_cert: None,
                client_key: None,
                extras: NetworkExtras::default(),
            }
        );
    }

    #[test]
    fn server_remove_removes_a_saved_entry() {
        let action = parse_input(&state(), "/server remove myserver");
        assert_eq!(action, InputAction::NetworkRemove("myserver".to_string()));
    }

    #[test]
    fn server_with_an_unknown_verb_is_unknown() {
        let action = parse_input(&state(), "/server frobnicate");
        assert_eq!(action, InputAction::Unknown("/server frobnicate".to_string()));
    }

    #[test]
    fn clear_with_no_argument_targets_just_the_active_buffer() {
        let action = parse_input(&state(), "/clear");
        assert_eq!(action, InputAction::ClearView { all: false });
    }

    #[test]
    fn clear_dash_all_or_all_targets_every_buffer() {
        for arg in ["-all", "all", "-ALL"] {
            let action = parse_input(&state(), &format!("/clear {arg}"));
            assert_eq!(action, InputAction::ClearView { all: true });
        }
    }

    #[test]
    fn notice_to_a_channel_vs_a_nick_echoes_into_the_right_kind_of_buffer() {
        let action = parse_input(&state(), "/notice #chan hello there");
        assert_eq!(
            action,
            InputAction::Notice {
                target: "#chan".to_string(),
                text: "hello there".to_string(),
                echo_buffer: BufferId::Channel("#chan".to_string()),
            }
        );

        let action = parse_input(&state(), "/notice SomeNick hey");
        assert_eq!(
            action,
            InputAction::Notice {
                target: "SomeNick".to_string(),
                text: "hey".to_string(),
                echo_buffer: BufferId::Query("somenick".to_string()),
            }
        );
    }

    #[test]
    fn notice_with_no_text_is_unknown() {
        let action = parse_input(&state(), "/notice #chan");
        assert_eq!(action, InputAction::Unknown("/notice #chan".to_string()));
    }

    #[test]
    fn invite_with_no_channel_means_the_active_one() {
        let action = parse_input(&state(), "/invite SomeNick");
        assert_eq!(
            action,
            InputAction::Invite {
                nick: "SomeNick".to_string(),
                channel: None
            }
        );
    }

    #[test]
    fn invite_with_an_explicit_channel() {
        let action = parse_input(&state(), "/invite SomeNick #other");
        assert_eq!(
            action,
            InputAction::Invite {
                nick: "SomeNick".to_string(),
                channel: Some("#other".to_string())
            }
        );
    }

    #[test]
    fn invite_with_no_nick_is_unknown() {
        let action = parse_input(&state(), "/invite");
        assert_eq!(action, InputAction::Unknown("/invite".to_string()));
    }

    #[test]
    fn ctcp_command_gets_uppercased_and_data_is_optional() {
        let action = parse_input(&state(), "/ctcp SomeNick ping hello");
        assert_eq!(
            action,
            InputAction::Ctcp {
                target: "SomeNick".to_string(),
                command: "PING".to_string(),
                data: Some("hello".to_string()),
            }
        );

        let action = parse_input(&state(), "/ctcp SomeNick version");
        assert_eq!(
            action,
            InputAction::Ctcp {
                target: "SomeNick".to_string(),
                command: "VERSION".to_string(),
                data: None
            }
        );
    }

    #[test]
    fn ctcp_with_no_command_is_unknown() {
        let action = parse_input(&state(), "/ctcp SomeNick");
        assert_eq!(action, InputAction::Unknown("/ctcp SomeNick".to_string()));
    }

    #[test]
    fn query_and_its_q_alias_open_a_nick_with_an_optional_message() {
        for cmd in ["query", "q"] {
            let action = parse_input(&state(), &format!("/{cmd} SomeNick hello there"));
            assert_eq!(
                action,
                InputAction::Query {
                    nick: Some("SomeNick".to_string()),
                    message: Some("hello there".to_string()),
                }
            );
        }
    }

    #[test]
    fn query_with_no_argument_at_all() {
        let action = parse_input(&state(), "/query");
        assert_eq!(
            action,
            InputAction::Query {
                nick: None,
                message: None
            }
        );
    }

    #[test]
    fn cycle_with_no_argument_targets_the_active_channel() {
        let action = parse_input(&state(), "/cycle");
        assert_eq!(
            action,
            InputAction::Cycle {
                channel: None,
                message: None
            }
        );
    }

    #[test]
    fn cycle_with_just_a_message_targets_the_active_channel() {
        let action = parse_input(&state(), "/cycle brb rejoining");
        assert_eq!(
            action,
            InputAction::Cycle {
                channel: None,
                message: Some("brb rejoining".to_string()),
            }
        );
    }

    #[test]
    fn cycle_with_an_explicit_channel_and_message() {
        let action = parse_input(&state(), "/cycle #other resetting my state");
        assert_eq!(
            action,
            InputAction::Cycle {
                channel: Some("#other".to_string()),
                message: Some("resetting my state".to_string()),
            }
        );
    }

    #[test]
    fn cycle_with_an_explicit_channel_and_no_message() {
        let action = parse_input(&state(), "/cycle #other");
        assert_eq!(
            action,
            InputAction::Cycle {
                channel: Some("#other".to_string()),
                message: None
            }
        );
    }

    #[test]
    fn away_with_a_reason() {
        let action = parse_input(&state(), "/away brb lunch");
        assert_eq!(
            action,
            InputAction::Away {
                reason: Some("brb lunch".to_string())
            }
        );
    }

    #[test]
    fn away_with_no_reason_clears_it() {
        let action = parse_input(&state(), "/away");
        assert_eq!(action, InputAction::Away { reason: None });
    }

    #[test]
    fn back_always_clears_regardless_of_trailing_text() {
        let action = parse_input(&state(), "/back i'm here");
        assert_eq!(action, InputAction::Away { reason: None });
    }

    #[test]
    fn quit_with_a_reason() {
        let action = parse_input(&state(), "/quit see ya");
        assert_eq!(
            action,
            InputAction::Quit {
                reason: Some("see ya".to_string())
            }
        );
    }

    #[test]
    fn quit_with_no_reason() {
        let action = parse_input(&state(), "/quit");
        assert_eq!(action, InputAction::Quit { reason: None });
    }

    #[test]
    fn simple_server_commands_pass_through_their_args_uppercased_command() {
        let cases: &[(&str, &str, &str)] = &[
            ("oper", "abcX hunter2", "OPER"),
            ("rehash", "", "REHASH"),
            ("knock", "#other", "KNOCK"),
            ("links", "", "LINKS"),
            ("lusers", "", "LUSERS"),
            ("map", "", "MAP"),
            ("motd", "irc.example.com", "MOTD"),
            ("stats", "u", "STATS"),
            ("trace", "", "TRACE"),
            ("admin", "", "ADMIN"),
            ("time", "", "TIME"),
            ("version", "", "VERSION"),
            ("userhost", "abcX dana", "USERHOST"),
            ("ison", "abcX dana", "ISON"),
            ("accept", "abcX", "ACCEPT"),
            ("silence", "*!*@spammer.example", "SILENCE"),
            ("info", "", "INFO"),
            ("kline", "300 *@spammer.example :spam", "KLINE"),
            ("unkline", "*@spammer.example", "UNKLINE"),
            ("dline", "300 1.2.3.4 :abuse", "DLINE"),
            ("undline", "1.2.3.4", "UNDLINE"),
            ("xline", "300 *badword* :forbidden realname", "XLINE"),
            ("unxline", "*badword*", "UNXLINE"),
            ("resv", "300 BadNick :nick reserved", "RESV"),
            ("unresv", "BadNick", "UNRESV"),
            ("gline", "300 *@spammer.example :network-wide spam", "GLINE"),
            ("ungline", "*@spammer.example", "UNGLINE"),
            ("testline", "*@spammer.example", "TESTLINE"),
            ("operwall", "server maintenance in 5 minutes", "OPERWALL"),
            ("grant", "abcX oper", "GRANT"),
            ("chghost", "abcX new.host.example", "CHGHOST"),
            ("sethost", "chatnet/staff/abcX", "SETHOST"),
            ("close", "", "CLOSE"),
            ("unreject", "1.2.3.4", "UNREJECT"),
            ("testmask", "*@1.2.3.4", "TESTMASK"),
            ("etrace", "", "ETRACE"),
            ("operspy", "WHO #DogHouse", "OPERSPY"),
            ("snote", "+c", "SNOTE"),
        ];
        for (cmd, arg, expected_command) in cases {
            let line = if arg.is_empty() {
                format!("/{cmd}")
            } else {
                format!("/{cmd} {arg}")
            };
            let action = parse_input(&state(), &line);
            assert_eq!(
                action,
                InputAction::ServerCommand {
                    command: expected_command.to_string(),
                    rest: arg.to_string()
                },
                "for /{cmd}"
            );
        }
    }

    #[test]
    fn kill_wraps_a_multi_word_reason_in_a_trailing_colon_param() {
        let action = parse_input(&state(), "/kill BadNick spamming all day");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "KILL".to_string(),
                rest: "BadNick :spamming all day".to_string(),
            }
        );
    }

    #[test]
    fn kill_with_no_reason_sends_just_the_nick() {
        let action = parse_input(&state(), "/kill BadNick");
        assert_eq!(
            action,
            InputAction::ServerCommand { command: "KILL".to_string(), rest: "BadNick".to_string() }
        );
    }

    #[test]
    fn kill_with_no_nick_is_unknown() {
        let action = parse_input(&state(), "/kill");
        assert_eq!(action, InputAction::Unknown("/kill".to_string()));
    }

    #[test]
    fn squit_needs_a_server_and_carries_a_free_text_reason() {
        let action = parse_input(&state(), "/squit leaf.example.com netsplit test");
        assert_eq!(
            action,
            InputAction::Squit {
                server: "leaf.example.com".to_string(),
                reason: "netsplit test".to_string(),
            }
        );
    }

    #[test]
    fn squit_with_no_server_is_unknown() {
        let action = parse_input(&state(), "/squit");
        assert_eq!(action, InputAction::Unknown("/squit".to_string()));
    }

    #[test]
    fn wallops_needs_a_message() {
        let action = parse_input(&state(), "/wallops server going down soon");
        assert_eq!(
            action,
            InputAction::Wallops {
                message: "server going down soon".to_string()
            }
        );

        let action = parse_input(&state(), "/wallops");
        assert_eq!(action, InputAction::Unknown("/wallops".to_string()));
    }

    #[test]
    fn window_with_a_name_switches_to_it() {
        let action = parse_input(&state(), "/window #chatnet");
        assert_eq!(action, InputAction::SwitchBuffer("#chatnet".to_string()));
    }

    #[test]
    fn window_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/window");
        assert_eq!(action, InputAction::Unknown("/window".to_string()));
    }

    #[test]
    fn window_with_a_number_jumps_by_index() {
        let action = parse_input(&state(), "/window 3");
        assert_eq!(action, InputAction::SwitchBufferByIndex(3));
    }

    #[test]
    fn window_zero_is_unknown_not_a_jump() {
        let action = parse_input(&state(), "/window 0");
        assert_eq!(action, InputAction::Unknown("/window 0".to_string()));
    }

    #[test]
    fn window_list() {
        let action = parse_input(&state(), "/window list");
        assert_eq!(action, InputAction::WindowList);
        let action = parse_input(&state(), "/window LIST");
        assert_eq!(action, InputAction::WindowList);
    }

    #[test]
    fn window_name_sets_a_display_override() {
        let action = parse_input(&state(), "/window name my-favorite-channel");
        assert_eq!(action, InputAction::WindowName("my-favorite-channel".to_string()));
    }

    #[test]
    fn window_name_with_no_name_is_unknown() {
        let action = parse_input(&state(), "/window name");
        assert_eq!(action, InputAction::Unknown("/window name".to_string()));
    }

    #[test]
    fn window_item_bare_and_next_both_mean_cycle_forward() {
        assert_eq!(
            parse_input(&state(), "/window item"),
            InputAction::WindowItemCycle(true)
        );
        assert_eq!(
            parse_input(&state(), "/window item next"),
            InputAction::WindowItemCycle(true)
        );
    }

    #[test]
    fn window_item_prev_cycles_backward() {
        assert_eq!(
            parse_input(&state(), "/window item prev"),
            InputAction::WindowItemCycle(false)
        );
        assert_eq!(
            parse_input(&state(), "/window item previous"),
            InputAction::WindowItemCycle(false)
        );
    }

    #[test]
    fn window_item_remove_takes_no_argument() {
        assert_eq!(
            parse_input(&state(), "/window item remove"),
            InputAction::WindowItemRemove
        );
    }

    #[test]
    fn window_item_add_needs_a_name() {
        assert_eq!(
            parse_input(&state(), "/window item add k9"),
            InputAction::WindowItemAdd("k9".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/window item add"),
            InputAction::Unknown("/window item add".to_string())
        );
    }

    #[test]
    fn window_item_goto_needs_a_name() {
        assert_eq!(
            parse_input(&state(), "/window item goto k9"),
            InputAction::WindowItemGoto("k9".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/window item goto"),
            InputAction::Unknown("/window item goto".to_string())
        );
    }

    #[test]
    fn window_new_takes_and_ignores_the_placement_argument() {
        assert_eq!(parse_input(&state(), "/window new"), InputAction::WindowNew);
        assert_eq!(parse_input(&state(), "/window new hidden"), InputAction::WindowNew);
    }

    #[test]
    fn window_immortal_with_and_without_a_value() {
        assert_eq!(
            parse_input(&state(), "/window immortal"),
            InputAction::WindowImmortal(None)
        );
        assert_eq!(
            parse_input(&state(), "/window immortal on"),
            InputAction::WindowImmortal(Some("on".to_string()))
        );
    }

    #[test]
    fn window_stick_takes_no_arguments() {
        assert_eq!(parse_input(&state(), "/window stick"), InputAction::WindowStick);
    }

    #[test]
    fn window_hide_reuses_unsplit() {
        assert_eq!(parse_input(&state(), "/window hide"), InputAction::Unsplit(None));
        assert_eq!(
            parse_input(&state(), "/window hide #irust-test"),
            InputAction::Unsplit(Some("#irust-test".to_string()))
        );
    }

    #[test]
    fn window_show_reuses_split() {
        assert_eq!(
            parse_input(&state(), "/window show #irust-test"),
            InputAction::Split {
                target: "#irust-test".to_string(),
                orientation: None
            }
        );
        assert_eq!(
            parse_input(&state(), "/window show #irust-test -right"),
            InputAction::Split {
                target: "#irust-test".to_string(),
                orientation: Some(SplitOrientation::Vertical)
            }
        );
        assert_eq!(
            parse_input(&state(), "/window show"),
            InputAction::Unknown("/window show".to_string())
        );
    }

    #[test]
    fn window_logfile_reuses_log_open() {
        assert_eq!(
            parse_input(&state(), "/window logfile /tmp/win.log"),
            InputAction::Log {
                action: LogToggle::Open("/tmp/win.log".to_string()),
                target: None
            }
        );
        assert_eq!(
            parse_input(&state(), "/window logfile"),
            InputAction::Unknown("/window logfile".to_string())
        );
    }

    #[test]
    fn window_log_on_off_toggle() {
        assert_eq!(
            parse_input(&state(), "/window log on"),
            InputAction::Log {
                action: LogToggle::Start,
                target: None
            }
        );
        assert_eq!(
            parse_input(&state(), "/window log on /tmp/win2.log"),
            InputAction::Log {
                action: LogToggle::Open("/tmp/win2.log".to_string()),
                target: None
            }
        );
        assert_eq!(
            parse_input(&state(), "/window log off"),
            InputAction::Log {
                action: LogToggle::Close,
                target: None
            }
        );
        // A fresh state's active channel isn't logging yet -- toggle
        // with no prior state means "currently off", so it turns on.
        assert_eq!(
            parse_input(&state(), "/window log toggle"),
            InputAction::Log {
                action: LogToggle::Start,
                target: None
            }
        );
    }

    #[test]
    fn rawlog_open_and_close() {
        assert_eq!(
            parse_input(&state(), "/rawlog open /tmp/raw.log"),
            InputAction::RawlogOpen("/tmp/raw.log".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/rawlog open"),
            InputAction::Unknown("/rawlog open".to_string())
        );
        assert_eq!(parse_input(&state(), "/rawlog close"), InputAction::RawlogClose);
    }

    #[test]
    fn scrollback_goto_relative_and_absolute() {
        assert_eq!(
            parse_input(&state(), "/scrollback goto +10"),
            InputAction::ScrollbackGoto(ScrollbackGoto::Relative(10))
        );
        assert_eq!(
            parse_input(&state(), "/scrollback goto -10"),
            InputAction::ScrollbackGoto(ScrollbackGoto::Relative(-10))
        );
        assert_eq!(
            parse_input(&state(), "/scrollback goto 42"),
            InputAction::ScrollbackGoto(ScrollbackGoto::Line(42))
        );
        assert_eq!(
            parse_input(&state(), "/scrollback goto sometimestamp"),
            InputAction::Unknown("/scrollback goto sometimestamp".to_string())
        );
    }

    #[test]
    fn scrollback_goto_bare_time_defaults_to_todays_date() {
        assert_eq!(
            parse_input(&state(), "/scrollback goto 14:30"),
            InputAction::ScrollbackGoto(ScrollbackGoto::Timestamp {
                date: None,
                hour: 14,
                minute: 30,
                second: 0,
            })
        );
        assert_eq!(
            parse_input(&state(), "/scrollback goto 14:30:05"),
            InputAction::ScrollbackGoto(ScrollbackGoto::Timestamp {
                date: None,
                hour: 14,
                minute: 30,
                second: 5,
            })
        );
    }

    #[test]
    fn scrollback_goto_days_ago_form() {
        assert_eq!(
            parse_input(&state(), "/scrollback goto -2 09:00"),
            InputAction::ScrollbackGoto(ScrollbackGoto::Timestamp {
                date: Some(ScrollbackGotoDate::DaysAgo(2)),
                hour: 9,
                minute: 0,
                second: 0,
            })
        );
    }

    #[test]
    fn scrollback_goto_day_and_optional_month_form() {
        assert_eq!(
            parse_input(&state(), "/scrollback goto 25 20:00"),
            InputAction::ScrollbackGoto(ScrollbackGoto::Timestamp {
                date: Some(ScrollbackGotoDate::OnDay { day: 25, month: None }),
                hour: 20,
                minute: 0,
                second: 0,
            })
        );
        assert_eq!(
            parse_input(&state(), "/scrollback goto 25.12 20:00:30"),
            InputAction::ScrollbackGoto(ScrollbackGoto::Timestamp {
                date: Some(ScrollbackGotoDate::OnDay {
                    day: 25,
                    month: Some(12)
                }),
                hour: 20,
                minute: 0,
                second: 30,
            })
        );
    }

    #[test]
    fn scrollback_goto_timestamp_rejects_out_of_range_components() {
        for bad in [
            "/scrollback goto 24:00",      // hour out of range
            "/scrollback goto 12:60",      // minute out of range
            "/scrollback goto 12:00:60",   // second out of range
            "/scrollback goto 32 12:00",   // day out of range
            "/scrollback goto 5.13 12:00", // month out of range
            "/scrollback goto 0 12:00",    // day zero
        ] {
            assert_eq!(
                parse_input(&state(), bad),
                InputAction::Unknown(bad.to_string()),
                "expected Unknown for {bad:?}"
            );
        }
    }

    #[test]
    fn scrollback_goto_timestamp_with_too_many_tokens_is_unknown() {
        let line = "/scrollback goto 25.12 20:00 extra";
        assert_eq!(parse_input(&state(), line), InputAction::Unknown(line.to_string()));
    }

    mod resolve_timestamp {
        use super::*;
        use chrono::TimeZone;

        fn local_dt(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<chrono::Local> {
            chrono::Local.with_ymd_and_hms(y, m, d, h, mi, s).unwrap()
        }

        #[test]
        fn bare_time_earlier_today_resolves_to_today() {
            let now = local_dt(2025, 6, 15, 20, 0, 0);
            let resolved = resolve_scrollback_timestamp(None, 14, 30, 0, now).unwrap();
            assert_eq!(resolved, local_dt(2025, 6, 15, 14, 30, 0).timestamp());
        }

        #[test]
        fn bare_time_later_today_resolves_to_yesterday() {
            let now = local_dt(2025, 6, 15, 8, 0, 0);
            let resolved = resolve_scrollback_timestamp(None, 23, 59, 0, now).unwrap();
            assert_eq!(resolved, local_dt(2025, 6, 14, 23, 59, 0).timestamp());
        }

        #[test]
        fn bare_time_exactly_now_resolves_to_today_not_yesterday() {
            // Not strictly > now, so the "still in the future" branch
            // never triggers -- an exact match is today's, same as
            // `Buffer::first_index_at_or_after`'s own `>=` semantics.
            let now = local_dt(2025, 6, 15, 14, 30, 0);
            let resolved = resolve_scrollback_timestamp(None, 14, 30, 0, now).unwrap();
            assert_eq!(resolved, now.timestamp());
        }

        #[test]
        fn days_ago_subtracts_whole_days_including_a_month_boundary() {
            let now = local_dt(2025, 6, 2, 12, 0, 0);
            let resolved = resolve_scrollback_timestamp(Some(ScrollbackGotoDate::DaysAgo(3)), 9, 0, 0, now).unwrap();
            assert_eq!(resolved, local_dt(2025, 5, 30, 9, 0, 0).timestamp());
        }

        #[test]
        fn on_day_with_no_month_and_day_already_reached_this_month_stays_this_month() {
            // Today is the 15th; day 5 already happened this month
            // (safely in the past), so there's no reason to assume last
            // month instead.
            let now = local_dt(2025, 6, 15, 12, 0, 0);
            let target = ScrollbackGotoDate::OnDay { day: 5, month: None };
            let resolved = resolve_scrollback_timestamp(Some(target), 9, 0, 0, now).unwrap();
            assert_eq!(resolved, local_dt(2025, 6, 5, 9, 0, 0).timestamp());
        }

        #[test]
        fn on_day_with_no_month_and_a_day_later_than_today_means_last_month() {
            // Today is the 15th; day 20 hasn't happened yet this month
            // (it's still ahead, within June) -- irssi assumes last
            // month's 20th was meant instead, since a future date makes
            // no sense for scrollback history.
            let now = local_dt(2025, 6, 15, 12, 0, 0);
            let target = ScrollbackGotoDate::OnDay { day: 20, month: None };
            let resolved = resolve_scrollback_timestamp(Some(target), 9, 0, 0, now).unwrap();
            assert_eq!(resolved, local_dt(2025, 5, 20, 9, 0, 0).timestamp());
        }

        #[test]
        fn on_day_with_no_month_rolls_back_into_december_of_last_year_from_january() {
            let now = local_dt(2025, 1, 5, 12, 0, 0);
            let target = ScrollbackGotoDate::OnDay { day: 20, month: None };
            let resolved = resolve_scrollback_timestamp(Some(target), 9, 0, 0, now).unwrap();
            assert_eq!(resolved, local_dt(2024, 12, 20, 9, 0, 0).timestamp());
        }

        #[test]
        fn on_day_with_a_past_month_this_year_stays_this_year() {
            let now = local_dt(2025, 6, 15, 12, 0, 0);
            let target = ScrollbackGotoDate::OnDay {
                day: 10,
                month: Some(3),
            };
            let resolved = resolve_scrollback_timestamp(Some(target), 9, 0, 0, now).unwrap();
            assert_eq!(resolved, local_dt(2025, 3, 10, 9, 0, 0).timestamp());
        }

        #[test]
        fn on_day_with_a_future_month_this_year_means_last_year() {
            let now = local_dt(2025, 3, 15, 12, 0, 0);
            let target = ScrollbackGotoDate::OnDay {
                day: 25,
                month: Some(12),
            };
            let resolved = resolve_scrollback_timestamp(Some(target), 20, 0, 0, now).unwrap();
            assert_eq!(resolved, local_dt(2024, 12, 25, 20, 0, 0).timestamp());
        }

        #[test]
        fn an_explicit_date_still_in_the_future_is_none_no_yesterday_adjustment() {
            let now = local_dt(2025, 6, 15, 12, 0, 0);
            // Same month, given explicitly -- not "last year's," genuinely
            // just later this same month, still in the future.
            let target = ScrollbackGotoDate::OnDay {
                day: 20,
                month: Some(6),
            };
            assert_eq!(resolve_scrollback_timestamp(Some(target), 9, 0, 0, now), None);
        }

        #[test]
        fn an_impossible_calendar_date_resolves_to_none_not_a_panic() {
            let now = local_dt(2025, 6, 15, 12, 0, 0);
            let target = ScrollbackGotoDate::OnDay {
                day: 30,
                month: Some(2),
            }; // Feb 30th
            assert_eq!(resolve_scrollback_timestamp(Some(target), 9, 0, 0, now), None);
        }

        #[test]
        fn an_absurdly_large_days_ago_resolves_to_none_not_a_panic() {
            let now = local_dt(2025, 6, 15, 12, 0, 0);
            let target = ScrollbackGotoDate::DaysAgo(u32::MAX);
            assert_eq!(resolve_scrollback_timestamp(Some(target), 9, 0, 0, now), None);
        }
    }

    #[test]
    fn scrollback_home_end_status() {
        assert_eq!(parse_input(&state(), "/scrollback home"), InputAction::ScrollbackHome);
        assert_eq!(parse_input(&state(), "/scrollback end"), InputAction::ScrollbackEnd);
        assert_eq!(
            parse_input(&state(), "/scrollback status"),
            InputAction::ScrollbackStatus
        );
    }

    #[test]
    fn foreach_needs_a_scope_and_command() {
        assert_eq!(
            parse_input(&state(), "/foreach channel echo hi"),
            InputAction::Foreach {
                scope: ForeachScope::Channel,
                command: "echo hi".to_string()
            }
        );
        assert_eq!(
            parse_input(&state(), "/foreach server uptime"),
            InputAction::Foreach {
                scope: ForeachScope::Server,
                command: "uptime".to_string()
            }
        );
        assert_eq!(
            parse_input(&state(), "/foreach bogus echo hi"),
            InputAction::Unknown("/foreach bogus echo hi".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/foreach channel"),
            InputAction::Unknown("/foreach channel".to_string())
        );
    }

    #[test]
    fn window_item_unknown_subcommand_is_unknown() {
        assert_eq!(
            parse_input(&state(), "/window item bogus"),
            InputAction::Unknown("/window item bogus".to_string())
        );
    }

    #[test]
    fn window_with_a_plain_name_still_switches_buffers() {
        let action = parse_input(&state(), "/window #chatnet");
        assert_eq!(action, InputAction::SwitchBuffer("#chatnet".to_string()));
    }

    #[test]
    fn set_with_no_argument_lists_everything() {
        let action = parse_input(&state(), "/set");
        assert_eq!(action, InputAction::Set { key: None, value: None });
    }

    #[test]
    fn set_with_just_a_key_shows_it() {
        let action = parse_input(&state(), "/set bell");
        assert_eq!(
            action,
            InputAction::Set {
                key: Some("bell".to_string()),
                value: None
            }
        );
    }

    #[test]
    fn set_with_a_key_and_value_changes_it() {
        let action = parse_input(&state(), "/set bell off");
        assert_eq!(
            action,
            InputAction::Set {
                key: Some("bell".to_string()),
                value: Some("off".to_string())
            }
        );
    }

    #[test]
    fn format_with_no_argument_lists_everything() {
        let action = parse_input(&state(), "/format");
        assert_eq!(
            action,
            InputAction::Format {
                name: None,
                template: None,
                mode: FormatMode::GetOrSet
            }
        );
    }

    #[test]
    fn format_with_just_a_name_shows_it() {
        let action = parse_input(&state(), "/format join");
        assert_eq!(
            action,
            InputAction::Format {
                name: Some("join".to_string()),
                template: None,
                mode: FormatMode::GetOrSet
            }
        );
    }

    #[test]
    fn format_with_a_name_and_template_changes_it() {
        let action = parse_input(&state(), "/format join -- $nick arrived!");
        assert_eq!(
            action,
            InputAction::Format {
                name: Some("join".to_string()),
                template: Some("-- $nick arrived!".to_string()),
                mode: FormatMode::GetOrSet,
            }
        );
    }

    #[test]
    fn format_parses_delete_flag_with_a_name() {
        let action = parse_input(&state(), "/format -delete join");
        assert_eq!(
            action,
            InputAction::Format {
                name: Some("join".to_string()),
                template: None,
                mode: FormatMode::Delete
            }
        );
    }

    #[test]
    fn format_parses_reset_flag_with_a_name() {
        let action = parse_input(&state(), "/format -reset join");
        assert_eq!(
            action,
            InputAction::Format {
                name: Some("join".to_string()),
                template: None,
                mode: FormatMode::Reset
            }
        );
    }

    #[test]
    fn format_delete_with_no_name_is_unknown() {
        let action = parse_input(&state(), "/format -delete");
        assert_eq!(action, InputAction::Unknown("/format -delete".to_string()));
    }

    #[test]
    fn format_reset_with_no_name_is_unknown() {
        let action = parse_input(&state(), "/format -reset");
        assert_eq!(action, InputAction::Unknown("/format -reset".to_string()));
    }

    #[test]
    fn recode_with_no_argument_lists() {
        let action = parse_input(&state(), "/recode");
        assert_eq!(action, InputAction::Recode(RecodeSub::List));
    }

    #[test]
    fn recode_add_with_just_a_charset_defaults_the_target() {
        let action = parse_input(&state(), "/recode add cp1251");
        assert_eq!(
            action,
            InputAction::Recode(RecodeSub::Add {
                target: None,
                charset: "cp1251".to_string()
            })
        );
    }

    #[test]
    fn recode_add_with_a_target_and_charset_sets_both() {
        let action = parse_input(&state(), "/recode add #chan cp1251");
        assert_eq!(
            action,
            InputAction::Recode(RecodeSub::Add {
                target: Some("#chan".to_string()),
                charset: "cp1251".to_string()
            })
        );
    }

    #[test]
    fn recode_add_accepts_a_tag_slash_target_as_the_target() {
        let action = parse_input(&state(), "/recode add libera/#chan iso-8859-1");
        assert_eq!(
            action,
            InputAction::Recode(RecodeSub::Add {
                target: Some("libera/#chan".to_string()),
                charset: "iso-8859-1".to_string()
            })
        );
    }

    #[test]
    fn recode_add_with_no_charset_is_unknown() {
        let action = parse_input(&state(), "/recode add");
        assert_eq!(action, InputAction::Unknown("/recode add".to_string()));
    }

    #[test]
    fn recode_remove_with_no_target_defaults_it() {
        let action = parse_input(&state(), "/recode remove");
        assert_eq!(action, InputAction::Recode(RecodeSub::Remove { target: None }));
    }

    #[test]
    fn recode_remove_with_a_target_removes_that_one() {
        let action = parse_input(&state(), "/recode remove #chan");
        assert_eq!(
            action,
            InputAction::Recode(RecodeSub::Remove {
                target: Some("#chan".to_string())
            })
        );
    }

    #[test]
    fn recode_unknown_subcommand_is_unknown() {
        let action = parse_input(&state(), "/recode bogus");
        assert_eq!(action, InputAction::Unknown("/recode bogus".to_string()));
    }

    #[test]
    fn hilight_with_no_argument_lists_everything() {
        let action = parse_input(&state(), "/hilight");
        assert_eq!(
            action,
            InputAction::Hilight {
                pattern: None,
                is_regex: false,
                is_mask: false,
                channels: Vec::new(),
                color: None,
                actcolor: None,
                priority: None,
            }
        );
    }

    #[test]
    fn hilight_with_a_word_adds_it() {
        let action = parse_input(&state(), "/hilight urgent");
        assert_eq!(
            action,
            InputAction::Hilight {
                pattern: Some("urgent".to_string()),
                is_regex: false,
                is_mask: false,
                channels: Vec::new(),
                color: None,
                actcolor: None,
                priority: None,
            }
        );
    }

    #[test]
    fn hilight_regex_mask_and_channels_flags_parse_together() {
        let action = parse_input(&state(), r"/hilight -regex -mask -channels #ops,#routing ^Bad.*Bot$");
        assert_eq!(
            action,
            InputAction::Hilight {
                pattern: Some("^Bad.*Bot$".to_string()),
                is_regex: true,
                is_mask: true,
                channels: vec!["#ops".to_string(), "#routing".to_string()],
                color: None,
                actcolor: None,
                priority: None,
            }
        );
    }

    #[test]
    fn hilight_color_flag_parses_alongside_the_others() {
        let action = parse_input(&state(), "/hilight -color 203 -channels #ops urgent");
        assert_eq!(
            action,
            InputAction::Hilight {
                pattern: Some("urgent".to_string()),
                is_regex: false,
                is_mask: false,
                channels: vec!["#ops".to_string()],
                color: Some("203".to_string()),
                actcolor: None,
                priority: None,
            }
        );
    }

    #[test]
    fn hilight_color_with_no_value_is_unknown() {
        assert_eq!(
            parse_input(&state(), "/hilight -color"),
            InputAction::Unknown("/hilight -color".to_string())
        );
    }

    #[test]
    fn hilight_actcolor_and_priority_flags_parse_together() {
        let action = parse_input(&state(), "/hilight -actcolor yellow -priority 5 urgent");
        assert_eq!(
            action,
            InputAction::Hilight {
                pattern: Some("urgent".to_string()),
                is_regex: false,
                is_mask: false,
                channels: Vec::new(),
                color: None,
                actcolor: Some("yellow".to_string()),
                priority: Some(5),
            }
        );
    }

    #[test]
    fn hilight_priority_accepts_a_negative_value() {
        let action = parse_input(&state(), "/hilight -priority -3 urgent");
        assert_eq!(
            action,
            InputAction::Hilight {
                pattern: Some("urgent".to_string()),
                is_regex: false,
                is_mask: false,
                channels: Vec::new(),
                color: None,
                actcolor: None,
                priority: Some(-3),
            }
        );
    }

    #[test]
    fn hilight_actcolor_with_no_value_is_unknown() {
        assert_eq!(
            parse_input(&state(), "/hilight -actcolor"),
            InputAction::Unknown("/hilight -actcolor".to_string())
        );
    }

    #[test]
    fn hilight_priority_with_a_non_numeric_value_is_unknown() {
        assert_eq!(
            parse_input(&state(), "/hilight -priority notanumber urgent"),
            InputAction::Unknown("/hilight -priority notanumber urgent".to_string())
        );
    }

    #[test]
    fn hilight_rejects_an_invalid_regex() {
        let action = parse_input(&state(), "/hilight -regex (unterminated");
        assert_eq!(
            action,
            InputAction::Unknown("/hilight -regex (unterminated".to_string())
        );
    }

    #[test]
    fn hilight_flags_without_a_pattern_is_unknown() {
        let action = parse_input(&state(), "/hilight -regex");
        assert_eq!(action, InputAction::Unknown("/hilight -regex".to_string()));
    }

    #[test]
    fn dehilight_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/dehilight");
        assert_eq!(action, InputAction::Unknown("/dehilight".to_string()));
    }

    #[test]
    fn dehilight_with_a_word_removes_it() {
        let action = parse_input(&state(), "/dehilight urgent");
        assert_eq!(action, InputAction::Dehilight("urgent".to_string()));
    }

    #[test]
    fn statusbar_with_no_argument_lists() {
        let action = parse_input(&state(), "/statusbar");
        assert_eq!(action, InputAction::StatusbarList);
    }

    #[test]
    fn statusbar_list_is_explicit_too() {
        let action = parse_input(&state(), "/statusbar list");
        assert_eq!(action, InputAction::StatusbarList);
    }

    #[test]
    fn statusbar_reset() {
        let action = parse_input(&state(), "/statusbar reset");
        assert_eq!(action, InputAction::StatusbarReset);
    }

    #[test]
    fn statusbar_remove_with_no_item_is_unknown() {
        let action = parse_input(&state(), "/statusbar remove");
        assert_eq!(action, InputAction::Unknown("/statusbar remove".to_string()));
    }

    #[test]
    fn statusbar_remove_with_an_item() {
        let action = parse_input(&state(), "/statusbar remove act");
        assert_eq!(action, InputAction::StatusbarRemove("act".to_string()));
    }

    #[test]
    fn statusbar_add_with_no_item_is_unknown() {
        let action = parse_input(&state(), "/statusbar add");
        assert_eq!(action, InputAction::Unknown("/statusbar add".to_string()));
    }

    #[test]
    fn statusbar_add_bare() {
        let action = parse_input(&state(), "/statusbar add lag");
        assert_eq!(
            action,
            InputAction::StatusbarAdd {
                item: "lag".to_string(),
                before: None,
                after: None,
                alignment: None
            }
        );
    }

    #[test]
    fn statusbar_add_with_flags() {
        let action = parse_input(&state(), "/statusbar add -before window -alignment left act");
        assert_eq!(
            action,
            InputAction::StatusbarAdd {
                item: "act".to_string(),
                before: Some("window".to_string()),
                after: None,
                alignment: Some("left".to_string()),
            }
        );
    }

    #[test]
    fn statusbar_add_after_flag() {
        let action = parse_input(&state(), "/statusbar add -after user window");
        assert_eq!(
            action,
            InputAction::StatusbarAdd {
                item: "window".to_string(),
                before: None,
                after: Some("user".to_string()),
                alignment: None,
            }
        );
    }

    #[test]
    fn layout_with_no_argument_lists() {
        let action = parse_input(&state(), "/layout");
        assert_eq!(action, InputAction::LayoutList);
    }

    #[test]
    fn layout_list_is_explicit_too() {
        let action = parse_input(&state(), "/layout list");
        assert_eq!(action, InputAction::LayoutList);
    }

    #[test]
    fn layout_save() {
        let action = parse_input(&state(), "/layout save");
        assert_eq!(action, InputAction::LayoutSave);
    }

    #[test]
    fn layout_reset() {
        let action = parse_input(&state(), "/layout reset");
        assert_eq!(action, InputAction::LayoutReset);
    }

    #[test]
    fn layout_with_an_unknown_verb_is_unknown() {
        let action = parse_input(&state(), "/layout bogus");
        assert_eq!(action, InputAction::Unknown("/layout bogus".to_string()));
    }

    #[test]
    fn exec_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/exec");
        assert_eq!(action, InputAction::Unknown("/exec".to_string()));
    }

    #[test]
    fn exec_keeps_the_whole_command_line_intact() {
        let action = parse_input(&state(), "/exec echo hello | grep hi");
        assert_eq!(action, InputAction::Exec("echo hello | grep hi".to_string()));
    }

    #[test]
    fn alias_with_no_argument_lists_everything() {
        let action = parse_input(&state(), "/alias");
        assert_eq!(
            action,
            InputAction::Alias {
                name: None,
                command: None
            }
        );
    }

    #[test]
    fn alias_with_just_a_name_shows_it() {
        let action = parse_input(&state(), "/alias j");
        assert_eq!(
            action,
            InputAction::Alias {
                name: Some("j".to_string()),
                command: None
            }
        );
    }

    #[test]
    fn alias_with_a_name_and_command_defines_it() {
        let action = parse_input(&state(), "/alias j join");
        assert_eq!(
            action,
            InputAction::Alias {
                name: Some("j".to_string()),
                command: Some("join".to_string())
            }
        );
    }

    #[test]
    fn completion_with_no_argument_lists_everything() {
        let action = parse_input(&state(), "/completion");
        assert_eq!(
            action,
            InputAction::Completion {
                key: None,
                value: None,
                delete: false
            }
        );
    }

    #[test]
    fn completion_with_a_key_and_value_defines_it() {
        let action = parse_input(&state(), "/completion brb be right back");
        assert_eq!(
            action,
            InputAction::Completion {
                key: Some("brb".to_string()),
                value: Some("be right back".to_string()),
                delete: false,
            }
        );
    }

    #[test]
    fn completion_dash_delete_removes_one() {
        let action = parse_input(&state(), "/completion -delete brb");
        assert_eq!(
            action,
            InputAction::Completion {
                key: Some("brb".to_string()),
                value: None,
                delete: true
            }
        );
    }

    #[test]
    fn completion_dash_delete_with_no_key_is_unknown() {
        let action = parse_input(&state(), "/completion -delete");
        assert_eq!(action, InputAction::Unknown("/completion -delete".to_string()));
    }

    #[test]
    fn ignore_with_no_argument_lists() {
        let action = parse_input(&state(), "/ignore");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: None,
                levels: Vec::new(),
                is_regex: false,
                is_except: false,
                is_replies: false,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_with_a_mask_adds_it_with_no_level_restriction() {
        let action = parse_input(&state(), "/ignore BadNick");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("BadNick".to_string()),
                levels: Vec::new(),
                is_regex: false,
                is_except: false,
                is_replies: false,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_with_levels_parses_and_lowercases_them() {
        let action = parse_input(&state(), "/ignore BadNick NOTICES CTCPS");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("BadNick".to_string()),
                levels: vec!["notices".to_string(), "ctcps".to_string()],
                is_regex: false,
                is_except: false,
                is_replies: false,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_with_explicit_all_means_no_restriction() {
        let action = parse_input(&state(), "/ignore BadNick ALL");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("BadNick".to_string()),
                levels: Vec::new(),
                is_regex: false,
                is_except: false,
                is_replies: false,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_with_regex_flag_parses_the_pattern_and_marks_is_regex() {
        let action = parse_input(&state(), r"/ignore -regex ^Bad.*\d+$");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some(r"^Bad.*\d+$".to_string()),
                levels: Vec::new(),
                is_regex: true,
                is_except: false,
                is_replies: false,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_with_regex_flag_and_levels_parses_both() {
        let action = parse_input(&state(), "/ignore -regex ^Bad notices");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("^Bad".to_string()),
                levels: vec!["notices".to_string()],
                is_regex: true,
                is_except: false,
                is_replies: false,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_with_except_flag_marks_is_except() {
        let action = parse_input(&state(), "/ignore -except gooduser");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("gooduser".to_string()),
                levels: Vec::new(),
                is_regex: false,
                is_except: true,
                is_replies: false,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_with_replies_flag_marks_is_replies() {
        let action = parse_input(&state(), "/ignore -replies Troll");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("Troll".to_string()),
                levels: Vec::new(),
                is_regex: false,
                is_except: false,
                is_replies: true,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_flags_combine_in_any_order() {
        let action = parse_input(&state(), "/ignore -replies -regex -except ^Bad notices");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("^Bad".to_string()),
                levels: vec!["notices".to_string()],
                is_regex: true,
                is_except: true,
                is_replies: true,
                time_secs: None,
            }
        );
    }

    #[test]
    fn ignore_with_time_flag_parses_the_duration_in_seconds() {
        let action = parse_input(&state(), "/ignore -time 5m BadNick");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("BadNick".to_string()),
                levels: Vec::new(),
                is_regex: false,
                is_except: false,
                is_replies: false,
                time_secs: Some(300),
            }
        );
    }

    #[test]
    fn ignore_with_time_flag_combines_with_other_flags_in_any_order() {
        let action = parse_input(&state(), "/ignore -regex -time 1h ^Bad notices");
        assert_eq!(
            action,
            InputAction::Ignore {
                mask: Some("^Bad".to_string()),
                levels: vec!["notices".to_string()],
                is_regex: true,
                is_except: false,
                is_replies: false,
                time_secs: Some(3600),
            }
        );
    }

    /// An invalid duration is rejected up front, same treatment as an
    /// invalid `-regex` pattern above -- no silently-wrong entry stored.
    #[test]
    fn ignore_with_an_invalid_time_is_rejected() {
        let action = parse_input(&state(), "/ignore -time notaduration BadNick");
        assert_eq!(
            action,
            InputAction::Unknown("/ignore -time notaduration BadNick".to_string())
        );
    }

    /// An invalid pattern is rejected up front, not stored as a dead
    /// entry that will never match anything.
    #[test]
    fn ignore_with_an_invalid_regex_is_rejected() {
        let action = parse_input(&state(), "/ignore -regex (unterminated");
        assert_eq!(action, InputAction::Unknown("/ignore -regex (unterminated".to_string()));
    }

    #[test]
    fn ignore_with_regex_flag_but_no_pattern_is_rejected() {
        let action = parse_input(&state(), "/ignore -regex");
        assert_eq!(action, InputAction::Unknown("/ignore -regex".to_string()));
    }

    #[test]
    fn ignore_with_an_unknown_level_is_unknown() {
        let action = parse_input(&state(), "/ignore BadNick BOGUS");
        assert_eq!(action, InputAction::Unknown("/ignore BadNick BOGUS".to_string()));
    }

    #[test]
    fn unignore_needs_a_mask_or_id() {
        let action = parse_input(&state(), "/unignore 1");
        assert_eq!(action, InputAction::Unignore("1".to_string()));

        let action = parse_input(&state(), "/unignore");
        assert_eq!(action, InputAction::Unknown("/unignore".to_string()));
    }

    #[test]
    fn uptime_takes_no_arguments() {
        let action = parse_input(&state(), "/uptime");
        assert_eq!(action, InputAction::Uptime);
    }

    #[test]
    fn clientversion_takes_no_arguments() {
        let action = parse_input(&state(), "/clientversion");
        assert_eq!(action, InputAction::ClientVersion);
    }

    #[test]
    fn version_still_queries_the_server_not_the_client() {
        let action = parse_input(&state(), "/version");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "VERSION".to_string(),
                rest: String::new()
            }
        );
    }

    #[test]
    fn unalias_needs_a_name() {
        let action = parse_input(&state(), "/unalias j");
        assert_eq!(action, InputAction::Unalias("j".to_string()));

        let action = parse_input(&state(), "/unalias");
        assert_eq!(action, InputAction::Unknown("/unalias".to_string()));
    }

    #[test]
    fn eval_needs_an_argument() {
        let action = parse_input(&state(), "/eval echo hi");
        assert_eq!(action, InputAction::Eval("echo hi".to_string()));

        let action = parse_input(&state(), "/eval");
        assert_eq!(action, InputAction::Unknown("/eval".to_string()));
    }

    #[test]
    fn echo_needs_text() {
        assert_eq!(
            parse_input(&state(), "/echo hello there"),
            InputAction::Echo("hello there".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/echo"),
            InputAction::Unknown("/echo".to_string())
        );
    }

    #[test]
    fn toggle_with_and_without_a_value() {
        assert_eq!(
            parse_input(&state(), "/toggle bell"),
            InputAction::Toggle {
                key: "bell".to_string(),
                value: None
            }
        );
        assert_eq!(
            parse_input(&state(), "/toggle bell on"),
            InputAction::Toggle {
                key: "bell".to_string(),
                value: Some("on".to_string())
            }
        );
        assert_eq!(
            parse_input(&state(), "/toggle"),
            InputAction::Unknown("/toggle".to_string())
        );
    }

    #[test]
    fn beep_takes_no_arguments() {
        assert_eq!(parse_input(&state(), "/beep"), InputAction::Beep);
    }

    #[test]
    fn cat_and_cd_need_a_path() {
        assert_eq!(
            parse_input(&state(), "/cat notes.txt"),
            InputAction::Cat("notes.txt".to_string())
        );
        assert_eq!(parse_input(&state(), "/cat"), InputAction::Unknown("/cat".to_string()));
        assert_eq!(parse_input(&state(), "/cd /tmp"), InputAction::Cd("/tmp".to_string()));
        assert_eq!(parse_input(&state(), "/cd"), InputAction::Unknown("/cd".to_string()));
    }

    #[test]
    fn nctcp_needs_target_and_command() {
        assert_eq!(
            parse_input(&state(), "/nctcp dana ping 12345"),
            InputAction::Nctcp {
                target: "dana".to_string(),
                command: "PING".to_string(),
                data: Some("12345".to_string())
            }
        );
        assert_eq!(
            parse_input(&state(), "/nctcp dana"),
            InputAction::Unknown("/nctcp dana".to_string())
        );
    }

    #[test]
    fn action_needs_an_explicit_target_unlike_me() {
        assert_eq!(
            parse_input(&state(), "/action #irust-test waves"),
            InputAction::Action {
                target: "#irust-test".to_string(),
                text: "waves".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
        assert_eq!(
            parse_input(&state(), "/action dana waves"),
            InputAction::Action {
                target: "dana".to_string(),
                text: "waves".to_string(),
                echo_buffer: BufferId::Query("dana".to_string()),
            }
        );
    }

    #[test]
    fn wall_defaults_to_the_active_channel_or_takes_an_explicit_one() {
        assert_eq!(
            parse_input(&state(), "/wall everyone chill"),
            InputAction::Wall {
                channel: None,
                text: "everyone chill".to_string()
            }
        );
        assert_eq!(
            parse_input(&state(), "/wall #other calm down"),
            InputAction::Wall {
                channel: Some("#other".to_string()),
                text: "calm down".to_string()
            }
        );
    }

    #[test]
    fn unsilence_reuses_the_silence_server_command_with_a_minus_prefix() {
        assert_eq!(
            parse_input(&state(), "/unsilence *!*@bad.host"),
            InputAction::ServerCommand {
                command: "SILENCE".to_string(),
                rest: "-*!*@bad.host".to_string()
            }
        );
        assert_eq!(
            parse_input(&state(), "/unsilence"),
            InputAction::Unknown("/unsilence".to_string())
        );
    }

    #[test]
    fn hash_passes_through_like_the_other_simple_server_commands() {
        assert_eq!(
            parse_input(&state(), "/hash"),
            InputAction::ServerCommand {
                command: "HASH".to_string(),
                rest: String::new()
            }
        );
    }

    #[test]
    fn statusbar_info_needs_an_item_name() {
        assert_eq!(
            parse_input(&state(), "/statusbar info clock"),
            InputAction::StatusbarInfo("clock".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/statusbar info"),
            InputAction::Unknown("/statusbar info".to_string())
        );
    }

    /// `/quote` sends whatever follows it to the wire completely
    /// verbatim -- no uppercasing (unlike every other passthrough
    /// command here), no reformatting, case and spacing untouched.
    #[test]
    fn quote_sends_the_raw_line_verbatim() {
        let action = parse_input(&state(), "/quote PRIVMSG #chan :hi there");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "PRIVMSG #chan :hi there".to_string(),
                rest: String::new()
            }
        );
    }

    #[test]
    fn quote_with_no_argument_is_unknown() {
        let action = parse_input(&state(), "/quote");
        assert_eq!(action, InputAction::Unknown("/quote".to_string()));
    }

    #[test]
    fn list_is_a_plain_passthrough() {
        let action = parse_input(&state(), "/list");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "LIST".to_string(),
                rest: String::new()
            }
        );
        let action = parse_input(&state(), "/list *chat*");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "LIST".to_string(),
                rest: "*chat*".to_string()
            }
        );
    }

    #[test]
    fn squery_and_servlist_are_plain_passthroughs() {
        let action = parse_input(&state(), "/squery NickServ help");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "SQUERY".to_string(),
                rest: "NickServ help".to_string()
            }
        );
        let action = parse_input(&state(), "/servlist");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "SERVLIST".to_string(),
                rest: String::new()
            }
        );
    }

    #[test]
    fn lastlog_needs_a_pattern() {
        let action = parse_input(&state(), "/lastlog hello");
        assert_eq!(
            action,
            InputAction::Lastlog {
                pattern: "hello".to_string(),
                is_regex: false,
                case_sensitive: false,
                count: None
            }
        );
        let action = parse_input(&state(), "/lastlog");
        assert_eq!(action, InputAction::Unknown("/lastlog".to_string()));
    }

    #[test]
    fn lastlog_trailing_number_is_a_count_not_part_of_the_pattern() {
        let action = parse_input(&state(), "/lastlog outage 20");
        assert_eq!(
            action,
            InputAction::Lastlog {
                pattern: "outage".to_string(),
                is_regex: false,
                case_sensitive: false,
                count: Some(20)
            }
        );
    }

    #[test]
    fn lastlog_a_lone_number_is_the_pattern_not_a_count() {
        let action = parse_input(&state(), "/lastlog 20");
        assert_eq!(
            action,
            InputAction::Lastlog {
                pattern: "20".to_string(),
                is_regex: false,
                case_sensitive: false,
                count: None
            }
        );
    }

    #[test]
    fn lastlog_regex_and_case_flags_parse_together_with_a_count() {
        let action = parse_input(&state(), r"/lastlog -regex -case err\d+ 5");
        assert_eq!(
            action,
            InputAction::Lastlog {
                pattern: r"err\d+".to_string(),
                is_regex: true,
                case_sensitive: true,
                count: Some(5)
            }
        );
    }

    #[test]
    fn lastlog_rejects_an_invalid_regex() {
        let action = parse_input(&state(), "/lastlog -regex (unterminated");
        assert_eq!(
            action,
            InputAction::Unknown("/lastlog -regex (unterminated".to_string())
        );
    }

    #[test]
    fn notify_with_no_argument_lists_unlike_unnotify_which_needs_one() {
        assert_eq!(parse_input(&state(), "/notify"), InputAction::Notify { nick: None });
        assert_eq!(
            parse_input(&state(), "/notify stranger"),
            InputAction::Notify {
                nick: Some("stranger".to_string())
            }
        );
        assert_eq!(
            parse_input(&state(), "/unnotify stranger"),
            InputAction::Unnotify {
                nick: "stranger".to_string()
            }
        );
        assert_eq!(
            parse_input(&state(), "/unnotify"),
            InputAction::Unknown("/unnotify".to_string())
        );
    }

    #[test]
    fn window_item_move_takes_a_window_number_or_an_item_name() {
        assert_eq!(parse_input(&state(), "/window item move 3"), InputAction::WindowItemMove(3));
        assert_eq!(
            parse_input(&state(), "/window item move #chatnet"),
            InputAction::WindowItemAdd("#chatnet".to_string()),
            "a name pulls that item into this window, same as `add`"
        );
        assert!(matches!(parse_input(&state(), "/window item move"), InputAction::Unknown(_)));
    }

    #[test]
    fn reconnect_cmds_and_redraw_take_no_arguments() {
        assert_eq!(parse_input(&state(), "/reconnect"), InputAction::Reconnect);
        assert_eq!(parse_input(&state(), "/cmds"), InputAction::Cmds(None));
        assert_eq!(parse_input(&state(), "/redraw"), InputAction::Redraw);
        assert_eq!(parse_input(&state(), "/resize"), InputAction::Redraw, "irssi's /resize re-reads the terminal size");
    }

    #[test]
    fn rmreconns_takes_no_arguments() {
        assert_eq!(parse_input(&state(), "/rmreconns"), InputAction::RmReconns);
    }

    #[test]
    fn rmrejoins_takes_no_arguments() {
        assert_eq!(parse_input(&state(), "/rmrejoins"), InputAction::RmRejoins);
    }

    #[test]
    fn cmds_takes_an_optional_command_name() {
        assert_eq!(
            parse_input(&state(), "/cmds bind"),
            InputAction::Cmds(Some("bind".to_string()))
        );
        assert_eq!(
            parse_input(&state(), "/cmds SET"),
            InputAction::Cmds(Some("set".to_string()))
        );
    }

    #[test]
    fn help_is_an_alias_for_cmds() {
        assert_eq!(parse_input(&state(), "/help"), InputAction::Cmds(None));
        assert_eq!(
            parse_input(&state(), "/help bind"),
            InputAction::Cmds(Some("bind".to_string()))
        );
    }

    #[test]
    fn update_takes_no_arguments() {
        assert_eq!(parse_input(&state(), "/update"), InputAction::Update);
    }

    #[test]
    fn upgrade_takes_no_arguments() {
        assert_eq!(parse_input(&state(), "/upgrade"), InputAction::Upgrade);
    }

    #[test]
    fn restart_takes_no_arguments() {
        assert_eq!(parse_input(&state(), "/restart"), InputAction::Restart);
    }

    #[test]
    fn autoban_bare_lists_patterns() {
        assert_eq!(
            parse_input(&state(), "/autoban"),
            InputAction::Autoban { pattern: None, is_regex: false, reason: None }
        );
    }

    #[test]
    fn autoban_with_flags_adds_a_pattern() {
        assert_eq!(
            parse_input(&state(), "/autoban -reason spam webcam"),
            InputAction::Autoban { pattern: Some("webcam".to_string()), is_regex: false, reason: Some("spam".to_string()) }
        );
        assert_eq!(
            parse_input(&state(), "/autoban -regex ^bad$"),
            InputAction::Autoban { pattern: Some("^bad$".to_string()), is_regex: true, reason: None }
        );
    }

    #[test]
    fn autoban_rejects_an_invalid_regex() {
        assert_eq!(parse_input(&state(), "/autoban -regex ["), InputAction::Unknown("/autoban -regex [".to_string()));
    }

    #[test]
    fn unautoban_takes_an_id_or_pattern() {
        assert_eq!(parse_input(&state(), "/unautoban 1"), InputAction::Unautoban("1".to_string()));
        assert_eq!(parse_input(&state(), "/unautoban webcam"), InputAction::Unautoban("webcam".to_string()));
    }

    #[test]
    fn pluginreload_is_bare_or_takes_one_script_name() {
        assert_eq!(parse_input(&state(), "/pluginreload"), InputAction::PluginReload(None));
        assert_eq!(
            parse_input(&state(), "/pluginreload example"),
            InputAction::PluginReload(Some("example".to_string()))
        );
    }

    #[test]
    fn pluginload_and_pluginunload_require_a_name() {
        assert_eq!(
            parse_input(&state(), "/pluginload example"),
            InputAction::PluginLoad("example".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/pluginload"),
            InputAction::Unknown("/pluginload".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/pluginunload example"),
            InputAction::PluginUnload("example".to_string())
        );
        assert_eq!(
            parse_input(&state(), "/pluginunload"),
            InputAction::Unknown("/pluginunload".to_string())
        );
    }

    #[test]
    fn bare_ping_fires_the_lag_check_but_ping_with_a_target_is_a_ctcp() {
        assert_eq!(parse_input(&state(), "/ping"), InputAction::Ping);
        assert_eq!(
            parse_input(&state(), "/ping dana"),
            InputAction::Ctcp {
                target: "dana".to_string(),
                command: "PING".to_string(),
                data: None
            }
        );
    }

    #[test]
    fn names_and_who_default_to_the_active_channel() {
        let action = parse_input(&state(), "/names");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "NAMES".to_string(),
                rest: "#irust-test".to_string()
            }
        );
        let action = parse_input(&state(), "/who");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "WHO".to_string(),
                rest: "#irust-test".to_string()
            }
        );
    }

    #[test]
    fn names_and_who_with_an_explicit_channel() {
        let action = parse_input(&state(), "/names #other");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "NAMES".to_string(),
                rest: "#other".to_string()
            }
        );
    }

    #[test]
    fn names_with_no_active_channel_and_no_argument_is_unknown() {
        let mut st = state();
        st.active_buffer = BufferId::Status;
        let action = parse_input(&st, "/names");
        assert_eq!(action, InputAction::Unknown("/names".to_string()));
    }

    #[test]
    fn whowas_passes_through_like_the_other_simple_server_commands() {
        let action = parse_input(&state(), "/whowas SomeNick");
        assert_eq!(
            action,
            InputAction::ServerCommand {
                command: "WHOWAS".to_string(),
                rest: "SomeNick".to_string()
            }
        );
    }

    #[test]
    fn msg_star_targets_the_active_buffer() {
        let action = parse_input(&state(), "/msg * hello there");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "#irust-test".to_string(),
                text: "hello there".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn msg_star_with_no_active_buffer_is_unknown() {
        let mut st = state();
        st.active_buffer = BufferId::Status;
        let action = parse_input(&st, "/msg * hello");
        assert_eq!(action, InputAction::Unknown("/msg * hello".to_string()));
    }

    #[test]
    fn a_service_nick_used_as_a_command_is_shorthand_for_msg() {
        // Echoed into the active channel, not a query -- irssi gives
        // the notice in whichever channel the command was run from
        // (#chatnet, #routing, ...), never a separate query window.
        let action = parse_input(&state(), "/k9 help");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "k9".to_string(),
                text: "help".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn a_service_nick_shorthand_preserves_the_typed_case() {
        let action = parse_input(&state(), "/K9 help");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "K9".to_string(),
                text: "help".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn cw_aliases_to_the_real_chatworld_nick() {
        let action = parse_input(&state(), "/cw jump");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "ChatWorld".to_string(),
                text: "jump".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn ns_aliases_to_nickserv() {
        let action = parse_input(&state(), "/ns identify hunter2");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "NickServ".to_string(),
                text: "identify hunter2".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn ms_aliases_to_memoserv() {
        let action = parse_input(&state(), "/ms list");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "MemoServ".to_string(),
                text: "list".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn a_service_nick_shorthand_echoes_into_whichever_channel_is_active() {
        let action = parse_input(&state(), "/os akill add *@bad.example");
        assert_eq!(
            action,
            InputAction::SendMessage {
                target: "os".to_string(),
                text: "akill add *@bad.example".to_string(),
                echo_buffer: BufferId::Channel("#irust-test".to_string()),
            }
        );
    }

    #[test]
    fn a_service_nick_shorthand_with_no_text_falls_through_to_a_plugin_command() {
        let action = parse_input(&state(), "/k9");
        assert_eq!(
            action,
            InputAction::Command {
                name: "k9".to_string(),
                arg: "".to_string(),
            }
        );
    }

    #[test]
    fn unrecognized_command_is_passed_through_for_a_plugin_to_handle() {
        let action = parse_input(&state(), "/nope some arg");
        assert_eq!(
            action,
            InputAction::Command {
                name: "nope".to_string(),
                arg: "some arg".to_string(),
            }
        );
    }

    #[test]
    fn network_add_parses_bouncer_flags() {
        let st = ClientState::new("me", "#c", 11);
        let action = parse_input(
            &st,
            "/network add cyber 127.0.0.1:31337 -nick abcX -password abcX/cyber:pw -auto -fingerprint AA:BB -noverify -sasl acc secret",
        );
        let InputAction::NetworkAdd { name, extras, .. } = action else {
            panic!("not a NetworkAdd");
        };
        assert_eq!(name, "cyber");
        assert_eq!(extras.password.as_deref(), Some("abcX/cyber:pw"));
        assert_eq!(extras.autoconnect, Some(true));
        assert_eq!(extras.fingerprint.as_deref(), Some("AA:BB"));
        assert_eq!(extras.tls_verify, Some(false));
        assert_eq!(extras.sasl, Some(("acc".to_string(), "secret".to_string())));
    }

    #[test]
    fn network_modify_only_sets_given_extras() {
        let st = ClientState::new("me", "#c", 11);
        let InputAction::NetworkModify { extras, nick, .. } = parse_input(&st, "/network modify cyber -noauto") else {
            panic!("not a NetworkModify");
        };
        assert_eq!(extras.autoconnect, Some(false));
        assert_eq!(extras.password, None);
        assert_eq!(nick, None);
    }
}
