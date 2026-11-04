
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use directories::{ProjectDirs, UserDirs};
use serde::{Deserialize, Serialize};

pub static BELL_ENABLED: AtomicBool = AtomicBool::new(true);
/// Same idea as `BELL_ENABLED`, read by `irust-tui`'s buffer-line
/// rendering to decide whether to draw the leading `[HH:MM:SS] `.
pub static SHOW_TIMESTAMPS: AtomicBool = AtomicBool::new(true);
pub static SHOW_JOINPART: AtomicBool = AtomicBool::new(true);
pub static SHOW_AWAY_ONCE: AtomicBool = AtomicBool::new(true);
pub static SHOW_QUIT_ONCE: AtomicBool = AtomicBool::new(false);
pub static AUTOBAN_ENABLED: AtomicBool = AtomicBool::new(false);
pub static WINDOW_HISTORY: AtomicBool = AtomicBool::new(false);
pub static DCC_AUTOGET: AtomicBool = AtomicBool::new(false);
/// `/set dcc_autoaccept_lowports on` -- irssi's own real setting
/// (default off): allows auto-accept even when the offer's port is
/// below 1024 (otherwise skipped, a privileged-port safety default).
pub static DCC_AUTOACCEPT_LOWPORTS: AtomicBool = AtomicBool::new(false);
pub static DCC_AUTORESUME: AtomicBool = AtomicBool::new(false);
pub static FLOOD_INTERVAL_MS: AtomicU64 = AtomicU64::new(DEFAULT_FLOOD_INTERVAL_MS);
pub static SCROLLBACK_LINES: AtomicU64 = AtomicU64::new(DEFAULT_SCROLLBACK_LINES);
pub static PASTE_GRACE_MS: AtomicU64 = AtomicU64::new(DEFAULT_PASTE_GRACE_MS);

static IDENT: OnceLock<RwLock<String>> = OnceLock::new();
static REALNAME: OnceLock<RwLock<String>> = OnceLock::new();
static DEFAULT_NICK: OnceLock<RwLock<String>> = OnceLock::new();
static THEME: OnceLock<RwLock<String>> = OnceLock::new();
static DCC_DOWNLOADS_DIR: OnceLock<RwLock<String>> = OnceLock::new();
static PART_MESSAGE: OnceLock<RwLock<String>> = OnceLock::new();
/// Same again, for `/set completion_char` -- read by `irust-tui`'s
/// tab-completion rendering, same reasoning `theme` already has for why
/// this crate can't just hand `irust-tui` a `Settings` reference (it's
/// the persistence layer, shared by everything, not just the TUI).
static COMPLETION_CHAR: OnceLock<RwLock<String>> = OnceLock::new();
static DCC_PORT_RANGE: OnceLock<RwLock<String>> = OnceLock::new();
/// Same again, for `/set dcc_own_ip <ip>` (irssi's own `dcc_own_ip`) --
/// empty means unset (every DCC offer announces the connection's own
/// local socket address, the original behavior; wrong behind NAT,
/// where that's a private LAN address the remote peer can't reach).
static DCC_OWN_IP: OnceLock<RwLock<String>> = OnceLock::new();
static CMDCHARS: OnceLock<RwLock<String>> = OnceLock::new();
static RECODE_OUT_DEFAULT_CHARSET: OnceLock<RwLock<String>> = OnceLock::new();
static AUTOLOG_IGNORE_TARGETS: OnceLock<RwLock<String>> = OnceLock::new();
static DCC_AUTOGET_MASKS: OnceLock<RwLock<String>> = OnceLock::new();
static DCC_AUTOGET_MAX_SIZE: OnceLock<RwLock<String>> = OnceLock::new();

pub fn ident() -> String {
    IDENT
        .get_or_init(|| RwLock::new(DEFAULT_IDENT.to_string()))
        .read()
        .unwrap()
        .clone()
}

pub fn realname() -> String {
    REALNAME
        .get_or_init(|| RwLock::new(DEFAULT_REALNAME.to_string()))
        .read()
        .unwrap()
        .clone()
}

/// Empty when no global default nick has been `/set`.
pub fn default_nick() -> String {
    DEFAULT_NICK
        .get_or_init(|| RwLock::new(String::new()))
        .read()
        .unwrap()
        .clone()
}

pub fn theme() -> String {
    THEME
        .get_or_init(|| RwLock::new(DEFAULT_THEME.to_string()))
        .read()
        .unwrap()
        .clone()
}

pub fn dcc_downloads_dir() -> String {
    DCC_DOWNLOADS_DIR
        .get_or_init(|| RwLock::new(default_dcc_downloads_dir()))
        .read()
        .unwrap()
        .clone()
}

pub fn part_message() -> String {
    PART_MESSAGE
        .get_or_init(|| RwLock::new(String::new()))
        .read()
        .unwrap()
        .clone()
}

pub fn completion_char() -> String {
    COMPLETION_CHAR
        .get_or_init(|| RwLock::new(DEFAULT_COMPLETION_CHAR.to_string()))
        .read()
        .unwrap()
        .clone()
}

/// Empty when no port range has been `/set`.
pub fn dcc_port_range() -> String {
    DCC_PORT_RANGE
        .get_or_init(|| RwLock::new(String::new()))
        .read()
        .unwrap()
        .clone()
}

/// Empty when no override IP has been `/set`.
pub fn dcc_own_ip() -> String {
    DCC_OWN_IP
        .get_or_init(|| RwLock::new(String::new()))
        .read()
        .unwrap()
        .clone()
}

pub fn cmdchars() -> String {
    CMDCHARS
        .get_or_init(|| RwLock::new(DEFAULT_CMDCHARS.to_string()))
        .read()
        .unwrap()
        .clone()
}

/// Empty when no global outgoing default charset has been `/set`.
pub fn recode_out_default_charset() -> String {
    RECODE_OUT_DEFAULT_CHARSET
        .get_or_init(|| RwLock::new(String::new()))
        .read()
        .unwrap()
        .clone()
}

/// Empty when no target is excluded from automatic logging.
pub fn autolog_ignore_targets() -> String {
    AUTOLOG_IGNORE_TARGETS
        .get_or_init(|| RwLock::new(String::new()))
        .read()
        .unwrap()
        .clone()
}

pub fn dcc_autoget() -> bool {
    DCC_AUTOGET.load(Ordering::Relaxed)
}

pub fn dcc_autoaccept_lowports() -> bool {
    DCC_AUTOACCEPT_LOWPORTS.load(Ordering::Relaxed)
}

pub fn dcc_autoresume() -> bool {
    DCC_AUTORESUME.load(Ordering::Relaxed)
}

/// Empty when no mask list has been `/set`.
pub fn dcc_autoget_masks() -> String {
    DCC_AUTOGET_MASKS
        .get_or_init(|| RwLock::new(String::new()))
        .read()
        .unwrap()
        .clone()
}

/// Plain byte count as a string; `"0"` (the default) means unlimited.
pub fn dcc_autoget_max_size() -> String {
    DCC_AUTOGET_MAX_SIZE
        .get_or_init(|| RwLock::new(DEFAULT_DCC_AUTOGET_MAX_SIZE.to_string()))
        .read()
        .unwrap()
        .clone()
}

fn set_ident_global(value: &str) {
    *IDENT
        .get_or_init(|| RwLock::new(DEFAULT_IDENT.to_string()))
        .write()
        .unwrap() = value.to_string();
}

fn set_realname_global(value: &str) {
    *REALNAME
        .get_or_init(|| RwLock::new(DEFAULT_REALNAME.to_string()))
        .write()
        .unwrap() = value.to_string();
}

fn set_default_nick_global(value: &str) {
    *DEFAULT_NICK.get_or_init(|| RwLock::new(String::new())).write().unwrap() = value.to_string();
}

fn set_theme_global(value: &str) {
    *THEME
        .get_or_init(|| RwLock::new(DEFAULT_THEME.to_string()))
        .write()
        .unwrap() = value.to_string();
}

fn set_dcc_downloads_dir_global(value: &str) {
    *DCC_DOWNLOADS_DIR
        .get_or_init(|| RwLock::new(default_dcc_downloads_dir()))
        .write()
        .unwrap() = value.to_string();
}

fn set_part_message_global(value: &str) {
    *PART_MESSAGE.get_or_init(|| RwLock::new(String::new())).write().unwrap() = value.to_string();
}

fn set_completion_char_global(value: &str) {
    *COMPLETION_CHAR
        .get_or_init(|| RwLock::new(DEFAULT_COMPLETION_CHAR.to_string()))
        .write()
        .unwrap() = value.to_string();
}

fn set_dcc_port_range_global(value: &str) {
    *DCC_PORT_RANGE
        .get_or_init(|| RwLock::new(String::new()))
        .write()
        .unwrap() = value.to_string();
}

fn set_dcc_own_ip_global(value: &str) {
    *DCC_OWN_IP.get_or_init(|| RwLock::new(String::new())).write().unwrap() = value.to_string();
}

fn set_cmdchars_global(value: &str) {
    *CMDCHARS
        .get_or_init(|| RwLock::new(DEFAULT_CMDCHARS.to_string()))
        .write()
        .unwrap() = value.to_string();
}

fn set_recode_out_default_charset_global(value: &str) {
    *RECODE_OUT_DEFAULT_CHARSET
        .get_or_init(|| RwLock::new(String::new()))
        .write()
        .unwrap() = value.to_string();
}

fn set_autolog_ignore_targets_global(value: &str) {
    *AUTOLOG_IGNORE_TARGETS
        .get_or_init(|| RwLock::new(String::new()))
        .write()
        .unwrap() = value.to_string();
}

fn set_dcc_autoget_masks_global(value: &str) {
    *DCC_AUTOGET_MASKS
        .get_or_init(|| RwLock::new(String::new()))
        .write()
        .unwrap() = value.to_string();
}

fn set_dcc_autoget_max_size_global(value: &str) {
    *DCC_AUTOGET_MAX_SIZE
        .get_or_init(|| RwLock::new(DEFAULT_DCC_AUTOGET_MAX_SIZE.to_string()))
        .write()
        .unwrap() = value.to_string();
}

const DEFAULT_IDENT: &str = "irust";
const DEFAULT_REALNAME: &str = "irust TUI";
/// See `FLOOD_INTERVAL_MS`'s own doc comment.
const DEFAULT_FLOOD_INTERVAL_MS: u64 = 600;
/// See `SCROLLBACK_LINES`'s own doc comment.
const DEFAULT_SCROLLBACK_LINES: u64 = 1000;
/// See `PASTE_GRACE_MS`'s own doc comment.
const DEFAULT_PASTE_GRACE_MS: u64 = 150;
/// "classic" is every color irust shipped with before `/set theme`
/// existed -- picking it as the default means a user who never touches
/// this setting sees zero visual change.
const DEFAULT_THEME: &str = "classic";
pub const THEME_NAMES: &[&str] = &["classic", "vivid", "mono"];
/// See `CMDCHARS`'s own doc comment. Matches irssi's own default.
const DEFAULT_CMDCHARS: &str = "/";
/// See `DCC_AUTOGET_MAX_SIZE`'s own doc comment. `"0"` means unlimited,
/// the same meaning irssi's own `"0k"` default has.
const DEFAULT_DCC_AUTOGET_MAX_SIZE: &str = "0";

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Settings {
    /// Server/proxy/ctcp/misc/dcc keys -- see `settings_net.rs`.
    #[serde(default)]
    pub net: crate::settings_net::NetSettings,
    #[serde(default = "default_true")]
    pub bell: bool,
    #[serde(default = "default_true")]
    pub show_timestamps: bool,
    /// `/set show_joinpart off` -- hides channel join/part lines from
    /// the buffer view entirely (irssi's own `/set show_quit_reason`-
    /// adjacent kind of noise control, but scoped to just joins/parts).
    #[serde(default = "default_true")]
    pub show_joinpart: bool,
    /// `/set show_away_once off` -- see `SHOW_AWAY_ONCE`'s own doc
    /// comment. Irssi's own real default is on.
    #[serde(default = "default_true")]
    pub show_away_once: bool,
    /// `/set show_quit_once on` -- see `SHOW_QUIT_ONCE`'s own doc
    /// comment. Irssi's own real default is off.
    #[serde(default)]
    pub show_quit_once: bool,
    /// `/set autoban on|off` -- see `AUTOBAN_ENABLED`'s own doc comment.
    /// Default OFF.
    #[serde(default)]
    pub autoban: bool,
    /// `/set window_history on` -- see `WINDOW_HISTORY`'s own doc
    /// comment. Irssi's own real default is off.
    #[serde(default)]
    pub window_history: bool,
    #[serde(default)]
    pub ident: String,
    /// `USER` command's realname/gecos field (shown in WHOIS). Global,
    /// same reasoning as `ident` above -- irssi's `/set realname`. Empty
    /// means "never configured", same as `ident`.
    #[serde(default)]
    pub realname: String,
    #[serde(default)]
    pub nick: String,
    /// `/set theme classic|vivid|mono` -- which color preset
    /// `irust-tui`'s rendering uses. See `THEME_NAMES` for the closed
    /// set of valid values.
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_dcc_downloads_dir")]
    pub dcc_downloads_dir: String,
    /// `/set part_message <text>` -- the reason sent when `/part` is
    /// typed with no reason of its own, irssi's own `part_message`
    /// setting. Empty (the default) means what it already meant before
    /// this setting existed: a bare `PART <channel>` with no reason at
    /// all.
    #[serde(default)]
    pub part_message: String,
    #[serde(default = "default_completion_char")]
    pub completion_char: String,
    #[serde(default = "default_flood_interval_ms")]
    pub flood_interval_ms: u64,
    /// `/set dcc_port_range <start>-<end>` -- irssi's own `dcc_port`.
    /// Empty (the default) means unset: every DCC listener binds to
    /// whatever port the OS hands out, the original behavior. Useful
    /// behind a firewall/NAT that only forwards a specific range.
    #[serde(default)]
    pub dcc_port_range: String,
    /// `/set dcc_own_ip <ip>` -- irssi's own `dcc_own_ip`. Empty (the
    /// default) means unset: every DCC offer announces the connection's
    /// own local socket address, the original behavior; wrong behind
    /// NAT, where that's a private LAN address a remote peer can't
    /// reach.
    #[serde(default)]
    pub dcc_own_ip: String,
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: u64,
    /// `/set paste_grace_ms <n>` -- see `PASTE_GRACE_MS`'s own doc
    /// comment. 150 (the default) is byte-identical to
    /// `RAW_PASTE_GRACE_WAIT`'s own tuned value, unchanged unless this
    /// is set to something else.
    #[serde(default = "default_paste_grace_ms")]
    pub paste_grace_ms: u64,
    /// `/set cmdchars <chars>` -- see `CMDCHARS`'s own doc comment.
    #[serde(default = "default_cmdchars")]
    pub cmdchars: String,
    /// `/set recode_out_default_charset <charset>` -- see
    /// `RECODE_OUT_DEFAULT_CHARSET`'s own doc comment. Empty (the
    /// default) means unset, matching irssi's own default.
    #[serde(default)]
    pub recode_out_default_charset: String,
    #[serde(default)]
    pub autolog_ignore_targets: String,
    /// `/set dcc_autoget on` -- see `DCC_AUTOGET`'s own doc comment.
    #[serde(default)]
    pub dcc_autoget: bool,
    /// `/set dcc_autoaccept_lowports on` -- see
    /// `DCC_AUTOACCEPT_LOWPORTS`'s own doc comment.
    #[serde(default)]
    pub dcc_autoaccept_lowports: bool,
    /// `/set dcc_autoresume on` -- see `DCC_AUTORESUME`'s own doc
    /// comment.
    #[serde(default)]
    pub dcc_autoresume: bool,
    /// `/set dcc_autoget_masks <masks>` -- see `DCC_AUTOGET_MASKS`'s own
    /// doc comment.
    #[serde(default)]
    pub dcc_autoget_masks: String,
    /// `/set dcc_autoget_max_size <bytes>` -- see
    /// `DCC_AUTOGET_MAX_SIZE`'s own doc comment.
    #[serde(default = "default_dcc_autoget_max_size")]
    pub dcc_autoget_max_size: String,
    /// Values of the table-driven completion/history/log/flood keys (see
    /// `settings_history_log`); only keys the user changed are stored.
    #[serde(default)]
    pub history_log: std::collections::BTreeMap<String, String>,
    /// Keys owned by `lookandfeel.rs` (only the ones changed from their
    /// default). Kept last: TOML needs plain values before tables.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub lookandfeel: std::collections::BTreeMap<String, String>,
}

fn default_true() -> bool {
    true
}

fn default_theme() -> String {
    DEFAULT_THEME.to_string()
}

fn default_dcc_downloads_dir() -> String {
    UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(|p| p.to_string_lossy().to_string()))
        .unwrap_or_else(|| ".".to_string())
}

const DEFAULT_COMPLETION_CHAR: &str = ":";

fn default_completion_char() -> String {
    DEFAULT_COMPLETION_CHAR.to_string()
}

fn default_flood_interval_ms() -> u64 {
    DEFAULT_FLOOD_INTERVAL_MS
}

fn default_scrollback_lines() -> u64 {
    DEFAULT_SCROLLBACK_LINES
}

fn default_paste_grace_ms() -> u64 {
    DEFAULT_PASTE_GRACE_MS
}

fn default_cmdchars() -> String {
    DEFAULT_CMDCHARS.to_string()
}

fn default_dcc_autoget_max_size() -> String {
    DEFAULT_DCC_AUTOGET_MAX_SIZE.to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            net: Default::default(),
            bell: true,
            show_timestamps: true,
            show_joinpart: true,
            show_away_once: true,
            show_quit_once: false,
            autoban: false,
            window_history: false,
            ident: String::new(),
            realname: String::new(),
            nick: String::new(),
            theme: default_theme(),
            dcc_downloads_dir: default_dcc_downloads_dir(),
            part_message: String::new(),
            completion_char: default_completion_char(),
            flood_interval_ms: default_flood_interval_ms(),
            dcc_port_range: String::new(),
            dcc_own_ip: String::new(),
            scrollback_lines: default_scrollback_lines(),
            paste_grace_ms: default_paste_grace_ms(),
            cmdchars: default_cmdchars(),
            recode_out_default_charset: String::new(),
            autolog_ignore_targets: String::new(),
            dcc_autoget: false,
            dcc_autoaccept_lowports: false,
            dcc_autoresume: false,
            dcc_autoget_masks: String::new(),
            dcc_autoget_max_size: default_dcc_autoget_max_size(),
            history_log: Default::default(),
            lookandfeel: Default::default(),
        }
    }
}

impl Settings {
    fn apply(&self) {
        self.net.publish();
        BELL_ENABLED.store(self.bell, Ordering::Relaxed);
        SHOW_TIMESTAMPS.store(self.show_timestamps, Ordering::Relaxed);
        SHOW_JOINPART.store(self.show_joinpart, Ordering::Relaxed);
        SHOW_AWAY_ONCE.store(self.show_away_once, Ordering::Relaxed);
        SHOW_QUIT_ONCE.store(self.show_quit_once, Ordering::Relaxed);
        AUTOBAN_ENABLED.store(self.autoban, Ordering::Relaxed);
        WINDOW_HISTORY.store(self.window_history, Ordering::Relaxed);
        set_ident_global(&self.ident);
        set_realname_global(&self.realname);
        set_default_nick_global(&self.nick);
        set_theme_global(&self.theme);
        set_dcc_downloads_dir_global(&self.dcc_downloads_dir);
        set_part_message_global(&self.part_message);
        set_completion_char_global(&self.completion_char);
        FLOOD_INTERVAL_MS.store(self.flood_interval_ms, Ordering::Relaxed);
        set_dcc_port_range_global(&self.dcc_port_range);
        set_dcc_own_ip_global(&self.dcc_own_ip);
        SCROLLBACK_LINES.store(self.scrollback_lines, Ordering::Relaxed);
        PASTE_GRACE_MS.store(self.paste_grace_ms, Ordering::Relaxed);
        set_cmdchars_global(&self.cmdchars);
        set_recode_out_default_charset_global(&self.recode_out_default_charset);
        set_autolog_ignore_targets_global(&self.autolog_ignore_targets);
        DCC_AUTOGET.store(self.dcc_autoget, Ordering::Relaxed);
        DCC_AUTOACCEPT_LOWPORTS.store(self.dcc_autoaccept_lowports, Ordering::Relaxed);
        DCC_AUTORESUME.store(self.dcc_autoresume, Ordering::Relaxed);
        set_dcc_autoget_masks_global(&self.dcc_autoget_masks);
        set_dcc_autoget_max_size_global(&self.dcc_autoget_max_size);
        crate::settings_history_log::publish(&self.history_log);
        crate::lookandfeel::publish(&self.lookandfeel);
    }

    /// `(key, current value)` for every known setting, sorted for a
    /// stable `/set` (no args) listing.
    pub fn list(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![
            ("autolog_ignore_targets", self.autolog_ignore_targets.clone()),
            ("bell", self.bell.to_string()),
            ("cmdchars", self.cmdchars.clone()),
            ("completion_char", self.completion_char.clone()),
            ("dcc_autoaccept_lowports", self.dcc_autoaccept_lowports.to_string()),
            ("dcc_autoget", self.dcc_autoget.to_string()),
            ("dcc_autoget_masks", self.dcc_autoget_masks.clone()),
            ("dcc_autoget_max_size", self.dcc_autoget_max_size.clone()),
            ("dcc_autoresume", self.dcc_autoresume.to_string()),
            ("dcc_downloads_dir", self.dcc_downloads_dir.clone()),
            ("dcc_own_ip", self.dcc_own_ip.clone()),
            ("dcc_port_range", self.dcc_port_range.clone()),
            ("flood_interval_ms", self.flood_interval_ms.to_string()),
            ("ident", self.ident.clone()),
            ("nick", self.nick.clone()),
            ("part_message", self.part_message.clone()),
            ("paste_grace_ms", self.paste_grace_ms.to_string()),
            ("realname", self.realname.clone()),
            ("recode_out_default_charset", self.recode_out_default_charset.clone()),
            ("scrollback_lines", self.scrollback_lines.to_string()),
            ("show_away_once", self.show_away_once.to_string()),
            ("show_joinpart", self.show_joinpart.to_string()),
            ("show_quit_once", self.show_quit_once.to_string()),
            ("autoban", self.autoban.to_string()),
            ("show_timestamps", self.show_timestamps.to_string()),
            ("theme", self.theme.clone()),
            ("window_history", self.window_history.to_string()),
        ];
        rows.extend(crate::settings_history_log::SPECS.iter().map(|spec| {
            (spec.key, self.history_log.get(spec.key).cloned().unwrap_or_else(|| spec.default.to_string()))
        }));
        rows.extend(crate::lookandfeel::DEFS.iter().map(|d| {
            (d.key, self.lookandfeel.get(d.key).cloned().unwrap_or_else(|| d.default.to_string()))
        }));
        rows.extend(self.net.list());
        rows
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let key = crate::settings_net::canonical_key(key);
        if let Some(v) = self.net.get(key) {
            return Some(v);
        }
        if let Some(spec) = crate::settings_history_log::spec_for(key) {
            return Some(self.history_log.get(spec.key).cloned().unwrap_or_else(|| spec.default.to_string()));
        }
        match key {
            "autolog_ignore_targets" => Some(self.autolog_ignore_targets.clone()),
            "bell" => Some(self.bell.to_string()),
            "cmdchars" => Some(self.cmdchars.clone()),
            "completion_char" => Some(self.completion_char.clone()),
            "dcc_autoaccept_lowports" => Some(self.dcc_autoaccept_lowports.to_string()),
            "dcc_autoget" => Some(self.dcc_autoget.to_string()),
            "dcc_autoget_masks" => Some(self.dcc_autoget_masks.clone()),
            "dcc_autoget_max_size" => Some(self.dcc_autoget_max_size.clone()),
            "dcc_autoresume" => Some(self.dcc_autoresume.to_string()),
            "dcc_downloads_dir" => Some(self.dcc_downloads_dir.clone()),
            "dcc_own_ip" => Some(self.dcc_own_ip.clone()),
            "dcc_port_range" => Some(self.dcc_port_range.clone()),
            "flood_interval_ms" => Some(self.flood_interval_ms.to_string()),
            "ident" => Some(self.ident.clone()),
            "nick" => Some(self.nick.clone()),
            "part_message" => Some(self.part_message.clone()),
            "paste_grace_ms" => Some(self.paste_grace_ms.to_string()),
            "realname" => Some(self.realname.clone()),
            "recode_out_default_charset" => Some(self.recode_out_default_charset.clone()),
            "scrollback_lines" => Some(self.scrollback_lines.to_string()),
            "show_away_once" => Some(self.show_away_once.to_string()),
            "show_joinpart" => Some(self.show_joinpart.to_string()),
            "show_quit_once" => Some(self.show_quit_once.to_string()),
            "autoban" => Some(self.autoban.to_string()),
            "show_timestamps" => Some(self.show_timestamps.to_string()),
            "theme" => Some(self.theme.clone()),
            "window_history" => Some(self.window_history.to_string()),
            k => crate::lookandfeel::DEFS.iter().find(|d| d.key == k).map(|d| {
                self.lookandfeel.get(k).cloned().unwrap_or_else(|| d.default.to_string())
            }),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) -> anyhow::Result<()> {
        let key = crate::settings_net::canonical_key(key);
        if self.net.set(key, value)? {
            self.apply();
            return Ok(());
        }
        if let Some(spec) = crate::settings_history_log::spec_for(key) {
            let normalized = crate::settings_history_log::validate(key, value)?;
            self.history_log.insert(spec.key.to_string(), normalized);
            self.apply();
            return Ok(());
        }
        match key {
            "ident" => {
                if value.is_empty() || value.contains(char::is_whitespace) {
                    anyhow::bail!("invalid ident '{value}' -- must be non-empty with no spaces");
                }
                self.ident = value.to_string();
            }
            "realname" => self.realname = value.to_string(),
            "nick" => {
                if value.is_empty() || value.contains(char::is_whitespace) {
                    anyhow::bail!("invalid nick '{value}' -- must be non-empty with no spaces");
                }
                self.nick = value.to_string();
            }
            // Empty is valid and meaningful here: a bare `PART` with no
            // reason at all, same as before this setting existed.
            "part_message" => self.part_message = value.to_string(),
            "completion_char" => {
                if value.is_empty() {
                    anyhow::bail!("invalid completion_char -- must be non-empty");
                }
                self.completion_char = value.to_string();
            }
            "cmdchars" => {
                if value.is_empty() || value.contains(char::is_whitespace) {
                    anyhow::bail!("invalid cmdchars '{value}' -- must be one or more non-space characters");
                }
                self.cmdchars = value.to_string();
            }
            // Empty is valid and meaningful here: "no global outgoing
            // default configured," same convention as `part_message`.
            "recode_out_default_charset" => {
                if !value.is_empty() && !crate::recode::is_valid_charset(value) {
                    anyhow::bail!("invalid recode_out_default_charset '{value}' -- not a recognized charset");
                }
                self.recode_out_default_charset = value.to_string();
            }
            // Empty is valid and meaningful here too: nothing excluded,
            // irust's original always-log-everything behavior.
            "autolog_ignore_targets" => self.autolog_ignore_targets = value.to_string(),
            "dcc_autoget_masks" => self.dcc_autoget_masks = value.to_string(),
            "dcc_autoget_max_size" => {
                let parsed: u64 = value.parse().map_err(|_| {
                    anyhow::anyhow!(
                        "invalid dcc_autoget_max_size '{value}' -- must be a non-negative integer (bytes, 0 = unlimited)"
                    )
                })?;
                self.dcc_autoget_max_size = parsed.to_string();
            }
            "dcc_downloads_dir" => {
                if value.trim().is_empty() {
                    anyhow::bail!("invalid dcc_downloads_dir -- must be non-empty");
                }
                self.dcc_downloads_dir = value.to_string();
            }
            "flood_interval_ms" => {
                let parsed: u64 = value.parse().map_err(|_| {
                    anyhow::anyhow!("invalid flood_interval_ms '{value}' -- must be a non-negative integer")
                })?;
                self.flood_interval_ms = parsed;
            }
            "dcc_port_range" => {
                let (start, end) = value
                    .split_once('-')
                    .and_then(|(a, b)| Some((a.parse::<u16>().ok()?, b.parse::<u16>().ok()?)))
                    .ok_or_else(|| {
                        anyhow::anyhow!("invalid dcc_port_range '{value}' -- expected <start>-<end>, e.g. 51000-51010")
                    })?;
                if start == 0 || start > end {
                    anyhow::bail!("invalid dcc_port_range '{value}' -- start must be nonzero and <= end");
                }
                self.dcc_port_range = format!("{start}-{end}");
            }
            "dcc_own_ip" => {
                if value.parse::<std::net::Ipv4Addr>().is_err() {
                    anyhow::bail!("invalid dcc_own_ip '{value}' -- must be a plain IPv4 address");
                }
                self.dcc_own_ip = value.to_string();
            }
            "scrollback_lines" => {
                let parsed: u64 = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid scrollback_lines '{value}' -- must be a positive integer"))?;
                if parsed == 0 {
                    anyhow::bail!("invalid scrollback_lines -- must be at least 1");
                }
                self.scrollback_lines = parsed;
            }
            "paste_grace_ms" => {
                let parsed: u64 = value.parse().map_err(|_| {
                    anyhow::anyhow!("invalid paste_grace_ms '{value}' -- must be a non-negative integer")
                })?;
                self.paste_grace_ms = parsed;
            }
            "theme" => {
                let normalized = value.to_lowercase();
                // Built-in first (can't be shadowed by a same-named
                // custom theme, see `custom_themes.rs`'s own doc
                // comment), then anything hand-defined in
                // `custom_themes.toml` -- irssi-parity "fully custom
                // `.theme` files".
                if !THEME_NAMES.contains(&normalized.as_str())
                    && !crate::custom_themes::custom_theme_names().contains(&normalized)
                {
                    anyhow::bail!(
                        "invalid theme '{value}' -- expected one of: {}, or a name defined in custom_themes.toml",
                        THEME_NAMES.join(", ")
                    );
                }
                self.theme = normalized;
            }
            "bell"
            | "show_timestamps"
            | "show_joinpart"
            | "show_away_once"
            | "show_quit_once"
            | "autoban"
            | "window_history"
            | "dcc_autoget"
            | "dcc_autoaccept_lowports"
            | "dcc_autoresume" => {
                let parsed = match value.to_lowercase().as_str() {
                    "on" | "true" | "yes" | "1" => true,
                    "off" | "false" | "no" | "0" => false,
                    _ => anyhow::bail!("invalid value '{value}' for '{key}' -- expected on/off"),
                };
                match key {
                    "bell" => self.bell = parsed,
                    "show_timestamps" => self.show_timestamps = parsed,
                    "show_joinpart" => self.show_joinpart = parsed,
                    "show_away_once" => self.show_away_once = parsed,
                    "show_quit_once" => self.show_quit_once = parsed,
                    "autoban" => self.autoban = parsed,
                    "window_history" => self.window_history = parsed,
                    "dcc_autoget" => self.dcc_autoget = parsed,
                    "dcc_autoaccept_lowports" => self.dcc_autoaccept_lowports = parsed,
                    "dcc_autoresume" => self.dcc_autoresume = parsed,
                    _ => unreachable!(),
                }
            }
            _ if crate::lookandfeel::is_key(key) => crate::lookandfeel::set(&mut self.lookandfeel, key, value)?,
            _ => anyhow::bail!("unknown setting '{key}'"),
        }
        self.apply();
        Ok(())
    }
}

fn settings_path() -> Option<std::path::PathBuf> {
    ProjectDirs::from("", "", "irust").map(|dirs| dirs.data_dir().join("settings.toml"))
}

pub fn load_settings() -> Settings {
    let settings: Settings = settings_path()
        .and_then(|path| fs::read_to_string(&path).ok().map(|contents| (path, contents)))
        .and_then(|(path, contents)| crate::warnings::parse_toml_or_warn(&contents, &path))
        .unwrap_or_default();
    settings.apply();
    settings
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    let Some(path) = settings_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let toml_str =
        toml::to_string_pretty(settings).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&path, toml_str)?;
    // It can hold `proxy_password`: owner-only.
    crate::perms::restrict_to_owner(&path);
    Ok(())
}

static USER_DEFAULTS_NOTICE: OnceLock<Vec<String>> = OnceLock::new();

fn fill_user_defaults(settings: &mut Settings) -> Vec<String> {
    let env_or = |var: &str, fallback: &dyn Fn() -> String| {
        std::env::var(var).ok().filter(|s| !s.is_empty()).unwrap_or_else(fallback)
    };

    let mut filled = Vec::new();
    if settings.ident.is_empty() {
        settings.ident = env_or("IRCUSER", &whoami::username);
        filled.push(format!("ident = {}", settings.ident));
    }
    if settings.realname.is_empty() {
        settings.realname = env_or("IRCNAME", &whoami::realname);
        filled.push(format!("realname = {}", settings.realname));
    }
    if settings.nick.is_empty() {
        let ident = settings.ident.clone();
        settings.nick = env_or("IRCNICK", &|| ident.clone());
        filled.push(format!("nick = {}", settings.nick));
    }
    filled
}

pub fn init_user_defaults(settings: &mut Settings) {
    let filled = fill_user_defaults(settings);
    if filled.is_empty() {
        return;
    }
    settings.apply();
    let _ = save_settings(settings);

    let mut lines = vec!["-- The following settings were initialized:".to_string()];
    lines.extend(filled.into_iter().map(|f| format!("--   {f}")));
    let _ = USER_DEFAULTS_NOTICE.set(lines);
}

/// The lines `init_user_defaults` recorded, if it filled anything in on
/// this run -- empty every later run, once `ident`/`realname`/`nick`
/// are all persisted and no longer empty.
pub fn user_defaults_notice() -> Vec<String> {
    USER_DEFAULTS_NOTICE.get().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_settings() -> std::sync::MutexGuard<'static, ()> {
        crate::lookandfeel::test_lock()
    }

    #[test]
    fn defaults_are_both_on() {
        let _guard = lock_settings();
        let s = Settings::default();
        assert!(s.bell);
        assert!(s.show_timestamps);
        assert!(s.show_joinpart);
    }

    #[test]
    fn empty_file_round_trips_to_defaults() {
        let _guard = lock_settings();
        let parsed: Settings = toml::from_str("").unwrap();
        assert_eq!(parsed, Settings::default());
    }

    #[test]
    fn set_parses_on_off_and_rejects_garbage() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.set("bell", "off").is_ok());
        assert!(!s.bell);
        assert!(s.set("bell", "on").is_ok());
        assert!(s.bell);
        assert!(s.set("bell", "sideways").is_err());
        assert!(s.set("nonexistent", "on").is_err());
    }

    #[test]
    fn show_joinpart_toggles_and_publishes_to_the_live_global() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.set("show_joinpart", "off").is_ok());
        assert!(!s.show_joinpart);
        assert!(!SHOW_JOINPART.load(Ordering::Relaxed));
        assert!(s.set("show_joinpart", "on").is_ok());
        assert!(SHOW_JOINPART.load(Ordering::Relaxed));
    }

    #[test]
    fn show_away_once_defaults_on_show_quit_once_defaults_off_both_toggle() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.show_away_once, "irssi's own real default is on");
        assert!(!s.show_quit_once, "irssi's own real default is off");
        assert!(s.set("show_away_once", "off").is_ok());
        assert!(!s.show_away_once);
        assert!(!SHOW_AWAY_ONCE.load(Ordering::Relaxed));
        assert!(s.set("show_quit_once", "on").is_ok());
        assert!(s.show_quit_once);
        assert!(SHOW_QUIT_ONCE.load(Ordering::Relaxed));
    }

    #[test]
    fn window_history_defaults_off_and_toggles() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(!s.window_history, "irssi's own real default is off");
        assert!(s.set("window_history", "on").is_ok());
        assert!(s.window_history);
        assert!(WINDOW_HISTORY.load(Ordering::Relaxed));
        assert!(s.set("window_history", "off").is_ok());
        assert!(!WINDOW_HISTORY.load(Ordering::Relaxed));
    }

    #[test]
    fn table_driven_keys_set_get_list_and_persist_as_a_string_map() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.get("max_command_history"), Some("100".to_string()));
        assert!(s.set("max_command_history", "250").is_ok());
        assert_eq!(s.get("max_command_history"), Some("250".to_string()));
        assert_eq!(crate::settings_history_log::get_int("max_command_history"), 250);
        assert!(s.list().contains(&("max_command_history", "250".to_string())));
        assert!(s.set("max_command_history", "lots").is_err());
        assert_eq!(s.get("max_command_history"), Some("250".to_string()), "a rejected value changes nothing");
        // Round trip through the persisted form.
        let text = toml::to_string(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        back.apply();
        assert_eq!(crate::settings_history_log::get_int("max_command_history"), 250);
        Settings::default().apply();
        assert_eq!(crate::settings_history_log::get_int("max_command_history"), 100, "defaults return on reset");
    }

    #[test]
    fn lookandfeel_overrides_survive_a_toml_round_trip_and_default_ones_are_not_written() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(!toml::to_string_pretty(&s).unwrap().contains("lookandfeel"));
        assert!(s.set("timestamp_format", "%H:%M").is_ok());
        assert_eq!(s.get("timestamp_format"), Some("%H:%M".to_string()));
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("[lookandfeel]"), "{text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back.lookandfeel.get("timestamp_format").map(String::as_str), Some("%H:%M"));
        assert!(s.set("timestamp_format", "%H:%M:%S").is_ok());
        assert!(s.lookandfeel.is_empty());
        s.apply();
    }

    #[test]
    fn get_and_list_reflect_current_values() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        s.show_timestamps = false;
        assert_eq!(s.get("show_timestamps"), Some("false".to_string()));
        assert_eq!(s.get("nonexistent"), None);
        // The table-driven keys (`settings_history_log`) are covered by
        // that module's own tests; this one pins the hand-written keys.
        let hand_written: Vec<_> = s
            .list()
            .into_iter()
            .filter(|(k, _)| crate::settings_history_log::spec_for(k).is_none() && !crate::lookandfeel::is_key(k) && !crate::settings_net::NetSettings::knows(k))
            .collect();
        assert_eq!(
            hand_written,
            vec![
                ("autolog_ignore_targets", String::new()),
                ("bell", "true".to_string()),
                ("cmdchars", "/".to_string()),
                ("completion_char", ":".to_string()),
                ("dcc_autoaccept_lowports", "false".to_string()),
                ("dcc_autoget", "false".to_string()),
                ("dcc_autoget_masks", String::new()),
                ("dcc_autoget_max_size", "0".to_string()),
                ("dcc_autoresume", "false".to_string()),
                ("dcc_downloads_dir", default_dcc_downloads_dir()),
                ("dcc_own_ip", String::new()),
                ("dcc_port_range", String::new()),
                ("flood_interval_ms", "600".to_string()),
                ("ident", String::new()),
                ("nick", String::new()),
                ("part_message", String::new()),
                ("paste_grace_ms", "150".to_string()),
                ("realname", String::new()),
                ("recode_out_default_charset", String::new()),
                ("scrollback_lines", "1000".to_string()),
                ("show_away_once", "true".to_string()),
                ("show_joinpart", "true".to_string()),
                ("show_quit_once", "false".to_string()),
                ("autoban", "false".to_string()),
                ("show_timestamps", "false".to_string()),
                ("theme", "classic".to_string()),
                ("window_history", "false".to_string()),
            ]
        );
    }

    #[test]
    fn nick_defaults_empty_sets_and_rejects_empty_or_spaces() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.nick, "");
        assert_eq!(default_nick(), "");
        assert!(s.set("nick", "abcX").is_ok());
        assert_eq!(s.nick, "abcX");
        assert_eq!(default_nick(), "abcX");
        assert!(s.set("nick", "").is_err());
        assert!(s.set("nick", "abc X").is_err());
    }

    #[test]
    fn flood_interval_ms_defaults_to_600_sets_and_rejects_non_numeric() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.flood_interval_ms, 600);
        assert_eq!(FLOOD_INTERVAL_MS.load(Ordering::Relaxed), 600);
        assert!(s.set("flood_interval_ms", "1200").is_ok());
        assert_eq!(s.flood_interval_ms, 1200);
        assert_eq!(FLOOD_INTERVAL_MS.load(Ordering::Relaxed), 1200);
        assert!(
            s.set("flood_interval_ms", "0").is_ok(),
            "0 (no pacing) is a valid, if risky, choice"
        );
        assert_eq!(s.flood_interval_ms, 0);
        assert!(s.set("flood_interval_ms", "fast").is_err());
        assert!(
            s.set("flood_interval_ms", "-5").is_err(),
            "negative makes no sense for a u64 duration"
        );
    }

    #[test]
    fn dcc_port_range_defaults_empty_sets_and_rejects_malformed_or_backwards() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.dcc_port_range, "");
        assert_eq!(dcc_port_range(), "");
        assert!(s.set("dcc_port_range", "51000-51010").is_ok());
        assert_eq!(s.dcc_port_range, "51000-51010");
        assert_eq!(dcc_port_range(), "51000-51010");
        assert!(
            s.set("dcc_port_range", "51010-51000").is_err(),
            "start must not exceed end"
        );
        assert!(
            s.set("dcc_port_range", "0-100").is_err(),
            "port 0 is not a real port to bind"
        );
        assert!(s.set("dcc_port_range", "51000").is_err(), "missing the -end half");
        assert!(s.set("dcc_port_range", "abc-def").is_err());
    }

    #[test]
    fn dcc_own_ip_defaults_empty_sets_and_rejects_non_ipv4() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.dcc_own_ip, "");
        assert_eq!(dcc_own_ip(), "");
        assert!(s.set("dcc_own_ip", "203.0.113.5").is_ok());
        assert_eq!(s.dcc_own_ip, "203.0.113.5");
        assert_eq!(dcc_own_ip(), "203.0.113.5");
        assert!(s.set("dcc_own_ip", "not-an-ip").is_err());
        assert!(
            s.set("dcc_own_ip", "::1").is_err(),
            "IPv6 not accepted -- DCC itself is IPv4-only in this codebase"
        );
    }

    #[test]
    fn scrollback_lines_defaults_to_1000_sets_and_rejects_zero_or_non_numeric() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.scrollback_lines, 1000);
        assert_eq!(SCROLLBACK_LINES.load(Ordering::Relaxed), 1000);
        assert!(s.set("scrollback_lines", "5000").is_ok());
        assert_eq!(s.scrollback_lines, 5000);
        assert_eq!(SCROLLBACK_LINES.load(Ordering::Relaxed), 5000);
        assert!(
            s.set("scrollback_lines", "0").is_err(),
            "a 0-capacity buffer makes no sense"
        );
        assert!(s.set("scrollback_lines", "lots").is_err());
    }

    #[test]
    fn paste_grace_ms_defaults_to_150_sets_and_rejects_non_numeric() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.paste_grace_ms, 150);
        assert_eq!(PASTE_GRACE_MS.load(Ordering::Relaxed), 150);
        assert!(s.set("paste_grace_ms", "300").is_ok());
        assert_eq!(s.paste_grace_ms, 300);
        assert_eq!(PASTE_GRACE_MS.load(Ordering::Relaxed), 300);
        assert!(
            s.set("paste_grace_ms", "0").is_ok(),
            "0 (no grace wait at all) is a valid, if risky, choice"
        );
        assert_eq!(s.paste_grace_ms, 0);
        assert!(s.set("paste_grace_ms", "fast").is_err());
        assert!(s.set("paste_grace_ms", "-5").is_err());
    }

    #[test]
    fn dcc_downloads_dir_defaults_and_set_and_rejects_empty() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.dcc_downloads_dir, default_dcc_downloads_dir());
        assert!(s.set("dcc_downloads_dir", "/tmp/irust-dcc").is_ok());
        assert_eq!(s.dcc_downloads_dir, "/tmp/irust-dcc");
        assert_eq!(dcc_downloads_dir(), "/tmp/irust-dcc");
        assert!(s.set("dcc_downloads_dir", "   ").is_err());
        assert_eq!(
            s.dcc_downloads_dir, "/tmp/irust-dcc",
            "a rejected value must not change the current dir"
        );
    }

    #[test]
    fn part_message_defaults_empty_and_accepts_empty_on_purpose() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.part_message, "");
        assert!(s.set("part_message", "bbl").is_ok());
        assert_eq!(s.part_message, "bbl");
        assert_eq!(part_message(), "bbl");
        // Setting it back to empty is a valid, meaningful choice (bare
        // PART with no reason), not an error.
        assert!(s.set("part_message", "").is_ok());
        assert_eq!(s.part_message, "");
    }

    #[test]
    fn completion_char_defaults_to_colon_and_rejects_empty() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.completion_char, ":");
        assert!(s.set("completion_char", ",").is_ok());
        assert_eq!(s.completion_char, ",");
        assert_eq!(completion_char(), ",");
        assert!(s.set("completion_char", "").is_err());
        assert_eq!(
            s.completion_char, ",",
            "a rejected value must not change the current char"
        );
    }

    #[test]
    fn cmdchars_defaults_to_slash() {
        let _guard = lock_settings();
        let s = Settings::default();
        assert_eq!(s.cmdchars, "/");
    }

    #[test]
    fn cmdchars_rejects_empty_and_whitespace() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.set("cmdchars", "").is_err());
        assert!(s.set("cmdchars", "/ .").is_err());
        assert_eq!(s.cmdchars, "/", "a rejected value must not change the current cmdchars");
    }

    #[test]
    fn cmdchars_accepts_multiple_characters() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.set("cmdchars", "/.").is_ok());
        assert_eq!(s.cmdchars, "/.");
        assert_eq!(cmdchars(), "/.");
    }

    #[test]
    fn recode_out_default_charset_defaults_to_empty() {
        let _guard = lock_settings();
        let s = Settings::default();
        assert_eq!(s.recode_out_default_charset, "");
    }

    #[test]
    fn recode_out_default_charset_rejects_an_unrecognized_charset() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.set("recode_out_default_charset", "not-a-real-charset").is_err());
        assert_eq!(
            s.recode_out_default_charset, "",
            "a rejected value must not change the current default"
        );
    }

    #[test]
    fn recode_out_default_charset_accepts_a_real_charset_and_its_aliases() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.set("recode_out_default_charset", "windows-1251").is_ok());
        assert_eq!(s.recode_out_default_charset, "windows-1251");
        assert_eq!(recode_out_default_charset(), "windows-1251");
        // Empty is a valid, meaningful choice too: back to "unset."
        assert!(s.set("recode_out_default_charset", "").is_ok());
        assert_eq!(s.recode_out_default_charset, "");
    }

    #[test]
    fn dcc_autoget_family_defaults_are_off_and_empty() {
        let _guard = lock_settings();
        let s = Settings::default();
        assert!(!s.dcc_autoget);
        assert!(!s.dcc_autoaccept_lowports);
        assert!(!s.dcc_autoresume);
        assert_eq!(s.dcc_autoget_masks, "");
        assert_eq!(s.dcc_autoget_max_size, "0");
    }

    #[test]
    fn dcc_autoget_toggles_on_off() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.set("dcc_autoget", "on").is_ok());
        assert!(s.dcc_autoget);
        assert!(dcc_autoget());
        assert!(s.set("dcc_autoaccept_lowports", "on").is_ok());
        assert!(s.dcc_autoaccept_lowports);
        assert!(dcc_autoaccept_lowports());
        assert!(s.set("dcc_autoresume", "on").is_ok());
        assert!(s.dcc_autoresume);
        assert!(dcc_autoresume());
        assert!(s.set("dcc_autoget", "off").is_ok());
        assert!(!s.dcc_autoget);
    }

    #[test]
    fn dcc_autoget_max_size_rejects_garbage() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s.set("dcc_autoget_max_size", "not-a-number").is_err());
        assert!(s.set("dcc_autoget_max_size", "-5").is_err());
        assert_eq!(
            s.dcc_autoget_max_size, "0",
            "a rejected value must not change the current size"
        );
        assert!(s.set("dcc_autoget_max_size", "1048576").is_ok());
        assert_eq!(s.dcc_autoget_max_size, "1048576");
        assert_eq!(dcc_autoget_max_size(), "1048576");
    }

    #[test]
    fn dcc_autoget_masks_accepts_empty_and_nonempty() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert!(s
            .set("dcc_autoget_masks", "*!*@trusted.example.com AnotherNick")
            .is_ok());
        assert_eq!(s.dcc_autoget_masks, "*!*@trusted.example.com AnotherNick");
        assert_eq!(dcc_autoget_masks(), "*!*@trusted.example.com AnotherNick");
        assert!(s.set("dcc_autoget_masks", "").is_ok());
        assert_eq!(s.dcc_autoget_masks, "");
    }

    #[test]
    fn ident_and_realname_set_and_get() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.get("ident"), Some(String::new()));
        assert_eq!(s.get("realname"), Some(String::new()));
        assert!(s.set("ident", "abcX").is_ok());
        assert_eq!(s.ident, "abcX");
        assert!(s.set("realname", "Alex Smith").is_ok());
        assert_eq!(s.realname, "Alex Smith");
        // ident can't contain whitespace -- it's the USER command's
        // first bare param, a space would break the wire protocol.
        assert!(s.set("ident", "has space").is_err());
    }

    #[test]
    fn fill_user_defaults_uses_env_overrides_when_empty() {
        let _guard = lock_settings();
        unsafe {
            std::env::set_var("IRCUSER", "envuser");
            std::env::set_var("IRCNAME", "Env Name");
            std::env::set_var("IRCNICK", "envnick");
        }
        let mut s = Settings::default();
        let filled = fill_user_defaults(&mut s);
        unsafe {
            std::env::remove_var("IRCUSER");
            std::env::remove_var("IRCNAME");
            std::env::remove_var("IRCNICK");
        }
        assert_eq!(s.ident, "envuser");
        assert_eq!(s.realname, "Env Name");
        assert_eq!(s.nick, "envnick");
        assert_eq!(
            filled,
            vec!["ident = envuser".to_string(), "realname = Env Name".to_string(), "nick = envnick".to_string()]
        );
    }

    #[test]
    fn fill_user_defaults_nick_falls_back_to_ident_not_env_when_ircnick_unset() {
        let _guard = lock_settings();
        unsafe {
            std::env::remove_var("IRCUSER");
            std::env::remove_var("IRCNICK");
            std::env::set_var("IRCNAME", "Whoever");
        }
        let mut s = Settings::default();
        fill_user_defaults(&mut s);
        unsafe {
            std::env::remove_var("IRCNAME");
        }
        assert_eq!(s.nick, s.ident);
        assert!(!s.nick.is_empty());
    }

    #[test]
    fn fill_user_defaults_never_overwrites_an_already_configured_value() {
        let _guard = lock_settings();
        unsafe {
            std::env::set_var("IRCUSER", "should-not-be-used");
        }
        let mut s = Settings { ident: "already-set".to_string(), ..Settings::default() };
        let filled = fill_user_defaults(&mut s);
        unsafe {
            std::env::remove_var("IRCUSER");
        }
        assert_eq!(s.ident, "already-set");
        assert!(!filled.iter().any(|f| f.starts_with("ident")));
    }

    #[test]
    fn theme_defaults_to_classic_accepts_known_names_rejects_unknown() {
        let _guard = lock_settings();
        let mut s = Settings::default();
        assert_eq!(s.theme, "classic");
        assert_eq!(s.get("theme"), Some("classic".to_string()));

        assert!(s.set("theme", "vivid").is_ok());
        assert_eq!(s.theme, "vivid");
        assert!(s.set("theme", "MONO").is_ok(), "should be case-insensitive");
        assert_eq!(s.theme, "mono");

        assert!(s.set("theme", "nonexistent").is_err());
        assert_eq!(s.theme, "mono", "a rejected value must not change the current theme");
    }
}
