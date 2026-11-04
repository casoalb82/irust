
use std::collections::{BTreeMap, HashMap};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Bool,
    /// Inclusive bounds.
    Int(i64, i64),
    Time,
    Size,
    Str,
}

pub struct Def {
    pub key: &'static str,
    pub kind: Kind,
    pub default: &'static str,
}

const fn d(key: &'static str, kind: Kind, default: &'static str) -> Def {
    Def { key, kind, default }
}

/// Every key here has an effect. Defaults are irssi's own.
pub static DEFS: &[Def] = &[
    // server
    d("alternate_nick", Kind::Str, ""),
    d("sasl_disconnect_on_failure", Kind::Bool, "on"),
    d("skip_motd", Kind::Bool, "off"),
    d("channels_rejoin_unavailable", Kind::Bool, "on"),
    d("server_connect_timeout", Kind::Time, "5min"),
    // irssi's own fixed delay between reconnect attempts -- see
    // `run_server`'s retry loop in `src/main.rs`, which reads this
    // live instead of the capped exponential backoff irust used to grow.
    d("server_reconnect_time", Kind::Time, "5min"),
    d("starttls_sts", Kind::Bool, "on"),
    d("hostname", Kind::Str, ""),
    d("usermode", Kind::Str, "+i"),
    d("lag_check_time", Kind::Time, "60s"),
    d("lag_max_before_disconnect", Kind::Time, "5min"),
    // proxy
    d("use_proxy", Kind::Bool, "off"),
    d("proxy_address", Kind::Str, ""),
    d("proxy_port", Kind::Int(1, 65535), "6667"),
    d("proxy_string", Kind::Str, "CONNECT %s %d"),
    d("proxy_string_after", Kind::Str, ""),
    d("proxy_password", Kind::Str, ""),
    // ctcp
    d("ctcp_version_reply", Kind::Str, "irust v$J - running on $sysname $sysarch"),
    // irssi defaults to "$Y" (realname). Empty here: USERINFO stays unanswered
    // unless the user opts in (existing privacy decision, see client.rs).
    d("ctcp_userinfo_reply", Kind::Str, ""),
    // misc
    d("auto_whowas", Kind::Bool, "on"),
    d("channel_sync", Kind::Bool, "on"),
    d("channel_max_who_sync", Kind::Int(0, i64::MAX), "1000"),
    d("ban_type", Kind::Str, "normal"),
    d("knockout_time", Kind::Time, "5min"),
    d("kick_first_on_kickban", Kind::Bool, "off"),
    d("netjoin_max_nicks", Kind::Int(0, i64::MAX), "10"),
    d("netsplit_max_nicks", Kind::Int(0, i64::MAX), "10"),
    d("netsplit_nicks_hide_threshold", Kind::Int(0, i64::MAX), "15"),
    d("max_wildcard_modes", Kind::Int(0, i64::MAX), "6"),
    d("split_line_start", Kind::Str, ""),
    d("split_line_end", Kind::Str, ""),
    d("split_line_on_space", Kind::Bool, "on"),
    d("join_auto_chans_on_invite", Kind::Bool, "on"),
    d("recode", Kind::Bool, "on"),
    d("recode_fallback", Kind::Str, "CP1252"),
    d("recode_autodetect_utf8", Kind::Bool, "on"),
    d("hide_netsplit_quits", Kind::Bool, "on"),
    d("notice_channel_context", Kind::Bool, "on"),
    // irssi default: on. Lets a bare `/1`, `/2`, ... jump to that
    // window by number, same as `/window <n>` already does -- see the
    // numeric check in `parse_input`'s final (service-nick/plugin)
    // fallback arm.
    d("window_number_commands", Kind::Bool, "on"),
    // irssi default: 1sec. Below this the statusbar/window-list lag
    // item shows nothing at all instead of "Lag: Nms" -- see
    // `irust-tui`'s `Item::Lag` rendering.
    d("lag_min_show", Kind::Time, "1sec"),
    // dcc
    d("dcc_autorename", Kind::Bool, "off"),
    d("dcc_file_create_mode", Kind::Int(0, 777), "644"),
    // irssi: 5min. irust's DCC listeners have always waited 2 minutes.
    d("dcc_timeout", Kind::Time, "2min"),
    d("dcc_upload_path", Kind::Str, "~"),
    d("dcc_send_replace_space_with_underscore", Kind::Bool, "off"),
    d("dcc_mirc_ctcp", Kind::Bool, "off"),
    d("settings_autosave", Kind::Bool, "on"),
    // irssi's own default is "": when non-empty, queues `MODE
    // <ournick> <this string>` right after the server confirms we
    // became an IRC operator (`381`) -- see `client.rs`'s handling of
    // that numeric.
    d("opermode", Kind::Str, ""),
];

fn def(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

/// irssi's own key names for settings irust already has under another
/// name -- `/set dcc_download_path` etc. land on the existing key.
pub fn canonical_key(key: &str) -> &str {
    match key {
        "dcc_download_path" => "dcc_downloads_dir",
        "dcc_port" => "dcc_port_range",
        "user_name" => "ident",
        "real_name" => "realname",
        other => other,
    }
}

/// irssi's `parse_on_off`-style boolean vocabulary.
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

pub fn parse_time_ms(text: &str) -> Option<u64> {
    let mut s = text.trim();
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let mut total: u64 = 0;
    loop {
        let digits_len = s.bytes().take_while(u8::is_ascii_digit).count();
        if digits_len == 0 {
            return None;
        }
        let number: u64 = s[..digits_len].parse().ok()?;
        s = &s[digits_len..];
        // Skip punctuation (irssi allows "1h, 30m").
        s = s.trim_start_matches(|c: char| c.is_ascii_punctuation() && c != '-');
        let unit_len = s.bytes().take_while(u8::is_ascii_alphabetic).count();
        let unit = s[..unit_len].to_lowercase();
        s = s[unit_len..].trim_start();
        if unit.is_empty() {
            if !s.is_empty() {
                return None;
            }
            return total.checked_add(number.checked_mul(1000)?);
        }
        let is_prefix = |full: &str| full.starts_with(unit.as_str());
        let ms = if is_prefix("days") {
            if number > 24 {
                return None;
            }
            number * 1000 * 3600 * 24
        } else if is_prefix("hours") {
            number * 1000 * 3600
        } else if is_prefix("minutes") || is_prefix("mins") {
            number * 1000 * 60
        } else if is_prefix("seconds") || is_prefix("secs") {
            number * 1000
        } else if is_prefix("milliseconds")
            || is_prefix("millisecs")
            || is_prefix("mseconds")
            || is_prefix("msecs")
        {
            number
        } else {
            return None;
        };
        total = total.checked_add(ms)?;
        s = s.trim_start_matches(|c: char| c.is_ascii_punctuation() && c != '-');
        if s.is_empty() {
            break;
        }
    }
    if total > (1u64 << 31) {
        return None;
    }
    Some(total)
}

/// irssi's `parse_size` (`src/core/misc.c`): a number with an optional
/// k/M/G/T suffix (1024-based); no suffix = bytes.
pub fn parse_size(text: &str) -> Option<u64> {
    let s = text.trim();
    let digits_len = s.bytes().take_while(u8::is_ascii_digit).count();
    if digits_len == 0 {
        return None;
    }
    let number: u64 = s[..digits_len].parse().ok()?;
    let mult: u64 = match s[digits_len..].trim().to_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1 << 10,
        "m" | "mb" | "mib" => 1 << 20,
        "g" | "gb" | "gib" => 1 << 30,
        "t" | "tb" | "tib" => 1 << 40,
        _ => return None,
    };
    number.checked_mul(mult)
}

fn validate(def: &Def, value: &str) -> anyhow::Result<String> {
    let key = def.key;
    match def.kind {
        Kind::Bool => match parse_bool(value) {
            Some(true) => Ok("on".to_string()),
            Some(false) => Ok("off".to_string()),
            None => anyhow::bail!("invalid value '{value}' for '{key}' -- expected on/off"),
        },
        Kind::Int(lo, hi) => {
            let n: i64 = value
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid value '{value}' for '{key}' -- expected a number"))?;
            if n < lo || n > hi {
                anyhow::bail!("invalid value '{value}' for '{key}' -- must be between {lo} and {hi}");
            }
            Ok(n.to_string())
        }
        Kind::Time => {
            if parse_time_ms(value).is_none() {
                anyhow::bail!("invalid value '{value}' for '{key}' -- expected a time like 30s, 5min, 1h");
            }
            Ok(value.trim().to_string())
        }
        Kind::Size => {
            if parse_size(value).is_none() {
                anyhow::bail!("invalid value '{value}' for '{key}' -- expected a size like 512k, 10M");
            }
            Ok(value.trim().to_string())
        }
        Kind::Str => Ok(value.to_string()),
    }
}

/// Per-key overrides as typed by the user; a key not present here means
/// "irssi's default". Persisted as a `[net]` table in settings.toml so
/// only what was changed is written.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct NetSettings {
    values: BTreeMap<String, String>,
}

impl NetSettings {
    pub fn knows(key: &str) -> bool {
        def(key).is_some()
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let def = def(key)?;
        Some(self.values.get(key).cloned().unwrap_or_else(|| def.default.to_string()))
    }

    /// `Ok(true)` if `key` belongs to this family and was applied,
    /// `Ok(false)` if it is not one of ours (caller keeps looking).
    pub fn set(&mut self, key: &str, value: &str) -> anyhow::Result<bool> {
        let Some(def) = def(key) else { return Ok(false) };
        let normalized = validate(def, value)?;
        if normalized == def.default {
            self.values.remove(key);
        } else {
            self.values.insert(key.to_string(), normalized);
        }
        self.publish();
        Ok(true)
    }

    pub fn list(&self) -> Vec<(&'static str, String)> {
        DEFS.iter()
            .map(|d| (d.key, self.values.get(d.key).cloned().unwrap_or_else(|| d.default.to_string())))
            .collect()
    }

    /// Makes the current values the live ones read by the typed getters.
    pub fn publish(&self) {
        let mut live = HashMap::new();
        for d in DEFS {
            if let Some(v) = self.values.get(d.key) {
                live.insert(d.key, v.clone());
            }
        }
        *live_map().write().unwrap() = live;
    }
}

fn live_map() -> &'static RwLock<HashMap<&'static str, String>> {
    static LIVE: OnceLock<RwLock<HashMap<&'static str, String>>> = OnceLock::new();
    LIVE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn live(key: &str) -> String {
    let def = def(key).unwrap_or_else(|| panic!("settings_net: unknown key {key}"));
    live_map()
        .read()
        .unwrap()
        .get(key)
        .cloned()
        .unwrap_or_else(|| def.default.to_string())
}

pub fn str_of(key: &str) -> String {
    live(key)
}

pub fn bool_of(key: &str) -> bool {
    parse_bool(&live(key)).unwrap_or(false)
}

pub fn int_of(key: &str) -> i64 {
    live(key).parse().unwrap_or(0)
}

/// Milliseconds; falls back to the default's value if the stored text
/// somehow stopped parsing (hand-edited settings.toml).
pub fn time_ms_of(key: &str) -> u64 {
    parse_time_ms(&live(key)).unwrap_or_else(|| parse_time_ms(def(key).map_or("0", |d| d.default)).unwrap_or(0))
}

pub fn size_of(key: &str) -> u64 {
    parse_size(&live(key)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_follows_irssi_rules() {
        assert_eq!(parse_time_ms("5min"), Some(300_000));
        assert_eq!(parse_time_ms("30"), Some(30_000), "bare number is seconds");
        assert_eq!(parse_time_ms("1h30m"), Some(5_400_000));
        assert_eq!(parse_time_ms("1day"), Some(86_400_000));
        assert_eq!(parse_time_ms("250msecs"), Some(250));
        assert_eq!(parse_time_ms("5 secs"), None, "irssi rejects a space between number and unit");
        assert_eq!(parse_time_ms("5m"), Some(300_000), "unit is a prefix match");
        assert_eq!(parse_time_ms("abc"), None);
        assert_eq!(parse_time_ms("5parsecs"), None);
        assert_eq!(parse_time_ms("-5s"), None);
        assert_eq!(parse_time_ms(""), None);
        assert_eq!(parse_time_ms("25days"), None, "irssi refuses > 24 days");
    }

    #[test]
    fn size_parses_suffixes() {
        assert_eq!(parse_size("0k"), Some(0));
        assert_eq!(parse_size("10M"), Some(10 << 20));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("1x"), None);
    }

    #[test]
    fn set_validates_by_kind_and_defaults_are_not_persisted() {
        let mut n = NetSettings::default();
        assert_eq!(n.get("skip_motd").as_deref(), Some("off"));
        assert!(n.set("skip_motd", "yes").unwrap());
        assert_eq!(n.get("skip_motd").as_deref(), Some("on"));
        assert!(n.set("skip_motd", "maybe").is_err());
        assert!(n.set("proxy_port", "70000").is_err());
        assert!(n.set("server_connect_timeout", "soon").is_err());
        assert!(n.set("server_connect_timeout", "30s").unwrap());
        assert_eq!(n.get("server_reconnect_time").as_deref(), Some("5min"));
        assert!(n.set("server_reconnect_time", "1min").unwrap());
        assert_eq!(n.get("server_reconnect_time").as_deref(), Some("1min"));
        assert_eq!(n.get("starttls_sts").as_deref(), Some("on"));
        // Setting back to the default drops the override.
        assert!(n.set("skip_motd", "off").unwrap());
        assert_eq!(n.values.get("skip_motd"), None);
        assert!(!n.set("not_ours", "x").unwrap());
    }

    #[test]
    fn every_key_has_a_valid_default() {
        for d in DEFS {
            validate(d, d.default).unwrap_or_else(|e| panic!("default of {} invalid: {e}", d.key));
        }
        let mut seen = std::collections::HashSet::new();
        assert!(DEFS.iter().all(|d| seen.insert(d.key)), "duplicate key in DEFS");
    }

    #[test]
    fn irssi_aliases_land_on_existing_keys() {
        assert_eq!(canonical_key("dcc_download_path"), "dcc_downloads_dir");
        assert_eq!(canonical_key("user_name"), "ident");
        assert_eq!(canonical_key("hostname"), "hostname");
    }
}
