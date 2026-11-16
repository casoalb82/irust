
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

mod dcc;
mod dcc_transfer;
mod iplookup;
mod replay_watermark;
mod updater;

use irust_caps::builtin::{
    AccountNotify, AccountTag, AwayNotify, Batch, ChgHost, DraftBot, EchoMessage, ExtendedJoin, InviteNotify,
    MessageTags, MultiPrefix, Sasl, SaslExternal, ServerTime, Setname, Starttls, UserhostInNames,
};
use irust_caps::{Capability, CapabilityRegistry};
use irust_config::{AppConfig, NetworkPreset};
use irust_core::{BufferId, BufferSummary, ClientState, FormatMode, InputAction, RecodeSub, Servers};
use irust_net::{ConnectOptions, Connection};
use irust_plugin::{PluginHookResult, PluginHost};
use irust_proto::parse;
use tokio::sync::{mpsc, Mutex};

/// Fallback only for a malformed/unreadable `server_reconnect_time` --
/// `time_ms_of` already validates at `/set` time, so this never fires
/// in practice. Matches irssi's own `server_reconnect_time` default.
const RECONNECT_FALLBACK_DELAY: Duration = Duration::from_secs(300);

fn lag_check_period() -> Duration {
    Duration::from_millis(irust_config::settings_net::time_ms_of("lag_check_time").max(1_000))
}
const LAG_PING_TOKEN: &str = "irust-lag";

fn lag_pong_miss_limit() -> u32 {
    let period = lag_check_period().as_millis().max(1);
    let max = irust_config::settings_net::time_ms_of("lag_max_before_disconnect") as u128;
    max.div_ceil(period).max(1) as u32
}

/// How often `/notify`'s watch list gets polled with `ISON`. Modest on
/// purpose -- a friends list doesn't need sub-minute freshness, and this
/// keeps it cheap on a busy server even with a long list.
const NOTIFY_CHECK_INTERVAL: Duration = Duration::from_secs(60);

const DEFAULT_FALLBACK_NICK: &str = "irust-abcX";

/// `/set nick <value>`'s global default, or `None` if it's still unset
/// (empty) -- `Option::or_else`-friendly wrapper around
/// `irust_config::settings::default_nick()` for `resolve_conn_params`'s
/// nick fallback chain.
fn non_empty_default_nick() -> Option<String> {
    let nick = irust_config::settings::default_nick();
    if nick.is_empty() {
        None
    } else {
        Some(nick)
    }
}

/// Same fallback, for the ad hoc "raw host, no config entry at all"
/// connect path -- that branch has no `resolved.sasl_account` to try
/// either, so this is just default-nick-or-hardcoded-fallback.
fn default_nick_or_fallback() -> String {
    non_empty_default_nick().unwrap_or_else(|| DEFAULT_FALLBACK_NICK.to_string())
}

const CMD_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "Messaging",
        &["/msg", "/me", "/action", "/notice", "/ctcp", "/nctcp", "/wall", "/ping"],
    ),
    (
        "Channels",
        &["/join", "/part", "/cycle", "/topic", "/invite", "/channel"],
    ),
    (
        "Channel moderation",
        &[
            "/kick",
            "/ban",
            "/unban",
            "/kickban",
            "/knockout",
            "/mode",
            "/op",
            "/deop",
            "/voice",
            "/devoice",
        ],
    ),
    (
        "Info",
        &["/who", "/names", "/whois", "/whowas", "/list", "/lastlog", "/ip"],
    ),
    (
        "Connection",
        &[
            "/connect",
            "/disconnect",
            "/server",
            "/reconnect",
            "/network",
            "/netsplit",
            "/quit",
        ],
    ),
    (
        "Settings",
        &[
            "/set",
            "/toggle",
            "/alias",
            "/unalias",
            "/bind",
            "/nickcolors",
            "/textcolors",
            "/notify",
            "/unnotify",
            "/ignore",
            "/unignore",
            "/hilight",
            "/dehilight",
        ],
    ),
    (
        "Windows & view",
        &[
            "/window",
            "/wc",
            "/split",
            "/unsplit",
            "/split_focus",
            "/clear",
            "/redraw",
            "/statusbar",
            "/layout",
            "/log",
        ],
    ),
    (
        "Other",
        &[
            "/away",
            "/back",
            "/setname",
            "/quote",
            "/uptime",
            "/exec",
            "/eval",
            "/wait",
            "/echo",
            "/beep",
            "/cat",
            "/cd",
            "/dcc",
            "/pluginreload",
            "/pluginload",
            "/pluginunload",
            "/pluginexec",
            "/pluginreset",
            "/load",
            "/unload",
            "/reload",
            "/script",
            "/update",
        ],
    ),
];

/// Word-wraps already-formatted `words` (e.g. `"/kick"`) into comma-
/// separated lines no wider than `width` columns, each starting with
/// `"--   "` (two spaces past the plain `"-- "` header prefix, so
/// wrapped command lines visually nest under their category header).
fn wrap_cmd_list(words: &[&str], width: usize) -> Vec<String> {
    let prefix = "--   ";
    let mut lines = Vec::new();
    let mut current = prefix.to_string();
    for (i, word) in words.iter().enumerate() {
        let piece = if i + 1 == words.len() {
            (*word).to_string()
        } else {
            format!("{word}, ")
        };
        if current.len() > prefix.len() && current.len() + piece.len() > width {
            lines.push(current.trim_end().to_string());
            current = prefix.to_string();
        }
        current.push_str(&piece);
    }
    if current.len() > prefix.len() {
        lines.push(current.trim_end().to_string());
    }
    lines
}

fn cmds_overview_lines() -> Vec<String> {
    let mut lines = Vec::new();
    for (i, (title, cmds)) in CMD_CATEGORIES.iter().enumerate() {
        if i > 0 {
            lines.push(String::new());
        }
        lines.push(format!("-- \x02{title}\x02"));
        lines.extend(wrap_cmd_list(cmds, 60));
    }
    lines.push(String::new());
    lines.push("-- Type /cmds <command> for that command's usage (e.g. /cmds bind).".to_string());
    lines
}

fn cmd_detail_for(cmd: &str) -> Option<Vec<String>> {
    if cmd == "bind" {
        let mut lines = vec![
            "-- \x02/bind\x02".to_string(),
            "--   /bind <key> <action>".to_string(),
            "--   /bind -delete <key>".to_string(),
            "--   /bind -reset".to_string(),
            "--   /bind  (no args: lists current bindings)".to_string(),
            String::new(),
            "-- \x02Actions\x02".to_string(),
        ];
        lines.extend(wrap_cmd_list(irust_core::BINDABLE_ACTIONS, 60));
        lines.push(String::new());
        lines.push("-- \x02Example\x02".to_string());
        lines.push("--   /bind ctrl-g nicklist_down".to_string());
        return Some(lines);
    }
    // (usage forms, an example invocation -- `None` only for commands
    // that take no arguments at all, where an example would just repeat
    // the bare form).
    let (forms, example): (&[&str], Option<&str>) = match cmd {
        "msg" => (&["/msg <target> <text>", "/msg * <text>  (whichever buffer is active)"], Some("/msg abcX are you around?")),
        "me" => (&["/me <text>  - send an action (CTCP ACTION) to the active buffer"], Some("/me waves")),
        "action" => (
            &["/action <target> <text>  - like /me, but to an explicit target, no need to switch windows"],
            Some("/action #chatnet waves"),
        ),
        "join" | "j" => (&["/join <#channel> [key]"], Some("/join #chatnet")),
        "channel" => (
            &[
                "/channel add|modify <#channel> <network> [-auto|-noauto] [-bots <mask1,mask2>] [-botcmd <command>] [password]",
                "/channel remove <#channel> <network>",
                "/channel  - bare, or 'list': shows every configured channel",
                "  -botcmd must come last -- it takes the rest of the line",
            ],
            Some("/channel add #chatnet chatnet -auto -bots K9,HelperBot -botcmd identify hunter2"),
        ),
        "part" => (&["/part [#channel] [message]  - default: the active channel"], Some("/part #chatnet be right back")),
        "nick" => (&["/nick <newnick>"], Some("/nick abcX_")),
        "topic" => (&["/topic [#channel] [text]", "/topic [#channel] -delete"], Some("/topic #chatnet welcome to ChatNet")),
        "kick" => (&["/kick [#channel] <nick> [reason]"], Some("/kick #chatnet baduser spamming the channel")),
        "ban" => (
            &[
                "/ban [#channel] <nick|mask>",
                "/ban [#channel] *  (bans everyone currently in the channel)",
            ],
            Some("/ban #chatnet baduser"),
        ),
        "unban" => (&["/unban [#channel] <nick|mask>"], Some("/unban #chatnet *!*@baduser.example.com")),
        "quiet" => (&["/quiet [#channel] <nick|mask>"], Some("/quiet #chatnet baduser")),
        "unquiet" => (&["/unquiet [#channel] <nick|mask> [more masks...]"], Some("/unquiet #chatnet baduser")),
        "kickban" => (&["/kickban [#channel] <nick> [reason]  - ban then kick"], Some("/kickban #chatnet baduser spamming")),
        "knockout" => (
            &[
                "/knockout [#channel] [minutes] <nick> [reason]",
                "  - temporary kickban, auto-unbanned after <minutes>",
            ],
            Some("/knockout #chatnet 10 baduser cool off for a bit"),
        ),
        "mode" => (&["/mode <modestring> [args...]  - on the active channel"], Some("/mode +m")),
        "op" => (&["/op <nick|mask> [more...]  - wildcards expand against the channel's nicklist"], Some("/op abcX")),
        "deop" => (&["/deop <nick|mask> [more...]"], Some("/deop abcX")),
        "voice" => (&["/voice <nick|mask> [more...]"], Some("/voice newbie")),
        "devoice" => (&["/devoice <nick|mask> [more...]"], Some("/devoice newbie")),
        "color" => (&["/color <nick> [index]  - no index: cycle to the next color"], Some("/color dana 3")),
        "wc" => (&["/wc [name|index]", "/wc *", "/wc all  - the last two close every buffer", "/wc queries (or /wc q)  - closes every query, channels untouched"], Some("/wc #chatnet")),
        "split" => (
            &[
                "/split [-v|-h] <name|index>  - opens another pane besides the focused one",
                "  -h stacks panes top-to-bottom (default), -v puts them side by side",
            ],
            Some("/split -v #chatnet"),
        ),
        "unsplit" => (
            &["/unsplit [name|index]  - no arg: closes the most recently opened pane", "/unsplit -all"],
            None,
        ),
        "split_focus" => (
            &[
                "/split_focus [name|index]  - jumps to a specific pane",
                "/split_focus  - no arg: cycles to the next pane",
                "/split_focus -prev  - cycles to the previous one",
            ],
            None,
        ),
        "nickcolors" => (&["/nickcolors [on|off]  - no arg: toggle"], Some("/nickcolors off")),
        "textcolors" => (&["/textcolors [on|off]  - no arg: toggle"], Some("/textcolors off")),
        "connect" => (&["/connect <network|host:port> [-tls|-notls] [-4|-6]"], Some("/connect chatnet")),
        "disconnect" => (&["/disconnect [network]  - default: the active one"], Some("/disconnect chatnet")),
        "network" => (
            &[
                "/network [list]",
                "/network add <name> <host:port> [-tls|-notls] [-nick n]",
                "         [-autojoin #chan] [-cert file] [-key file]",
                "         [-password pw] [-auto|-noauto] [-fingerprint sha256]",
                "         [-noverify] [-sasl account password]",
                "/network modify <name> [<host:port>] [same flags as add]  - only given fields change",
                "/network remove <name>",
            ],
            Some("/network add libera irc.libera.chat:6697 -nick abcX -autojoin #chatnet"),
        ),
        "server" => (
            &[
                "/server [list]  - lists saved server/network entries",
                "/server add <tag> <address> [port] [-notls|-tls] [-nick <n>] [-autojoin <#chan>] [-auto|-noauto] [-password <pw>]",
                "/server connect <tag>  - same as /connect <tag>",
                "/server modify <tag> [address[:port]] [same flags as add]",
                "/server remove <tag>",
            ],
            Some("/server add chatnet irc.example.net 6697"),
        ),
        "cycle" => (&["/cycle [#channel] [message]  - part then rejoin"], Some("/cycle #chatnet")),
        "clear" => (&["/clear [-all]  - default: just the active buffer"], Some("/clear -all")),
        "notice" => (&["/notice <target> <text>"], Some("/notice #chatnet server restarting in 5 minutes")),
        "invite" => (&["/invite <nick> [#channel]  - default channel: the active one"], Some("/invite dana #chatnet")),
        "ctcp" => (&["/ctcp <target> <command> [data]"], Some("/ctcp abcX VERSION")),
        "nctcp" => (
            &["/nctcp <target> <command> [data]  - an unsolicited CTCP reply, not a request"],
            Some("/nctcp abcX PING 12345"),
        ),
        "wall" => (
            &["/wall [#channel] <text>  - ops-only channel message (no channel: the active one)"],
            Some("/wall heads up, staff only"),
        ),
        "query" | "q" => (&["/query [nick] [message]"], Some("/query dana hey, got a sec?")),
        "whois" | "wi" => (&["/whois [nick]  - default: the active query's nick, or yourself"], Some("/whois dana")),
        "ip" => (&["/ip <nick>  - resolve a nick's real IP (oper only)"], Some("/ip baduser")),
        "away" => (&["/away [reason]"], Some("/away back in 10 minutes")),
        "back" => (&["/back  - clears away status"], None),
        "quit" => (
            &["/quit [reason]  - no reason: picks one at random from a built-in pool"],
            Some("/quit see you tomorrow"),
        ),
        "quote" => (&["/quote <raw IRC line>  - sent to the server exactly as typed"], Some("/quote PRIVMSG #chatnet :hi from /quote")),
        "oper" => (&["/oper <user> <password>"], Some("/oper abcX hunter2")),
        "rehash" => (&["/rehash [reason]"], Some("/rehash reloading updated ircd.conf")),
        "knock" => (&["/knock <#channel>"], Some("/knock #staff-only")),
        "links" => (&["/links  - server topology"], None),
        "lusers" => (&["/lusers  - network user/server counts"], None),
        "map" => (&["/map  - server link tree"], None),
        "motd" => (&["/motd [server]"], Some("/motd")),
        "stats" => (&["/stats <letter>  - e.g. /stats u for uptime"], Some("/stats u")),
        "trace" => (&["/trace [target]"], Some("/trace London.UK.EU.ChatNet.Gg")),
        "admin" => (&["/admin [server]"], Some("/admin")),
        "time" => (&["/time [server]"], Some("/time")),
        "version" => (&["/version [server]"], Some("/version")),
        "userhost" => (&["/userhost <nick> [nick2...]"], Some("/userhost abcX dana")),
        "ison" => (&["/ison <nick> [nick2...]"], Some("/ison abcX dana")),
        "accept" => (&["/accept <nick>|-<nick>|*  - manage your callerid (+g) whitelist"], Some("/accept dana")),
        "silence" => (&["/silence [+mask|-mask]  - no arg: list your silence masks"], Some("/silence +*!*@baduser.example.com")),
        "unsilence" => (&["/unsilence <mask>  - same as /silence -mask"], Some("/unsilence *!*@baduser.example.com")),
        "whowas" => (&["/whowas <nick> [count]"], Some("/whowas oldnick 5")),
        "info" => (&["/info  - server/build info"], None),
        "kill" => (&["/kill <nick> [reason]  (oper)"], Some("/kill baduser ban evasion")),
        "kline" => (&["/kline [duration] <user@host> [reason]  (oper)"], Some("/kline 60 *@baduser.example.com repeated spam")),
        "unkline" => (&["/unkline <user@host>  (oper)"], Some("/unkline *@baduser.example.com")),
        "dline" => (&["/dline [duration] <ip> [reason]  (oper)"], Some("/dline 60 203.0.113.5 open proxy")),
        "undline" => (&["/undline <ip>  (oper)"], Some("/undline 203.0.113.5")),
        "xline" => (&["/xline [duration] <gecos mask> [reason]  (oper)"], Some("/xline *sp4mbot* known spam client")),
        "unxline" => (&["/unxline <gecos mask>  (oper)"], Some("/unxline *sp4mbot*")),
        "resv" => (&["/resv [duration] <channel|nick> [reason]  (oper)"], Some("/resv #stafftest reserved for staff")),
        "unresv" => (&["/unresv <channel|nick>  (oper)"], Some("/unresv #stafftest")),
        "gline" => (&["/gline [duration] <user@host> [reason]  (oper, network-wide)"], Some("/gline 1440 *@baduser.example.com network-wide abuse")),
        "ungline" => (&["/ungline <user@host>  (oper)"], Some("/ungline *@baduser.example.com")),
        "testline" => (&["/testline <mask>  - check what K/D/X-line a mask would match (oper)"], Some("/testline baduser@203.0.113.5")),
        "operwall" => (&["/operwall <message>  - sent to every oper (oper)"], Some("/operwall heads up, restarting the hub in 5")),
        "grant" => (&["/grant <nick> <operclass>  (oper)"], Some("/grant abcX admin")),
        "chghost" => (&["/chghost <nick> <newhost>  (oper)"], Some("/chghost abcX chatnet/staff/abcX")),
        "sethost" => (&["/sethost <newhost>  - sets your OWN vhost, unlike /chghost (oper)"], Some("/sethost chatnet/staff/abcX")),
        "close" => (&["/close  - drop all unregistered (still-connecting) connections (oper)"], None),
        "unreject" => (&["/unreject <mask>  (oper)"], Some("/unreject *@203.0.113.5")),
        "testmask" => (&["/testmask <mask>  - count how many users a ban mask would match (oper)"], Some("/testmask *!*@*.example.com")),
        "etrace" => (&["/etrace [target]  (oper)"], Some("/etrace")),
        "operspy" => (&["/operspy  - toggles OperSpy mode (oper)"], None),
        "snote" => (&["/snote <+flags|-flags>  - server notice mask (oper)"], Some("/snote +c")),
        "list" => (&["/list [pattern]"], Some("/list *chat*")),
        "hash" => (&["/hash  - ircd hash-table stats (oper, if supported)"], None),
        "lastlog" => (
            &["/lastlog [-regex] [-case] <pattern> [count]  - search the active buffer's scrollback"],
            Some("/lastlog -case outage 20"),
        ),
        "log" => (
            &[
                "/log start|stop [name|index]  - no target: the active window",
                "/log open <file>  - custom log path for the active window",
                "/log close  - same as /log stop, for the active window",
                "/log  - no args: reports the active window's logging status",
            ],
            Some("/log stop #chatnet"),
        ),
        "notify" => (&["/notify [nick]  - no arg: show your watch list"], Some("/notify dana")),
        "unnotify" => (&["/unnotify <nick>"], Some("/unnotify dana")),
        "reconnect" => (&["/reconnect  - takes no arguments"], None),
        "netsplit" => (
            &["/netsplit  - lists currently-recognized netsplits, takes no arguments"],
            None,
        ),
        "cmds" | "help" => (&["/cmds [command]", "  - bare /cmds lists categories; /cmds <command> shows that command's usage", "  (/help is an alias for /cmds)"], Some("/cmds bind")),
        "redraw" => (&["/redraw  - forces a full terminal repaint, takes no arguments"], None),
        "update" => (&["/update  - self-update to the latest release, takes no arguments"], None),
        "upgrade" => (&["/upgrade  - self-update AND restart: quits every network, then relaunches into the new build"], None),
        "restart" => (&["/restart  - same restart as /upgrade, without checking for an update first"], None),
        "pluginreload" => (&["/pluginreload [name]  - no name: reload every loaded plugin"], Some("/pluginreload example")),
        "pluginload" => (&["/pluginload <name>"], Some("/pluginload example")),
        "pluginunload" => (&["/pluginunload <name>"], Some("/pluginunload example")),
        "pluginexec" => (
            &["/pluginexec <lua code>  - runs an ad-hoc snippet inline, not a saved plugin"],
            Some("/pluginexec irust.on_command('foo', function() end)"),
        ),
        "pluginreset" => (&["/pluginreset  - unloads every currently-loaded plugin at once"], None),
        "load" => (&["/load <name>  - irssi name for /pluginload"], Some("/load example")),
        "unload" => (&["/unload <name>  - irssi name for /pluginunload"], Some("/unload example")),
        "reload" => (&["/reload [name]  - irssi name for /pluginreload"], Some("/reload example")),
        "script" => (
            &[
                "/script load <name>  - irssi name for /pluginload",
                "/script unload <name>  - irssi name for /pluginunload",
                "/script exec <lua code>  - irssi name for /pluginexec",
                "/script reset  - irssi name for /pluginreset",
            ],
            Some("/script load example"),
        ),
        "ping" => (&["/ping [nick]  - no nick: measure your own lag to the server"], Some("/ping dana")),
        "names" => (&["/names [#channel]  - default: the active channel"], Some("/names #chatnet")),
        "who" => (&["/who [#channel]  - default: the active channel"], Some("/who #chatnet")),
        "ts" => (&["/ts  - the topic of every channel you are in"], None),
        "die" => (&["/die  (oper)  - asks the server to shut down; only IRC operators may"], None),
        "ircnet" => (&["/ircnet [list|add|remove|modify ...]  - alias of /network"], None),
        "unquery" => (&["/unquery [nick]  - close that query, or the active one"], Some("/unquery dana")),
        "ver" => (&["/ver [nick|#channel|*]  - CTCP VERSION; no argument in a query asks the query partner"], Some("/ver dana")),
        "flushbuffer" => (&["/flushbuffer  - irssi flushes buffered log writes; irust never buffers them"], None),
        "squit" => (&["/squit <server> [reason]  (oper)"], Some("/squit Istanbul.TR.EU.ChatNet.Gg scheduled maintenance")),
        "sconnect" => (
            &["/sconnect <new server> [[<port>] <existing server>]  (oper)"],
            Some("/sconnect hub2.example.net 6667 hub1.example.net"),
        ),
        "wallops" => (&["/wallops <message>  (oper)"], Some("/wallops restarting the hub in 5 minutes")),
        "window" => (
            &[
                "/window <name|index>", "/window list", "/window name <name>",
                "/window item add|remove|goto <name> | [prev|next]  - group several windows into one slot",
                "/window new  - an empty scratch window (no channel/query attached)",
                "/window immortal [on|off|toggle]  - blocks the active window from being closed",
                "/window stick  - excludes the active window from Ctrl-N/P cycling",
                "/window hide [name|index]  - same as /unsplit",
                "/window show [name|index] [-right]  - same as /split",
                "/window server <name>  - switch the active window to a connected network",
            ],
            Some("/window #chatnet"),
        ),
        "alias" => (&["/alias [name] [command]  - no args: list aliases; name only: show it"], Some("/alias k9 msg k9")),
        "unalias" => (&["/unalias <name>"], Some("/unalias k9")),
        "eval" => (
            &[
                "/eval <command(s)>  - split on ';', $-expandos expanded, resubmitted as commands",
                "$N nick, $C channel, $T target, $Q query, $tag/$chatnet, $topic, $usermode, $cumode",
                "$Y realname, $J version, $Z time, $W cwd, $F uptime, $[N]name pads to N chars",
            ],
            Some("/eval echo hi, $N! you are in $C"),
        ),
        "wait" => (
            &["/wait <milliseconds>  - delays processing whatever's queued next"],
            Some("/eval echo one; wait 500; echo two"),
        ),
        "set" => (&["/set [key] [value]  - no args: list settings; key only: show its value"], Some("/set nickcolors on")),
        "toggle" => (&["/toggle <key> [on|off]  - no value: flips the current on/off setting"], Some("/toggle bell")),
        "exec" => (&["/exec <shell command>  - output is echoed into the active buffer"], Some("/exec uptime")),
        "echo" => (&["/echo <text>  - local only, printed into the active buffer"], Some("/echo testing 1 2 3")),
        "beep" => (&["/beep  - rings the terminal bell right now, regardless of /set bell"], None),
        "cat" => (&["/cat <file>  - dumps a local text file into the active buffer"], Some("/cat ~/notes.txt")),
        "cd" => (&["/cd <directory>  - changes irust's own working directory"], Some("/cd ~/irc-logs")),
        "uptime" => (&["/uptime  - takes no arguments"], None),
        "clientversion" => (&["/clientversion  - takes no arguments"], None),
        "setname" => (
            &["/setname <realname>  - live, requires the server to support the setname IRCv3 cap"],
            Some("/setname Alex Smith"),
        ),
        "completion" => (&["/completion [key] [value]", "/completion -delete <key>"], Some("/completion afk brb, away from keyboard")),
        "ignore" => (
            &[
                "/ignore [-regex] [-except] [-replies] [mask|pattern] [level...|all]  - no args: list ignores",
                "  levels: msgs, notices, ctcps, joins, parts, quits, nicks, invites, wallops",
                "  -except: a carve-out from a broader ignore that also matches",
                "  -replies: also ignore a channel message addressing this nick (\"nick: ...\")",
            ],
            Some("/ignore -regex ^Bad.*Bot$ all"),
        ),
        "unignore" => (&["/unignore <mask>"], Some("/unignore *!*@baduser.example.com")),
        "statusbar" => (
            &[
                "/statusbar [list]  - show the current bottom-bar layout",
                "/statusbar add [-before <item>|-after <item>] [-alignment left|right] <item>",
                "/statusbar remove <item>",
                "/statusbar reset  - back to the default layout",
                "/statusbar info <item>  - that item's alignment and position",
                "  items: clock, user, window, lag, act",
            ],
            Some("/statusbar add -after window act"),
        ),
        "layout" => (
            &[
                "/layout [list]  - show which networks have a saved layout",
                "/layout save  - snapshot every connected network's channels/queries",
                "/layout reset  - clear the entire saved layout",
            ],
            Some("/layout save"),
        ),
        "hilight" => (
            &[
                "/hilight [-regex] [-mask] [-channels #a,#b] [-color value] <pattern>  - no arg: list hilights",
            ],
            Some("/hilight -regex -mask -channels #ops -color 203 ^Bad.*Bot$"),
        ),
        "dehilight" => (&["/dehilight <word>"], Some("/dehilight abcX")),
        "autoban" => (
            &["/autoban [-regex] [-reason <text>] <pattern>  - no arg: list patterns. Needs `/set autoban on`."],
            Some("/autoban -reason spam \"come watch me on my webcam\""),
        ),
        "unautoban" => (&["/unautoban <id|pattern>"], Some("/unautoban 1")),
        "dcc" => (
            &[
                "/dcc chat <nick>",
                "/dcc send [-passive] <nick> <path>  - -passive: use when you're behind a NAT",
                "/dcc get <id>  - accept an incoming SEND offer, id from /dcc list",
                "/dcc list  - show queued offers and active transfers",
                "/dcc close <id>  - drop an offer, or cancel an active transfer (id from /dcc list)",
            ],
            Some("/dcc send dana ~/movie.mkv"),
        ),
        _ => return None,
    };
    let header_cmd = forms.first().and_then(|f| f.split_whitespace().next()).unwrap_or(cmd);
    let mut lines = vec![format!("-- \x02{header_cmd}\x02")];
    lines.extend(forms.iter().map(|f| format!("--   {f}")));
    if let Some(example) = example {
        lines.push(String::new());
        lines.push("-- \x02Example\x02".to_string());
        lines.push(format!("--   {example}"));
    }
    Some(lines)
}

const PLUGIN_HOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// Every server channel/action queue currently wired up, keyed by tag --
/// shared between the dispatcher (which adds/removes entries as
/// `/connect`/`/disconnect` happen) and nothing else.
type ServerChannels = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<InputAction>>>>;

/// Everything needed to attempt one connection, resolved once up front
/// so `run_server`'s reconnect loop doesn't have to re-touch config.
#[derive(Clone)]
struct ConnParams {
    host: String,
    port: u16,
    nick: String,
    /// First autojoin channel, if the network has one configured --
    /// `None` means this server starts parked on its status window.
    channel: Option<String>,
    tls: bool,
    starttls: bool,
    verify_tls: bool,
    /// SHA-256 cert-pinning fingerprint (irssi-parity `ssl_pinned_cert`)
    /// -- when set, wins over `verify_tls`; see
    /// `irust_net::tls::connector_pinned`'s own doc comment.
    tls_fingerprint: Option<String>,
    server_password: Option<String>,
    sasl_creds: Option<(String, String)>,
    client_auth: Option<(String, String)>,
    ip_version: Option<u8>,
}

fn split_host_port(target: &str, default_port: u16) -> (String, u16) {
    if let Some(rest) = target.strip_prefix('[') {
        if let Some((host, after)) = rest.split_once(']') {
            let port = after
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(default_port);
            return (host.to_string(), port);
        }
    }
    if target.matches(':').count() > 1 {
        // More than one colon with no brackets: a bare IPv6 literal,
        // not a `host:port` pair -- see the doc comment above.
        return (target.to_string(), default_port);
    }
    match target
        .rsplit_once(':')
        .and_then(|(h, p)| Some((h, p.parse::<u16>().ok()?)))
    {
        Some((h, p)) if !h.is_empty() => (h.to_string(), p),
        _ => (target.to_string(), default_port),
    }
}

fn resolve_conn_params(
    name: &str,
    app_config: &AppConfig,
    tls_override: Option<bool>,
    ip_version: Option<u8>,
) -> anyhow::Result<ConnParams> {
    let preset = irust_config::builtin_presets()
        .into_iter()
        .find(|p| p.id.eq_ignore_ascii_case(name));
    let preset = match preset {
        Some(p) => p,
        None if app_config.networks.contains_key(name) => NetworkPreset {
            id: name.to_string(),
            display_name: name.to_string(),
            servers: Vec::new(),
            default_port: 6697,
            tls: true,
            autojoin: Vec::new(),
        },
        None => {
            let (host, port) = split_host_port(name, 6697);
            return Ok(ConnParams {
                host,
                port,
                nick: default_nick_or_fallback(),
                channel: None,
                tls: tls_override.unwrap_or(true),
                starttls: false,
                verify_tls: true,
                tls_fingerprint: None,
                server_password: None,
                sasl_creds: None,
                client_auth: None,
                ip_version,
            });
        }
    };
    let resolved = irust_config::resolve_network(&preset, app_config);
    let host = resolved
        .host
        .clone()
        .ok_or_else(|| anyhow::anyhow!("network '{name}' has no host configured"))?;
    let nick = resolved
        .nick
        .clone()
        .or_else(non_empty_default_nick)
        .or_else(|| resolved.sasl_account.clone())
        .unwrap_or_else(|| DEFAULT_FALLBACK_NICK.to_string());
    let channel = resolved.autojoin.first().cloned();
    let sasl_creds = match (&resolved.sasl_account, &resolved.sasl_password) {
        (Some(a), Some(p)) => Some((a.clone(), p.clone())),
        _ => None,
    };
    let client_auth = match (&resolved.client_cert, &resolved.client_key) {
        (Some(cert), Some(key)) => Some((cert.clone(), key.clone())),
        _ => None,
    };
    Ok(ConnParams {
        host,
        port: resolved.port,
        nick,
        channel,
        tls: if resolved.starttls {
            false
        } else {
            tls_override.unwrap_or(resolved.tls)
        },
        starttls: resolved.starttls,
        verify_tls: resolved.tls_verify,
        tls_fingerprint: resolved.tls_fingerprint.clone(),
        server_password: resolved.server_password.clone(),
        sasl_creds,
        client_auth,
        ip_version,
    })
}

fn apply_saved_layout(
    params: &mut ConnParams,
    layout: Option<irust_config::NetworkLayout>,
) -> (Vec<(String, Option<String>)>, Vec<String>) {
    let Some(layout) = layout else {
        return (Vec::new(), Vec::new());
    };
    let mut chans = layout.channels.into_iter();
    params.channel = chans.next().map(|c| c.name);
    let extra_channels = chans.map(|c| (c.name, c.key)).collect();
    (extra_channels, layout.queries)
}

fn channel_setup_extra_channels(
    entries: std::collections::BTreeMap<String, irust_config::ChannelSetup>,
    already_covered: impl Fn(&str) -> bool,
) -> Vec<(String, Option<String>)> {
    entries
        .into_iter()
        .filter(|(name, entry)| entry.autojoin && !already_covered(name))
        .map(|(name, entry)| (name, entry.password))
        .collect()
}

fn chunk_channel_mode(modestring: &str, args: &[String]) -> Vec<(String, Vec<String>)> {
    const MAX_MODES_PER_LINE: usize = 4;
    if args.is_empty() {
        return vec![(modestring.to_string(), Vec::new())];
    }

    let mut pairs: Vec<(char, char)> = Vec::new();
    let mut sign = '+';
    for c in modestring.chars() {
        if c == '+' || c == '-' {
            sign = c;
        } else {
            pairs.push((sign, c));
        }
    }
    if pairs.len() != args.len() {
        return vec![(modestring.to_string(), args.to_vec())];
    }

    pairs
        .chunks(MAX_MODES_PER_LINE)
        .zip(args.chunks(MAX_MODES_PER_LINE))
        .map(|(pchunk, achunk)| {
            let mut out = String::new();
            let mut last_sign = None;
            for (sign, letter) in pchunk {
                if last_sign != Some(*sign) {
                    out.push(*sign);
                    last_sign = Some(*sign);
                }
                out.push(*letter);
            }
            (out, achunk.to_vec())
        })
        .collect()
}

/// Lowercased, with a leading `#`/`&` stripped if present -- so `/window
/// brazil` and `/window #brazil` both reach `#brazil` the same way.
fn normalize_buffer_name(name: &str) -> String {
    name.strip_prefix(['#', '&']).unwrap_or(name).to_lowercase()
}

async fn close_resolved_buffer(
    servers: &Arc<Mutex<Servers>>,
    tag: &str,
    conn: &mut Connection,
    dcc_sessions: &dcc::DccSessions,
    active: BufferId,
) -> bool {
    let part_line = {
        let guard = servers.lock().await;
        guard.state(tag).and_then(|st| match &active {
            BufferId::Channel(key) => st
                .channels
                .get(key)
                .filter(|c| c.joined)
                .map(|c| format!("PART {}", c.display_name)),
            BufferId::Status | BufferId::Query(_) | BufferId::Empty(_) => None,
        })
    };
    if let BufferId::Query(key) = &active {
        if key.starts_with("dcc:") {
            if let Some(handle) = dcc_sessions.lock().await.remove(&(tag.to_string(), key.clone())) {
                let _ = handle.write_tx.send(dcc::DccOutbound::Close);
            }
        }
    }
    let part_ok = match part_line {
        Some(line) => conn.write_line(&line).await.is_ok(),
        None => true,
    };
    let (closed, is_immortal) = {
        let mut guard = servers.lock().await;
        match guard.state_mut(tag) {
            Some(st) => {
                let is_immortal = st.immortal.contains(&active);
                (st.close_buffer(&active), is_immortal)
            }
            None => (false, false),
        }
    };
    if !closed {
        let msg = if is_immortal {
            "-- window is immortal, if you really want to close it, say /window immortal off".to_string()
        } else {
            "-- can't close the status window".to_string()
        };
        push_status_warning(servers, tag, msg).await;
    }
    part_ok
}

fn active_target(state: &ClientState) -> Option<String> {
    match &state.active_buffer {
        BufferId::Channel(key) => state.channels.get(key).map(|c| c.display_name.clone()),
        BufferId::Query(key) => state.query_buffers.get(key).map(|b| b.name.clone()),
        _ => None,
    }
}

fn resolve_buffer_by_name(state: &ClientState, query: &str) -> Result<BufferId, String> {
    let summaries = state.buffer_summaries();
    let query_norm = normalize_buffer_name(query);
    if let Some(s) = summaries
        .iter()
        .find(|s| normalize_buffer_name(&s.display_name) == query_norm)
    {
        return Ok(s.id.clone());
    }
    let matches: Vec<&BufferSummary> = summaries
        .iter()
        .filter(|s| normalize_buffer_name(&s.display_name).starts_with(&query_norm))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("-- no buffer matching '{query}'")),
        many => {
            let names: Vec<&str> = many.iter().map(|s| s.display_name.as_str()).collect();
            Err(format!("-- ambiguous '{query}': {}", names.join(", ")))
        }
    }
}

fn apply_log_toggle(st: &mut ClientState, id: &BufferId, action: irust_core::LogToggle) -> Result<String, String> {
    let Some(shown) = st.buffer(id).map(|b| b.shown_name().to_string()) else {
        return Err("-- no such window".to_string());
    };
    match action {
        irust_core::LogToggle::Stop => {
            let buf = st.buffer_mut(id).expect("just confirmed this id resolves");
            if !buf.is_logging() {
                return Ok(format!("-- logging is already off for {shown}"));
            }
            buf.stop_logging();
            Ok(format!("-- logging stopped for {shown}"))
        }
        irust_core::LogToggle::Start => {
            let name = st.buffer(id).map(|b| b.name.clone()).unwrap_or_default();
            let Some(path) = st.log_path_for(&name) else {
                return Err("-- logging isn't set up for this connection".to_string());
            };
            let buf = st.buffer_mut(id).expect("just confirmed this id resolves");
            if buf.is_logging() {
                return Ok(format!("-- logging is already on for {shown}"));
            }
            buf.resume_logging(path);
            Ok(format!("-- logging started for {shown}"))
        }
        irust_core::LogToggle::Close => {
            let buf = st.buffer_mut(id).expect("just confirmed this id resolves");
            if !buf.is_logging() {
                return Ok(format!("-- logging is already off for {shown}"));
            }
            buf.stop_logging();
            Ok(format!("-- log closed for {shown}"))
        }
        irust_core::LogToggle::Open(path) => {
            let buf = st.buffer_mut(id).expect("just confirmed this id resolves");
            buf.resume_logging(PathBuf::from(&path));
            Ok(format!("-- logging {shown} to {path}"))
        }
    }
}

/// `/log` with no verb -- reports the target buffer's current logging
/// state (and where, if it's on) instead of guessing what the user
/// wanted toggled.
fn log_status_line(st: &ClientState, id: &BufferId) -> String {
    let Some(buf) = st.buffer(id) else {
        return "-- no such window".to_string();
    };
    match buf.log_path() {
        Some(path) => format!("-- logging {} to {}", buf.shown_name(), path.display()),
        None => format!("-- logging is off for {}", buf.shown_name()),
    }
}

fn netsplit_summary_lines(st: &ClientState) -> Vec<String> {
    let splits = st.recent_netsplits();
    if splits.is_empty() {
        return vec!["-- no active netsplits".to_string()];
    }
    splits
        .iter()
        .map(|s| {
            format!(
                "-- Netsplit {} <-> {}: {} nick{}, split {} ago",
                s.server_a,
                s.server_b,
                s.nick_count,
                if s.nick_count == 1 { "" } else { "s" },
                format_short_duration(s.since)
            )
        })
        .collect()
}

fn format_short_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// When the irust process itself started -- set once, at the very top
/// of `main()`, read back by `/uptime` (irssi's own client-side
/// `UPTIME`, unrelated to any server's uptime).
static START_TIME: OnceLock<Instant> = OnceLock::new();
static RESTART_AFTER_QUIT: AtomicBool = AtomicBool::new(false);
static RESTART_EXE: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

fn expand_tilde(path: &str) -> String {
    if path == "~" {
        std::env::var("HOME").unwrap_or_else(|_| path.to_string())
    } else if let Some(rest) = path.strip_prefix("~/") {
        match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => path.to_string(),
        }
    } else {
        path.to_string()
    }
}

fn foreach_prefixed(command: &str) -> String {
    if command.starts_with('/') {
        command.to_string()
    } else {
        format!("/{command}")
    }
}

fn format_uptime(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    format!(
        "Uptime: {}d {}h {}m {}s",
        secs / 86400,
        secs / 3600 % 24,
        secs / 60 % 60,
        secs % 60
    )
}

/// `/hilight`'s listing display for one entry -- the bare pattern when
/// no flags are set (the common case, unchanged from before flags
/// existed), otherwise the pattern plus which flags apply.
fn format_hilight_entry(e: &irust_config::HilightEntry) -> String {
    let mut flags = Vec::new();
    if e.is_regex {
        flags.push("-regex".to_string());
    }
    if e.is_mask {
        flags.push("-mask".to_string());
    }
    if !e.channels.is_empty() {
        flags.push(format!("-channels {}", e.channels.join(",")));
    }
    if let Some(color) = &e.color {
        flags.push(format!("-color {color}"));
    }
    if let Some(actcolor) = &e.actcolor {
        flags.push(format!("-actcolor {actcolor}"));
    }
    if e.priority != 0 {
        flags.push(format!("-priority {}", e.priority));
    }
    if flags.is_empty() {
        e.pattern.clone()
    } else {
        format!("{} ({})", e.pattern, flags.join(" "))
    }
}

fn starter_aliases() -> HashMap<String, String> {
    [
        ("bye", "quit"),
        ("exit", "quit"),
        ("signoff", "quit"),
        ("b", "ban"),
        ("bans", "ban"),
        ("c", "clear"),
        ("date", "time"),
        ("host", "userhost"),
        ("j", "join"),
        ("k", "kick"),
        ("kb", "kickban"),
        ("kn", "knockout"),
        ("leave", "part"),
        ("p", "part"),
        ("n", "names"),
        ("t", "topic"),
        ("ub", "unban"),
        ("w", "who"),
        ("ww", "whowas"),
        ("wg", "window"),
        ("umode", "mode $N"),
        ("exemptlist", "mode $C +e"),
        ("invitelist", "mode $C +I"),
        ("wii", "whois"),
        ("say", "msg *"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn expand_alias(aliases: &HashMap<String, String>, state: &ClientState, text: &str, depth: u8) -> String {
    const MAX_DEPTH: u8 = 5;
    if depth >= MAX_DEPTH {
        return text.to_string();
    }
    let Some(rest) = text.strip_prefix('/') else {
        return text.to_string();
    };
    let mut parts = rest.splitn(2, ' ');
    let name = parts.next().unwrap_or("").to_lowercase();
    let arg = parts.next().unwrap_or("");
    let Some(template) = aliases.get(&name) else {
        return text.to_string();
    };

    let args: Vec<&str> = if arg.is_empty() {
        Vec::new()
    } else {
        arg.split(' ').collect()
    };
    let mut used_star = false;
    // Set on any `$0`-`$9` reference, even one past the end of `args`
    // (nothing substituted that time) -- the alias author still meant
    // to consume the arg stream positionally, so the trailing verbatim-
    // append below must stay suppressed just like it is for `$*`.
    let mut used_positional = false;
    let mut expanded = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            expanded.push(c);
            continue;
        }
        match chars.peek() {
            Some('*') => {
                chars.next();
                expanded.push_str(arg);
                used_star = true;
            }
            Some('N') => {
                chars.next();
                expanded.push_str(&state.nick);
            }
            Some('C') => {
                chars.next();
                if let BufferId::Channel(key) = &state.active_buffer {
                    if let Some(chan) = state.channels.get(key) {
                        expanded.push_str(&chan.display_name);
                    }
                }
            }
            Some(d) if d.is_ascii_digit() => {
                let d = chars.next().unwrap();
                used_positional = true;
                if let Some(a) = args.get(d.to_digit(10).unwrap() as usize) {
                    expanded.push_str(a);
                }
            }
            _ => expanded.push('$'),
        }
    }
    if !used_star && !used_positional && !arg.is_empty() {
        if !expanded.is_empty() {
            expanded.push(' ');
        }
        expanded.push_str(arg);
    }
    expand_alias(aliases, state, &format!("/{expanded}"), depth + 1)
}

fn ignore_level_for(command: &str, second_param: Option<&str>) -> &'static str {
    match command {
        "NOTICE" => return "notices",
        "WALLOPS" => return "wallops",
        "INVITE" => return "invites",
        _ => {}
    }
    let is_ctcp = second_param.is_some_and(|t| t.starts_with('\u{1}'));
    let is_action = second_param.is_some_and(|t| t.starts_with("\u{1}ACTION"));
    if is_ctcp && !is_action {
        "ctcps"
    } else {
        "msgs"
    }
}

/// The display name of whichever channel is active on `tag`'s server, if
/// any (`None` while viewing the status window or a query buffer) --
/// shared by every command that implicitly targets "the channel you're
/// looking at" (`/part`, `/topic`, `/kick`, `/mode` and its `/op`-family
/// shortcuts, `/cycle`).
async fn active_channel_name(servers: &Arc<Mutex<Servers>>, tag: &str) -> Option<String> {
    let guard = servers.lock().await;
    let st = guard.state(tag)?;
    match &st.active_buffer {
        BufferId::Channel(key) => st.channels.get(key).map(|c| c.display_name.clone()),
        BufferId::Status | BufferId::Query(_) | BufferId::Empty(_) => None,
    }
}

async fn push_status_or_pending(servers: &Arc<Mutex<Servers>>, tag: Option<&str>, status: String) {
    match tag {
        Some(tag) => push_status(servers, tag, status).await,
        None => servers.lock().await.push_pending_message(false, status),
    }
}

fn shown_setting_value<'a>(key: &str, value: &'a str) -> &'a str {
    if key == "proxy_password" && !value.is_empty() { "(hidden)" } else { value }
}

async fn push_status(servers: &Arc<Mutex<Servers>>, tag: &str, status: String) {
    let mut guard = servers.lock().await;
    let Some(st) = guard.state_mut(tag) else { return };
    let active = st.active_buffer.clone();
    match active {
        BufferId::Status => st.status.push_system(status),
        BufferId::Channel(key) => {
            if let Some(chan) = st.channels.get_mut(&key) {
                chan.buffer.push_system(status);
            }
        }
        BufferId::Query(key) => {
            if let Some(buf) = st.query_buffers.get_mut(&key) {
                buf.push_system(status);
            }
        }
        BufferId::Empty(id) => {
            if let Some(buf) = st.empty_windows.get_mut(&id) {
                buf.push_system(status);
            }
        }
    }
}

async fn push_status_warning(servers: &Arc<Mutex<Servers>>, tag: &str, status: String) {
    let mut guard = servers.lock().await;
    let Some(st) = guard.state_mut(tag) else { return };
    let active = st.active_buffer.clone();
    match active {
        BufferId::Status => st.status.push_system_warning(status),
        BufferId::Channel(key) => {
            if let Some(chan) = st.channels.get_mut(&key) {
                chan.buffer.push_system_warning(status);
            }
        }
        BufferId::Query(key) => {
            if let Some(buf) = st.query_buffers.get_mut(&key) {
                buf.push_system_warning(status);
            }
        }
        BufferId::Empty(id) => {
            if let Some(buf) = st.empty_windows.get_mut(&id) {
                buf.push_system_warning(status);
            }
        }
    }
}

/// `/ignore -time`'s expiry sweep, factored out of the tick handler so
/// it's directly testable without spinning up the whole `run_server`
/// select-loop machinery. Removes every entry whose `expires_at` has
/// passed; returns whether anything was actually removed, so the
/// caller only pays for a `save_ignores` write when something changed.
fn sweep_expired_ignores(list: &mut Vec<irust_config::IgnoreEntry>, now: i64) -> bool {
    let before = list.len();
    list.retain(|e| e.expires_at.map(|exp| exp > now).unwrap_or(true));
    list.len() != before
}

fn plugins_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "irust").map(|dirs| dirs.config_dir().join("plugins"))
}

fn snapshot_layouts(servers: &Servers) -> Vec<(String, irust_config::NetworkLayout)> {
    servers
        .tags()
        .iter()
        .filter_map(|net_tag| {
            let st = servers.state(net_tag)?;
            let channels = st
                .channels
                .values()
                .filter(|c| c.joined)
                .map(|c| irust_config::LayoutChannel { name: c.display_name.clone(), key: c.key.clone() })
                .collect();
            let queries = st.query_buffers.values().map(|b| b.shown_name().to_string()).collect();
            Some((net_tag.clone(), irust_config::NetworkLayout { channels, queries }))
        })
        .collect()
}

async fn restart_with_session_preserved(
    servers: &Arc<Mutex<Servers>>,
    server_channels: &ServerChannels,
    restart_tx: &mpsc::UnboundedSender<()>,
    exe: Option<PathBuf>,
    reason: String,
) {
    let restart_networks: Vec<irust_config::restart_state::RestartNetwork> = {
        let guard = servers.lock().await;
        let layout_snapshot = snapshot_layouts(&guard);
        for (net_tag, layout) in layout_snapshot {
            let _ = irust_config::save_layout(&net_tag, layout);
        }
        guard
            .tags()
            .iter()
            .filter_map(|net_tag| {
                let st = guard.state(net_tag)?;
                st.connected
                    .then(|| irust_config::restart_state::RestartNetwork { tag: net_tag.clone(), nick: Some(st.nick.clone()) })
            })
            .collect()
    };
    let _ = irust_config::restart_state::save_restart_state(restart_networks);
    RESTART_AFTER_QUIT.store(true, Ordering::SeqCst);
    *RESTART_EXE.lock().unwrap() = exe;
    let _ = restart_tx.send(());
    let chans = server_channels.lock().await;
    for tx in chans.values() {
        let _ = tx.send(InputAction::Quit { reason: Some(reason.clone()) });
    }
}

async fn extra_protect_nicks(
    servers: &Arc<Mutex<Servers>>,
    tag: &str,
    echo_buffer: &BufferId,
    target: &str,
) -> Vec<String> {
    match echo_buffer {
        BufferId::Channel(key) => {
            let guard = servers.lock().await;
            guard
                .state(tag)
                .and_then(|st| st.channels.get(key))
                .map(|c| c.nicklist.sorted().into_iter().map(|e| e.nick).collect())
                .unwrap_or_default()
        }
        BufferId::Status | BufferId::Empty(_) => Vec::new(),
        BufferId::Query(_) => vec![target.to_string()],
    }
}

const WIRE_LINE_SPLIT_LIMIT: usize = 400;

fn split_for_wire(text: &str) -> Vec<String> {
    irust_core::split::split_for_wire(text, WIRE_LINE_SPLIT_LIMIT)
}

struct SendJob {
    echo_buffer: BufferId,
    target: String,
    text: String,
    extra_nicks: Vec<String>,
    is_action: bool,
    raw: bool,
}

fn queue_send(
    send_tx: &mpsc::UnboundedSender<SendJob>,
    echo_buffer: BufferId,
    target: String,
    text: String,
    extra_nicks: Vec<String>,
    is_action: bool,
) {
    let _ = send_tx.send(SendJob {
        echo_buffer,
        target,
        text,
        extra_nicks,
        is_action,
        raw: false,
    });
}

/// Gap between two `irust.say` lines from one plugin command -- a
/// multi-line burst (e.g. block-letter text) sent all at once can get
/// the client flood-killed by the server.
const PLUGIN_SAY_GAP: Duration = Duration::from_millis(500);

/// Sends the lines a plugin command queued with `irust.say` to the
/// active window, one every `PLUGIN_SAY_GAP`, through the same ordered
/// pipeline as typed text (so they're echoed locally too) but `raw`.
fn queue_plugin_says(
    send_tx: &mpsc::UnboundedSender<SendJob>,
    echo_buffer: BufferId,
    target: String,
    lines: Vec<String>,
) {
    let send_tx = send_tx.clone();
    tokio::spawn(async move {
        for (i, text) in lines.into_iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(PLUGIN_SAY_GAP).await;
            }
            let job = SendJob {
                echo_buffer: echo_buffer.clone(),
                target: target.clone(),
                text,
                extra_nicks: Vec::new(),
                is_action: false,
                raw: true,
            };
            if send_tx.send(job).is_err() {
                break;
            }
        }
    });
}

struct ProcessedJob {
    echo_buffer: BufferId,
    target: String,
    chunks: Vec<String>,
    is_action: bool,
}

fn spawn_notify_hook(plugin_host: &Arc<PluginHost>, f: impl FnOnce(&PluginHost) + Send + 'static) {
    let plugin_host = plugin_host.clone();
    tokio::spawn(async move {
        let _ = tokio::time::timeout(
            PLUGIN_HOOK_TIMEOUT,
            tokio::task::spawn_blocking(move || f(&plugin_host)),
        )
        .await;
    });
}

async fn bind_dcc_listener() -> std::io::Result<tokio::net::TcpListener> {
    let range = irust_config::settings::dcc_port_range();
    if let Some((start, end)) = range
        .split_once('-')
        .and_then(|(a, b)| Some((a.parse::<u16>().ok()?, b.parse::<u16>().ok()?)))
    {
        for port in start..=end {
            if let Ok(listener) = tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                return Ok(listener);
            }
        }
    }
    tokio::net::TcpListener::bind("0.0.0.0:0").await
}

fn dcc_announce_ip(conn: &Connection) -> Option<std::net::Ipv4Addr> {
    let override_ip = irust_config::settings::dcc_own_ip();
    if !override_ip.is_empty() {
        if let Ok(ip) = override_ip.parse() {
            return Some(ip);
        }
    }
    conn.local_addr().and_then(|a| match a.ip() {
        std::net::IpAddr::V4(v4) => Some(v4),
        std::net::IpAddr::V6(_) => None,
    })
}

async fn process_job(plugin_host: Arc<PluginHost>, tag: String, job: SendJob) -> ProcessedJob {
    let SendJob {
        echo_buffer,
        target,
        text,
        extra_nicks,
        is_action,
        raw,
    } = job;
    let final_text = if raw {
        text
    } else {
        let target2 = target.clone();
        let text2 = text.clone();
        let result = tokio::time::timeout(
            PLUGIN_HOOK_TIMEOUT,
            tokio::task::spawn_blocking(move || plugin_host.on_send_for(&tag, &target2, &text2, &extra_nicks)),
        )
        .await;
        match result {
            Ok(Ok(PluginHookResult::Replaced(t))) => t,
            _ => text,
        }
    };
    let chunks = split_for_wire(&final_text);
    ProcessedJob {
        echo_buffer,
        target,
        chunks,
        is_action,
    }
}

async fn commit_job(
    wire_tx: &mpsc::UnboundedSender<String>,
    servers: &Arc<Mutex<Servers>>,
    redraw_tx: &mpsc::UnboundedSender<()>,
    tag: &str,
    job: ProcessedJob,
) {
    let ProcessedJob {
        echo_buffer,
        target,
        chunks,
        is_action,
    } = job;
    {
        let mut guard = servers.lock().await;
        if let Some(st) = guard.state_mut(tag) {
            let sender_nick = st.nick.clone();
            let color_index = st.nick_colors.resolve(&sender_nick);
            let echo_active = st.echo_message_active;
            match &echo_buffer {
                BufferId::Status | BufferId::Empty(_) => {}
                BufferId::Channel(key) => {
                    // `/k9 ...` / `/cw ...` typed in a channel: `target` is the service nick, the echo belongs to the
                    // channel we are looking at. Only a send TO a channel may (re)create that channel's entry; creating
                    // one named after the service made a fake "k9" window appear on every such command.
                    let ensured_key = if target.eq_ignore_ascii_case(key) {
                        st.ensure_channel(&target)
                    } else {
                        key.clone()
                    };
                    if &ensured_key != key {
                        irust_config::debug_log::line(
                            "BUG",
                            tag,
                            &format!("echo_buffer key {key:?} != ensure_channel({target:?}) = {ensured_key:?}"),
                        );
                    }
                    if !echo_active {
                        if let Some(chan) = st.channels.get_mut(&ensured_key) {
                            let rank = chan.nicklist.prefix_for(&sender_nick);
                            for chunk in &chunks {
                                if is_action {
                                    chan.buffer.push_action(
                                        sender_nick.clone(),
                                        color_index,
                                        rank,
                                        chunk.clone(),
                                        false,
                                    );
                                } else {
                                    chan.buffer
                                        .push_chat(sender_nick.clone(), color_index, rank, chunk.clone(), false);
                                }
                            }
                        }
                    }
                }
                BufferId::Query(key) => {
                    st.last_sent_msg = Some(target.clone());
                    let is_svc = irust_core::service_notices::is_service_nick(&target);
                    let already_exists = st.query_buffers.contains_key(key);
                    irust_config::debug_log::line(
                        "SVC-OUT",
                        &target,
                        &format!("is_service={is_svc} already_exists={already_exists} key={key:?}"),
                    );
                    let buf = if is_svc {
                        if already_exists {
                            st.get_or_create_query_buffer(key, &target)
                        } else {
                            st.current_buffer_mut()
                        }
                    } else if already_exists || irust_config::lookandfeel::get_bool("autocreate_own_query") {
                        st.get_or_create_query_buffer(key, &target)
                    } else {
                        st.current_buffer_mut()
                    };
                    // No channel roster for a query -- rank prefixes
                    // aren't a thing outside a channel context.
                    if !echo_active {
                        for chunk in &chunks {
                            if is_action {
                                buf.push_action(sender_nick.clone(), color_index, None, chunk.clone(), false);
                            } else {
                                buf.push_chat(sender_nick.clone(), color_index, None, chunk.clone(), false);
                            }
                        }
                    }
                }
            }
        }
    }
    let _ = redraw_tx.send(());

    for chunk in &chunks {
        let line = if is_action {
            format!("PRIVMSG {target} :\u{1}ACTION {chunk}\u{1}")
        } else {
            format!("PRIVMSG {target} :{chunk}")
        };
        let _ = wire_tx.send(line);
    }
}

fn bouncer_login_from_password(server_password: Option<&str>) -> Option<String> {
    server_password
        .and_then(|p| p.split_once(':'))
        .map(|(user, _)| user.to_string())
}

async fn connect_and_register(params: &ConnParams, tag: &str) -> anyhow::Result<(Connection, CapabilityRegistry)> {
    let client_auth_paths = params
        .client_auth
        .as_ref()
        .map(|(cert, key)| (std::path::Path::new(cert.as_str()), std::path::Path::new(key.as_str())));
    let mut conn = Connection::connect(ConnectOptions {
        host: &params.host,
        port: params.port,
        tls: params.tls,
        verify_tls: params.verify_tls,
        ip_version: params.ip_version,
        client_auth: client_auth_paths,
        tag,
        tls_fingerprint: params.tls_fingerprint.as_deref(),
    })
    .await?;

    if params.starttls {
        conn.write_line("STARTTLS").await?;
        let reply = conn.read_line().await;
        let confirmed = match &reply {
            Some(Ok(line)) => parse(line).is_some_and(|m| m.command == "670"),
            _ => false,
        };
        if !confirmed {
            let why = match reply {
                Some(Ok(line)) => format!("server replied {line:?} instead of 670"),
                Some(Err(e)) => format!("failed reading the server's reply: {e}"),
                None => "connection closed before replying".to_string(),
            };
            anyhow::bail!("STARTTLS failed: {why}");
        }
        conn = conn
            .upgrade_to_tls(
                &params.host,
                params.verify_tls,
                params.tls_fingerprint.as_deref(),
                client_auth_paths,
            )
            .await?;
    }

    for line in irust_net::proxy::registration_prelude(&params.host, params.port) {
        conn.write_line(&line).await?;
    }

    // ZNC-style bouncer login (`PASS user:pass`, no SASL) -- must go out
    // before registration, same as weechat's `password` field.
    if let Some(password) = &params.server_password {
        conn.write_line(&format!("PASS {password}")).await?;
    }

    let mut caps: Vec<Box<dyn Capability>> = vec![
        Box::new(MessageTags),
        Box::new(AwayNotify),
        Box::new(Batch),
        Box::new(EchoMessage),
        Box::new(ServerTime),
        Box::new(ChgHost),
        Box::new(MultiPrefix),
        Box::new(UserhostInNames),
        Box::new(AccountNotify),
        Box::new(AccountTag),
        Box::new(ExtendedJoin),
        Box::new(InviteNotify),
        Box::new(Setname),
        Box::new(DraftBot),
    ];
    if !params.tls && !params.starttls {
        caps.push(Box::new(Starttls::new(params.tls || params.starttls)));
    }
    // EXTERNAL checked first -- a network configured with a client cert
    // has already committed to CertFP, and if `sasl_creds` also happened
    // to be set (shouldn't normally be, see `client_auth`'s own doc
    // comment) EXTERNAL is the one that actually has a cert behind it.
    if params.client_auth.is_some() {
        caps.push(Box::new(SaslExternal::new()));
    } else if let Some((account, password)) = &params.sasl_creds {
        caps.push(Box::new(Sasl::new(account.clone(), password.clone())));
    }
    let mut registry = CapabilityRegistry::new(caps);
    for line in registry.start() {
        conn.write_line(&line).await?;
    }
    conn.write_line(&format!("NICK {}", params.nick)).await?;
    conn.write_line(&format!(
        "USER {} 0 * :{}",
        irust_config::settings::ident(),
        irust_config::settings::realname()
    ))
    .await?;
    if let Some(line) = irust_net::proxy::after_user(&params.host, params.port) {
        conn.write_line(&line).await?;
    }

    Ok((conn, registry))
}

#[allow(clippy::too_many_arguments)]
async fn start_server(
    tag: String,
    params: ConnParams,
    make_active: bool,
    app_config: Arc<Mutex<AppConfig>>,
    servers: Arc<Mutex<Servers>>,
    server_channels: ServerChannels,
    redraw_tx: mpsc::UnboundedSender<()>,
    force_redraw_tx: mpsc::UnboundedSender<()>,
    bell_tx: mpsc::UnboundedSender<()>,
    quit_ack_tx: mpsc::UnboundedSender<String>,
    plugin_host: Arc<PluginHost>,
    color_overrides: HashMap<String, usize>,
    ignores: Arc<Mutex<Vec<irust_config::IgnoreEntry>>>,
    hilights: Arc<Mutex<Vec<irust_config::HilightEntry>>>,
    autobans: Arc<Mutex<Vec<irust_config::AutobanEntry>>>,
    quit_messages: Arc<Mutex<Vec<String>>>,
    dcc_sessions: dcc::DccSessions,
    dcc_transfers: dcc_transfer::DccTransfers,
    dcc_resume_waiters: dcc_transfer::DccResumeWaiters,
    extra_channels: Vec<(String, Option<String>)>,
    seed_banner: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let (conn, registry) = connect_and_register(&params, &tag).await?;

    let mut state = match &params.channel {
        Some(channel) => ClientState::with_color_overrides(
            params.nick.clone(),
            channel.clone(),
            irust_tui::color::len(),
            color_overrides,
        ),
        None => ClientState::without_initial_channel(params.nick.clone(), irust_tui::color::len(), color_overrides),
    };
    if seed_banner.swap(false, Ordering::SeqCst) {
        for line in irust_tui::app::banner_lines() {
            state.status.push_system(line);
        }
    }
    for (name, key) in extra_channels {
        if let Some(key) = key {
            state.record_join_key(&name, key);
        }
        state.ensure_channel(&name);
    }
    state.bouncer_login = bouncer_login_from_password(params.server_password.as_deref());
    if let Some(dirs) = directories::ProjectDirs::from("", "", "irust") {
        // Per-tag subdirectory: two different networks joining a
        // same-named channel would otherwise interleave into one log
        // file.
        state.enable_logging(dirs.data_dir().join("logs").join(&tag));
    }

    servers.lock().await.add(tag.clone(), state, make_active);

    let (action_tx, action_rx) = mpsc::unbounded_channel::<InputAction>();
    server_channels.lock().await.insert(tag.clone(), action_tx.clone());

    tokio::spawn(run_server(
        tag,
        conn,
        registry,
        params,
        app_config,
        servers,
        action_rx,
        action_tx,
        redraw_tx,
        force_redraw_tx,
        bell_tx,
        quit_ack_tx,
        plugin_host,
        ignores,
        hilights,
        autobans,
        quit_messages,
        dcc_sessions,
        dcc_transfers,
        dcc_resume_waiters,
    ));
    Ok(())
}

struct DccGetContext<'a> {
    servers: &'a Arc<Mutex<Servers>>,
    conn: &'a mut Connection,
    redraw_tx: &'a mpsc::UnboundedSender<()>,
    dcc_transfers: &'a dcc_transfer::DccTransfers,
    dcc_resume_waiters: &'a dcc_transfer::DccResumeWaiters,
}

async fn dcc_get_or_resume(id: u64, is_auto: bool, tag: &str, ctx: DccGetContext<'_>) {
    let DccGetContext {
        servers,
        conn,
        redraw_tx,
        dcc_transfers,
        dcc_resume_waiters,
    } = ctx;
    let offer = {
        let mut guard = servers.lock().await;
        guard.state_mut(tag).and_then(|st| {
            let is_incoming = st.pending_dcc_send.get(&id).is_some_and(|o| o.incoming);
            if is_incoming {
                st.pending_dcc_send.remove(&id)
            } else {
                None
            }
        })
    };
    let Some(offer) = offer else {
        push_status_warning(servers, tag, format!("-- no incoming DCC SEND offer with id {id}")).await;
        return;
    };
    let Some(safe_name) = irust_core::dcc_transfer::sanitize_filename(&offer.filename) else {
        push_status_warning(
            servers,
            tag,
            format!("-- {}: unsafe filename, refusing", offer.filename),
        )
        .await;
        return;
    };
    let downloads_dir = PathBuf::from(irust_config::settings::dcc_downloads_dir());
    let dest_path = downloads_dir.join(&safe_name);
    if is_auto {
        if tokio::fs::metadata(&dest_path).await.is_ok() && !irust_config::settings::dcc_autoresume() {
            push_status(
                                        servers,
                                        tag,
                                        format!(
                                            "-- not auto-accepting {} from {}: a local file already exists (dcc_autoresume is off) -- /dcc get {id} to accept manually",
                                            offer.filename, offer.peer_nick
                                        ),
                                    )
                                    .await;
            let mut guard = servers.lock().await;
            if let Some(st) = guard.state_mut(tag) {
                st.pending_dcc_send.insert(id, offer);
            }
            return;
        }
    }
    let resume_from = tokio::fs::metadata(&dest_path)
        .await
        .ok()
        .map(|m| m.len())
        .filter(|&len| len > 0 && len < offer.size)
        .unwrap_or(0);
    // `dcc_autorename`: not resuming and the file is already there -> name.1, name.2 ...
    let dest_path = if resume_from == 0 { irust_core::dcc_transfer::autorename(&dest_path) } else { dest_path };

    let size = offer.size;
    let peer_nick = offer.peer_nick.clone();
    let tag2 = tag.to_string();
    let servers2 = servers.clone();
    let redraw_tx2 = redraw_tx.clone();
    let dcc_transfers2 = dcc_transfers.clone();

    if offer.port == 0 {
        let Some(token) = offer.token.clone() else { return };
        let mut resume_rx_opt = None;
        if resume_from > 0 {
            let (resume_tx, resume_rx) = tokio::sync::oneshot::channel::<u64>();
            dcc_resume_waiters
                .lock()
                .await
                .insert(dcc_transfer::WaitKey::Token(token.clone()), resume_tx);
            let resume_line = irust_core::dcc_transfer::passive_resume_ctcp(&offer.filename, resume_from, &token);
            let _ = conn.write_line(&format!("PRIVMSG {peer_nick} :{resume_line}")).await;
            resume_rx_opt = Some(resume_rx);
        }
        let listener = match bind_dcc_listener().await {
            Ok(l) => l,
            Err(err) => {
                push_status_warning(servers, tag, format!("-- couldn't open a DCC listener: {err}")).await;
                return;
            }
        };
        let Some(ip) = dcc_announce_ip(conn) else {
            push_status_warning(servers, tag, "-- can't determine a DCC-usable IPv4 address".to_string()).await;
            return;
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let reverse_offer = irust_core::dcc_transfer::reverse_offer_ctcp(&offer.filename, ip, port, size, &token);
        let _ = conn.write_line(&format!("PRIVMSG {peer_nick} :{reverse_offer}")).await;
        push_status(
            servers,
            tag,
            format!(
                "-- receiving {} from {peer_nick}, waiting for their connection...",
                offer.filename
            ),
        )
        .await;

        let dcc_resume_waiters2 = dcc_resume_waiters.clone();
        let filename2 = safe_name.clone();
        tokio::spawn(async move {
            let resolved_resume_from = if let Some(rx) = resume_rx_opt {
                let result = tokio::time::timeout(Duration::from_secs(15), rx).await;
                dcc_resume_waiters2
                    .lock()
                    .await
                    .remove(&dcc_transfer::WaitKey::Token(token));
                match result {
                    Ok(Ok(position)) => position,
                    _ => 0,
                }
            } else {
                0
            };
            let accept = tokio::time::timeout(irust_core::dcc_transfer::offer_timeout(), listener.accept()).await;
            match accept {
                Ok(Ok((stream, _))) => {
                    dcc_transfer::run_get_session(
                        stream,
                        dcc_transfer::TransferParams {
                            tag: tag2,
                            peer_nick,
                            id,
                            filename: filename2,
                            path: dest_path,
                            size,
                            resume_from: resolved_resume_from,
                            servers: servers2,
                            redraw_tx: redraw_tx2,
                            transfers: dcc_transfers2,
                        },
                    )
                    .await;
                }
                _ => {
                    push_status_warning(
                        &servers2,
                        &tag2,
                        format!("-- {peer_nick} never connected for the DCC GET"),
                    )
                    .await;
                }
            }
        });
    } else {
        // Active offer: connect directly,
        // negotiating resume first if needed.
        let offer_ip = offer.ip.clone();
        let offer_port = offer.port;
        let mut resume_rx_opt = None;
        if resume_from > 0 {
            let (resume_tx, resume_rx) = tokio::sync::oneshot::channel::<u64>();
            dcc_resume_waiters
                .lock()
                .await
                .insert(dcc_transfer::WaitKey::Port(offer_port), resume_tx);
            let resume_line = irust_core::dcc_transfer::resume_ctcp(&offer.filename, offer_port, resume_from);
            let _ = conn.write_line(&format!("PRIVMSG {peer_nick} :{resume_line}")).await;
            resume_rx_opt = Some(resume_rx);
        }
        push_status(
            servers,
            tag,
            format!("-- connecting to {peer_nick} for {}...", offer.filename),
        )
        .await;

        let dcc_resume_waiters2 = dcc_resume_waiters.clone();
        let filename2 = safe_name.clone();
        tokio::spawn(async move {
            let resolved_resume_from = if let Some(rx) = resume_rx_opt {
                let result = tokio::time::timeout(Duration::from_secs(15), rx).await;
                dcc_resume_waiters2
                    .lock()
                    .await
                    .remove(&dcc_transfer::WaitKey::Port(offer_port));
                match result {
                    Ok(Ok(position)) => position,
                    _ => 0,
                }
            } else {
                0
            };
            let connected = tokio::time::timeout(
                Duration::from_secs(30),
                tokio::net::TcpStream::connect((offer_ip.as_str(), offer_port)),
            )
            .await;
            match connected {
                Ok(Ok(stream)) => {
                    dcc_transfer::run_get_session(
                        stream,
                        dcc_transfer::TransferParams {
                            tag: tag2,
                            peer_nick,
                            id,
                            filename: filename2,
                            path: dest_path,
                            size,
                            resume_from: resolved_resume_from,
                            servers: servers2,
                            redraw_tx: redraw_tx2,
                            transfers: dcc_transfers2,
                        },
                    )
                    .await;
                }
                Ok(Err(err)) => {
                    push_status_warning(&servers2, &tag2, format!("-- couldn't connect to {peer_nick}: {err}")).await;
                }
                Err(_elapsed) => {
                    push_status_warning(&servers2, &tag2, format!("-- connecting to {peer_nick} timed out")).await;
                }
            }
        });
    }
}

fn split_server_tag(text: &str, tags: &[String]) -> (Option<String>, String) {
    if !text.starts_with('/') {
        return (None, text.to_string());
    }
    let mut parts = text.splitn(3, ' ');
    let (Some(cmd), Some(second)) = (parts.next(), parts.next()) else {
        return (None, text.to_string());
    };
    let Some(name) = second.strip_prefix('-') else {
        return (None, text.to_string());
    };
    match tags.iter().find(|t| t.eq_ignore_ascii_case(name)) {
        Some(tag) => {
            let rest = parts.next().unwrap_or("");
            let joined = if rest.is_empty() { cmd.to_string() } else { format!("{cmd} {rest}") };
            (Some(tag.clone()), joined)
        }
        None => (None, text.to_string()),
    }
}

#[cfg(test)]
mod server_tag_tests {
    use super::split_server_tag;

    fn tags() -> Vec<String> {
        vec!["chatnet".into(), "libera".into()]
    }

    #[test]
    fn strips_a_known_tag() {
        assert_eq!(
            split_server_tag("/join -libera #rust", &tags()),
            (Some("libera".to_string()), "/join #rust".to_string())
        );
        assert_eq!(
            split_server_tag("/msg -LIBERA nick hi there", &tags()),
            (Some("libera".to_string()), "/msg nick hi there".to_string())
        );
    }

    #[test]
    fn leaves_flags_and_plain_text_alone() {
        assert_eq!(split_server_tag("/connect -tls x", &tags()), (None, "/connect -tls x".to_string()));
        assert_eq!(split_server_tag("hello -libera", &tags()), (None, "hello -libera".to_string()));
    }
}


#[allow(clippy::too_many_arguments)]
async fn run_server(
    tag: String,
    mut conn: Connection,
    mut registry: CapabilityRegistry,
    params: ConnParams,
    app_config: Arc<Mutex<AppConfig>>,
    servers: Arc<Mutex<Servers>>,
    mut action_rx: mpsc::UnboundedReceiver<InputAction>,
    // A clone of the sender side of `action_rx` itself -- lets a
    // handler (namely `/knockout`) schedule a delayed follow-up action
    // back into this same server's own queue (its automatic unban)
    // without needing to touch `conn` from a detached task.
    action_tx: mpsc::UnboundedSender<InputAction>,
    redraw_tx: mpsc::UnboundedSender<()>,
    force_redraw_tx: mpsc::UnboundedSender<()>,
    bell_tx: mpsc::UnboundedSender<()>,
    quit_ack_tx: mpsc::UnboundedSender<String>,
    plugin_host: Arc<PluginHost>,
    ignores: Arc<Mutex<Vec<irust_config::IgnoreEntry>>>,
    hilights: Arc<Mutex<Vec<irust_config::HilightEntry>>>,
    autobans: Arc<Mutex<Vec<irust_config::AutobanEntry>>>,
    quit_messages: Arc<Mutex<Vec<String>>>,
    dcc_sessions: dcc::DccSessions,
    dcc_transfers: dcc_transfer::DccTransfers,
    dcc_resume_waiters: dcc_transfer::DccResumeWaiters,
) {
    let mut lag_interval = tokio::time::interval(lag_check_period());
    // Keeps the statusbar clock visibly ticking even when nothing else
    // is happening -- redraws are otherwise event-driven, so without
    // this the displayed time would only update on activity.
    let mut clock_interval = tokio::time::interval(Duration::from_secs(1));
    // `channels_rejoin_unavailable`: retry channels a 437 refused (irssi: every 5 minutes).
    let mut rejoin_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + irust_core::rejoin::REJOIN_INTERVAL,
        irust_core::rejoin::REJOIN_INTERVAL,
    );
    let mut last_day_seen = irust_core::today();
    // When our own lag-measuring PING went out, so the matching PONG can
    // compute the round trip -- cleared once matched, so a PONG that
    // doesn't match (e.g. the server's own unrelated PING/PONG
    // handshake) is ignored rather than mismeasured.
    let mut lag_ping_sent: Option<Instant> = None;
    // Consecutive lag PINGs sent with no PONG back yet -- see
    // `lag_pong_miss_limit`'s own doc comment.
    let mut lag_pongs_missed: u32 = 0;

    let mut notify_list: Vec<String> = Vec::new();
    let mut notify_online: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut notify_check_pending = false;
    let mut notify_interval = tokio::time::interval(NOTIFY_CHECK_INTERVAL);
    let mut monitor_synced_this_connection = false;

    // Lines a background task (a plugin hook that had to make a
    // blocking network call of its own) hands back to actually go out
    // on the wire --
    // see `spawn_on_send`. Kept alive across reconnects (`conn` itself
    // gets replaced, this channel doesn't need to be).
    let (wire_tx, mut wire_rx) = mpsc::unbounded_channel::<String>();

    let (send_tx, mut send_rx) = mpsc::unbounded_channel::<SendJob>();
    {
        let plugin_host = plugin_host.clone();
        let wire_tx = wire_tx.clone();
        let servers = servers.clone();
        let redraw_tx = redraw_tx.clone();
        let tag = tag.clone();
        tokio::spawn(async move {
            let (result_tx, mut result_rx) = mpsc::unbounded_channel::<tokio::task::JoinHandle<ProcessedJob>>();
            {
                let plugin_host = plugin_host.clone();
                let tag = tag.clone();
                tokio::spawn(async move {
                    while let Some(job) = send_rx.recv().await {
                        let handle = tokio::spawn(process_job(plugin_host.clone(), tag.clone(), job));
                        if result_tx.send(handle).is_err() {
                            break;
                        }
                    }
                });
            }
            while let Some(handle) = result_rx.recv().await {
                if let Ok(done) = handle.await {
                    commit_job(&wire_tx, &servers, &redraw_tx, &tag, done).await;
                }
            }
        });
    }

    // Bouncer playback filter (see `replay_watermark`): when this network was last alive.
    let mut replay_cutoff = replay_watermark::begin(&tag);

    'reconnect: loop {
        'session: loop {
            tokio::select! {
                Some(line) = wire_rx.recv() => {
                    let _ = conn.write_line(&line).await;
                }
                line = conn.read_line() => {
                    match line {
                        Some(Ok(raw)) => {
                            if let Some(msg) = parse(&raw) {
                                let mut starttls_requested = false;
                                for reply in registry.handle_message(&msg) {
                                    if reply == "STARTTLS" {
                                        starttls_requested = true;
                                        continue;
                                    }
                                    let _ = conn.write_line(&reply).await;
                                }
                                if starttls_requested {
                                    let _ = conn.write_line("STARTTLS").await;
                                    let confirmed = match conn.read_line().await {
                                        Some(Ok(line)) => parse(&line).is_some_and(|m| m.command == "670"),
                                        _ => false,
                                    };
                                    if !confirmed {
                                        irust_config::debug_log::line(
                                            "TLS",
                                            &tag,
                                            "opportunistic STARTTLS: server didn't confirm with 670 -- dropping the connection rather than continue in an uncertain state",
                                        );
                                        break 'session;
                                    }
                                    let client_auth_paths = params
                                        .client_auth
                                        .as_ref()
                                        .map(|(cert, key)| (std::path::Path::new(cert.as_str()), std::path::Path::new(key.as_str())));
                                    match conn
                                        .upgrade_to_tls(&params.host, params.verify_tls, params.tls_fingerprint.as_deref(), client_auth_paths)
                                        .await
                                    {
                                        Ok(upgraded) => {
                                            conn = upgraded;
                                            if irust_config::settings_net::bool_of("starttls_sts") {
                                                let mut cfg = app_config.lock().await;
                                                cfg.networks.entry(tag.clone()).or_default().starttls = Some(true);
                                                let _ = irust_config::save_config(&cfg);
                                            }
                                        }
                                        Err(e) => {
                                            irust_config::debug_log::line(
                                                "TLS",
                                                &tag,
                                                &format!("opportunistic STARTTLS handshake failed: {e:#}"),
                                            );
                                            break 'session;
                                        }
                                    }
                                }
                                if msg.command == "CAP" {
                                    let echo_active = registry
                                        .active_caps()
                                        .iter()
                                        .any(|c| c == "echo-message");
                                    let setname_active =
                                        registry.active_caps().iter().any(|c| c == "setname");
                                    if let Some(st) = servers.lock().await.state_mut(&tag) {
                                        st.echo_message_active = echo_active;
                                        st.setname_active = setname_active;
                                    }
                                }

                                // A bouncer replays the last private lines of every query on
                                // each attach; drop the ones already seen, keep what arrived
                                // while away. (`touch` moves the watermark on, `replay_cutoff`
                                // stays the value this connection started with.)
                                replay_watermark::touch(&tag);
                                if replay_watermark::is_stale_private(replay_cutoff, &msg) {
                                    continue;
                                }

                                if msg.command == "PONG"
                                    && msg.params.last().map(String::as_str) == Some(LAG_PING_TOKEN)
                                {
                                    if let Some(sent) = lag_ping_sent.take() {
                                        let ms = sent.elapsed().as_millis() as u64;
                                        if let Some(st) = servers.lock().await.state_mut(&tag) {
                                            st.lag_ms = Some(ms);
                                        }
                                        let _ = redraw_tx.send(());
                                    }
                                    lag_pongs_missed = 0;
                                }

                                if msg.command == "303" && notify_check_pending {
                                    notify_check_pending = false;
                                    let online_now: std::collections::HashSet<String> = msg
                                        .params
                                        .last()
                                        .map(|s| s.split_whitespace().map(|n| n.to_lowercase()).collect())
                                        .unwrap_or_default();
                                    let mut newly_online = Vec::new();
                                    let mut newly_offline = Vec::new();
                                    for nick in &notify_list {
                                        let now_online = online_now.contains(nick);
                                        let was_online = notify_online.contains(nick);
                                        if now_online && !was_online {
                                            newly_online.push(nick.clone());
                                        } else if !now_online && was_online {
                                            newly_offline.push(nick.clone());
                                        }
                                    }
                                    if !newly_online.is_empty() || !newly_offline.is_empty() {
                                        if let Some(st) = servers.lock().await.state_mut(&tag) {
                                            for nick in &newly_online {
                                                st.status.push_system(format!("-- {nick} is online"));
                                            }
                                            for nick in &newly_offline {
                                                st.status.push_system(format!("-- {nick} went offline"));
                                            }
                                        }
                                        let _ = redraw_tx.send(());
                                    }
                                    notify_online =
                                        notify_list.iter().filter(|n| online_now.contains(*n)).cloned().collect();
                                    continue;
                                }

                                if msg.command == "730" || msg.command == "731" {
                                    // `730 RPL_MONONLINE`: `<target>[!<user>@<host>]`
                                    // per entry -- strip any hostmask, only the nick
                                    // is tracked (same granularity `notify_online`
                                    // already used under `ISON`). `731
                                    // RPL_MONOFFLINE` entries are bare nicks already.
                                    if let Some(last) = msg.params.last() {
                                        let mut changed = Vec::new();
                                        for entry in last.split(',') {
                                            let nick = entry.split('!').next().unwrap_or(entry).trim();
                                            if nick.is_empty() {
                                                continue;
                                            }
                                            let key = nick.to_lowercase();
                                            let went_online = msg.command == "730";
                                            let state_changed = if went_online {
                                                notify_online.insert(key)
                                            } else {
                                                notify_online.remove(&key)
                                            };
                                            if state_changed {
                                                changed.push((nick.to_string(), went_online));
                                            }
                                        }
                                        if !changed.is_empty() {
                                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                                for (nick, went_online) in &changed {
                                                    let verb =
                                                        if *went_online { "is online" } else { "went offline" };
                                                    st.status.push_system(format!("-- {nick} {verb}"));
                                                }
                                            }
                                            let _ = redraw_tx.send(());
                                        }
                                    }
                                    continue;
                                }
                                if msg.command == "734" {
                                    if let Some(st) = servers.lock().await.state_mut(&tag) {
                                        st.status.push_system(
                                            "-- /notify: the server's MONITOR list is full, some nicks may not be watched"
                                                .to_string(),
                                        );
                                    }
                                    let _ = redraw_tx.send(());
                                    continue;
                                }
                                if msg.command == "732" || msg.command == "733" {
                                    continue;
                                }

                                let is_ctcp = msg
                                    .params
                                    .get(1)
                                    .is_some_and(|t| t.starts_with('\u{1}'));
                                let hook_source = if msg.command == "PRIVMSG" && !is_ctcp {
                                    match (msg.prefix.clone(), msg.params.first().cloned(), msg.params.get(1).cloned()) {
                                        (Some(prefix), Some(target), Some(text)) => {
                                            let is_self = servers
                                                .lock()
                                                .await
                                                .state(&tag)
                                                .is_some_and(|st| st.is_self(&prefix.name));
                                            if is_self { None } else { Some((prefix, target, text)) }
                                        }
                                        _ => None,
                                    }
                                } else {
                                    None
                                };

                                if matches!(msg.command.as_str(), "PRIVMSG" | "NOTICE" | "WALLOPS" | "INVITE") {
                                    if let Some(p) = msg.prefix.as_ref() {
                                        let level = ignore_level_for(&msg.command, msg.params.get(1).map(String::as_str));
                                        let list = ignores.lock().await;
                                        let mut ignored = irust_core::is_ignored(&list, p, level);
                                        if !ignored && msg.command == "PRIVMSG" {
                                            if let (Some(target), Some(text)) =
                                                (msg.params.first(), msg.params.get(1))
                                            {
                                                if target.starts_with(['#', '&']) {
                                                    let channel_nicks = servers
                                                        .lock()
                                                        .await
                                                        .state(&tag)
                                                        .and_then(|st| st.channels.get(&target.to_lowercase()))
                                                        .map(|c| {
                                                            c.nicklist
                                                                .sorted()
                                                                .into_iter()
                                                                .map(|e| e.nick)
                                                                .collect::<Vec<String>>()
                                                        })
                                                        .unwrap_or_default();
                                                    ignored = irust_core::is_ignored_reply(
                                                        &list,
                                                        &channel_nicks,
                                                        text,
                                                        level,
                                                    );
                                                }
                                            }
                                        }
                                        drop(list);
                                        if ignored {
                                            continue;
                                        }
                                    }
                                }

                                let hilight_patterns = hilights.lock().await.clone();
                                let ignore_entries = ignores.lock().await.clone();
                                let autoban_entries = autobans.lock().await.clone();
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.state_mut(&tag) {
                                    let (replies, highlighted) = irust_core::handle_message_with_highlight(
                                        st,
                                        &msg,
                                        &hilight_patterns,
                                        &ignore_entries,
                                        &autoban_entries,
                                    );
                                    if msg.params.iter().any(|p| irust_core::contains_risky_emoji_sequence(p)) {
                                        let _ = force_redraw_tx.send(());
                                    }
                                    let ready_ip = st.pending_ip.take_if(|p| p.host.is_some());
                                    let dcc_resumes = std::mem::take(&mut st.pending_dcc_resume_requests);
                                    let dcc_accepts = std::mem::take(&mut st.pending_dcc_accept_replies);
                                    // Offers `client.rs`'s `dcc_autoget_eligible`
                                    // already approved -- same take-and-drain
                                    // idea as the two above, this crate has no
                                    // socket/async capability to actually
                                    // perform the GET/RESUME itself.
                                    let auto_accepts = std::mem::take(&mut st.pending_auto_dcc_accept);
                                    // `join_auto_chans_on_invite`: invited channels `client.rs` queued.
                                    let invite_joins = std::mem::take(&mut st.pending_invite_autojoin);
                                    // `opermode`: queued by `client.rs` on `381`.
                                    let opermode_cmd = std::mem::take(&mut st.pending_opermode).map(|m| (st.nick.clone(), m));
                                    let autobans_triggered = std::mem::take(&mut st.pending_autoban);
                                    drop(guard);
                                    if let Some((ournick, modestring)) = opermode_cmd {
                                        let _ = conn.write_line(&format!("MODE {ournick} {modestring}")).await;
                                    }
                                    for channel in invite_joins {
                                        // irssi only joins invited channels that have an autojoin
                                        // `/channel` entry on this network.
                                        if let Some(setup) = irust_config::load_channel_setup(&tag, &channel) {
                                            if setup.autojoin {
                                                let line = match &setup.password {
                                                    Some(key) => format!("JOIN {channel} {key}"),
                                                    None => format!("JOIN {channel}"),
                                                };
                                                let _ = conn.write_line(&line).await;
                                            }
                                        }
                                    }
                                    for triggered in autobans_triggered {
                                        if let Some(st) = servers.lock().await.state_mut(&tag) {
                                            if let Some(chan) =
                                                st.channels.get_mut(&triggered.channel.to_lowercase())
                                            {
                                                chan.buffer.push_system(format!(
                                                    "-- AUTOBAN: detected spam from {} in {} ({})",
                                                    triggered.nick, triggered.channel, triggered.reason
                                                ));
                                            }
                                        }
                                        let mask = triggered.mask.clone().unwrap_or_else(|| triggered.nick.clone());
                                        let _ = action_tx.send(InputAction::Kickban {
                                            channel: Some(triggered.channel),
                                            nick: triggered.nick,
                                            mask,
                                            reason: Some(triggered.reason),
                                        });
                                    }
                                    for (peer, resume) in dcc_resumes {
                                        let key = match &resume.token {
                                            Some(t) => dcc_transfer::WaitKey::Token(t.clone()),
                                            None => dcc_transfer::WaitKey::Port(resume.port),
                                        };
                                        if let Some(tx) = dcc_resume_waiters.lock().await.remove(&key) {
                                            let _ = tx.send(resume.position);
                                            let accept_line = match &resume.token {
                                                Some(t) => irust_core::dcc_transfer::passive_accept_ctcp(
                                                    &resume.filename,
                                                    resume.position,
                                                    t,
                                                ),
                                                None => irust_core::dcc_transfer::accept_ctcp(
                                                    &resume.filename,
                                                    resume.port,
                                                    resume.position,
                                                ),
                                            };
                                            let _ = conn.write_line(&format!("PRIVMSG {peer} :{accept_line}")).await;
                                        }
                                    }
                                    for (_peer, accept) in dcc_accepts {
                                        let key = match &accept.token {
                                            Some(t) => dcc_transfer::WaitKey::Token(t.clone()),
                                            None => dcc_transfer::WaitKey::Port(accept.port),
                                        };
                                        if let Some(tx) = dcc_resume_waiters.lock().await.remove(&key) {
                                            let _ = tx.send(accept.position);
                                        }
                                    }
                                    for id in auto_accepts {
                                        dcc_get_or_resume(
                                            id,
                                            true,
                                            &tag,
                                            DccGetContext {
                                                servers: &servers,
                                                conn: &mut conn,
                                                redraw_tx: &redraw_tx,
                                                dcc_transfers: &dcc_transfers,
                                                dcc_resume_waiters: &dcc_resume_waiters,
                                            },
                                        )
                                        .await;
                                    }
                                    // `beep_msg_level`/`beep_when_*`: a second reason
                                    // to ring, next to a highlighted message.
                                    let beep_level_hit = servers
                                        .lock()
                                        .await
                                        .state(&tag)
                                        .is_some_and(|st| irust_core::beep::msg_beep_wanted(st, &msg));
                                    if (highlighted || beep_level_hit)
                                        && !irust_core::is_replayed_message(&msg)
                                        && irust_config::settings::BELL_ENABLED
                                            .load(std::sync::atomic::Ordering::Relaxed)
                                    {
                                        let _ = bell_tx.send(());
                                    }
                                    for reply in replies {
                                        let _ = conn.write_line(&reply).await;
                                    }
                                    if let Some(pending) = ready_ip {
                                        let servers = servers.clone();
                                        let redraw_tx = redraw_tx.clone();
                                        let tag = tag.clone();
                                        tokio::spawn(async move {
                                            let host = pending.host.unwrap();
                                            let line = iplookup::lookup(&host)
                                                .await
                                                .unwrap_or_else(|| format!("-- ip lookup for {host} failed"));
                                            let mut guard = servers.lock().await;
                                            if let Some(st) = guard.state_mut(&tag) {
                                                match st.buffer_mut(&pending.echo_buffer) {
                                                    Some(buffer) => {
                                                        buffer.push_system_before_whois_end(line, pending.start_len)
                                                    }
                                                    None => st.status.push_system_before_whois_end(line, pending.start_len),
                                                }
                                            }
                                            drop(guard);
                                            let _ = redraw_tx.send(());
                                        });
                                    }
                                }

                                match msg.command.as_str() {
                                    "JOIN" => {
                                        if let (Some(prefix), Some(channel)) = (msg.prefix.as_ref(), msg.params.first()) {
                                            let (nick, channel) = (prefix.name.clone(), channel.clone());
                                            {
                                                let (nick, channel) = (nick.clone(), channel.clone());
                                                spawn_notify_hook(&plugin_host, move |h| h.on_join(&nick, &channel));
                                            }

                                            let is_self =
                                                servers.lock().await.state(&tag).is_some_and(|st| st.is_self(&nick));
                                            if is_self {
                                                if let Some(entry) = irust_config::load_channel_setup(&tag, &channel) {
                                                    if entry.botmasks.is_empty() && !entry.autosendcmd.is_empty() {
                                                        let _ = conn
                                                            .write_line(&format!(
                                                                "PRIVMSG {channel} :{}",
                                                                entry.autosendcmd
                                                            ))
                                                            .await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    "366" => {
                                        if let Some(channel) = msg.params.get(1) {
                                            if let Some(entry) = irust_config::load_channel_setup(&tag, channel) {
                                                if !entry.botmasks.is_empty() && !entry.autosendcmd.is_empty() {
                                                    let matched = servers.lock().await.state(&tag).and_then(|st| {
                                                        st.channels.get(&channel.to_lowercase()).and_then(|c| {
                                                            c.nicklist.sorted().into_iter().map(|e| e.nick).find(
                                                                |nick| {
                                                                    entry.botmasks.iter().any(|mask| {
                                                                        irust_core::nicklist::matches_glob(mask, nick)
                                                                    })
                                                                },
                                                            )
                                                        })
                                                    });
                                                    if let Some(bot_nick) = matched {
                                                        let _ = conn
                                                            .write_line(&format!(
                                                                "PRIVMSG {bot_nick} :{}",
                                                                entry.autosendcmd
                                                            ))
                                                            .await;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    "PART" => {
                                        if let (Some(prefix), Some(channel)) = (msg.prefix.as_ref(), msg.params.first()) {
                                            let (nick, channel) = (prefix.name.clone(), channel.clone());
                                            let reason = msg.params.get(1).cloned().unwrap_or_default();
                                            spawn_notify_hook(&plugin_host, move |h| h.on_part(&nick, &channel, &reason));
                                        }
                                    }
                                    "QUIT" => {
                                        if let Some(prefix) = msg.prefix.as_ref() {
                                            let nick = prefix.name.clone();
                                            let reason = msg.params.first().cloned().unwrap_or_default();
                                            spawn_notify_hook(&plugin_host, move |h| h.on_quit(&nick, &reason));
                                        }
                                    }
                                    "NICK" => {
                                        if let (Some(prefix), Some(new_nick)) = (msg.prefix.as_ref(), msg.params.first()) {
                                            let (old_nick, new_nick) = (prefix.name.clone(), new_nick.clone());
                                            let is_self = servers
                                                .lock()
                                                .await
                                                .state(&tag)
                                                .is_some_and(|st| st.is_self(&old_nick));
                                            if is_self {
                                                plugin_host.update_nick(&new_nick);
                                            }
                                            spawn_notify_hook(&plugin_host, move |h| h.on_nick(&old_nick, &new_nick));
                                        }
                                    }
                                    "TOPIC" => {
                                        if let (Some(prefix), Some(channel)) = (msg.prefix.as_ref(), msg.params.first()) {
                                            let (nick, channel) = (prefix.name.clone(), channel.clone());
                                            let topic = msg.params.get(1).cloned().unwrap_or_default();
                                            spawn_notify_hook(&plugin_host, move |h| h.on_topic(&nick, &channel, &topic));
                                        }
                                    }
                                    "MODE" => {
                                        if let (Some(prefix), Some(target)) = (msg.prefix.as_ref(), msg.params.first()) {
                                            let (nick, target) = (prefix.name.clone(), target.clone());
                                            let modestring = msg.params.get(1..).map(|p| p.join(" ")).unwrap_or_default();
                                            spawn_notify_hook(&plugin_host, move |h| h.on_mode(&nick, &target, &modestring));
                                        }
                                    }
                                    "001" => {
                                        let network = tag.clone();
                                        spawn_notify_hook(&plugin_host, move |h| h.on_connect(&network));
                                        if let Some(nick) = msg.params.first() {
                                            plugin_host.update_nick(nick);
                                        }
                                    }
                                    "381" => {
                                        let ph = plugin_host.clone();
                                        let result = tokio::time::timeout(
                                            PLUGIN_HOOK_TIMEOUT,
                                            tokio::task::spawn_blocking(move || ph.on_oper()),
                                        )
                                        .await;
                                        if let Ok(Ok(lines)) = result {
                                            for line in lines {
                                                let _ = conn.write_line(&line).await;
                                            }
                                        }
                                    }
                                    _ => {}
                                }

                                if !monitor_synced_this_connection {
                                    let ready = {
                                        let guard = servers.lock().await;
                                        guard
                                            .state(&tag)
                                            .map(|st| st.connected && st.monitor_limit.is_some())
                                            .unwrap_or(false)
                                    };
                                    if ready {
                                        monitor_synced_this_connection = true;
                                        if !notify_list.is_empty() {
                                            let targets = notify_list.join(",");
                                            let _ = conn.write_line(&format!("MONITOR + {targets}")).await;
                                            let _ = conn.write_line("MONITOR S").await;
                                        }
                                    }
                                }

                                if let Some((prefix, target, text)) = hook_source {
                                    let plugin_host = plugin_host.clone();
                                    let servers = servers.clone();
                                    let redraw_tx = redraw_tx.clone();
                                    let tag = tag.clone();
                                    tokio::spawn(async move {
                                        let nick = prefix.name.clone();
                                        let target2 = target.clone();
                                        let text2 = text.clone();
                                        let tag2 = tag.clone();
                                        let result = tokio::time::timeout(
                                            PLUGIN_HOOK_TIMEOUT,
                                            tokio::task::spawn_blocking(move || {
                                                plugin_host.on_message_for(&tag2, true, &nick, &target2, &text2)
                                            }),
                                        )
                                        .await;
                                        if let Ok(Ok(PluginHookResult::Replaced(new_text))) = result {
                                            let mut guard = servers.lock().await;
                                            if let Some(st) = guard.state_mut(&tag) {
                                                let replaced = if target.starts_with(['#', '&']) {
                                                    st.channels.get_mut(&target.to_lowercase()).is_some_and(
                                                        |chan| {
                                                            chan.buffer.replace_last_chat_text(
                                                                &prefix.name,
                                                                &text,
                                                                new_text.clone(),
                                                            )
                                                        },
                                                    )
                                                } else {
                                                    st.query_buffers
                                                        .get_mut(&prefix.name.to_lowercase())
                                                        .is_some_and(|buf| {
                                                            buf.replace_last_chat_text(
                                                                &prefix.name,
                                                                &text,
                                                                new_text.clone(),
                                                            )
                                                        })
                                                };
                                                if !replaced {
                                                    let line = format!(
                                                        "-- {} (replaced): {new_text}",
                                                        prefix.name
                                                    );
                                                    if target.starts_with(['#', '&']) {
                                                        if let Some(chan) =
                                                            st.channels.get_mut(&target.to_lowercase())
                                                        {
                                                            chan.buffer.push_system(line);
                                                        }
                                                    } else if let Some(buf) =
                                                        st.query_buffers.get_mut(&prefix.name.to_lowercase())
                                                    {
                                                        buf.push_system(line);
                                                    }
                                                }
                                            }
                                            drop(guard);
                                            let _ = redraw_tx.send(());
                                        }
                                    });
                                }
                            }
                            irust_config::debug_log::line("RDRW", &tag, &format!("requested after: {raw}"));
                            let _ = redraw_tx.send(());
                        }
                        Some(Err(_)) | None => break 'session,
                    }
                }
                _ = lag_interval.tick() => {
                    // Still alive: move the bouncer-replay watermark on and write it out
                    // (also covers a quiet session that never receives another line).
                    replay_watermark::touch(&tag);
                    replay_watermark::flush();
                    // A ping already in flight with no answer means the
                    // PREVIOUS tick's PING went unanswered for a full
                    // the check period -- one miss.
                    if irust_config::settings_net::time_ms_of("lag_check_time") == 0 {
                        // lag_check_time 0 = lag checking off (irssi).
                        lag_ping_sent = None;
                        lag_pongs_missed = 0;
                    } else {
                    if lag_ping_sent.is_some() {
                        lag_pongs_missed += 1;
                        if lag_pongs_missed >= lag_pong_miss_limit() {
                            irust_config::debug_log::line(
                                "LAG",
                                &tag,
                                &format!("no PONG after {lag_pongs_missed} lag pings -- connection presumed dead, reconnecting"),
                            );
                            break 'session;
                        }
                    }
                    lag_ping_sent = Some(Instant::now());
                    let _ = conn.write_line(&format!("PING :{LAG_PING_TOKEN}")).await;
                    }
                }
                _ = rejoin_interval.tick() => {
                    let line = servers
                        .lock()
                        .await
                        .state_mut(&tag)
                        .and_then(|st| irust_core::rejoin::tick(&mut st.rejoin_channels));
                    if let Some(line) = line {
                        let _ = conn.write_line(&line).await;
                    }
                }
                _ = clock_interval.tick() => {
                    let today = irust_core::today();
                    let mut guard = servers.lock().await;
                    if let Some(st) = guard.state_mut(&tag) {
                        if today != last_day_seen {
                            st.push_daychange_to_all_buffers(today);
                            st.rotate_logs(today);
                        }
                        st.flush_pending_netsplits();
                    }
                    drop(guard);
                    if today != last_day_seen {
                        last_day_seen = today;
                    }
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let mut list = ignores.lock().await;
                    if sweep_expired_ignores(&mut list, now) {
                        let _ = irust_config::save_ignores(&list);
                    }
                    drop(list);
                    let _ = redraw_tx.send(());
                }
                _ = notify_interval.tick() => {
                    // Skipped entirely once `MONITOR` is doing the job --
                    // it's push-based, nothing to poll for.
                    let using_monitor = servers
                        .lock()
                        .await
                        .state(&tag)
                        .is_some_and(|st| st.monitor_limit.is_some());
                    if !using_monitor && !notify_list.is_empty() {
                        notify_check_pending = true;
                        let _ = conn.write_line(&format!("ISON :{}", notify_list.join(" "))).await;
                    }
                }
                Some(action) = action_rx.recv() => {
                    match action {
                        InputAction::SendMessage { target, text, echo_buffer } => {
                            let extra_nicks = extra_protect_nicks(&servers, &tag, &echo_buffer, &target).await;
                            let dcc_key = match &echo_buffer {
                                BufferId::Query(key) if key.starts_with("dcc:") => Some(key.clone()),
                                _ => None,
                            };
                            match dcc_key {
                                Some(key) => {
                                    {
                                        let mut guard = servers.lock().await;
                                        if let Some(st) = guard.state_mut(&tag) {
                                            let sender_nick = st.nick.clone();
                                            let color_index = st.nick_colors.resolve(&sender_nick);
                                            if let BufferId::Query(qkey) = &echo_buffer {
                                                let buf = st.get_or_create_query_buffer(qkey, &target);
                                                buf.push_chat(sender_nick, color_index, None, text.clone(), false);
                                            }
                                        }
                                    }
                                    let sender = dcc_sessions
                                        .lock()
                                        .await
                                        .get(&(tag.clone(), key))
                                        .map(|h| h.write_tx.clone());
                                    match sender {
                                        Some(tx) => {
                                            let _ = tx.send(dcc::DccOutbound::Line(text));
                                        }
                                        None => {
                                            push_status_warning(&servers, &tag, "-- DCC CHAT is not connected".to_string())
                                                .await;
                                        }
                                    }
                                }
                                None => {
                                    if let Some(st) = servers.lock().await.state_mut(&tag) {
                                        st.note_own_message(&target, &text);
                                    }
                                    queue_send(&send_tx, echo_buffer, target, text, extra_nicks, false);
                                }
                            }
                        }
                        InputAction::SetName(new_name) => {
                            let active =
                                servers.lock().await.state(&tag).map(|st| st.setname_active).unwrap_or(false);
                            if active {
                                let _ = conn.write_line(&format!("SETNAME :{new_name}")).await;
                            } else {
                                push_status_warning(&servers, &tag, "-- the server doesn't support setname".to_string())
                                    .await;
                            }
                        }
                        InputAction::Join { channel, key } => {
                            let key = key.or_else(|| {
                                irust_config::load_channel_setup(&tag, &channel).and_then(|e| e.password)
                            });
                            if let Some(key) = &key {
                                if let Some(st) = servers.lock().await.state_mut(&tag) {
                                    st.record_join_key(&channel, key.clone());
                                }
                            }
                            let line = match &key {
                                Some(key) => format!("JOIN {channel} {key}"),
                                None => format!("JOIN {channel}"),
                            };
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::Cycle { channel, message } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let part_line = match &message {
                                    Some(msg) => format!("PART {channel} :{msg}"),
                                    None => format!("PART {channel}"),
                                };
                                let _ = conn.write_line(&part_line).await;
                                let key = servers
                                    .lock()
                                    .await
                                    .state(&tag)
                                    .and_then(|st| st.channel(&channel))
                                    .and_then(|c| c.key.clone());
                                let join_line = match &key {
                                    Some(key) => format!("JOIN {channel} {key}"),
                                    None => format!("JOIN {channel}"),
                                };
                                let _ = conn.write_line(&join_line).await;
                            }
                        }
                        InputAction::ClearView { all } => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                if all {
                                    st.status.clear_view();
                                    for chan in st.channels.values_mut() {
                                        chan.buffer.clear_view();
                                    }
                                    for buf in st.query_buffers.values_mut() {
                                        buf.clear_view();
                                    }
                                } else {
                                    st.current_buffer_mut().clear_view();
                                }
                            }
                        }
                        InputAction::Lastlog { pattern, is_regex, case_sensitive, count } => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let buf = st.current_buffer_mut();
                                let matches = buf.lastlog(&pattern, is_regex, case_sensitive, count);
                                if matches.is_empty() {
                                    buf.push_system(format!("-- lastlog: no matches for {pattern:?}"));
                                } else {
                                    buf.push_system(format!(
                                        "-- lastlog: {} matches for {pattern:?}",
                                        matches.len()
                                    ));
                                    for m in matches {
                                        buf.push_system(format!("-- {m}"));
                                    }
                                }
                            }
                            let _ = redraw_tx.send(());
                        }
                        InputAction::WindowList => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let summaries = st.buffer_summaries();
                                let active = st.active_buffer.clone();
                                let lines: Vec<String> = summaries
                                    .iter()
                                    .enumerate()
                                    .map(|(i, s)| {
                                        let marker = if s.id == active { "*" } else { " " };
                                        format!("-- {marker}{:>2}: {}", i + 1, s.display_name)
                                    })
                                    .collect();
                                let buf = st.current_buffer_mut();
                                for line in lines {
                                    buf.push_system(line);
                                }
                            }
                        }
                        InputAction::Cmds(cmd) => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let buf = st.current_buffer_mut();
                                match cmd {
                                    None => {
                                        for line in cmds_overview_lines() {
                                            buf.push_system(line);
                                        }
                                    }
                                    Some(cmd) => match cmd_detail_for(&cmd) {
                                        Some(lines) => {
                                            for line in lines {
                                                buf.push_system(line);
                                            }
                                        }
                                        None => buf.push_system(format!(
                                            "-- No help for /{cmd} - try /cmds with no arguments for the full command list."
                                        )),
                                    },
                                }
                            }
                            let _ = redraw_tx.send(());
                        }
                        InputAction::WindowName(name) => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let buf = st.current_buffer_mut();
                                buf.custom_name = Some(name.clone());
                                buf.push_system(format!("-- this window is now named {name}"));
                            }
                        }
                        InputAction::WindowNew => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                st.create_empty_window();
                                st.current_buffer_mut().push_system("-- new window".to_string());
                            }
                        }
                        InputAction::WindowImmortal(value) => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let active = st.active_buffer.clone();
                                let currently = st.immortal.contains(&active);
                                let parsed = match value.as_deref() {
                                    None => Some(currently),
                                    Some(v) if v.eq_ignore_ascii_case("on") => Some(true),
                                    Some(v) if v.eq_ignore_ascii_case("off") => Some(false),
                                    Some(v) if v.eq_ignore_ascii_case("toggle") => Some(!currently),
                                    Some(_) => None,
                                };
                                match parsed {
                                    Some(new_value) => {
                                        if new_value {
                                            st.immortal.insert(active);
                                        } else {
                                            st.immortal.remove(&active);
                                        }
                                        let msg = format!(
                                            "-- this window is {}immortal",
                                            if new_value { "" } else { "not " }
                                        );
                                        st.current_buffer_mut().push_system(msg);
                                    }
                                    None => {
                                        st.current_buffer_mut()
                                            .push_system_warning("-- expected on/off/toggle".to_string());
                                    }
                                }
                            }
                        }
                        InputAction::WindowStick => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let active = st.active_buffer.clone();
                                let now_sticky = if st.sticky.contains(&active) {
                                    st.sticky.remove(&active);
                                    false
                                } else {
                                    st.sticky.insert(active);
                                    true
                                };
                                let msg = format!(
                                    "-- this window is {}sticky",
                                    if now_sticky { "" } else { "no longer " }
                                );
                                st.current_buffer_mut().push_system(msg);
                            }
                        }
                        InputAction::WindowItemAdd(name) => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let outcome = resolve_buffer_by_name(st, &name).and_then(|id| {
                                    st.window_item_add(&id).map_err(|e| format!("-- {e}"))
                                });
                                let msg = match outcome {
                                    Ok(()) => format!("-- {name} grouped into this window"),
                                    Err(msg) => msg,
                                };
                                st.current_buffer_mut().push_system(msg);
                            }
                        }
                        InputAction::WindowItemMove(slot) => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let msg = match st.window_item_move_to_slot(slot) {
                                    Ok(()) => format!("-- moved into window {slot}"),
                                    Err(e) => format!("-- {e}"),
                                };
                                st.current_buffer_mut().push_system(msg);
                            }
                        }
                        InputAction::WindowItemRemove => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                let msg = match st.window_item_remove() {
                                    Ok(()) => "-- this window is on its own again".to_string(),
                                    Err(e) => format!("-- {e}"),
                                };
                                st.current_buffer_mut().push_system(msg);
                            }
                        }
                        InputAction::WindowItemCycle(forward) => {
                            let switched = servers
                                .lock()
                                .await
                                .state_mut(&tag)
                                .map(|st| st.window_item_cycle(forward))
                                .unwrap_or(false);
                            if !switched {
                                push_status(&servers, &tag, "-- this window isn't grouped with anything".to_string())
                                    .await;
                            }
                        }
                        InputAction::WindowItemGoto(name) => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                match resolve_buffer_by_name(st, &name) {
                                    Ok(id) => {
                                        if !st.window_item_goto(&id) {
                                            st.current_buffer_mut()
                                                .push_system(format!("-- {name} isn't in this window's group"));
                                        }
                                    }
                                    Err(msg) => st.current_buffer_mut().push_system(msg),
                                }
                            }
                        }
                        InputAction::Notice { target, text, echo_buffer } => {
                            let line = format!("NOTICE {target} :{text}");
                            {
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.state_mut(&tag) {
                                    let status = format!("-- notice({target}): {text}");
                                    match &echo_buffer {
                                        BufferId::Status => st.status.push_system(status),
                                        BufferId::Channel(key) => {
                                            if let Some(chan) = st.channels.get_mut(key) {
                                                chan.buffer.push_system(status);
                                            }
                                        }
                                        BufferId::Query(key) => {
                                            let buf = st.get_or_create_query_buffer(key, &target);
                                            buf.push_system(status);
                                        }
                                        BufferId::Empty(_) => {}
                                    }
                                }
                            }
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::Invite { nick, channel } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let _ = conn.write_line(&format!("INVITE {nick} {channel}")).await;
                                push_status(&servers, &tag, format!("-- inviting {nick} to {channel}")).await;
                            }
                        }
                        InputAction::Ctcp { target, command, data } => {
                            let line = match &data {
                                Some(d) => format!("PRIVMSG {target} :\u{1}{command} {d}\u{1}"),
                                None => format!("PRIVMSG {target} :\u{1}{command}\u{1}"),
                            };
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::Nctcp { target, command, data } => {
                            let line = match &data {
                                Some(d) => format!("NOTICE {target} :\u{1}{command} {d}\u{1}"),
                                None => format!("NOTICE {target} :\u{1}{command}\u{1}"),
                            };
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::Wall { channel, text } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => {
                                    let guard = servers.lock().await;
                                    match guard.state(&tag).map(|st| &st.active_buffer) {
                                        Some(BufferId::Channel(key)) => Some(key.clone()),
                                        _ => None,
                                    }
                                }
                            };
                            match target {
                                Some(channel) => {
                                    let _ = conn.write_line(&format!("NOTICE @{channel} :{text}")).await;
                                }
                                None => {
                                    push_status_warning(&servers, &tag, "-- no channel to /wall".to_string()).await;
                                }
                            }
                        }
                        InputAction::RawlogOpen(path) => {
                            let status = match conn.open_rawlog(std::path::Path::new(&path)) {
                                Ok(()) => format!("-- rawlog: appending raw lines to {path}"),
                                Err(e) => format!("-- rawlog: failed to open {path}: {e}"),
                            };
                            push_status(&servers, &tag, status).await;
                        }
                        InputAction::RawlogClose => {
                            let status = if conn.close_rawlog() {
                                "-- rawlog: closed".to_string()
                            } else {
                                "-- rawlog: wasn't open".to_string()
                            };
                            push_status(&servers, &tag, status).await;
                        }
                        InputAction::Query { nick, message } => match nick {
                            Some(nick) => {
                                let key = nick.to_lowercase();
                                {
                                    let mut guard = servers.lock().await;
                                    if let Some(st) = guard.state_mut(&tag) {
                                        st.get_or_create_query_buffer(&key, &nick);
                                        st.active_buffer = BufferId::Query(key.clone());
                                    }
                                }
                                if let Some(msg) = message {
                                    let echo_buffer = BufferId::Query(key.clone());
                                    let extra_nicks =
                                        extra_protect_nicks(&servers, &tag, &echo_buffer, &nick).await;
                                    queue_send(&send_tx, echo_buffer, nick, msg, extra_nicks, false);
                                }
                            }
                            None => {
                                // irssi's own `/query` with no args: closes
                                // the active query buffer; a no-op if the
                                // active buffer isn't one.
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.state_mut(&tag) {
                                    if matches!(st.active_buffer, BufferId::Query(_)) {
                                        let active = st.active_buffer.clone();
                                        st.close_buffer(&active);
                                    }
                                }
                            }
                        },
                        InputAction::Whois(nick) => {
                            {
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.state_mut(&tag) {
                                    let active = st.active_buffer.clone();
                                    st.set_pending_whois(nick.to_lowercase(), active);
                                }
                            }
                            let _ = conn.write_line(&format!("WHOIS {nick}")).await;
                        }
                        InputAction::Ip(nick) => {
                            {
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.state_mut(&tag) {
                                    let active = st.active_buffer.clone();
                                    let start_len = st.buffer_mut(&active).map(|b| b.lines.len()).unwrap_or(0);
                                    st.set_pending_whois(nick.to_lowercase(), active.clone());
                                    st.pending_ip = Some(irust_core::PendingIp {
                                        nick: nick.to_lowercase(),
                                        echo_buffer: active,
                                        host: None,
                                        start_len,
                                    });
                                }
                            }
                            let _ = conn.write_line(&format!("WHOIS {nick}")).await;
                        }
                        InputAction::Part { channel, message } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                // `/set part_message` -- irssi's own
                                // setting for the reason a bare `/part`
                                // sends. Empty (the default) means what
                                // it already meant before this setting
                                // existed: no reason at all.
                                let default_part_message = irust_config::settings::part_message();
                                let effective_message = message.clone().or(if default_part_message.is_empty() {
                                    None
                                } else {
                                    Some(default_part_message)
                                });
                                let line = match &effective_message {
                                    Some(m) => format!("PART {channel} :{m}"),
                                    None => format!("PART {channel}"),
                                };
                                let _ = conn.write_line(&line).await;
                                if let Some(st) = servers.lock().await.state_mut(&tag) {
                                    st.close_buffer(&BufferId::Channel(channel.to_lowercase()));
                                }
                            }
                        }
                        InputAction::RmRejoins => {
                            // irssi parity (`channel-rejoin.c`'s
                            // `cmd_rmrejoins`): scoped to this server
                            // only, not every connected one -- unlike
                            // `/rmreconns`'s deliberate global broadcast.
                            let cleared = {
                                let mut guard = servers.lock().await;
                                match guard.state_mut(&tag) {
                                    Some(st) => {
                                        let n = st.rejoin_channels.len();
                                        st.rejoin_channels.clear();
                                        n
                                    }
                                    None => 0,
                                }
                            };
                            let status = if cleared == 0 {
                                "-- rmrejoins: nothing pending".to_string()
                            } else {
                                format!("-- rmrejoins: cleared {cleared} pending rejoin(s)")
                            };
                            push_status_or_pending(&servers, Some(&tag), status).await;
                        }
                        InputAction::Action { target, text, echo_buffer } => {
                            let extra_nicks = extra_protect_nicks(&servers, &tag, &echo_buffer, &target).await;
                            let dcc_key = match &echo_buffer {
                                BufferId::Query(key) if key.starts_with("dcc:") => Some(key.clone()),
                                _ => None,
                            };
                            match dcc_key {
                                Some(key) => {
                                    {
                                        let mut guard = servers.lock().await;
                                        if let Some(st) = guard.state_mut(&tag) {
                                            let sender_nick = st.nick.clone();
                                            let color_index = st.nick_colors.resolve(&sender_nick);
                                            if let BufferId::Query(qkey) = &echo_buffer {
                                                let buf = st.get_or_create_query_buffer(qkey, &target);
                                                buf.push_action(sender_nick, color_index, None, text.clone(), false);
                                            }
                                        }
                                    }
                                    let sender = dcc_sessions
                                        .lock()
                                        .await
                                        .get(&(tag.clone(), key))
                                        .map(|h| h.write_tx.clone());
                                    match sender {
                                        Some(tx) => {
                                            let line = if irust_config::settings_net::bool_of("dcc_mirc_ctcp") {
                                                format!("\u{1}ACTION {text}\u{1}")
                                            } else {
                                                format!("CTCP_MESSAGE \u{1}ACTION {text}\u{1}")
                                            };
                                            let _ = tx.send(dcc::DccOutbound::Line(line));
                                        }
                                        None => {
                                            push_status_warning(&servers, &tag, "-- DCC CHAT is not connected".to_string())
                                                .await;
                                        }
                                    }
                                }
                                None => {
                                    queue_send(&send_tx, echo_buffer, target, text, extra_nicks, true);
                                }
                            }
                        }
                        InputAction::Nick(new_nick) => {
                            let _ = conn.write_line(&format!("NICK {new_nick}")).await;
                        }
                        InputAction::Topic { channel, text } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let line = match text {
                                    Some(t) => format!("TOPIC {channel} :{t}"),
                                    None => format!("TOPIC {channel}"),
                                };
                                let _ = conn.write_line(&line).await;
                            }
                        }
                        InputAction::Kick { channel, nick, reason } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let line = match reason {
                                    Some(r) => format!("KICK {channel} {nick} :{r}"),
                                    None => format!("KICK {channel} {nick}"),
                                };
                                let _ = conn.write_line(&line).await;
                            }
                        }
                        InputAction::Ban { channel, mask } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                if mask.is_none() {
                                    // Fresh listing query -- clear any
                                    // stale entries from a previous query
                                    // so the RPL_BANLIST replies below
                                    // don't just keep appending forever.
                                    let mut guard = servers.lock().await;
                                    if let Some(st) = guard.state_mut(&tag) {
                                        if let Some(chan) = st.channels.get_mut(&channel.to_lowercase()) {
                                            chan.bans.clear();
                                        }
                                    }
                                }
                                let line = match &mask {
                                    Some(m) => format!("MODE {channel} +b {m}"),
                                    None => format!("MODE {channel} +b"),
                                };
                                let _ = conn.write_line(&line).await;
                            }
                        }
                        InputAction::Unban { channel, masks } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let modestring = "-b".repeat(masks.len());
                                for (chunk_modestring, chunk_args) in chunk_channel_mode(&modestring, &masks) {
                                    let mut line = format!("MODE {channel} {chunk_modestring}");
                                    for a in &chunk_args {
                                        line.push(' ');
                                        line.push_str(a);
                                    }
                                    let _ = conn.write_line(&line).await;
                                }
                            }
                        }
                        InputAction::Quiet { channel, mask } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let _ = conn.write_line(&format!("MODE {channel} +q {mask}")).await;
                            }
                        }
                        InputAction::Unquiet { channel, masks } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let modestring = "-q".repeat(masks.len());
                                for (chunk_modestring, chunk_args) in chunk_channel_mode(&modestring, &masks) {
                                    let mut line = format!("MODE {channel} {chunk_modestring}");
                                    for a in &chunk_args {
                                        line.push(' ');
                                        line.push_str(a);
                                    }
                                    let _ = conn.write_line(&line).await;
                                }
                            }
                        }
                        InputAction::Kickban { channel, nick, mask, reason } => {
                            let target = match channel {
                                Some(c) => Some(c),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let kick_line = match &reason {
                                    Some(r) => format!("KICK {channel} {nick} :{r}"),
                                    None => format!("KICK {channel} {nick}"),
                                };
                                let ban_line = format!("MODE {channel} +b {mask}");
                                // irssi `kick_first_on_kickban` (default off: ban first).
                                if irust_config::settings_net::bool_of("kick_first_on_kickban") {
                                    let _ = conn.write_line(&kick_line).await;
                                    let _ = conn.write_line(&ban_line).await;
                                } else {
                                    let _ = conn.write_line(&ban_line).await;
                                    let _ = conn.write_line(&kick_line).await;
                                }
                            }
                        }
                        InputAction::Knockout { channel, minutes, nick, mask, reason } => {
                            let target = match &channel {
                                Some(c) => Some(c.clone()),
                                None => active_channel_name(&servers, &tag).await,
                            };
                            if let Some(channel) = target {
                                let _ = conn.write_line(&format!("MODE {channel} +b {mask}")).await;
                                let kick_line = match &reason {
                                    Some(r) => format!("KICK {channel} {nick} :{r}"),
                                    None => format!("KICK {channel} {nick}"),
                                };
                                let _ = conn.write_line(&kick_line).await;

                                // An explicit minutes argument wins; otherwise irssi's
                                // `/set knockout_time` (default 5min).
                                let delay = match minutes {
                                    Some(m) => Duration::from_secs(m * 60),
                                    None => Duration::from_millis(irust_config::settings_net::time_ms_of("knockout_time")),
                                };
                                let unban_tx = action_tx.clone();
                                let unban_channel = channel.clone();
                                let unban_mask = mask.clone();
                                tokio::spawn(async move {
                                    tokio::time::sleep(delay).await;
                                    let _ = unban_tx.send(InputAction::Unban {
                                        channel: Some(unban_channel),
                                        masks: vec![unban_mask],
                                    });
                                });
                            }
                        }
                        InputAction::ChannelMode { modestring, args } => {
                            if let Some(channel) = active_channel_name(&servers, &tag).await {
                                for (chunk_modestring, chunk_args) in chunk_channel_mode(&modestring, &args) {
                                    let mut line = format!("MODE {channel} {chunk_modestring}");
                                    for a in &chunk_args {
                                        line.push(' ');
                                        line.push_str(a);
                                    }
                                    let _ = conn.write_line(&line).await;
                                }
                            }
                        }
                        InputAction::UserMode { target, modestring, args } => {
                            let mut line = format!("MODE {target} {modestring}");
                            for a in &args {
                                line.push(' ');
                                line.push_str(a);
                            }
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::Color { nick, index } => {
                            let status = {
                                let mut guard = servers.lock().await;
                                match guard.state_mut(&tag) {
                                    Some(st) => match index {
                                        Some(idx) => {
                                            st.nick_colors.set_override(&nick, idx);
                                            let _ = irust_config::save_nick_colors(st.nick_colors.overrides());
                                            format!("-- color for {nick} set to {idx}")
                                        }
                                        None => {
                                            let resolved = st.nick_colors.resolve(&nick);
                                            format!("-- {nick}'s color is {resolved}")
                                        }
                                    },
                                    None => continue,
                                }
                            };
                            push_status(&servers, &tag, status).await;
                        }
                        InputAction::CloseBuffer(name) => {
                            let target = {
                                let guard = servers.lock().await;
                                guard.state(&tag).and_then(|st| {
                                    let target = match &name {
                                        Some(n) => match resolve_buffer_by_name(st, n) {
                                            Ok(id) => id,
                                            Err(msg) => return Some(Err(msg)),
                                        },
                                        None => st.active_buffer.clone(),
                                    };
                                    Some(Ok(target))
                                })
                            };
                            match target {
                                Some(Ok(id)) => {
                                    close_resolved_buffer(&servers, &tag, &mut conn, &dcc_sessions, id).await;
                                }
                                Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::CloseBufferByIndex(n) => {
                            let target =
                                servers.lock().await.state(&tag).and_then(|st| st.buffer_id_at_index(n));
                            match target {
                                Some(id) => {
                                    close_resolved_buffer(&servers, &tag, &mut conn, &dcc_sessions, id).await;
                                }
                                None => push_status(&servers, &tag, format!("-- no such window: {n}")).await,
                            }
                        }
                        InputAction::CloseAllBuffers => {
                            let targets: Vec<BufferId> = servers
                                .lock()
                                .await
                                .state(&tag)
                                .map(|st| st.buffer_ids().into_iter().filter(|id| id != &BufferId::Status).collect())
                                .unwrap_or_default();
                            let total = targets.len();
                            for (i, id) in targets.into_iter().enumerate() {
                                // No manual pacing needed here anymore --
                                // `Connection::write_line` itself now
                                // paces every outbound line (see its own
                                // doc comment), which is what this used
                                // to do by hand just for this one loop.
                                if !close_resolved_buffer(&servers, &tag, &mut conn, &dcc_sessions, id).await {
                                    push_status_warning(
                                        &servers,
                                        &tag,
                                        format!(
                                            "-- connection dropped mid-close ({}/{total} done) -- \
                                             the rest will need /wc again once reconnected",
                                            i + 1
                                        ),
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                        InputAction::CloseAllQueries => {
                            let targets: Vec<BufferId> = servers
                                .lock()
                                .await
                                .state(&tag)
                                .map(|st| {
                                    st.buffer_ids()
                                        .into_iter()
                                        .filter(|id| matches!(id, BufferId::Query(_)))
                                        .collect()
                                })
                                .unwrap_or_default();
                            let total = targets.len();
                            for (i, id) in targets.into_iter().enumerate() {
                                if !close_resolved_buffer(&servers, &tag, &mut conn, &dcc_sessions, id).await {
                                    push_status_warning(
                                        &servers,
                                        &tag,
                                        format!(
                                            "-- connection dropped mid-close ({}/{total} done) -- \
                                             the rest will need /wc queries again once reconnected",
                                            i + 1
                                        ),
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                        InputAction::Split { target: name, orientation } => {
                            let outcome = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| match resolve_buffer_by_name(st, &name) {
                                    Ok(id) => {
                                        if let Some(o) = orientation {
                                            st.split_orientation = o;
                                        }
                                        if st.open_split(id) {
                                            Ok(())
                                        } else {
                                            Err("-- can't split onto that buffer".to_string())
                                        }
                                    }
                                    Err(msg) => Err(msg),
                                })
                            };
                            match outcome {
                                Some(Ok(())) => {
                                    let _ = redraw_tx.send(());
                                }
                                Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::SplitByIndex { index: n, orientation } => {
                            let outcome = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| match st.buffer_id_at_index(n) {
                                    Some(id) => {
                                        if let Some(o) = orientation {
                                            st.split_orientation = o;
                                        }
                                        if st.open_split(id) {
                                            Ok(())
                                        } else {
                                            Err("-- can't split onto that buffer".to_string())
                                        }
                                    }
                                    None => Err(format!("-- no such window: {n}")),
                                })
                            };
                            match outcome {
                                Some(Ok(())) => {
                                    let _ = redraw_tx.send(());
                                }
                                Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::Unsplit(name) => {
                            let outcome = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| match &name {
                                    Some(n) => match resolve_buffer_by_name(st, n) {
                                        Ok(id) => Ok(st.close_split(Some(&id))),
                                        Err(msg) => Err(msg),
                                    },
                                    None => Ok(st.close_split(None)),
                                })
                            };
                            match outcome {
                                Some(Ok(true)) => {
                                    let _ = redraw_tx.send(());
                                }
                                Some(Ok(false)) => {
                                    push_status(&servers, &tag, "-- no split pane to close".to_string()).await
                                }
                                Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::UnsplitByIndex(n) => {
                            let outcome = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| match st.buffer_id_at_index(n) {
                                    Some(id) => Ok(st.close_split(Some(&id))),
                                    None => Err(format!("-- no such window: {n}")),
                                })
                            };
                            match outcome {
                                Some(Ok(true)) => {
                                    let _ = redraw_tx.send(());
                                }
                                Some(Ok(false)) => {
                                    push_status(&servers, &tag, "-- no split pane to close".to_string()).await
                                }
                                Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::UnsplitAll => {
                            let closed = servers
                                .lock()
                                .await
                                .state_mut(&tag)
                                .map(|st| st.close_all_splits())
                                .unwrap_or(false);
                            if closed {
                                let _ = redraw_tx.send(());
                            } else {
                                push_status(&servers, &tag, "-- no split panes open".to_string()).await;
                            }
                        }
                        InputAction::SplitFocus(name) => {
                            let outcome = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| match resolve_buffer_by_name(st, &name) {
                                    Ok(id) => Ok(st.focus_split(&id)),
                                    Err(msg) => Err(msg),
                                })
                            };
                            match outcome {
                                Some(Ok(true)) => {
                                    let _ = redraw_tx.send(());
                                }
                                Some(Ok(false)) => {
                                    push_status(&servers, &tag, "-- that isn't a split pane".to_string()).await
                                }
                                Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::SplitFocusByIndex(n) => {
                            let outcome = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| match st.buffer_id_at_index(n) {
                                    Some(id) => Ok(st.focus_split(&id)),
                                    None => Err(format!("-- no such window: {n}")),
                                })
                            };
                            match outcome {
                                Some(Ok(true)) => {
                                    let _ = redraw_tx.send(());
                                }
                                Some(Ok(false)) => {
                                    push_status(&servers, &tag, "-- that isn't a split pane".to_string()).await
                                }
                                Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::SplitFocusCycle(forward) => {
                            let cycled = servers
                                .lock()
                                .await
                                .state_mut(&tag)
                                .map(|st| st.cycle_split_focus(forward))
                                .unwrap_or(false);
                            if cycled {
                                let _ = redraw_tx.send(());
                            } else {
                                push_status(&servers, &tag, "-- no split panes open".to_string()).await;
                            }
                        }
                        InputAction::Log { action, target } => {
                            let outcome = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| {
                                    let id = match &target {
                                        Some(n) => resolve_buffer_by_name(st, n),
                                        None => Ok(st.active_buffer.clone()),
                                    };
                                    id.and_then(|id| apply_log_toggle(st, &id, action))
                                })
                            };
                            match outcome {
                                Some(Ok(msg)) | Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::LogByIndex { action, index } => {
                            let outcome = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| match st.buffer_id_at_index(index) {
                                    Some(id) => apply_log_toggle(st, &id, action),
                                    None => Err(format!("-- no such window: {index}")),
                                })
                            };
                            match outcome {
                                Some(Ok(msg)) | Some(Err(msg)) => push_status(&servers, &tag, msg).await,
                                None => {}
                            }
                        }
                        InputAction::LogStatus => {
                            let msg = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| {
                                    let id = st.active_buffer.clone();
                                    log_status_line(st, &id)
                                })
                            };
                            if let Some(msg) = msg {
                                push_status(&servers, &tag, msg).await;
                            }
                        }
                        InputAction::Netsplit => {
                            let lines = {
                                let guard = servers.lock().await;
                                guard.state(&tag).map(netsplit_summary_lines).unwrap_or_default()
                            };
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                for line in lines {
                                    st.status.push_system(line);
                                }
                            }
                            let _ = redraw_tx.send(());
                        }
                        InputAction::NickColors(enabled) => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                st.nicklist_colors = enabled;
                                let msg = if enabled {
                                    "-- nicklist colors: on"
                                } else {
                                    "-- nicklist colors: off (away still shown in gray)"
                                };
                                st.current_buffer_mut().push_system(msg.to_string());
                            }
                            let _ = redraw_tx.send(());
                        }
                        InputAction::TextColors(enabled) => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                st.text_colors = enabled;
                                let msg = if enabled {
                                    "-- message text colors: on"
                                } else {
                                    "-- message text colors: off (nick labels stay colored)"
                                };
                                st.current_buffer_mut().push_system(msg.to_string());
                            }
                            let _ = redraw_tx.send(());
                        }
                        InputAction::Away { reason } => {
                            let line = match &reason {
                                Some(r) => format!("AWAY :{r}"),
                                None => "AWAY".to_string(),
                            };
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                st.away = reason.is_some();
                                st.away_reason = reason.clone();
                            }
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::Command { name, arg } => {
                            let (status, says) = plugin_host.run_command_says(&name, &arg);
                            if let Some(status) = status {
                                push_status(&servers, &tag, status).await;
                            }
                            if !says.is_empty() {
                                let dest = servers
                                    .lock()
                                    .await
                                    .state(&tag)
                                    .and_then(|st| active_target(st).map(|t| (st.active_buffer.clone(), t)));
                                match dest {
                                    Some((echo_buffer, target)) => {
                                        queue_plugin_says(&send_tx, echo_buffer, target, says);
                                    }
                                    None => {
                                        push_status_warning(
                                            &servers,
                                            &tag,
                                            format!("-- /{name}: not in a channel or query window"),
                                        )
                                        .await;
                                    }
                                }
                            }
                        }
                        InputAction::ServerCommand { command, rest } => {
                            if command == "WHO" {
                                let mut guard = servers.lock().await;
                                if let Some(st) = guard.state_mut(&tag) {
                                    st.manual_who_pending.push(rest.trim().to_lowercase());
                                }
                            }
                            let line =
                                if rest.is_empty() { command } else { format!("{command} {rest}") };
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::EchoThenServerCommand { echo, command, rest } => {
                            push_status(&servers, &tag, echo).await;
                            let line =
                                if rest.is_empty() { command } else { format!("{command} {rest}") };
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::Squit { server, reason } => {
                            let line = if reason.is_empty() {
                                format!("SQUIT {server}")
                            } else {
                                format!("SQUIT {server} :{reason}")
                            };
                            let _ = conn.write_line(&line).await;
                        }
                        InputAction::Wallops { message } => {
                            let _ = conn.write_line(&format!("WALLOPS :{message}")).await;
                        }
                        InputAction::Ping => {
                            lag_ping_sent = Some(Instant::now());
                            let _ = conn.write_line(&format!("PING :{LAG_PING_TOKEN}")).await;
                        }
                        InputAction::Redraw => {
                            let _ = force_redraw_tx.send(());
                        }
                        InputAction::Update => {
                            push_status(&servers, &tag, "-- checking for updates...".to_string()).await;
                            match updater::check_and_update().await {
                                updater::UpdateOutcome::UpToDate(version) => {
                                    push_status(&servers, &tag, format!("-- already up to date (v{version})"))
                                        .await;
                                }
                                updater::UpdateOutcome::Updated { to } => {
                                    push_status(
                                        &servers,
                                        &tag,
                                        format!("-- updated to v{to} on disk -- restart irust to use it"),
                                    )
                                    .await;
                                }
                                updater::UpdateOutcome::Failed(e) => {
                                    push_status(&servers, &tag, format!("-- update failed: {e}")).await;
                                }
                            }
                        }
                        InputAction::Upgrade => {
                        }
                        InputAction::Restart => {
                            // Same reasoning as `Upgrade` just above --
                            // the top-level router intercepts a typed
                            // `/restart` directly, this arm only exists
                            // to keep the match exhaustive.
                        }
                        InputAction::PluginReload(name) => {
                            let Some(dir) = plugins_dir() else {
                                push_status(&servers, &tag, "-- no config dir, nothing to reload".to_string())
                                    .await;
                                continue;
                            };
                            match name {
                                None => match plugin_host.reload(&dir) {
                                    Ok(count) => {
                                        push_status(
                                            &servers,
                                            &tag,
                                            format!("-- plugins reloaded ({count} script(s))"),
                                        )
                                        .await;
                                    }
                                    Err(e) => {
                                        push_status(&servers, &tag, format!("-- plugin reload failed: {e}"))
                                            .await;
                                    }
                                },
                                Some(name) => match plugin_host.load_named(&dir, &name) {
                                    Ok(Some(true)) => {
                                        push_status(&servers, &tag, format!("-- {name}: reloaded")).await;
                                    }
                                    Ok(Some(false)) => {
                                        push_status(
                                            &servers,
                                            &tag,
                                            format!("-- {name}: failed to load (syntax/runtime error)"),
                                        )
                                        .await;
                                    }
                                    Ok(None) => {
                                        push_status(&servers, &tag, format!("-- {name}: no such plugin file"))
                                            .await;
                                    }
                                    Err(e) => {
                                        push_status(&servers, &tag, format!("-- plugin reload failed: {e}"))
                                            .await;
                                    }
                                },
                            }
                        }
                        InputAction::PluginLoad(name) => {
                            let Some(dir) = plugins_dir() else {
                                push_status(&servers, &tag, "-- no config dir, nothing to load".to_string())
                                    .await;
                                continue;
                            };
                            match plugin_host.load_named(&dir, &name) {
                                Ok(Some(true)) => {
                                    push_status(&servers, &tag, format!("-- {name}: loaded")).await;
                                }
                                Ok(Some(false)) => {
                                    push_status(
                                        &servers,
                                        &tag,
                                        format!("-- {name}: failed to load (syntax/runtime error)"),
                                    )
                                    .await;
                                }
                                Ok(None) => {
                                    push_status(&servers, &tag, format!("-- {name}: no such plugin file")).await;
                                }
                                Err(e) => {
                                    push_status(&servers, &tag, format!("-- plugin load failed: {e}")).await;
                                }
                            }
                        }
                        InputAction::PluginExec(code) => {
                            match plugin_host.load_str(&code, "<pluginexec>") {
                                Ok(()) => {
                                    push_status(&servers, &tag, "-- executed".to_string()).await;
                                }
                                Err(e) => {
                                    push_status(&servers, &tag, format!("-- error: {e}")).await;
                                }
                            }
                        }
                        InputAction::PluginReset => {
                            let names = plugin_host.loaded_scripts();
                            let count = names.len();
                            for name in &names {
                                plugin_host.unload_script(name);
                            }
                            push_status(&servers, &tag, format!("-- {count} plugin(s) unloaded")).await;
                        }
                        InputAction::PluginUnload(name) => {
                            if plugin_host.unload_script(&name) {
                                push_status(&servers, &tag, format!("-- {name}: unloaded")).await;
                            } else {
                                push_status(&servers, &tag, format!("-- {name}: wasn't loaded")).await;
                            }
                        }
                        InputAction::Notify { nick } => {
                            match nick {
                                Some(nick) => {
                                    let key = nick.to_lowercase();
                                    if notify_list.contains(&key) {
                                        push_status(&servers, &tag, format!("-- already notifying about {nick}"))
                                            .await;
                                    } else {
                                        let using_monitor = servers
                                            .lock()
                                            .await
                                            .state(&tag)
                                            .is_some_and(|st| st.monitor_limit.is_some());
                                        notify_list.push(key.clone());
                                        if using_monitor {
                                            let _ = conn.write_line(&format!("MONITOR + {key}")).await;
                                            push_status(&servers, &tag, format!("-- now notifying about {nick}"))
                                                .await;
                                        } else {
                                            push_status(
                                                &servers,
                                                &tag,
                                                format!(
                                                    "-- now notifying about {nick} (checked every {}s)",
                                                    NOTIFY_CHECK_INTERVAL.as_secs()
                                                ),
                                            )
                                            .await;
                                        }
                                    }
                                }
                                None => {
                                    if notify_list.is_empty() {
                                        push_status(&servers, &tag, "-- notify list is empty".to_string()).await;
                                    } else {
                                        let mut sorted = notify_list.clone();
                                        sorted.sort();
                                        if let Some(st) = servers.lock().await.state_mut(&tag) {
                                            for nick in &sorted {
                                                let status =
                                                    if notify_online.contains(nick) { "online" } else { "offline" };
                                                st.status.push_system(format!("-- {nick}: {status}"));
                                            }
                                        }
                                        let _ = redraw_tx.send(());
                                    }
                                }
                            }
                        }
                        InputAction::Unnotify { nick } => {
                            let key = nick.to_lowercase();
                            if notify_list.iter().any(|n| n == &key) {
                                let using_monitor = servers
                                    .lock()
                                    .await
                                    .state(&tag)
                                    .is_some_and(|st| st.monitor_limit.is_some());
                                notify_list.retain(|n| n != &key);
                                notify_online.remove(&key);
                                if using_monitor {
                                    let _ = conn.write_line(&format!("MONITOR - {key}")).await;
                                }
                                push_status(&servers, &tag, format!("-- no longer notifying about {nick}")).await;
                            } else {
                                push_status(&servers, &tag, format!("-- not notifying about {nick}")).await;
                            }
                        }
                        InputAction::Quit { reason } => {
                            let reason = match reason {
                                Some(r) => r,
                                None => {
                                    let list = quit_messages.lock().await;
                                    if list.is_empty() {
                                        "irust".to_string()
                                    } else {
                                        let idx = (getrandom::u32().unwrap_or(0) as usize) % list.len();
                                        list[idx].clone()
                                    }
                                }
                            };
                            let write_result = conn.write_line(&format!("QUIT :{reason}")).await;
                            irust_config::debug_log::line(
                                "QUIT",
                                &tag,
                                &format!(
                                    "write_line({:?}) -> {}",
                                    reason,
                                    match &write_result {
                                        Ok(()) => "Ok".to_string(),
                                        Err(e) => format!("Err({e})"),
                                    }
                                ),
                            );
                            let drain_deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
                            loop {
                                let remaining = drain_deadline.saturating_duration_since(std::time::Instant::now());
                                if remaining.is_zero() {
                                    irust_config::debug_log::line("QUIT", &tag, "drain: timed out waiting for server close");
                                    break;
                                }
                                match tokio::time::timeout(remaining, conn.read_line()).await {
                                    Ok(Some(Ok(_))) => continue,
                                    Ok(Some(Err(e))) => {
                                        irust_config::debug_log::line("QUIT", &tag, &format!("drain: read error: {e}"));
                                        break;
                                    }
                                    Ok(None) => {
                                        irust_config::debug_log::line("QUIT", &tag, "drain: server closed its end");
                                        break;
                                    }
                                    Err(_) => {
                                        irust_config::debug_log::line("QUIT", &tag, "drain: timed out waiting for server close");
                                        break;
                                    }
                                }
                            }
                            let _ = quit_ack_tx.send(tag.clone());
                            {
                                let network = tag.clone();
                                spawn_notify_hook(&plugin_host, move |h| h.on_disconnect(&network));
                            }
                            let _ = redraw_tx.send(());
                            break 'reconnect;
                        }
                        InputAction::RmReconns => {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                st.current_buffer_mut().push_system("-- no pending reconnection".to_string());
                            }
                            let _ = redraw_tx.send(());
                        }
                        InputAction::Connect { .. }
                        | InputAction::NetworkList
                        | InputAction::NetworkAdd { .. }
                        | InputAction::NetworkModify { .. }
                        | InputAction::NetworkRemove(_)
                        | InputAction::Disconnect(_)
                        | InputAction::SwitchServer(_)
                        | InputAction::Reconnect
                        | InputAction::Alias { .. }
                        | InputAction::Unalias(_)
                        | InputAction::Eval(_)
                        | InputAction::Wait(_)
                        | InputAction::SwitchBuffer(_)
                        | InputAction::SwitchBufferByIndex(_)
                        | InputAction::ScrollbackGoto(_)
                        | InputAction::ScrollbackHome
                        | InputAction::ScrollbackEnd
                        | InputAction::ScrollbackStatus
                        | InputAction::Foreach { .. }
                        | InputAction::Echo(_)
                        | InputAction::TopicSummary
                        | InputAction::Toggle { .. }
                        | InputAction::Beep
                        | InputAction::Cat(_)
                        | InputAction::Cd(_)
                        | InputAction::StatusbarInfo(_)
                        | InputAction::Set { .. }
                        | InputAction::Format { .. }
                        | InputAction::Recode(_)
                        | InputAction::Exec(_)
                        | InputAction::Uptime
                        | InputAction::ClientVersion
                        | InputAction::Completion { .. }
                        | InputAction::Ignore { .. }
                        | InputAction::Unignore(_)
                        | InputAction::Hilight { .. }
                        | InputAction::Dehilight(_)
                        | InputAction::Autoban { .. }
                        | InputAction::Unautoban(_)
                        | InputAction::StatusbarList
                        | InputAction::StatusbarAdd { .. }
                        | InputAction::StatusbarRemove(_)
                        | InputAction::StatusbarReset
                        | InputAction::LayoutList
                        | InputAction::LayoutSave
                        | InputAction::LayoutReset
                        | InputAction::ChannelSetup { .. }
                        | InputAction::ChannelRemove { .. }
                        | InputAction::ChannelList
                        | InputAction::Bind { .. } => {}
                        InputAction::DccChat(nick) => {
                            let nick_lc = nick.to_lowercase();
                            let already_active =
                                dcc_sessions.lock().await.contains_key(&(tag.clone(), dcc::dcc_key(&nick)));
                            if already_active {
                                push_status(
                                    &servers,
                                    &tag,
                                    format!("-- already have an active DCC CHAT with {nick}"),
                                )
                                .await;
                                continue;
                            }
                            let pending = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).and_then(|st| st.pending_dcc.remove(&nick_lc))
                            };
                            match pending {
                                // They already offered us one -- accept
                                // it by connecting out to their address.
                                Some((ip, port)) => {
                                    push_status(
                                        &servers,
                                        &tag,
                                        format!("-- connecting to {nick}'s DCC CHAT offer..."),
                                    )
                                    .await;
                                    let tag2 = tag.clone();
                                    let nick2 = nick.clone();
                                    let servers2 = servers.clone();
                                    let redraw_tx2 = redraw_tx.clone();
                                    let dcc_sessions2 = dcc_sessions.clone();
                                    tokio::spawn(async move {
                                        let connected = tokio::time::timeout(
                                            Duration::from_secs(30),
                                            tokio::net::TcpStream::connect((ip.as_str(), port)),
                                        )
                                        .await;
                                        match connected {
                                            Ok(Ok(stream)) => {
                                                dcc::run_session(
                                                    stream,
                                                    tag2,
                                                    nick2,
                                                    servers2,
                                                    redraw_tx2,
                                                    dcc_sessions2,
                                                )
                                                .await;
                                            }
                                            Ok(Err(err)) => {
                                                push_status(
                                                    &servers2,
                                                    &tag2,
                                                    format!("-- couldn't connect to {nick2}'s DCC CHAT: {err}"),
                                                )
                                                .await;
                                            }
                                            Err(_elapsed) => {
                                                push_status(
                                                    &servers2,
                                                    &tag2,
                                                    format!("-- connecting to {nick2}'s DCC CHAT timed out"),
                                                )
                                                .await;
                                            }
                                        }
                                    });
                                }
                                // No pending offer from them -- send a
                                // fresh one and wait for them to connect.
                                None => {
                                    let Some(ip) = dcc_announce_ip(&conn) else {
                                        push_status(
                                            &servers,
                                            &tag,
                                            "-- can't determine a DCC-usable IPv4 address".to_string(),
                                        )
                                        .await;
                                        continue;
                                    };
                                    let listener = match bind_dcc_listener().await {
                                        Ok(l) => l,
                                        Err(err) => {
                                            push_status(
                                                &servers,
                                                &tag,
                                                format!("-- couldn't open a DCC listener: {err}"),
                                            )
                                            .await;
                                            continue;
                                        }
                                    };
                                    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                                    let ctcp = irust_core::dcc::offer_ctcp(ip, port);
                                    let _ = conn.write_line(&format!("PRIVMSG {nick} :{ctcp}")).await;
                                    push_status(
                                        &servers,
                                        &tag,
                                        format!("-- offered a DCC CHAT to {nick} ({ip}:{port}), waiting..."),
                                    )
                                    .await;
                                    let tag2 = tag.clone();
                                    let nick2 = nick.clone();
                                    let servers2 = servers.clone();
                                    let redraw_tx2 = redraw_tx.clone();
                                    let dcc_sessions2 = dcc_sessions.clone();
                                    tokio::spawn(async move {
                                        let accept = tokio::time::timeout(
                                            irust_core::dcc_transfer::offer_timeout(),
                                            listener.accept(),
                                        )
                                        .await;
                                        match accept {
                                            Ok(Ok((stream, _))) => {
                                                dcc::run_session(
                                                    stream,
                                                    tag2,
                                                    nick2,
                                                    servers2,
                                                    redraw_tx2,
                                                    dcc_sessions2,
                                                )
                                                .await;
                                            }
                                            _ => {
                                                push_status(
                                                    &servers2,
                                                    &tag2,
                                                    format!("-- DCC CHAT offer to {nick2} timed out"),
                                                )
                                                .await;
                                            }
                                        }
                                    });
                                }
                            }
                        }
                        InputAction::DccSend { nick, path, passive } => {
                            // `dcc_upload_path`: a relative path is looked up under it.
                            let file_path = irust_core::dcc_transfer::resolve_upload_path(&path);
                            let metadata = match tokio::fs::metadata(&file_path).await {
                                Ok(m) if m.is_file() => m,
                                Ok(_) => {
                                    push_status_warning(&servers, &tag, format!("-- {path} is not a regular file"))
                                        .await;
                                    continue;
                                }
                                Err(err) => {
                                    push_status_warning(&servers, &tag, format!("-- can't read {path}: {err}"))
                                        .await;
                                    continue;
                                }
                            };
                            let size = metadata.len();
                            let Some(filename) = file_path.file_name().and_then(|f| f.to_str()).map(str::to_string)
                            else {
                                push_status_warning(&servers, &tag, format!("-- {path} has no usable filename"))
                                    .await;
                                continue;
                            };
                            // `dcc_send_replace_space_with_underscore`: the announced name only.
                            let filename = irust_core::dcc_transfer::outgoing_filename(&filename);
                            let Some(ip) = dcc_announce_ip(&conn) else {
                                push_status_warning(
                                    &servers,
                                    &tag,
                                    "-- can't determine a DCC-usable IPv4 address".to_string(),
                                )
                                .await;
                                continue;
                            };
                            let id = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).map(|st| st.next_dcc_id())
                            };
                            let Some(id) = id else { continue };

                            if passive {
                                let token = id.to_string();
                                let (resume_tx, mut resume_rx) = tokio::sync::oneshot::channel::<u64>();
                                dcc_resume_waiters
                                    .lock()
                                    .await
                                    .insert(dcc_transfer::WaitKey::Token(token.clone()), resume_tx);
                                let offer = irust_core::dcc_transfer::passive_offer_ctcp(&filename, ip, size, &token);
                                let _ = conn.write_line(&format!("PRIVMSG {nick} :{offer}")).await;
                                {
                                    let mut guard = servers.lock().await;
                                    if let Some(st) = guard.state_mut(&tag) {
                                        st.pending_dcc_send.insert(
                                            id,
                                            irust_core::dcc_transfer::PendingSendOffer {
                                                id,
                                                peer_nick: nick.clone(),
                                                filename: filename.clone(),
                                                ip: ip.to_string(),
                                                port: 0,
                                                size,
                                                token: Some(token.clone()),
                                                incoming: false,
                                            },
                                        );
                                    }
                                }
                                push_status(
                                    &servers,
                                    &tag,
                                    format!(
                                        "-- offered {filename} ({size} bytes) to {nick} passively (id {id}), waiting for them to accept..."
                                    ),
                                )
                                .await;

                                let tag2 = tag.clone();
                                let nick2 = nick.clone();
                                let servers2 = servers.clone();
                                let redraw_tx2 = redraw_tx.clone();
                                let dcc_transfers2 = dcc_transfers.clone();
                                let dcc_resume_waiters2 = dcc_resume_waiters.clone();
                                let token2 = token.clone();
                                tokio::spawn(async move {
                                    let mut real_ip_port = None;
                                    for _ in 0..360 {
                                        let found = {
                                            let guard = servers2.lock().await;
                                            guard.state(&tag2).and_then(|st| st.pending_dcc_send.get(&id)).and_then(
                                                |p| if p.port != 0 { Some((p.ip.clone(), p.port)) } else { None },
                                            )
                                        };
                                        if let Some(ip_port) = found {
                                            real_ip_port = Some(ip_port);
                                            break;
                                        }
                                        tokio::time::sleep(Duration::from_millis(500)).await;
                                    }
                                    dcc_resume_waiters2.lock().await.remove(&dcc_transfer::WaitKey::Token(token2));
                                    let resume_from = resume_rx.try_recv().unwrap_or(0);
                                    {
                                        let mut guard = servers2.lock().await;
                                        if let Some(st) = guard.state_mut(&tag2) {
                                            st.pending_dcc_send.remove(&id);
                                        }
                                    }
                                    let Some((peer_ip, peer_port)) = real_ip_port else {
                                        push_status_warning(
                                            &servers2,
                                            &tag2,
                                            format!(
                                                "-- {nick2} never accepted the passive DCC SEND offer for {filename}"
                                            ),
                                        )
                                        .await;
                                        return;
                                    };
                                    let connected = tokio::time::timeout(
                                        Duration::from_secs(30),
                                        tokio::net::TcpStream::connect((peer_ip.as_str(), peer_port)),
                                    )
                                    .await;
                                    match connected {
                                        Ok(Ok(stream)) => {
                                            dcc_transfer::run_send_session(
                                                stream,
                                                dcc_transfer::TransferParams {
                                                    tag: tag2,
                                                    peer_nick: nick2,
                                                    id,
                                                    filename,
                                                    path: file_path,
                                                    size,
                                                    resume_from,
                                                    servers: servers2,
                                                    redraw_tx: redraw_tx2,
                                                    transfers: dcc_transfers2,
                                                },
                                            )
                                            .await;
                                        }
                                        Ok(Err(err)) => {
                                            push_status_warning(
                                                &servers2,
                                                &tag2,
                                                format!("-- couldn't connect to {nick2} for the DCC SEND: {err}"),
                                            )
                                            .await;
                                        }
                                        Err(_elapsed) => {
                                            push_status_warning(
                                                &servers2,
                                                &tag2,
                                                format!("-- connecting to {nick2} for the DCC SEND timed out"),
                                            )
                                            .await;
                                        }
                                    }
                                });
                            } else {
                                // Active: bind a listener, offer a real
                                // port -- same shape as DCC CHAT's own
                                // offer path.
                                let listener = match bind_dcc_listener().await {
                                    Ok(l) => l,
                                    Err(err) => {
                                        push_status_warning(
                                            &servers,
                                            &tag,
                                            format!("-- couldn't open a DCC listener: {err}"),
                                        )
                                        .await;
                                        continue;
                                    }
                                };
                                let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
                                let (resume_tx, mut resume_rx) = tokio::sync::oneshot::channel::<u64>();
                                dcc_resume_waiters.lock().await.insert(dcc_transfer::WaitKey::Port(port), resume_tx);
                                let offer = irust_core::dcc_transfer::send_offer_ctcp(&filename, ip, port, size);
                                let _ = conn.write_line(&format!("PRIVMSG {nick} :{offer}")).await;
                                {
                                    let mut guard = servers.lock().await;
                                    if let Some(st) = guard.state_mut(&tag) {
                                        st.pending_dcc_send.insert(
                                            id,
                                            irust_core::dcc_transfer::PendingSendOffer {
                                                id,
                                                peer_nick: nick.clone(),
                                                filename: filename.clone(),
                                                ip: ip.to_string(),
                                                port,
                                                size,
                                                token: None,
                                                incoming: false,
                                            },
                                        );
                                    }
                                }
                                push_status(
                                    &servers,
                                    &tag,
                                    format!("-- offered {filename} ({size} bytes) to {nick} ({ip}:{port}, id {id}), waiting..."),
                                )
                                .await;

                                let tag2 = tag.clone();
                                let nick2 = nick.clone();
                                let servers2 = servers.clone();
                                let redraw_tx2 = redraw_tx.clone();
                                let dcc_transfers2 = dcc_transfers.clone();
                                let dcc_resume_waiters2 = dcc_resume_waiters.clone();
                                tokio::spawn(async move {
                                    let accept =
                                        tokio::time::timeout(irust_core::dcc_transfer::offer_timeout(), listener.accept()).await;
                                    dcc_resume_waiters2.lock().await.remove(&dcc_transfer::WaitKey::Port(port));
                                    let resume_from = resume_rx.try_recv().unwrap_or(0);
                                    {
                                        let mut guard = servers2.lock().await;
                                        if let Some(st) = guard.state_mut(&tag2) {
                                            st.pending_dcc_send.remove(&id);
                                        }
                                    }
                                    match accept {
                                        Ok(Ok((stream, _))) => {
                                            dcc_transfer::run_send_session(
                                                stream,
                                                dcc_transfer::TransferParams {
                                                    tag: tag2,
                                                    peer_nick: nick2,
                                                    id,
                                                    filename,
                                                    path: file_path,
                                                    size,
                                                    resume_from,
                                                    servers: servers2,
                                                    redraw_tx: redraw_tx2,
                                                    transfers: dcc_transfers2,
                                                },
                                            )
                                            .await;
                                        }
                                        _ => {
                                            push_status_warning(
                                                &servers2,
                                                &tag2,
                                                format!("-- DCC SEND offer to {nick2} timed out"),
                                            )
                                            .await;
                                        }
                                    }
                                });
                            }
                        }
                        InputAction::DccGet(id) => {
                            dcc_get_or_resume(
                                id,
                                false,
                                &tag,
                                DccGetContext {
                                    servers: &servers,
                                    conn: &mut conn,
                                    redraw_tx: &redraw_tx,
                                    dcc_transfers: &dcc_transfers,
                                    dcc_resume_waiters: &dcc_resume_waiters,
                                },
                            )
                            .await;
                        }
                        InputAction::DccResume(id) => {
                            dcc_get_or_resume(
                                id,
                                false,
                                &tag,
                                DccGetContext {
                                    servers: &servers,
                                    conn: &mut conn,
                                    redraw_tx: &redraw_tx,
                                    dcc_transfers: &dcc_transfers,
                                    dcc_resume_waiters: &dcc_resume_waiters,
                                },
                            )
                            .await;
                        }
                        InputAction::DccList => {
                            let mut lines: Vec<String> = Vec::new();
                            {
                                let guard = servers.lock().await;
                                if let Some(st) = guard.state(&tag) {
                                    let mut offers: Vec<_> = st.pending_dcc_send.values().collect();
                                    offers.sort_by_key(|o| o.id);
                                    for o in offers {
                                        let dir = if o.incoming { "get" } else { "send" };
                                        let mode = if o.port == 0 { "passive" } else { "active" };
                                        lines.push(format!(
                                            "-- #{} {dir} {} {} ({} bytes) [{mode}, pending]",
                                            o.id, o.peer_nick, o.filename, o.size
                                        ));
                                    }
                                }
                            }
                            {
                                let guard = dcc_transfers.lock().await;
                                for handle in guard.values() {
                                    let p = handle.progress.lock().await;
                                    if p.tag != tag {
                                        continue;
                                    }
                                    let dir = match p.direction {
                                        dcc_transfer::Direction::Send => "send",
                                        dcc_transfer::Direction::Get => "get",
                                    };
                                    let pct = p.bytes_done.saturating_mul(100).checked_div(p.size).unwrap_or(0);
                                    lines.push(format!(
                                        "-- #{} {dir} {} {} {}/{} bytes ({pct}%) [{:?}]",
                                        p.id, p.peer_nick, p.filename, p.bytes_done, p.size, p.state
                                    ));
                                }
                            }
                            if lines.is_empty() {
                                push_status(&servers, &tag, "-- no DCC SEND/GET offers or transfers".to_string())
                                    .await;
                            } else {
                                for line in lines {
                                    push_status(&servers, &tag, line).await;
                                }
                            }
                        }
                        InputAction::DccClose(id) => {
                            let had_transfer = {
                                let guard = dcc_transfers.lock().await;
                                if let Some(handle) = guard.get(&(tag.clone(), id)) {
                                    let _ = handle.cancel_tx.send(());
                                    true
                                } else {
                                    false
                                }
                            };
                            if had_transfer {
                                continue;
                            }
                            let removed = {
                                let mut guard = servers.lock().await;
                                guard.state_mut(&tag).and_then(|st| st.pending_dcc_send.remove(&id)).is_some()
                            };
                            if removed {
                                push_status(&servers, &tag, format!("-- closed DCC offer #{id}")).await;
                            } else {
                                push_status_warning(&servers, &tag, format!("-- no DCC offer or transfer with id {id}"))
                                    .await;
                            }
                        }
                        InputAction::Unknown(_) => {}
                    }
                    let _ = redraw_tx.send(());
                }
            }
        }

        // Connection dropped unexpectedly (not a deliberate /quit) --
        // tell the user, then retry every `server_reconnect_time`.
        // Rejoining every known channel on success is handled for free
        // by irust_core's own "001" -> auto-JOIN-everything logic once
        // the fresh connection registers.
        {
            let mut guard = servers.lock().await;
            match guard.state_mut(&tag) {
                Some(st) => {
                    st.connected = false;
                    for chan in st.channels.values_mut() {
                        chan.joined = false;
                        chan.buffer
                            .push_system_leave("-- disconnected, reconnecting...".to_string());
                    }
                }
                // `/disconnect`-ed out from under us mid-flight -- nothing
                // left to reconnect for.
                None => return,
            }
        }
        {
            let network = tag.clone();
            spawn_notify_hook(&plugin_host, move |h| h.on_disconnect(&network));
        }
        let _ = redraw_tx.send(());

        loop {
            // `server_reconnect_time`: irssi's own fixed delay between
            // attempts, no exponential growth -- re-read every time
            // since it's a live `/set`.
            let reconnect_delay = {
                let ms = irust_config::settings_net::time_ms_of("server_reconnect_time");
                if ms > 0 {
                    Duration::from_millis(ms as u64)
                } else {
                    RECONNECT_FALLBACK_DELAY
                }
            };
            let sleep_fut = tokio::time::sleep(reconnect_delay);
            tokio::pin!(sleep_fut);
            loop {
                tokio::select! {
                    _ = &mut sleep_fut => break,
                    Some(action) = action_rx.recv() => {
                        if let InputAction::Quit { .. } = &action {
                            let _ = quit_ack_tx.send(tag.clone());
                            return;
                        }
                        if let InputAction::RmReconns = &action {
                            if let Some(st) = servers.lock().await.state_mut(&tag) {
                                st.current_buffer_mut().push_system(
                                    "-- reconnection to this server cancelled (use /reconnect to retry)"
                                        .to_string(),
                                );
                            }
                            let _ = redraw_tx.send(());
                            loop {
                                match action_rx.recv().await {
                                    Some(InputAction::Quit { .. }) => {
                                        let _ = quit_ack_tx.send(tag.clone());
                                        return;
                                    }
                                    Some(_) => {
                                        if let Some(st) = servers.lock().await.state_mut(&tag) {
                                            st.current_buffer_mut().push_system(
                                                "-- not connected, try again once reconnected".to_string(),
                                            );
                                        }
                                        let _ = redraw_tx.send(());
                                    }
                                    None => return,
                                }
                            }
                        }
                        if let Some(st) = servers.lock().await.state_mut(&tag) {
                            st.current_buffer_mut()
                                .push_system("-- not connected, try again once reconnected".to_string());
                        }
                        let _ = redraw_tx.send(());
                    }
                }
            }

            let current_nick = match servers.lock().await.state(&tag) {
                Some(st) => st.nick.clone(),
                None => return,
            };
            let mut retry_params = params.clone();
            retry_params.nick = current_nick;
            let resolved_ip = tokio::net::lookup_host((retry_params.host.as_str(), retry_params.port))
                .await
                .ok()
                .and_then(|mut addrs| addrs.next())
                .map(|addr| addr.ip().to_string())
                .unwrap_or_default();
            if let Some(st) = servers.lock().await.state_mut(&tag) {
                st.status.push_system(format!(
                    "-- Reconnecting to {} [{resolved_ip}] port {} - use /rmreconns to abort",
                    retry_params.host, retry_params.port
                ));
            }
            match connect_and_register(&retry_params, &tag).await {
                Ok((new_conn, new_registry)) => {
                    conn = new_conn;
                    registry = new_registry;
                    monitor_synced_this_connection = false;
                    replay_cutoff = replay_watermark::begin(&tag);
                    lag_ping_sent = None;
                    lag_pongs_missed = 0;
                    lag_interval.reset();
                    if let Some(st) = servers.lock().await.state_mut(&tag) {
                        st.status
                            .push_system(format!("-- Connection to {} established", retry_params.host));
                    }
                    break;
                }
                Err(e) => {
                    // irssi parity: `TXT_CANT_CONNECT`, same wording
                    // as the first-connect path above.
                    if let Some(st) = servers.lock().await.state_mut(&tag) {
                        st.status.push_system(format!(
                            "-- Unable to connect server {} port {}: {e}",
                            retry_params.host, retry_params.port
                        ));
                    }
                    continue;
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = START_TIME.set(Instant::now());

    if std::env::args().any(|a| a == "--list-networks") {
        for preset in irust_config::builtin_presets() {
            println!("{} ({})", preset.display_name, preset.id);
        }
        return Ok(());
    }

    if std::env::args().any(|a| a == "--debug") {
        if let Some(dirs) = directories::ProjectDirs::from("", "", "irust") {
            let path = dirs.config_dir().join("debug.log");
            match irust_config::debug_log::init(&path) {
                Ok(()) => eprintln!("irust: --debug logging to {}", path.display()),
                Err(err) => eprintln!("irust: could not open debug log at {}: {err}", path.display()),
            }
        }
    }

    let app_config = irust_config::load_config();
    let color_overrides = irust_config::load_nick_colors();

    let aliases = if irust_config::aliases_file_missing() {
        let seeded = starter_aliases();
        let _ = irust_config::save_aliases(&seeded);
        seeded
    } else {
        irust_config::load_aliases()
    };
    let aliases: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(aliases));
    let completions: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(irust_config::load_completions()));
    let ignores: Arc<Mutex<Vec<irust_config::IgnoreEntry>>> = Arc::new(Mutex::new(irust_config::load_ignores()));
    let hilights: Arc<Mutex<Vec<irust_config::HilightEntry>>> = Arc::new(Mutex::new(irust_config::load_hilights()));
    let autobans: Arc<Mutex<Vec<irust_config::AutobanEntry>>> = Arc::new(Mutex::new(irust_config::load_autoban()));
    let quit_messages: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(irust_config::load_quit_messages()));
    let keybinds: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(irust_config::load_keybinds()));
    let dcc_sessions: dcc::DccSessions = Arc::new(Mutex::new(HashMap::new()));
    let dcc_transfers: dcc_transfer::DccTransfers = Arc::new(Mutex::new(HashMap::new()));
    let dcc_resume_waiters: dcc_transfer::DccResumeWaiters = Arc::new(Mutex::new(HashMap::new()));
    let mut loaded_settings = irust_config::load_settings();
    irust_config::settings::init_user_defaults(&mut loaded_settings);
    let settings: Arc<Mutex<irust_config::Settings>> = Arc::new(Mutex::new(loaded_settings));
    let statusbar: Arc<Mutex<irust_config::Statusbar>> = Arc::new(Mutex::new(irust_config::load_statusbar()));

    let servers = Arc::new(Mutex::new(Servers::empty()));
    let server_channels: ServerChannels = Arc::new(Mutex::new(HashMap::new()));

    let plugin_host = Arc::new(PluginHost::new()?);
    if let Some(dir) = plugins_dir() {
        plugin_host.load_dir(&dir);
    }

    let (outgoing_tx, mut outgoing_rx) = mpsc::unbounded_channel::<String>();
    let (redraw_tx, redraw_rx) = mpsc::unbounded_channel::<()>();
    // `/redraw` -- distinct from `redraw_tx` above, which only requests
    // the next normal (diffed) draw. This one clears ratatui's diff
    // state first, forcing every cell to repaint -- for a resize glitch
    // or anything else that leaves stray content on screen.
    let (force_redraw_tx, force_redraw_rx) = mpsc::unbounded_channel::<()>();
    let (bell_tx, bell_rx) = mpsc::unbounded_channel::<()>();
    let (quit_ack_tx, quit_ack_rx) = mpsc::unbounded_channel::<String>();
    let (restart_tx, restart_rx) = mpsc::unbounded_channel::<()>();

    // irssi-style: only networks explicitly marked `autoconnect` in
    // config.toml get connected at startup. Everything else -- an
    // unmarked `[networks.X]`, or a totally empty/missing config.toml --
    // stays untouched until an explicit `/connect`.
    let mut autoconnect: Vec<String> = app_config
        .networks
        .iter()
        .filter(|(_, over)| over.autoconnect.unwrap_or(false))
        .map(|(name, _)| name.clone())
        .collect();
    autoconnect.sort();

    let restart_networks = irust_config::restart_state::take_restart_state();
    let mut restart_nicks: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for n in restart_networks {
        if !autoconnect.contains(&n.tag) {
            autoconnect.push(n.tag.clone());
        }
        if let Some(nick) = n.nick {
            restart_nicks.insert(n.tag, nick);
        }
    }
    autoconnect.sort();

    let seed_banner = Arc::new(AtomicBool::new(autoconnect.is_empty()));

    for name in &autoconnect {
        match resolve_conn_params(name, &app_config, None, None) {
            Ok(mut params) => {
                if let Some(nick) = restart_nicks.get(name) {
                    params.nick = nick.clone();
                }
                let saved_layout = irust_config::has_saved_layout(name).then(|| irust_config::load_layout(name));
                let (mut extra_channels, layout_queries) = apply_saved_layout(&mut params, saved_layout);
                let covered: std::collections::HashSet<String> = params
                    .channel
                    .iter()
                    .cloned()
                    .chain(extra_channels.iter().map(|(n, _)| n.clone()))
                    .map(|n| n.to_lowercase())
                    .collect();
                extra_channels.extend(channel_setup_extra_channels(
                    irust_config::load_channel_setups(name),
                    |n| covered.contains(&n.to_lowercase()),
                ));
                let start_result = start_server(
                    name.clone(),
                    params,
                    false,
                    Arc::new(Mutex::new(app_config.clone())),
                    servers.clone(),
                    server_channels.clone(),
                    redraw_tx.clone(),
                    force_redraw_tx.clone(),
                    bell_tx.clone(),
                    quit_ack_tx.clone(),
                    plugin_host.clone(),
                    color_overrides.clone(),
                    ignores.clone(),
                    hilights.clone(),
                    autobans.clone(),
                    quit_messages.clone(),
                    dcc_sessions.clone(),
                    dcc_transfers.clone(),
                    dcc_resume_waiters.clone(),
                    extra_channels,
                    seed_banner.clone(),
                )
                .await;
                match start_result {
                    Ok(()) => {
                        if !layout_queries.is_empty() {
                            let mut guard = servers.lock().await;
                            if let Some(st) = guard.state_mut(name) {
                                for nick in &layout_queries {
                                    st.get_or_create_query_buffer(&nick.to_lowercase(), nick);
                                }
                            }
                        }
                    }
                    Err(err) => eprintln!("Could not connect to '{name}': {err}"),
                }
            }
            Err(err) => eprintln!("Skipping autoconnect network '{name}': {err}"),
        }
    }

    let app_config: Arc<Mutex<AppConfig>> = Arc::new(Mutex::new(app_config));

    {
        let servers = servers.clone();
        let server_channels = server_channels.clone();
        let redraw_tx = redraw_tx.clone();
        let force_redraw_tx = force_redraw_tx.clone();
        let bell_tx = bell_tx.clone();
        let quit_ack_tx = quit_ack_tx.clone();
        let restart_tx = restart_tx.clone();
        let plugin_host = plugin_host.clone();
        let app_config = app_config.clone();
        let color_overrides = color_overrides.clone();
        let aliases = aliases.clone();
        let completions = completions.clone();
        let ignores = ignores.clone();
        let hilights = hilights.clone();
        let quit_messages = quit_messages.clone();
        let keybinds = keybinds.clone();
        let dcc_sessions = dcc_sessions.clone();
        let dcc_transfers = dcc_transfers.clone();
        let dcc_resume_waiters = dcc_resume_waiters.clone();
        let settings = settings.clone();
        let statusbar = statusbar.clone();
        let outgoing_tx_for_eval = outgoing_tx.clone();
        let bell_tx_for_beep = bell_tx.clone();
        tokio::spawn(async move {
            while let Some(text) = outgoing_rx.recv().await {
                let (active_tag, action) = {
                    let guard = servers.lock().await;
                    // irssi's `-<tag>` server override on any command
                    // (`/join -libera #x`, `/msg -cyber nick hi`): strip it
                    // and route to that server instead of the active one.
                    let (tag_override, text) = split_server_tag(&text, guard.tags());
                    let dummy_state; // extends the fallback's lifetime to cover both branches below
                    let state_ref: &ClientState = match tag_override
                        .as_deref()
                        .and_then(|t| guard.state(t))
                        .or_else(|| guard.active_state())
                    {
                        Some(st) => st,
                        // Nothing connected yet -- only `/connect` makes
                        // sense here; anything else naturally parses to
                        // `Unknown` against a channel-less scratch state.
                        None => {
                            dummy_state =
                                ClientState::without_initial_channel("irust", irust_tui::color::len(), HashMap::new());
                            &dummy_state
                        }
                    };
                    let expanded = {
                        let aliases = aliases.lock().await;
                        expand_alias(&aliases, state_ref, &text, 0)
                    };
                    let action = irust_core::parse_input(state_ref, &expanded);
                    (tag_override.or_else(|| guard.active_tag().map(str::to_string)), action)
                };

                match action {
                    InputAction::Connect {
                        target: name,
                        tls,
                        ip_version,
                    } => {
                        if servers.lock().await.contains(&name) {
                            if let Some(tag) = &active_tag {
                                push_status(&servers, tag, format!("-- already connected to {name}")).await;
                            }
                        } else {
                            let params = { resolve_conn_params(&name, &*app_config.lock().await, tls, ip_version) };
                            match params {
                                Ok(params) => {
                                    let looking_up = format!("-- Looking up {}", params.host);
                                    match &active_tag {
                                        Some(tag) => push_status(&servers, tag, looking_up).await,
                                        None => {
                                            servers.lock().await.push_pending_message(false, looking_up);
                                            let _ = redraw_tx.send(());
                                        }
                                    }
                                    let resolved_ip = tokio::net::lookup_host((params.host.as_str(), params.port))
                                        .await
                                        .ok()
                                        .and_then(|mut addrs| addrs.next())
                                        .map(|addr| addr.ip().to_string())
                                        .unwrap_or_default();
                                    let connecting = format!(
                                        "-- Connecting to {} [{resolved_ip}] port {}",
                                        params.host, params.port
                                    );
                                    match &active_tag {
                                        Some(tag) => push_status(&servers, tag, connecting).await,
                                        None => {
                                            servers.lock().await.push_pending_message(false, connecting);
                                            let _ = redraw_tx.send(());
                                        }
                                    }

                                    let covered: std::collections::HashSet<String> =
                                        params.channel.iter().map(|n| n.to_lowercase()).collect();
                                    let extra_channels =
                                        channel_setup_extra_channels(irust_config::load_channel_setups(&name), |n| {
                                            covered.contains(&n.to_lowercase())
                                        });
                                    let host = params.host.clone();
                                    let port = params.port;
                                    let result = start_server(
                                        name.clone(),
                                        params,
                                        true,
                                        app_config.clone(),
                                        servers.clone(),
                                        server_channels.clone(),
                                        redraw_tx.clone(),
                                        force_redraw_tx.clone(),
                                        bell_tx.clone(),
                                        quit_ack_tx.clone(),
                                        plugin_host.clone(),
                                        color_overrides.clone(),
                                        ignores.clone(),
                                        hilights.clone(),
                                        autobans.clone(),
                                        quit_messages.clone(),
                                        dcc_sessions.clone(),
                                        dcc_transfers.clone(),
                                        dcc_resume_waiters.clone(),
                                        extra_channels,
                                        seed_banner.clone(),
                                    )
                                    .await;
                                    match result {
                                        Ok(()) => {
                                            if let Some(st) = servers.lock().await.state_mut(&name) {
                                                // irssi parity: "Connection to
                                                // <host> established"
                                                // (`TXT_CONNECTION_ESTABLISHED`,
                                                // `fe-server.c`'s
                                                // `sig_server_connected`).
                                                st.status.push_system(format!("-- Connection to {host} established"));
                                            }
                                        }
                                        Err(err) => {
                                            let msg = format!(
                                                "-- Unable to connect server {host} port {port} ({name}): {err}"
                                            );
                                            if let Some(tag) = &active_tag {
                                                push_status_warning(&servers, tag, msg).await;
                                            } else {
                                                servers.lock().await.push_pending_message(true, msg);
                                                let _ = redraw_tx.send(());
                                            }
                                        }
                                    }
                                }
                                Err(err) => {
                                    let msg = format!("-- {err}");
                                    if let Some(tag) = &active_tag {
                                        push_status_warning(&servers, tag, msg).await;
                                    } else {
                                        servers.lock().await.push_pending_message(true, msg);
                                        let _ = redraw_tx.send(());
                                    }
                                }
                            }
                        }
                    }
                    InputAction::Reconnect => {
                        if let Some(target) = active_tag.clone() {
                            let extra_channels: Vec<(String, Option<String>)> = servers
                                .lock()
                                .await
                                .state(&target)
                                .map(|st| {
                                    st.channels
                                        .values()
                                        .filter(|c| !c.left_by_user)
                                        .map(|c| (c.display_name.clone(), c.key.clone()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            let tx = server_channels.lock().await.remove(&target);
                            if let Some(tx) = tx {
                                let _ = tx.send(InputAction::Quit {
                                    reason: Some("reconnect".to_string()),
                                });
                            }
                            servers.lock().await.remove(&target);
                            let params = { resolve_conn_params(&target, &*app_config.lock().await, None, None) };
                            match params {
                                Ok(params) => {
                                    let result = start_server(
                                        target.clone(),
                                        params,
                                        true,
                                        app_config.clone(),
                                        servers.clone(),
                                        server_channels.clone(),
                                        redraw_tx.clone(),
                                        force_redraw_tx.clone(),
                                        bell_tx.clone(),
                                        quit_ack_tx.clone(),
                                        plugin_host.clone(),
                                        color_overrides.clone(),
                                        ignores.clone(),
                                        hilights.clone(),
                                        autobans.clone(),
                                        quit_messages.clone(),
                                        dcc_sessions.clone(),
                                        dcc_transfers.clone(),
                                        dcc_resume_waiters.clone(),
                                        extra_channels,
                                        seed_banner.clone(),
                                    )
                                    .await;
                                    if let Err(err) = result {
                                        eprintln!("Could not reconnect to '{target}': {err}");
                                    }
                                }
                                Err(err) => eprintln!("Could not reconnect to '{target}': {err}"),
                            }
                        }
                    }
                    InputAction::NetworkList => {
                        let mut names: Vec<String> =
                            irust_config::builtin_presets().into_iter().map(|p| p.id).collect();
                        names.extend(app_config.lock().await.networks.keys().cloned());
                        names.sort();
                        names.dedup();
                        let status = if names.is_empty() {
                            "-- no networks configured".to_string()
                        } else {
                            format!("-- networks: {}", names.join(", "))
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::NetworkAdd {
                        name,
                        target,
                        tls,
                        nick,
                        autojoin,
                        client_cert,
                        client_key,
                        extras,
                    } => {
                        let is_preset = irust_config::builtin_presets()
                            .into_iter()
                            .any(|p| p.id.eq_ignore_ascii_case(&name));
                        let status = if is_preset {
                            format!("-- '{name}' is a builtin network name, pick another")
                        } else {
                            let (host, port) = split_host_port(&target, 6697);
                            let mut cfg = app_config.lock().await;
                            cfg.networks.insert(
                                name.clone(),
                                irust_config::NetworkOverride {
                                    host: Some(host),
                                    port: Some(port),
                                    tls: Some(tls),
                                    nick,
                                    autojoin: autojoin.map(|c| vec![c]),
                                    client_cert,
                                    client_key,
                                    server_password: extras.password,
                                    autoconnect: extras.autoconnect,
                                    tls_fingerprint: extras.fingerprint,
                                    tls_verify: extras.tls_verify,
                                    sasl_account: extras.sasl.as_ref().map(|(a, _)| a.clone()),
                                    sasl_password: extras.sasl.map(|(_, p)| p),
                                    ..Default::default()
                                },
                            );
                            let _ = irust_config::save_config(&cfg);
                            format!("-- network {name} added -- /connect {name} to use it")
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::NetworkModify {
                        name,
                        target,
                        tls,
                        nick,
                        autojoin,
                        client_cert,
                        client_key,
                        extras,
                    } => {
                        let status = {
                            let mut cfg = app_config.lock().await;
                            match cfg.networks.get_mut(&name) {
                                None => format!("-- no such network: {name}"),
                                Some(entry) => {
                                    if let Some(t) = target {
                                        let (host, port) = split_host_port(&t, entry.port.unwrap_or(6697));
                                        entry.host = Some(host);
                                        entry.port = Some(port);
                                    }
                                    if let Some(tls) = tls {
                                        entry.tls = Some(tls);
                                    }
                                    if nick.is_some() {
                                        entry.nick = nick;
                                    }
                                    if let Some(a) = autojoin {
                                        entry.autojoin = Some(vec![a]);
                                    }
                                    if client_cert.is_some() {
                                        entry.client_cert = client_cert;
                                    }
                                    if client_key.is_some() {
                                        entry.client_key = client_key;
                                    }
                                    if extras.password.is_some() {
                                        entry.server_password = extras.password;
                                    }
                                    if extras.autoconnect.is_some() {
                                        entry.autoconnect = extras.autoconnect;
                                    }
                                    if extras.fingerprint.is_some() {
                                        entry.tls_fingerprint = extras.fingerprint;
                                    }
                                    if extras.tls_verify.is_some() {
                                        entry.tls_verify = extras.tls_verify;
                                    }
                                    if let Some((account, password)) = extras.sasl {
                                        entry.sasl_account = Some(account);
                                        entry.sasl_password = Some(password);
                                    }
                                    let _ = irust_config::save_config(&cfg);
                                    format!("-- network {name} modified")
                                }
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::NetworkRemove(name) => {
                        let status = {
                            let mut cfg = app_config.lock().await;
                            if cfg.networks.remove(&name).is_some() {
                                let _ = irust_config::save_config(&cfg);
                                format!("-- network {name} removed")
                            } else {
                                format!("-- no such network: {name}")
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::ChannelList => {
                        let all = irust_config::load_all_channel_setups();
                        let status = if all.is_empty() {
                            "-- no channels configured".to_string()
                        } else {
                            let mut entries = Vec::new();
                            for (network, channels) in &all {
                                for (channel, entry) in channels {
                                    let mut flags = Vec::new();
                                    flags.push(if entry.autojoin { "auto" } else { "noauto" }.to_string());
                                    if entry.password.is_some() {
                                        flags.push("keyed".to_string());
                                    }
                                    if !entry.botmasks.is_empty() {
                                        flags.push(format!("bots={}", entry.botmasks.join(",")));
                                    }
                                    entries.push(format!("{channel}@{network} ({})", flags.join(", ")));
                                }
                            }
                            format!("-- channels: {}", entries.join(", "))
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::ChannelRemove { channel, network } => {
                        let status = match irust_config::remove_channel_setup(&network, &channel) {
                            Ok(true) => format!("-- {channel} on {network}: removed"),
                            Ok(false) => format!("-- {channel} on {network}: not configured"),
                            Err(err) => format!("-- couldn't save channels.toml: {err}"),
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::ChannelSetup {
                        action,
                        channel,
                        network,
                        autojoin,
                        botmasks,
                        botcmd,
                        password,
                    } => {
                        let mut entry = match action {
                            irust_core::ChannelSetupAction::Add => irust_config::ChannelSetup::default(),
                            irust_core::ChannelSetupAction::Modify => {
                                irust_config::load_channel_setup(&network, &channel).unwrap_or_default()
                            }
                        };
                        if let Some(a) = autojoin {
                            entry.autojoin = a;
                        }
                        if let Some(b) = botmasks {
                            entry.botmasks = b;
                        }
                        if let Some(c) = botcmd {
                            entry.autosendcmd = c;
                        }
                        if let Some(p) = password {
                            entry.password = Some(p);
                        }
                        let status = match irust_config::save_channel_setup(&network, &channel, entry) {
                            Ok(()) => format!("-- {channel} on {network}: saved"),
                            Err(err) => format!("-- couldn't save channels.toml: {err}"),
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Disconnect(explicit) => {
                        if let Some(target) = explicit.or_else(|| active_tag.clone()) {
                            let (connected, is_last) = {
                                let guard = servers.lock().await;
                                (guard.contains(&target), guard.tags().len() <= 1)
                            };
                            if !connected {
                                if let Some(tag) = &active_tag {
                                    push_status_warning(&servers, tag, format!("-- not connected to {target}")).await;
                                }
                            } else if is_last {
                                push_status_warning(
                                    &servers,
                                    &target,
                                    "-- can't disconnect the only remaining server".to_string(),
                                )
                                .await;
                            } else {
                                let tx = server_channels.lock().await.remove(&target);
                                if let Some(tx) = tx {
                                    let _ = tx.send(InputAction::Quit {
                                        reason: Some("disconnect".to_string()),
                                    });
                                }
                                servers.lock().await.remove(&target);
                            }
                        }
                    }
                    InputAction::SwitchServer(name) => {
                        let ok = servers.lock().await.switch_active(&name);
                        if !ok {
                            if let Some(tag) = &active_tag {
                                push_status_warning(&servers, tag, format!("-- no such server: {name}")).await;
                            }
                        }
                    }
                    InputAction::Alias { name, command } => {
                        let status = match (name, command) {
                            (None, _) => {
                                let map = aliases.lock().await;
                                if map.is_empty() {
                                    "-- no aliases defined".to_string()
                                } else {
                                    let mut names: Vec<&String> = map.keys().collect();
                                    names.sort();
                                    format!(
                                        "-- aliases: {}",
                                        names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                                    )
                                }
                            }
                            (Some(name), None) => {
                                let map = aliases.lock().await;
                                match map.get(&name) {
                                    Some(cmd) => format!("-- {name} = {cmd}"),
                                    None => format!("-- no such alias: {name}"),
                                }
                            }
                            (Some(name), Some(command)) => {
                                let mut map = aliases.lock().await;
                                map.insert(name.clone(), command.clone());
                                let _ = irust_config::save_aliases(&map);
                                format!("-- alias {name} = {command}")
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Eval(text) => {
                        let cmds = {
                            let guard = servers.lock().await;
                            let dummy_state;
                            let state_ref: &ClientState = match guard.active_state() {
                                Some(st) => st,
                                None => {
                                    dummy_state = ClientState::without_initial_channel(
                                        "irust",
                                        irust_tui::color::len(),
                                        HashMap::new(),
                                    );
                                    &dummy_state
                                }
                            };
                            let tag = guard.active_tag().unwrap_or("").to_string();
                            let uptime = START_TIME.get().map(|t| t.elapsed()).unwrap_or_default();
                            let ctx = irust_core::EvalContext {
                                state: state_ref,
                                tag: &tag,
                                version: env!("CARGO_PKG_VERSION"),
                                uptime,
                            };
                            irust_core::eval_commands(&text, &ctx, "/", 0)
                        };
                        for cmd in cmds {
                            let _ = outgoing_tx_for_eval.send(cmd);
                        }
                    }
                    InputAction::Wait(ms) => {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                    }
                    InputAction::SwitchBuffer(name) => {
                        if let Some(st) = servers.lock().await.active_state_mut() {
                            match resolve_buffer_by_name(st, &name) {
                                Ok(id) => st.active_buffer = id,
                                Err(msg) => st.current_buffer_mut().push_system(msg),
                            }
                        }
                    }
                    InputAction::SwitchBufferByIndex(n) => {
                        let mut guard = servers.lock().await;
                        if !guard.goto_global_index(n) {
                            let msg = format!("-- no such window: {n}");
                            match guard.active_state_mut() {
                                Some(st) => st.current_buffer_mut().push_system(msg),
                                None => guard.push_pending_message(true, msg),
                            }
                        }
                    }
                    InputAction::ScrollbackHome => {
                        if let Some(st) = servers.lock().await.active_state_mut() {
                            let total = st.current_buffer().lines.len();
                            st.active_scroll_offset = total.saturating_sub(1);
                        }
                    }
                    InputAction::ScrollbackEnd => {
                        if let Some(st) = servers.lock().await.active_state_mut() {
                            st.active_scroll_offset = 0;
                        }
                    }
                    InputAction::ScrollbackGoto(target) => {
                        if let Some(st) = servers.lock().await.active_state_mut() {
                            let total = st.current_buffer().lines.len();
                            let max_offset = total.saturating_sub(1);
                            let new_offset = match target {
                                irust_core::input::ScrollbackGoto::Relative(n) if n >= 0 => {
                                    Some(st.active_scroll_offset.saturating_sub(n as usize))
                                }
                                irust_core::input::ScrollbackGoto::Relative(n) => {
                                    Some((st.active_scroll_offset + n.unsigned_abs() as usize).min(max_offset))
                                }
                                irust_core::input::ScrollbackGoto::Line(n) => {
                                    Some(total.saturating_sub(n).min(max_offset))
                                }
                                irust_core::input::ScrollbackGoto::Timestamp {
                                    date,
                                    hour,
                                    minute,
                                    second,
                                } => {
                                    irust_core::input::resolve_scrollback_timestamp(
                                        date,
                                        hour,
                                        minute,
                                        second,
                                        chrono::Local::now(),
                                    )
                                    .and_then(|epoch| st.current_buffer().first_index_at_or_after(epoch))
                                    .map(|front_index| total.saturating_sub(front_index + 1))
                                }
                            };
                            if let Some(offset) = new_offset {
                                st.active_scroll_offset = offset;
                            }
                        }
                    }
                    InputAction::ScrollbackStatus => {
                        let (tag_for_status, msg) = {
                            let guard = servers.lock().await;
                            let tag = guard.active_tag().map(str::to_string);
                            let msg = guard.active_state().map(|st| {
                                format!(
                                    "-- scrollback: {} lines in this window",
                                    st.current_buffer().lines.len()
                                )
                            });
                            (tag, msg)
                        };
                        if let (Some(tag), Some(msg)) = (tag_for_status, msg) {
                            push_status(&servers, &tag, msg).await;
                        }
                    }
                    InputAction::Foreach { scope, command } => {
                        let queued = {
                            let guard = servers.lock().await;
                            let mut out = Vec::new();
                            match scope {
                                irust_core::input::ForeachScope::Server => {
                                    let original = guard.active_tag().map(str::to_string);
                                    for t in guard.tags() {
                                        out.push(format!("/server {t}"));
                                        out.push(foreach_prefixed(&command));
                                    }
                                    if let Some(orig) = original {
                                        out.push(format!("/server {orig}"));
                                    }
                                }
                                scope => {
                                    if let Some(st) = guard.active_state() {
                                        let ids = st.buffer_ids();
                                        let original = ids.iter().position(|id| id == &st.active_buffer);
                                        for (i, id) in ids.iter().enumerate() {
                                            let matches = match scope {
                                                irust_core::input::ForeachScope::Window => true,
                                                irust_core::input::ForeachScope::Channel => {
                                                    matches!(id, BufferId::Channel(_))
                                                }
                                                irust_core::input::ForeachScope::Query => {
                                                    matches!(id, BufferId::Query(_))
                                                }
                                                irust_core::input::ForeachScope::Server => unreachable!(),
                                            };
                                            if matches {
                                                out.push(format!("/window {}", i + 1));
                                                out.push(foreach_prefixed(&command));
                                            }
                                        }
                                        if let Some(orig) = original {
                                            out.push(format!("/window {}", orig + 1));
                                        }
                                    }
                                }
                            }
                            out
                        };
                        for cmd in queued {
                            let _ = outgoing_tx_for_eval.send(cmd);
                        }
                    }
                    InputAction::Echo(text) => {
                        push_status_or_pending(&servers, active_tag.as_deref(), text).await;
                    }
                    InputAction::TopicSummary => {
                        let lines = servers
                            .lock()
                            .await
                            .active_state()
                            .map(|st| st.topic_summary_lines())
                            .unwrap_or_default();
                        for line in lines {
                            push_status_or_pending(&servers, active_tag.as_deref(), line).await;
                        }
                    }
                    InputAction::Toggle { key, value } => {
                        let status = {
                            let mut s = settings.lock().await;
                            match s.get(&key) {
                                None => format!("-- unknown setting: {key}"),
                                Some(current) if current != "true" && current != "false" => {
                                    format!("-- {key} is not a boolean setting")
                                }
                                Some(current) => {
                                    let new_value = match value.as_deref() {
                                        Some(v) if v.eq_ignore_ascii_case("on") => "true",
                                        Some(v) if v.eq_ignore_ascii_case("off") => "false",
                                        Some(v) if v.eq_ignore_ascii_case("toggle") => {
                                            if current == "true" {
                                                "false"
                                            } else {
                                                "true"
                                            }
                                        }
                                        None => {
                                            if current == "true" {
                                                "false"
                                            } else {
                                                "true"
                                            }
                                        }
                                        Some(other) => {
                                            let msg = format!(
                                                "-- invalid value '{other}' for '{key}' -- expected on/off/toggle"
                                            );
                                            if let Some(tag) = &active_tag {
                                                push_status_warning(&servers, tag, msg).await;
                                            }
                                            continue;
                                        }
                                    };
                                    match s.set(&key, new_value) {
                                        Ok(()) => {
                                            if irust_config::settings_net::bool_of("settings_autosave") {
                                                let _ = irust_config::save_settings(&s);
                                            }
                                            format!("-- {key} = {new_value}")
                                        }
                                        Err(err) => format!("-- {err}"),
                                    }
                                }
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Beep => {
                        let _ = bell_tx_for_beep.send(());
                    }
                    InputAction::Cat(path) => {
                        let expanded = expand_tilde(&path);
                        let status = match std::fs::read_to_string(&expanded) {
                            Ok(contents) => contents.lines().map(str::to_string).collect::<Vec<_>>(),
                            Err(err) => vec![format!("-- failed to read {path}: {err}")],
                        };
                        if let Some(tag) = &active_tag {
                            for line in status {
                                push_status(&servers, tag, line).await;
                            }
                        }
                    }
                    InputAction::Cd(path) => {
                        let expanded = expand_tilde(&path);
                        let status = match std::env::set_current_dir(&expanded) {
                            Ok(()) => format!(
                                "-- changed directory to {}",
                                std::env::current_dir()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or(expanded)
                            ),
                            Err(err) => format!("-- failed to chdir to {path}: {err}"),
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::StatusbarInfo(name) => {
                        let status = {
                            let sb = statusbar.lock().await;
                            match irust_config::statusbar::Item::parse(&name) {
                                None => format!("-- unknown statusbar item: {name}"),
                                Some(item) => match sb.entries().iter().position(|e| e.item == item) {
                                    None => format!("-- '{name}' is not in the statusbar"),
                                    Some(idx) => {
                                        let entry = &sb.entries()[idx];
                                        let side_pos = sb
                                            .entries()
                                            .iter()
                                            .take(idx + 1)
                                            .filter(|e| e.alignment == entry.alignment)
                                            .count();
                                        format!(
                                            "-- {}: alignment={}, position={} overall ({} on the {} side)",
                                            item.name(),
                                            entry.alignment.name(),
                                            idx + 1,
                                            side_pos,
                                            entry.alignment.name()
                                        )
                                    }
                                },
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Set { key, value } => {
                        let status = match (key, value) {
                            (None, _) => {
                                let s = settings.lock().await;
                                let pairs = s.list();
                                format!(
                                    "-- settings: {}",
                                    pairs
                                        .iter()
                                        .map(|(k, v)| format!("{k}={}", shown_setting_value(k, v)))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            }
                            (Some(key), None) => {
                                let s = settings.lock().await;
                                match s.get(&key) {
                                    Some(v) => format!("-- {key} = {}", shown_setting_value(&key, &v)),
                                    None => format!("-- unknown setting: {key}"),
                                }
                            }
                            (Some(key), Some(value)) => {
                                let mut s = settings.lock().await;
                                match s.set(&key, &value) {
                                    Ok(()) => {
                                        if irust_config::settings_net::bool_of("settings_autosave") {
                                            let _ = irust_config::save_settings(&s);
                                        }
                                        format!("-- {key} = {}", shown_setting_value(&key, &value))
                                    }
                                    Err(err) => format!("-- {err}"),
                                }
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Format { name, template, mode } => {
                        let status = match mode {
                            FormatMode::Delete | FormatMode::Reset => {
                                // Parsing already rejects a bare
                                // `-delete`/`-reset` with no name, so
                                // `name` is always `Some` here.
                                let name = name.expect("format -delete/-reset always has a name");
                                let known = irust_config::theme_formats::FORMAT_TABLE
                                    .iter()
                                    .find(|(n, ..)| *n == name);
                                match known {
                                    None => format!("-- unknown format: {name}"),
                                    Some((_, _, _, setter)) => {
                                        let theme_name = irust_config::settings::theme();
                                        let mut themes = irust_config::load_custom_themes();
                                        let entry = themes.entry(theme_name.clone()).or_default();
                                        *setter(entry) = if matches!(mode, FormatMode::Delete) {
                                            Some(String::new())
                                        } else {
                                            None
                                        };
                                        let entry_now = entry.clone();
                                        if matches!(mode, FormatMode::Reset)
                                            && entry_now == irust_config::custom_themes::CustomTheme::default()
                                        {
                                            themes.remove(&theme_name);
                                            let _ = irust_config::save_custom_themes(&themes);
                                            irust_config::theme_formats::remove_cached_theme(&theme_name);
                                        } else {
                                            let _ = irust_config::save_custom_themes(&themes);
                                            irust_config::theme_formats::update_cached_theme(theme_name, entry_now);
                                        }
                                        if matches!(mode, FormatMode::Delete) {
                                            format!("-- format {name} set to empty")
                                        } else {
                                            format!("-- format {name} reset to default")
                                        }
                                    }
                                }
                            }
                            FormatMode::GetOrSet => match (name, template) {
                                (None, _) => {
                                    let entries: Vec<String> = irust_config::theme_formats::FORMAT_TABLE
                                        .iter()
                                        .map(|(name, ..)| {
                                            let value =
                                                irust_config::theme_formats::resolve_by_name(name).unwrap_or_default();
                                            format!("{name}={value}")
                                        })
                                        .collect();
                                    format!("-- formats: {}", entries.join(", "))
                                }
                                (Some(name), None) => match irust_config::theme_formats::resolve_by_name(&name) {
                                    Some(value) => format!("-- {name} = {value}"),
                                    None => format!("-- unknown format: {name}"),
                                },
                                (Some(name), Some(template)) => {
                                    let known = irust_config::theme_formats::FORMAT_TABLE
                                        .iter()
                                        .find(|(n, ..)| *n == name);
                                    match known {
                                        None => format!("-- unknown format: {name}"),
                                        Some((_, _, _, setter)) => {
                                            let theme_name = irust_config::settings::theme();
                                            let mut themes = irust_config::load_custom_themes();
                                            let entry = themes.entry(theme_name.clone()).or_default();
                                            *setter(entry) = Some(template.clone());
                                            let entry = entry.clone();
                                            let _ = irust_config::save_custom_themes(&themes);
                                            irust_config::theme_formats::update_cached_theme(theme_name.clone(), entry);
                                            if irust_config::settings::THEME_NAMES.contains(&theme_name.as_str()) {
                                                format!(
                                                "-- format {name} set (now overriding built-in theme '{theme_name}')"
                                            )
                                            } else {
                                                format!("-- format {name} set")
                                            }
                                        }
                                    }
                                }
                            },
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Recode(sub) => {
                        // Both `Add`/`Remove`'s "no target given" case
                        // need the active buffer's own target -- resolved
                        // once here, shared by both arms below.
                        let default_target = || async {
                            let tag = active_tag.clone()?;
                            let guard = servers.lock().await;
                            let st = guard.state(&tag)?;
                            active_target(st)
                        };
                        let status = match sub {
                            RecodeSub::List => {
                                let map = irust_config::recode::load_recode();
                                if map.is_empty() {
                                    "-- no recode conversions defined".to_string()
                                } else {
                                    let mut keys: Vec<&String> = map.keys().collect();
                                    keys.sort();
                                    let entries: Vec<String> =
                                        keys.iter().map(|k| format!("{k} = {}", map[*k])).collect();
                                    format!("-- recode: {}", entries.join(", "))
                                }
                            }
                            RecodeSub::Add { target, charset } => {
                                let resolved = match target {
                                    Some(t) => Some(t),
                                    None => default_target().await,
                                };
                                match resolved {
                                    None => "-- /recode add: no target given and not on a channel or query".to_string(),
                                    Some(_) if !irust_config::recode::is_valid_charset(&charset) => {
                                        format!("-- invalid charset: {charset}")
                                    }
                                    Some(key) => {
                                        let mut conversions = irust_config::recode::load_recode();
                                        conversions.insert(key.clone(), charset.clone());
                                        let _ = irust_config::recode::save_recode(&conversions);
                                        irust_config::recode::update_cache_entry(key.clone(), charset.clone());
                                        format!("-- recode: {key} = {charset}")
                                    }
                                }
                            }
                            RecodeSub::Remove { target } => {
                                let resolved = match target {
                                    Some(t) => Some(t),
                                    None => default_target().await,
                                };
                                match resolved {
                                    None => {
                                        "-- /recode remove: no target given and not on a channel or query".to_string()
                                    }
                                    Some(key) => {
                                        let mut conversions = irust_config::recode::load_recode();
                                        if conversions.remove(&key).is_some() {
                                            let _ = irust_config::recode::save_recode(&conversions);
                                            irust_config::recode::remove_cache_entry(&key);
                                            format!("-- recode: removed {key}")
                                        } else {
                                            format!("-- recode: no conversion found for {key}")
                                        }
                                    }
                                }
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::StatusbarList => {
                        let sb = statusbar.lock().await;
                        let status = format!("-- statusbar: {}", sb.list_text());
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::StatusbarAdd {
                        item,
                        before,
                        after,
                        alignment,
                    } => {
                        let mut sb = statusbar.lock().await;
                        let status = match sb.add(&item, before.as_deref(), after.as_deref(), alignment.as_deref()) {
                            Ok(()) => {
                                let _ = irust_config::save_statusbar(&sb);
                                format!("-- statusbar: {}", sb.list_text())
                            }
                            Err(err) => format!("-- {err}"),
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::StatusbarRemove(item) => {
                        let mut sb = statusbar.lock().await;
                        let status = match sb.remove(&item) {
                            Ok(()) => {
                                let _ = irust_config::save_statusbar(&sb);
                                format!("-- statusbar: {}", sb.list_text())
                            }
                            Err(err) => format!("-- {err}"),
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::StatusbarReset => {
                        let mut sb = statusbar.lock().await;
                        sb.reset();
                        let _ = irust_config::save_statusbar(&sb);
                        let status = format!("-- statusbar: {}", sb.list_text());
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::LayoutList => {
                        let all = irust_config::load_all_layouts();
                        let mut lines: Vec<String> = if all.is_empty() {
                            vec!["-- layout: nothing saved yet -- /layout save to snapshot the current one".to_string()]
                        } else {
                            vec!["-- layout:".to_string()]
                        };
                        for (net_tag, layout) in &all {
                            let chans: Vec<&str> = layout.channels.iter().map(|c| c.name.as_str()).collect();
                            lines.push(format!(
                                "--   {net_tag}: {} channel(s), {} quer{}",
                                chans.len(),
                                layout.queries.len(),
                                if layout.queries.len() == 1 { "y" } else { "ies" }
                            ));
                            if !chans.is_empty() {
                                lines.push(format!("--     channels: {}", chans.join(", ")));
                            }
                            if !layout.queries.is_empty() {
                                lines.push(format!("--     queries: {}", layout.queries.join(", ")));
                            }
                        }
                        if let Some(tag) = &active_tag {
                            for line in lines {
                                push_status(&servers, tag, line).await;
                            }
                        }
                    }
                    InputAction::LayoutSave => {
                        let snapshot = snapshot_layouts(&*servers.lock().await);
                        let saved_count = snapshot.len();
                        for (net_tag, layout) in snapshot {
                            let _ = irust_config::save_layout(&net_tag, layout);
                        }
                        let status = if saved_count == 0 {
                            "-- layout: nothing connected to save".to_string()
                        } else {
                            format!("-- layout: saved ({saved_count} network(s)) -- run /layout list to see it")
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::LayoutReset => {
                        let _ = irust_config::reset_all_layouts();
                        let status = "-- layout: reset -- startup will use config.toml's autojoin again".to_string();
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Exec(command) => {
                        let output = tokio::process::Command::new("sh")
                            .arg("-c")
                            .arg(&command)
                            .output()
                            .await;
                        let mut lines: Vec<String> = match output {
                            Ok(out) => {
                                let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
                                    .lines()
                                    .map(|l| format!("-- {l}"))
                                    .collect();
                                lines.extend(
                                    String::from_utf8_lossy(&out.stderr)
                                        .lines()
                                        .map(|l| format!("-- stderr: {l}")),
                                );
                                if !out.status.success() {
                                    lines.push(format!("-- exec: {command} exited with {}", out.status));
                                }
                                lines
                            }
                            Err(err) => vec![format!("-- exec: couldn't run '{command}': {err}")],
                        };
                        if lines.is_empty() {
                            lines.push("-- exec: (no output)".to_string());
                        }
                        if let Some(tag) = &active_tag {
                            for line in lines {
                                push_status(&servers, tag, line).await;
                            }
                        }
                    }
                    InputAction::Unalias(name) => {
                        let removed = {
                            let mut map = aliases.lock().await;
                            let removed = map.remove(&name).is_some();
                            if removed {
                                let _ = irust_config::save_aliases(&map);
                            }
                            removed
                        };
                        if let Some(tag) = &active_tag {
                            let status = if removed {
                                format!("-- removed alias {name}")
                            } else {
                                format!("-- no such alias: {name}")
                            };
                            push_status(&servers, tag, status).await;
                        }
                    }
                    InputAction::Ignore {
                        mask,
                        levels,
                        is_regex,
                        is_except,
                        is_replies,
                        time_secs,
                    } => {
                        let expires_at = time_secs.map(|secs| {
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs() as i64)
                                .unwrap_or(0);
                            now + secs as i64
                        });
                        let status = match mask {
                            None => {
                                let list = ignores.lock().await;
                                if list.is_empty() {
                                    "-- no ignores defined".to_string()
                                } else {
                                    let entries: Vec<String> = list
                                        .iter()
                                        .enumerate()
                                        .map(|(i, e)| {
                                            let lv = if e.levels.is_empty() {
                                                "all".to_string()
                                            } else {
                                                e.levels.join(",")
                                            };
                                            let mut flags = String::new();
                                            if e.is_regex {
                                                flags.push_str("-regex ");
                                            }
                                            if e.is_except {
                                                flags.push_str("-except ");
                                            }
                                            if e.is_replies {
                                                flags.push_str("-replies ");
                                            }
                                            format!("{}: {flags}{} ({lv})", i + 1, e.mask)
                                        })
                                        .collect();
                                    format!("-- ignores: {}", entries.join(", "))
                                }
                            }
                            Some(mask) => {
                                let mut list = ignores.lock().await;
                                let lv_display = if levels.is_empty() {
                                    "all".to_string()
                                } else {
                                    levels.join(",")
                                };
                                let existing = list.iter_mut().find(|e| {
                                    e.is_regex == is_regex
                                        && if is_regex {
                                            e.mask == mask
                                        } else {
                                            e.mask.eq_ignore_ascii_case(&mask)
                                        }
                                });
                                match existing {
                                    Some(existing) => {
                                        existing.levels = levels;
                                        existing.is_except = is_except;
                                        existing.is_replies = is_replies;
                                        existing.expires_at = expires_at;
                                        let _ = irust_config::save_ignores(&list);
                                        format!("-- now ignoring {mask} ({lv_display})")
                                    }
                                    None => {
                                        list.push(irust_config::IgnoreEntry {
                                            mask: mask.clone(),
                                            levels,
                                            is_regex,
                                            is_except,
                                            is_replies,
                                            expires_at,
                                        });
                                        let _ = irust_config::save_ignores(&list);
                                        format!("-- now ignoring {mask} ({lv_display})")
                                    }
                                }
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Unignore(mask) => {
                        let status = {
                            let mut list = ignores.lock().await;
                            // Accept either the literal mask or its 1-based
                            // position from `/ignore`'s listing.
                            let idx = mask
                                .parse::<usize>()
                                .ok()
                                .filter(|&n| n >= 1 && n <= list.len())
                                .map(|n| n - 1)
                                .or_else(|| list.iter().position(|e| e.mask.eq_ignore_ascii_case(&mask)));
                            match idx {
                                Some(i) => {
                                    let removed = list.remove(i);
                                    let _ = irust_config::save_ignores(&list);
                                    format!("-- no longer ignoring {}", removed.mask)
                                }
                                None => format!("-- not ignoring {mask}"),
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Hilight {
                        pattern,
                        is_regex,
                        is_mask,
                        channels,
                        color,
                        actcolor,
                        priority,
                    } => {
                        let status = match pattern {
                            None => {
                                let list = hilights.lock().await;
                                if list.is_empty() {
                                    "-- no hilights defined".to_string()
                                } else {
                                    let entries: Vec<String> = list
                                        .iter()
                                        .enumerate()
                                        .map(|(i, e)| format!("{}: {}", i + 1, format_hilight_entry(e)))
                                        .collect();
                                    format!("-- hilights: {}", entries.join(", "))
                                }
                            }
                            Some(pattern) => {
                                let mut list = hilights.lock().await;
                                let existing = list.iter_mut().find(|e| {
                                    e.is_regex == is_regex
                                        && e.is_mask == is_mask
                                        && if is_regex {
                                            e.pattern == pattern
                                        } else {
                                            e.pattern.eq_ignore_ascii_case(&pattern)
                                        }
                                });
                                match existing {
                                    Some(existing) => {
                                        existing.channels = channels;
                                        if color.is_some() {
                                            existing.color = color;
                                        }
                                        if actcolor.is_some() {
                                            existing.actcolor = actcolor;
                                        }
                                        if let Some(priority) = priority {
                                            existing.priority = priority;
                                        }
                                        let display = format_hilight_entry(existing);
                                        let _ = irust_config::save_hilights(&list);
                                        format!("-- now hilighting {display}")
                                    }
                                    None => {
                                        let entry = irust_config::HilightEntry {
                                            pattern,
                                            is_regex,
                                            is_mask,
                                            channels,
                                            color,
                                            actcolor,
                                            priority: priority.unwrap_or(0),
                                        };
                                        list.push(entry.clone());
                                        let _ = irust_config::save_hilights(&list);
                                        format!("-- now hilighting {}", format_hilight_entry(&entry))
                                    }
                                }
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Dehilight(word) => {
                        let status = {
                            let mut list = hilights.lock().await;
                            // Same convention as `/unignore`: accept
                            // either the literal pattern or its 1-based
                            // position from `/hilight`'s listing.
                            let idx = word
                                .parse::<usize>()
                                .ok()
                                .filter(|&n| n >= 1 && n <= list.len())
                                .map(|n| n - 1)
                                .or_else(|| list.iter().position(|e| e.pattern.eq_ignore_ascii_case(&word)));
                            match idx {
                                Some(i) => {
                                    let removed = list.remove(i);
                                    let _ = irust_config::save_hilights(&list);
                                    format!("-- no longer hilighting {}", removed.pattern)
                                }
                                None => format!("-- not hilighting {word}"),
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Autoban { pattern, is_regex, reason } => {
                        let status = match pattern {
                            None => {
                                let list = autobans.lock().await;
                                if list.is_empty() {
                                    "-- no autoban patterns defined".to_string()
                                } else {
                                    let entries: Vec<String> = list
                                        .iter()
                                        .enumerate()
                                        .map(|(i, e)| {
                                            format!(
                                                "{}: {}{}{}",
                                                i + 1,
                                                e.pattern,
                                                if e.is_regex { " (regex)" } else { "" },
                                                e.reason.as_deref().map(|r| format!(" [{r}]")).unwrap_or_default()
                                            )
                                        })
                                        .collect();
                                    format!("-- autoban patterns: {}", entries.join(", "))
                                }
                            }
                            Some(pattern) => {
                                let mut list = autobans.lock().await;
                                // Same dedup-and-replace convention as
                                // `/hilight`: re-adding the same pattern
                                // updates its reason instead of a second entry.
                                let existing = list.iter_mut().find(|e| {
                                    e.is_regex == is_regex
                                        && if is_regex {
                                            e.pattern == pattern
                                        } else {
                                            e.pattern.eq_ignore_ascii_case(&pattern)
                                        }
                                });
                                match existing {
                                    Some(existing) => {
                                        if reason.is_some() {
                                            existing.reason = reason;
                                        }
                                        let display = existing.pattern.clone();
                                        let _ = irust_config::save_autoban(&list);
                                        format!("-- autoban pattern updated: {display}")
                                    }
                                    None => {
                                        let entry = irust_config::AutobanEntry { pattern, is_regex, reason };
                                        list.push(entry.clone());
                                        let _ = irust_config::save_autoban(&list);
                                        format!("-- now autobanning: {}", entry.pattern)
                                    }
                                }
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Unautoban(word) => {
                        let status = {
                            let mut list = autobans.lock().await;
                            let idx = word
                                .parse::<usize>()
                                .ok()
                                .filter(|&n| n >= 1 && n <= list.len())
                                .map(|n| n - 1)
                                .or_else(|| list.iter().position(|e| e.pattern.eq_ignore_ascii_case(&word)));
                            match idx {
                                Some(i) => {
                                    let removed = list.remove(i);
                                    let _ = irust_config::save_autoban(&list);
                                    format!("-- no longer autobanning: {}", removed.pattern)
                                }
                                None => format!("-- no autoban pattern matching {word}"),
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Uptime => {
                        if let Some(tag) = &active_tag {
                            let elapsed = START_TIME.get().map(|t| t.elapsed()).unwrap_or_default();
                            push_status(&servers, tag, format!("-- {}", format_uptime(elapsed))).await;
                        }
                    }
                    InputAction::ClientVersion => {
                        if let Some(tag) = &active_tag {
                            push_status(&servers, tag, format!("-- irust {}", env!("CARGO_PKG_VERSION"))).await;
                        }
                    }
                    InputAction::Completion { key, value, delete } => {
                        let status = if delete {
                            let key = key.unwrap_or_default();
                            let mut map = completions.lock().await;
                            let removed = map.remove(&key.to_lowercase()).is_some();
                            if removed {
                                let _ = irust_config::save_completions(&map);
                            }
                            if removed {
                                format!("-- removed completion {key}")
                            } else {
                                format!("-- no such completion: {key}")
                            }
                        } else {
                            match (key, value) {
                                (None, _) => {
                                    let map = completions.lock().await;
                                    if map.is_empty() {
                                        "-- no completions defined".to_string()
                                    } else {
                                        let mut names: Vec<&String> = map.keys().collect();
                                        names.sort();
                                        format!(
                                            "-- completions: {}",
                                            names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                                        )
                                    }
                                }
                                (Some(key), None) => {
                                    let map = completions.lock().await;
                                    match map.get(&key.to_lowercase()) {
                                        Some(v) => format!("-- {key} = {v}"),
                                        None => format!("-- no such completion: {key}"),
                                    }
                                }
                                (Some(key), Some(value)) => {
                                    let mut map = completions.lock().await;
                                    map.insert(key.to_lowercase(), value.clone());
                                    let _ = irust_config::save_completions(&map);
                                    format!("-- completion {key} = {value}")
                                }
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Bind {
                        key,
                        action,
                        delete,
                        reset,
                    } => {
                        let status = if reset {
                            let mut map = keybinds.lock().await;
                            map.clear();
                            let _ = irust_config::save_keybinds(&map);
                            "-- all keybinds reset to their defaults".to_string()
                        } else if delete {
                            let key = key.unwrap_or_default();
                            let mut map = keybinds.lock().await;
                            let removed = map.remove(&key).is_some();
                            if removed {
                                let _ = irust_config::save_keybinds(&map);
                                format!("-- {key} reset to its default")
                            } else {
                                format!("-- {key} has no override set")
                            }
                        } else {
                            match (key, action) {
                                (None, _) => {
                                    let map = keybinds.lock().await;
                                    if map.is_empty() {
                                        "-- no keybind overrides set".to_string()
                                    } else {
                                        let mut names: Vec<&String> = map.keys().collect();
                                        names.sort();
                                        let entries: Vec<String> =
                                            names.iter().map(|k| format!("{k} = {}", map[*k])).collect();
                                        format!("-- keybinds: {}", entries.join(", "))
                                    }
                                }
                                (Some(key), Some(action)) => {
                                    let mut map = keybinds.lock().await;
                                    map.insert(key.clone(), action.clone());
                                    let _ = irust_config::save_keybinds(&map);
                                    format!("-- {key} = {action}")
                                }
                                (Some(_), None) => "-- usage: /bind <key> <action>".to_string(),
                            }
                        };
                        push_status_or_pending(&servers, active_tag.as_deref(), status).await;
                    }
                    InputAction::Quit { reason } => {
                        let chans = server_channels.lock().await;
                        for tx in chans.values() {
                            let _ = tx.send(InputAction::Quit { reason: reason.clone() });
                        }
                    }
                    InputAction::Upgrade => {
                        let exe_before_update = updater::cached_exe().ok();
                        push_status_or_pending(
                            &servers,
                            active_tag.as_deref(),
                            "-- checking for updates...".to_string(),
                        )
                        .await;
                        match updater::check_and_update().await {
                            updater::UpdateOutcome::UpToDate(version) => {
                                push_status_or_pending(
                                    &servers,
                                    active_tag.as_deref(),
                                    format!("-- already up to date (v{version})"),
                                )
                                .await;
                            }
                            updater::UpdateOutcome::Updated { to } => {
                                push_status_or_pending(
                                    &servers,
                                    active_tag.as_deref(),
                                    format!("-- updated to v{to}, disconnecting and restarting..."),
                                )
                                .await;
                                // irssi parity: an in-place upgrade
                                // shouldn't drop every connected network
                                // back to config.toml's cold-start state
                                // -- see `restart_with_session_preserved`.
                                restart_with_session_preserved(
                                    &servers,
                                    &server_channels,
                                    &restart_tx,
                                    exe_before_update,
                                    format!("upgrading to v{to}"),
                                )
                                .await;
                            }
                            updater::UpdateOutcome::Failed(e) => {
                                push_status_or_pending(
                                    &servers,
                                    active_tag.as_deref(),
                                    format!("-- update failed: {e}"),
                                )
                                .await;
                            }
                        }
                    }
                    InputAction::Restart => {
                        let exe = updater::cached_exe().ok();
                        push_status_or_pending(
                            &servers,
                            active_tag.as_deref(),
                            "-- restarting...".to_string(),
                        )
                        .await;
                        restart_with_session_preserved(
                            &servers,
                            &server_channels,
                            &restart_tx,
                            exe,
                            "restarting".to_string(),
                        )
                        .await;
                    }
                    InputAction::RmReconns => {
                        let chans = server_channels.lock().await;
                        for tx in chans.values() {
                            let _ = tx.send(InputAction::RmReconns);
                        }
                    }
                    other => {
                        if let Some(tag) = &active_tag {
                            let chans = server_channels.lock().await;
                            if let Some(tx) = chans.get(tag) {
                                let _ = tx.send(other);
                            }
                        } else {
                            servers.lock().await.push_pending_message(
                                true,
                                "-- not connected to any server -- /connect <network> first".to_string(),
                            );
                        }
                    }
                }
                let _ = redraw_tx.send(());
            }
        });
    }

    // irssi-parity startup banner (see `irust_tui::app::no_server_buffer`):
    // shown only when nothing was configured to autoconnect, same as
    // irssi's own `servers == NULL && lookup_servers == NULL` check.
    let show_startup_banner = autoconnect.is_empty();
    irust_tui::run(
        servers,
        outgoing_tx,
        redraw_rx,
        force_redraw_rx,
        bell_rx,
        quit_ack_rx,
        restart_rx,
        completions,
        keybinds,
        show_startup_banner,
    )
    .await?;
    replay_watermark::flush();
    if RESTART_AFTER_QUIT.load(Ordering::SeqCst) {
        use std::os::unix::process::CommandExt;
        let exe = RESTART_EXE
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow::anyhow!("upgrade: no pre-update executable path was captured"))?;
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        let err = std::process::Command::new(&exe).args(&args).exec();
        anyhow::bail!("upgrade: re-exec of {} failed: {err}", exe.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nicks(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn test_conn_params(channel: Option<&str>) -> ConnParams {
        ConnParams {
            host: "irc.example.com".to_string(),
            port: 6697,
            nick: "abcX".to_string(),
            channel: channel.map(str::to_string),
            tls: true,
            starttls: false,
            verify_tls: true,
            tls_fingerprint: None,
            server_password: None,
            sasl_creds: None,
            client_auth: None,
            ip_version: None,
        }
    }

    /// The critical backward-compat case: a network no `/layout save`
    /// was ever run for must connect exactly as before -- whatever
    /// `resolve_conn_params` already put in `params.channel` from the
    /// static `autojoin` list stays untouched, and there's nothing
    /// extra to seed.
    #[test]
    fn apply_saved_layout_with_none_leaves_params_and_extras_untouched() {
        let mut params = test_conn_params(Some("#chatnet"));
        let (extra_channels, queries) = apply_saved_layout(&mut params, None);
        assert_eq!(params.channel, Some("#chatnet".to_string()));
        assert!(extra_channels.is_empty());
        assert!(queries.is_empty());
    }

    /// A layout saved while genuinely parked on Status (every channel
    /// parted on purpose before saving) must override a stale
    /// static `autojoin` too, not just add to it -- `params.channel`
    /// goes to `None` rather than keeping the old autojoin channel.
    #[test]
    fn apply_saved_layout_with_an_empty_layout_clears_autojoins_channel() {
        let mut params = test_conn_params(Some("#chatnet"));
        let layout = irust_config::NetworkLayout {
            channels: Vec::new(),
            queries: Vec::new(),
        };
        let (extra_channels, queries) = apply_saved_layout(&mut params, Some(layout));
        assert_eq!(params.channel, None);
        assert!(extra_channels.is_empty());
        assert!(queries.is_empty());
    }

    /// The first saved channel takes over `params.channel`'s single
    /// slot (replacing whatever static `autojoin` put there); every
    /// other saved channel (with its `+k` key where it had one) and
    /// every saved query nick come back as the extras `start_server`
    /// seeds via its own `extra_channels` mechanism.
    #[test]
    fn apply_saved_layout_replaces_channel_and_returns_the_rest_plus_queries() {
        let mut params = test_conn_params(Some("#stale-autojoin"));
        let layout = irust_config::NetworkLayout {
            channels: vec![
                irust_config::LayoutChannel {
                    name: "#chatnet".to_string(),
                    key: None,
                },
                irust_config::LayoutChannel {
                    name: "#routing".to_string(),
                    key: Some("secret".to_string()),
                },
            ],
            queries: vec!["dana".to_string()],
        };
        let (extra_channels, queries) = apply_saved_layout(&mut params, Some(layout));
        assert_eq!(params.channel, Some("#chatnet".to_string()));
        assert_eq!(
            extra_channels,
            vec![("#routing".to_string(), Some("secret".to_string()))]
        );
        assert_eq!(queries, vec!["dana".to_string()]);
    }

    fn channel_setup(autojoin: bool, password: Option<&str>) -> irust_config::ChannelSetup {
        irust_config::ChannelSetup {
            autojoin,
            password: password.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn channel_setup_extra_channels_is_empty_with_no_entries() {
        let extras = channel_setup_extra_channels(std::collections::BTreeMap::new(), |_| false);
        assert!(extras.is_empty());
    }

    #[test]
    fn channel_setup_extra_channels_includes_only_auto_entries_with_their_key() {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert("#chatnet".to_string(), channel_setup(true, Some("secret")));
        entries.insert("#lurking".to_string(), channel_setup(false, None));
        let mut extras = channel_setup_extra_channels(entries, |_| false);
        extras.sort();
        assert_eq!(extras, vec![("#chatnet".to_string(), Some("secret".to_string()))]);
    }

    #[test]
    fn channel_setup_extra_channels_skips_already_covered_channels() {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert("#chatnet".to_string(), channel_setup(true, None));
        let extras = channel_setup_extra_channels(entries, |n| n == "#chatnet");
        assert!(
            extras.is_empty(),
            "already covered by params.channel/layout -- must not duplicate the JOIN"
        );
    }

    #[test]
    fn format_uptime_matches_irssis_own_dhms_layout() {
        let elapsed = Duration::from_secs(2 * 86400 + 3 * 3600 + 4 * 60 + 5);
        assert_eq!(format_uptime(elapsed), "Uptime: 2d 3h 4m 5s");
    }

    #[test]
    fn ignore_level_for_a_plain_message_is_msgs() {
        assert_eq!(ignore_level_for("PRIVMSG", Some("just chatting")), "msgs");
    }

    #[test]
    fn ignore_level_for_a_notice_is_notices_regardless_of_content() {
        assert_eq!(ignore_level_for("NOTICE", Some("hello")), "notices");
    }

    #[test]
    fn ignore_level_for_a_real_ctcp_request_is_ctcps() {
        assert_eq!(ignore_level_for("PRIVMSG", Some("\u{1}VERSION\u{1}")), "ctcps");
    }

    #[test]
    fn ignore_level_for_a_ctcp_action_is_msgs_not_ctcps() {
        assert_eq!(ignore_level_for("PRIVMSG", Some("\u{1}ACTION waves\u{1}")), "msgs");
    }

    #[test]
    fn ignore_level_for_wallops_and_invite_are_their_own_levels() {
        assert_eq!(ignore_level_for("WALLOPS", Some("restarting soon")), "wallops");
        assert_eq!(ignore_level_for("INVITE", Some("#chatnet")), "invites");
    }

    fn prefix(name: &str, user: &str, host: &str) -> irust_proto::Prefix {
        irust_proto::Prefix {
            name: name.to_string(),
            user: Some(user.to_string()),
            host: Some(host.to_string()),
        }
    }

    /// `/ignore -regex` entries match against the full `nick!user@host`,
    /// via the `regex` crate rather than `matches_glob`.
    #[test]
    fn is_ignored_matches_a_regex_entry_against_the_full_hostmask() {
        let ignores = vec![irust_config::IgnoreEntry {
            mask: r"^Bad.*!.*@spammer\.example$".to_string(),
            levels: Vec::new(),
            is_regex: true,
            ..Default::default()
        }];
        assert!(irust_core::is_ignored(
            &ignores,
            &prefix("BadNick", "u", "spammer.example"),
            "msgs"
        ));
        assert!(!irust_core::is_ignored(
            &ignores,
            &prefix("GoodNick", "u", "spammer.example"),
            "msgs"
        ));
    }

    /// A regex entry still respects `levels` the same way a glob entry
    /// does -- it only suppresses this, not every level.
    #[test]
    fn is_ignored_regex_entry_still_respects_levels() {
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "^Bad".to_string(),
            levels: vec!["notices".to_string()],
            is_regex: true,
            ..Default::default()
        }];
        assert!(irust_core::is_ignored(
            &ignores,
            &prefix("BadNick", "u", "h"),
            "notices"
        ));
        assert!(!irust_core::is_ignored(&ignores, &prefix("BadNick", "u", "h"), "msgs"));
    }

    /// `-except` overrides a broader ignore that also matches -- a
    /// targeted carve-out, irssi's own "negates the ignore" (its
    /// `/ignore` help text, verbatim).
    #[test]
    fn is_ignored_except_entry_overrides_a_broader_match() {
        let ignores = vec![
            irust_config::IgnoreEntry {
                mask: "*!*@spammer.example".to_string(),
                ..Default::default()
            },
            irust_config::IgnoreEntry {
                mask: "gooduser!*@spammer.example".to_string(),
                is_except: true,
                ..Default::default()
            },
        ];
        assert!(
            irust_core::is_ignored(&ignores, &prefix("BadNick", "u", "spammer.example"), "msgs"),
            "not excepted -- the broader ignore still applies"
        );
        assert!(
            !irust_core::is_ignored(&ignores, &prefix("gooduser", "u", "spammer.example"), "msgs"),
            "excepted -- carved out of the broader ignore"
        );
    }

    /// An `-except` entry only carves out the LEVEL(S) it's actually
    /// scoped to -- same level-restriction rule every other entry has.
    #[test]
    fn is_ignored_except_entry_still_respects_its_own_levels() {
        let ignores = vec![
            irust_config::IgnoreEntry {
                mask: "BadNick".to_string(),
                ..Default::default()
            },
            irust_config::IgnoreEntry {
                mask: "BadNick".to_string(),
                is_except: true,
                levels: vec!["notices".to_string()],
                ..Default::default()
            },
        ];
        assert!(
            !irust_core::is_ignored(&ignores, &prefix("BadNick", "u", "h"), "notices"),
            "excepted at the notices level"
        );
        assert!(
            irust_core::is_ignored(&ignores, &prefix("BadNick", "u", "h"), "msgs"),
            "the except entry doesn't apply to msgs, so the broad ignore still does"
        );
    }

    /// `-replies` extends an ignore to a DIFFERENT sender's message when
    /// it opens by addressing an ignored nick -- irssi's own "also
    /// ignore nicknames who are talking to anyone who matches the
    /// ignore" (its `/ignore` help text, verbatim).
    #[test]
    fn is_ignored_reply_catches_a_message_addressing_an_ignored_nick() {
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "Troll".to_string(),
            is_replies: true,
            ..Default::default()
        }];
        let channel_nicks = vec!["Troll".to_string(), "Innocent".to_string()];
        assert!(irust_core::is_ignored_reply(
            &ignores,
            &channel_nicks,
            "Troll: nobody asked",
            "msgs"
        ));
        assert!(irust_core::is_ignored_reply(
            &ignores,
            &channel_nicks,
            "Troll, seriously?",
            "msgs"
        ));
        assert!(
            !irust_core::is_ignored_reply(&ignores, &channel_nicks, "just chatting about Troll", "msgs"),
            "must open by addressing the nick, not just mention it anywhere"
        );
        assert!(!irust_core::is_ignored_reply(
            &ignores,
            &channel_nicks,
            "Innocent: hi",
            "msgs"
        ));
    }

    /// A `-replies` entry only checks nicks actually present in the
    /// channel -- a mask matching nobody there never fires.
    #[test]
    fn is_ignored_reply_ignores_a_mask_matching_nobody_in_the_channel() {
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "Troll".to_string(),
            is_replies: true,
            ..Default::default()
        }];
        let channel_nicks = vec!["Innocent".to_string()];
        assert!(!irust_core::is_ignored_reply(
            &ignores,
            &channel_nicks,
            "Troll: hi",
            "msgs"
        ));
    }

    /// A regular (non-`-replies`) entry never triggers the reply check
    /// -- `-replies` must be explicitly set.
    #[test]
    fn is_ignored_reply_ignores_a_non_replies_entry() {
        let ignores = vec![irust_config::IgnoreEntry {
            mask: "Troll".to_string(),
            ..Default::default()
        }];
        let channel_nicks = vec!["Troll".to_string()];
        assert!(!irust_core::is_ignored_reply(
            &ignores,
            &channel_nicks,
            "Troll: hi",
            "msgs"
        ));
    }

    #[test]
    fn sweep_expired_ignores_removes_only_entries_past_their_expiry() {
        let mut list = vec![
            irust_config::IgnoreEntry {
                mask: "Expired".to_string(),
                expires_at: Some(100),
                ..Default::default()
            },
            irust_config::IgnoreEntry {
                mask: "StillGood".to_string(),
                expires_at: Some(200),
                ..Default::default()
            },
            irust_config::IgnoreEntry {
                mask: "Forever".to_string(),
                expires_at: None,
                ..Default::default()
            },
        ];
        let removed_something = sweep_expired_ignores(&mut list, 150);
        assert!(removed_something);
        assert_eq!(
            list.iter().map(|e| e.mask.as_str()).collect::<Vec<_>>(),
            vec!["StillGood", "Forever"]
        );
    }

    #[test]
    fn sweep_expired_ignores_is_a_no_op_when_nothing_expired() {
        let mut list = vec![irust_config::IgnoreEntry {
            mask: "StillGood".to_string(),
            expires_at: Some(200),
            ..Default::default()
        }];
        let removed_something = sweep_expired_ignores(&mut list, 100);
        assert!(!removed_something);
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn split_host_port_handles_plain_host_and_port() {
        assert_eq!(
            split_host_port("irc.example.org:6697", 6667),
            ("irc.example.org".to_string(), 6697)
        );
    }

    #[test]
    fn split_host_port_defaults_the_port_when_absent() {
        assert_eq!(
            split_host_port("irc.example.org", 6697),
            ("irc.example.org".to_string(), 6697)
        );
    }

    #[test]
    fn split_host_port_handles_bracketed_ipv6_with_port() {
        assert_eq!(split_host_port("[::1]:6697", 6667), ("::1".to_string(), 6697));
    }

    #[test]
    fn split_host_port_handles_bracketed_ipv6_without_port() {
        assert_eq!(
            split_host_port("[2001:db8::1]", 6697),
            ("2001:db8::1".to_string(), 6697)
        );
    }

    #[test]
    fn split_host_port_treats_a_bare_ipv6_literal_as_one_host() {
        // No brackets, more than one colon -- can't safely guess where a
        // port would start, so the whole thing is the host, default port.
        assert_eq!(split_host_port("::1", 6697), ("::1".to_string(), 6697));
    }

    #[test]
    fn split_for_wire_leaves_a_short_message_alone() {
        assert_eq!(split_for_wire("hey there"), vec!["hey there".to_string()]);
    }

    #[test]
    fn split_for_wire_breaks_a_long_paste_join_at_word_boundaries() {
        let long = "word ".repeat(150); // 750 chars, well past the limit
        let chunks = split_for_wire(&long);
        assert!(chunks.len() > 1, "should have split into more than one chunk");
        for chunk in &chunks {
            assert!(chunk.chars().count() <= WIRE_LINE_SPLIT_LIMIT);
            // No word split mid-way -- every chunk is whole "word"s.
            assert!(
                chunk.split(' ').all(|w| w == "word"),
                "chunk had a mangled word: {chunk:?}"
            );
        }
        // Nothing lost: rejoining reproduces the same words.
        let rejoined: String = chunks.join(" ");
        assert_eq!(rejoined.split(' ').count(), long.trim().split(' ').count());
    }

    #[test]
    fn split_for_wire_hard_splits_a_single_word_longer_than_the_limit() {
        let no_spaces = "a".repeat(1000);
        let chunks = split_for_wire(&no_spaces);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= WIRE_LINE_SPLIT_LIMIT);
        }
        assert_eq!(chunks.concat().chars().count(), 1000);
    }

    #[test]
    fn format_uptime_of_zero() {
        assert_eq!(format_uptime(Duration::from_secs(0)), "Uptime: 0d 0h 0m 0s");
    }

    #[test]
    fn resolve_buffer_by_name_matches_exactly_case_insensitively() {
        let mut st = ClientState::new("abcX", "#chatnet", 11);
        st.ensure_channel("#doghouse");
        let id = resolve_buffer_by_name(&st, "#CHATNET").unwrap();
        assert_eq!(id, BufferId::Channel("#chatnet".to_string()));
    }

    #[test]
    fn resolve_buffer_by_name_ignores_a_missing_leading_hash() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let id = resolve_buffer_by_name(&st, "chatnet").unwrap();
        assert_eq!(id, BufferId::Channel("#chatnet".to_string()));
    }

    #[test]
    fn resolve_buffer_by_name_falls_back_to_a_unique_prefix() {
        let mut st = ClientState::new("abcX", "#chatnet", 11);
        st.ensure_channel("#doghouse");
        let id = resolve_buffer_by_name(&st, "#dog").unwrap();
        assert_eq!(id, BufferId::Channel("#doghouse".to_string()));
    }

    #[test]
    fn resolve_buffer_by_name_matches_the_status_window_too() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let id = resolve_buffer_by_name(&st, "status").unwrap();
        assert_eq!(id, BufferId::Status);
    }

    #[test]
    fn resolve_buffer_by_name_reports_no_match() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        assert!(resolve_buffer_by_name(&st, "nonexistent").is_err());
    }

    #[test]
    fn resolve_buffer_by_name_reports_ambiguous_prefix() {
        let mut st = ClientState::new("abcX", "#chatnet", 11);
        st.ensure_channel("#chatworld");
        let err = resolve_buffer_by_name(&st, "#chat").unwrap_err();
        assert!(err.contains("ambiguous"), "got: {err}");
    }

    fn alias_map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn expand_alias_with_no_vars_appends_trailing_args_verbatim() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let aliases = alias_map(&[("j", "join")]);
        assert_eq!(expand_alias(&aliases, &st, "/j #other", 0), "/join #other");
    }

    #[test]
    fn expand_alias_substitutes_own_nick_and_active_channel() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let aliases = alias_map(&[("umode", "mode $N"), ("exemptlist", "mode $C +e")]);
        assert_eq!(expand_alias(&aliases, &st, "/umode", 0), "/mode abcX");
        assert_eq!(expand_alias(&aliases, &st, "/exemptlist", 0), "/mode #chatnet +e");
    }

    #[test]
    fn expand_alias_star_consumes_args_so_nothing_gets_double_appended() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let aliases = alias_map(&[("say", "msg *")]);
        assert_eq!(expand_alias(&aliases, &st, "/say hello there", 0), "/msg * hello there");
    }

    #[test]
    fn expand_alias_positional_args_also_suppress_the_trailing_append() {
        let st = ClientState::new("testnick", "#chatnet", 11);
        let aliases = alias_map(&[("kb", "kick $0 requested by $N")]);
        assert_eq!(
            expand_alias(&aliases, &st, "/kb baduser spamming", 0),
            "/kick baduser requested by testnick",
            "args consumed via $0 must not ALSO be appended verbatim"
        );
    }

    #[test]
    fn expand_alias_leaves_an_unknown_command_untouched() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let aliases = alias_map(&[("j", "join")]);
        assert_eq!(expand_alias(&aliases, &st, "/notanalias foo", 0), "/notanalias foo");
    }

    #[test]
    fn expand_alias_leaves_plain_text_untouched() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let aliases = alias_map(&[("j", "join")]);
        assert_eq!(expand_alias(&aliases, &st, "just chatting", 0), "just chatting");
    }

    #[test]
    fn expand_alias_chains_through_another_alias() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let aliases = alias_map(&[("bye", "quit"), ("signoff", "bye")]);
        assert_eq!(expand_alias(&aliases, &st, "/signoff see ya", 0), "/quit see ya");
    }

    #[test]
    fn expand_alias_does_not_loop_forever_on_a_cycle() {
        let st = ClientState::new("abcX", "#chatnet", 11);
        let aliases = alias_map(&[("a", "b"), ("b", "a")]);
        // Must terminate (the depth cap) rather than hang or overflow the
        // stack -- the exact resulting string doesn't matter here.
        let _ = expand_alias(&aliases, &st, "/a", 0);
    }

    #[test]
    fn a_short_uniform_mode_is_a_single_unchanged_chunk() {
        let chunks = chunk_channel_mode("+oo", &nicks(&["bob", "sarah"]));
        assert_eq!(chunks, vec![("+oo".to_string(), nicks(&["bob", "sarah"]))]);
    }

    #[test]
    fn more_than_four_nicks_splits_into_multiple_mode_lines() {
        let chunks = chunk_channel_mode("+oooooo", &nicks(&["a", "b", "c", "d", "e", "f"]));
        assert_eq!(
            chunks,
            vec![
                ("+oooo".to_string(), nicks(&["a", "b", "c", "d"])),
                ("+oo".to_string(), nicks(&["e", "f"])),
            ]
        );
    }

    #[test]
    fn mixed_signs_within_one_chunk_are_preserved() {
        let chunks = chunk_channel_mode("+o-v", &nicks(&["bob", "sarah"]));
        assert_eq!(chunks, vec![("+o-v".to_string(), nicks(&["bob", "sarah"]))]);
    }

    #[test]
    fn a_sign_change_mid_chunk_after_splitting_still_starts_with_the_right_sign() {
        // 6 alternating +o/-v pairs -- after splitting into groups of 4,
        // the second chunk's first letter must still carry its own
        // sign even though the split point falls mid-alternation.
        let modestring = "+o-v".repeat(3); // +o-v+o-v+o-v
        let chunks = chunk_channel_mode(&modestring, &nicks(&["a", "b", "c", "d", "e", "f"]));
        assert_eq!(chunks[0].0, "+o-v+o-v");
        assert_eq!(chunks[1].0, "+o-v");
    }

    #[test]
    fn no_args_sends_the_modestring_alone_unchanged() {
        let chunks = chunk_channel_mode("+i", &[]);
        assert_eq!(chunks, vec![("+i".to_string(), Vec::new())]);
    }

    #[test]
    fn mismatched_letter_and_arg_counts_falls_back_to_one_unchanged_line() {
        // "+io" has 2 letters but only 1 nick makes sense (i takes no
        // arg) -- can't safely chunk this, so it must pass through as-is.
        let chunks = chunk_channel_mode("+io", &nicks(&["bob"]));
        assert_eq!(chunks, vec![("+io".to_string(), nicks(&["bob"]))]);
    }
}
