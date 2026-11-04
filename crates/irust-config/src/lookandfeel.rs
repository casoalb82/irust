
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use crate::levels;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Str,
    /// irssi time interval: `"5min"`, `"1sec"`, `"250msecs"`, bare number = seconds.
    Time,
    /// irssi message-level list, see [`crate::levels`].
    Level,
}

pub struct Def {
    pub key: &'static str,
    pub kind: Kind,
    pub default: &'static str,
    /// Extra validation beyond the kind's own (e.g. strftime syntax).
    pub check: Option<fn(&str) -> anyhow::Result<()>>,
}

/// Every key this module owns. Keep sorted by key.
pub static DEFS: &[Def] = &[
    def("active_window_ignore_refnum", Kind::Bool, "true"),
    def("activity_hide_level", Kind::Level, ""),
    def("activity_hide_targets", Kind::Str, ""),
    def("activity_hide_visible", Kind::Bool, "true"),
    def("activity_hide_window_hidelevel", Kind::Bool, "true"),
    def("activity_hilight_level", Kind::Level, "MSGS DCCMSGS"),
    def("activity_msg_level", Kind::Level, "PUBLIC NOTICES"),
    // irssi default: off, Act shows window numbers only ("Act: 2,3").
    // On, each entry is prefixed "N:" with the item's display name
    // ("Act: 2:#chat,3:#other").
    def("actlist_names", Kind::Bool, "false"),
    // irssi's own default is off: the Act list shows the item's own
    // name (a channel/query), not a `/window name` custom override --
    // see `buflist.rs`'s own use of `BufferSummary::real_name`.
    def("actlist_prefer_window_name", Kind::Bool, "false"),
    // irssi's own default is ",". irust's Act item has always separated
    // names with a space, so that stays the default.
    def("actlist_separator", Kind::Str, " "),
    // irssi's own default is on: sending a `/msg`/`/query` to a nick
    // we don't have a query buffer for yet opens one. Off falls back
    // to echoing the sent line into the currently active buffer
    // instead -- same fallback the service-nick case next door
    // already uses when it has no existing query either.
    def("autocreate_own_query", Kind::Bool, "true"),
    // irssi's own default also has NOTICES (a private notice opens a
    // query); irust sends those to status. DCCMSGS is accepted for
    // compatibility but nothing here opens a query for a DCC chat.
    def("autocreate_query_level", Kind::Level, "MSGS DCCMSGS"),
    // Every channel/query already gets its own buffer unconditionally
    // (`ensure_channel`, `get_or_create_query_buffer`) -- accepted as a
    // no-op, irust's model has no "lazy" window creation to toggle off.
    def("autocreate_windows", Kind::Bool, "true"),
    // A fresh self-JOIN already becomes the active buffer unconditionally.
    // Accepted as a no-op for the same reason as `autocreate_windows`.
    def("autofocus_new_items", Kind::Bool, "true"),
    // irssi default: off. away-notify (always requested) already updates
    // the nicklist silently; this additionally prints a channel line.
    def("away_notify_public", Kind::Bool, "false"),
    def("beep_msg_level", Kind::Level, ""),
    def("beep_when_away", Kind::Bool, "true"),
    def("beep_when_window_active", Kind::Bool, "true"),
    // irssi's own default is off: `$M` (eval.rs) shows the full channel
    // modestring including arguments (key, limit, ...). On, only the
    // mode letters are kept.
    def("chanmode_expando_strip", Kind::Bool, "false"),
    def("colors", Kind::Bool, "false"),
    // irssi's own default is on; irust never emphasised, so it starts off.
    def("emphasis", Kind::Bool, "false"),
    def("emphasis_italics", Kind::Bool, "false"),
    def("emphasis_multiword", Kind::Bool, "false"),
    def("emphasis_replace", Kind::Bool, "false"),
    def("empty_kill_clears_cutbuffer", Kind::Bool, "false"),
    def("hide_colors", Kind::Bool, "false"),
    def("hide_text_style", Kind::Bool, "false"),
    // Colour of a highlighted line's nick and of a hilighted window in the
    // window list. Empty = irust's built-in (orange / bold yellow); irssi's
    // own defaults are "%Y" / "%M".
    Def { key: "hilight_act_color", kind: Kind::Str, default: "", check: Some(check_color) },
    Def { key: "hilight_color", kind: Kind::Str, default: "", check: Some(check_color) },
    // irssi's own default is "PUBLIC DCCMSGS"; irust's /hilight rules have
    // always applied to DMs (MSGS) as well.
    def("hilight_level", Kind::Level, "PUBLIC MSGS DCCMSGS"),
    def("hilight_nick_matches", Kind::Bool, "true"),
    // irssi's own default is off (nick only counts at the START of the
    // message). irust has always matched the nick anywhere as a whole word,
    // so that stays the default; `off` gives irssi's addressed-to-me rule.
    def("hilight_nick_matches_everywhere", Kind::Bool, "true"),
    // irssi default: on. A query open with a nick that renames now
    // follows them to the new key instead of staying orphaned under
    // the old one.
    def("query_track_nick_changes", Kind::Bool, "true"),
    // irssi's own default is off; irust always printed login/logout lines.
    def("show_account_notify", Kind::Bool, "true"),
    def("show_extended_join", Kind::Bool, "false"),
    def("show_nickmode", Kind::Bool, "true"),
    // irssi's own default is on (a space is printed for a plain member so
    // nicks line up); irust has always printed `<nick>` for them.
    def("show_nickmode_empty", Kind::Bool, "false"),
    def("show_own_nickchange_once", Kind::Bool, "false"),
    Def {
        key: "timestamp_format",
        kind: Kind::Str,
        // irssi's own default is "%H:%M"; irust has always shown seconds.
        default: "%H:%M:%S",
        check: Some(check_strftime),
    },
    def("timestamp_level", Kind::Level, "ALL"),
    def("timestamp_timeout", Kind::Time, "0"),
    def("whois_hide_safe_channel_id", Kind::Bool, "true"),
    // irssi's own default is off: a query auto-created by an incoming DM
    // (`autocreate_query_level`) does not steal focus. On, the first
    // time such a query is created it also becomes the active buffer --
    // see the call site in `client.rs`'s PRIVMSG handler.
    def("window_auto_change", Kind::Bool, "false"),
    // irssi's own default is "HIDDEN": a newly created window starts
    // with this as its hidden-level (lines matching it raise no
    // activity, see `activity_hide_window_hidelevel` above). No
    // per-window override yet (`/window hidelevel`, still missing).
    def("window_default_hidelevel", Kind::Level, "HIDDEN"),
    // Window numbering IS position (`goto_buffer_index`) in irust --
    // there is no persistent refnum/gap model to renumber. Accepted as
    // a no-op; already true by construction.
    def("windows_auto_renumber", Kind::Bool, "true"),
];

const fn def(key: &'static str, kind: Kind, default: &'static str) -> Def {
    Def { key, kind, default, check: None }
}

fn check_strftime(value: &str) -> anyhow::Result<()> {
    use chrono::format::{Item, StrftimeItems};
    if value.is_empty() {
        anyhow::bail!("timestamp_format cannot be empty (use /set show_timestamps off to hide timestamps)");
    }
    if StrftimeItems::new(value).any(|item| matches!(item, Item::Error)) {
        anyhow::bail!("invalid strftime format '{value}'");
    }
    Ok(())
}

/// Colour names irust accepts besides irssi's `%X` codes.
const COLOR_NAMES: &[&str] = &[
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "gray", "grey", "darkgray", "darkgrey",
    "lightred", "lightgreen", "lightyellow", "lightblue", "lightmagenta", "lightcyan", "white",
];

/// `true` for "" (built-in), an irssi format colour code (`%Y`, `%m`, ...),
/// a colour name, `#rrggbb` or a 0-255 palette index.
pub fn is_valid_color(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    if let Some(code) = t.strip_prefix('%') {
        return code.len() == 1 && "kKrRgGyYbBmMpPcCwW".contains(code);
    }
    if let Some(hex) = t.strip_prefix('#') {
        return hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    t.parse::<u8>().is_ok() || COLOR_NAMES.contains(&t.to_ascii_lowercase().replace(['_', '-'], "").as_str())
}

fn check_color(value: &str) -> anyhow::Result<()> {
    if is_valid_color(value) {
        Ok(())
    } else {
        anyhow::bail!("invalid color '{value}' -- use an irssi code like %Y, a name like red, #rrggbb or 0-255")
    }
}

fn find(key: &str) -> Option<&'static Def> {
    DEFS.iter().find(|d| d.key == key)
}

/// `true` if `key` is owned by this registry.
pub fn is_key(key: &str) -> bool {
    find(key).is_some()
}

static VALUES: OnceLock<RwLock<BTreeMap<String, String>>> = OnceLock::new();

fn values() -> &'static RwLock<BTreeMap<String, String>> {
    VALUES.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Replaces the live overrides with `overrides` (unknown keys dropped).
/// Called from `Settings::apply`.
pub fn publish(overrides: &BTreeMap<String, String>) {
    let kept: BTreeMap<String, String> =
        overrides.iter().filter(|(k, _)| is_key(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    *values().write().unwrap() = kept;
}

thread_local! {
    /// Test-only per-thread override (`with_test_overrides`); always `None`
    /// in production, so `get` reads the global map there.
    static LOCAL: std::cell::RefCell<Option<BTreeMap<String, String>>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `pairs` set for this thread only (every other thread, and
/// the global map, are untouched), so tests of settings-dependent code can
/// run in parallel with tests that assume the defaults.
#[doc(hidden)]
pub fn with_test_overrides<T>(pairs: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        set(&mut map, key, value).unwrap_or_else(|e| panic!("test override {key}={value}: {e}"));
    }
    struct Restore(Option<BTreeMap<String, String>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            LOCAL.with(|l| *l.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(LOCAL.with(|l| l.borrow_mut().replace(map)));
    f()
}

/// Raw text value of `key` (override, else default). Panics on a key that
/// is not in [`DEFS`] -- a call-site typo, caught by the tests.
pub fn get(key: &str) -> String {
    let def = find(key).unwrap_or_else(|| panic!("lookandfeel key '{key}' is not registered"));
    let local = LOCAL.with(|l| l.borrow().as_ref().map(|m| m.get(key).cloned()));
    let value = match local {
        Some(v) => v,
        None => values().read().unwrap().get(key).cloned(),
    };
    value.unwrap_or_else(|| def.default.to_string())
}

pub fn get_bool(key: &str) -> bool {
    parse_bool(&get(key)).unwrap_or(false)
}

pub fn get_int(key: &str) -> i64 {
    get(key).trim().parse().unwrap_or(0)
}

/// A `Time` key in milliseconds.
pub fn get_time_ms(key: &str) -> i64 {
    parse_time_ms(&get(key)).unwrap_or(0)
}

/// A `Level` key as a bitmask.
pub fn get_level(key: &str) -> u32 {
    levels::level2bits(&get(key))
}

/// `/set timestamp_format`, the strftime pattern for line timestamps.
pub fn timestamp_format() -> String {
    get("timestamp_format")
}

pub fn parse_bool(value: &str) -> Option<bool> {
    match value.to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// irssi's `parse_time_interval`: one or more `<number><unit>` terms
/// (`"1h 30min"`), a bare number is seconds, units are matched by prefix in
/// irssi's order (days, hours, minutes/mins, seconds/secs, milliseconds/
/// millisecs/mseconds/msecs). Returns milliseconds.
pub fn parse_time_ms(text: &str) -> Option<i64> {
    let bytes = text.trim();
    let (negative, rest) = match bytes.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, bytes.strip_prefix('+').unwrap_or(bytes)),
    };
    let mut chars = rest.trim_start().chars().peekable();
    let mut total: i64 = 0;
    loop {
        let mut digits = String::new();
        while let Some(c) = chars.peek().copied().filter(|c| c.is_ascii_digit()) {
            digits.push(c);
            chars.next();
        }
        if digits.is_empty() {
            return None;
        }
        let number: i64 = digits.parse().ok()?;
        while chars.peek().is_some_and(|c| c.is_ascii_punctuation() && *c != '-') {
            chars.next();
        }
        let mut unit = String::new();
        while let Some(c) = chars.peek().copied().filter(|c| c.is_ascii_alphabetic()) {
            unit.push(c);
            chars.next();
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if unit.is_empty() {
            if chars.peek().is_some() {
                return None;
            }
            total += number * 1000;
            break;
        }
        let u = unit.to_ascii_lowercase();
        let is = |full: &str| full.starts_with(&u);
        total += if is("days") {
            if number > 24 {
                return None;
            }
            number * 1000 * 3600 * 24
        } else if is("hours") {
            number * 1000 * 3600
        } else if is("minutes") || is("mins") {
            number * 1000 * 60
        } else if is("seconds") || is("secs") {
            number * 1000
        } else if is("milliseconds") || is("millisecs") || is("mseconds") || is("msecs") {
            number
        } else {
            return None;
        };
        while chars.peek().is_some_and(|c| c.is_ascii_punctuation() && *c != '-') {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
    }
    if total > (1 << 31) {
        return None;
    }
    Some(if negative { -total } else { total })
}

/// `(key, current value)` for every key here, in table order.
pub fn list() -> Vec<(&'static str, String)> {
    DEFS.iter().map(|d| (d.key, get(d.key))).collect()
}

/// Validates `value` for `key` against `overrides` (the persisted map) and
/// stores it. Storing the default value removes the override so the file
/// stays minimal. `toggle` flips a bool, as in irssi.
pub fn set(overrides: &mut BTreeMap<String, String>, key: &str, value: &str) -> anyhow::Result<()> {
    let Some(def) = find(key) else { anyhow::bail!("unknown setting '{key}'") };
    let stored = match def.kind {
        Kind::Bool => {
            let parsed = if value.eq_ignore_ascii_case("toggle") {
                let current = overrides.get(key).map(String::as_str).unwrap_or(def.default);
                !parse_bool(current).unwrap_or(false)
            } else {
                parse_bool(value)
                    .ok_or_else(|| anyhow::anyhow!("invalid value '{value}' for '{key}' -- expected on/off"))?
            };
            parsed.to_string()
        }
        Kind::Int => {
            value.trim().parse::<i64>().map_err(|_| anyhow::anyhow!("invalid value '{value}' for '{key}' -- expected a number"))?;
            value.trim().to_string()
        }
        Kind::Time => {
            if parse_time_ms(value).is_none() {
                anyhow::bail!("invalid time '{value}' for '{key}' -- e.g. 30, 5min, 1h 30min, 250msecs");
            }
            value.trim().to_string()
        }
        Kind::Level => {
            let (_, bad) = levels::level2bits_checked(value);
            if bad {
                anyhow::bail!("invalid level in '{value}' for '{key}'");
            }
            value.trim().to_string()
        }
        Kind::Str => value.to_string(),
    };
    if let Some(check) = def.check {
        check(&stored)?;
    }
    if stored == def.default {
        overrides.remove(key);
    } else {
        overrides.insert(key.to_string(), stored);
    }
    Ok(())
}

/// Serialises tests (in this and dependent crates) that publish to the
/// process-global values. Not for production code.
#[doc(hidden)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irssi_time_syntax() {
        assert_eq!(parse_time_ms("30"), Some(30_000), "bare number is seconds");
        assert_eq!(parse_time_ms("5min"), Some(300_000));
        assert_eq!(parse_time_ms("1sec"), Some(1000));
        assert_eq!(parse_time_ms("5msecs"), Some(5));
        assert_eq!(parse_time_ms("1day"), Some(86_400_000));
        assert_eq!(parse_time_ms("1h 30min"), Some(5_400_000));
        assert_eq!(parse_time_ms("2hours"), Some(7_200_000));
        assert_eq!(parse_time_ms("0"), Some(0));
    }

    #[test]
    fn bad_times_are_rejected() {
        assert_eq!(parse_time_ms(""), None);
        assert_eq!(parse_time_ms("min"), None);
        assert_eq!(parse_time_ms("5 lightyears"), None);
        assert_eq!(parse_time_ms("5min x"), None);
        assert_eq!(parse_time_ms("2 hours"), None, "irssi only skips punctuation, not a space, before the unit");
        assert_eq!(parse_time_ms("30 days"), None, "irssi refuses > 24 days (overflow)");
    }

    #[test]
    fn every_default_is_valid_for_its_own_kind() {
        let mut map = BTreeMap::new();
        for d in DEFS {
            set(&mut map, d.key, d.default).unwrap_or_else(|e| panic!("default of {} rejected: {e}", d.key));
            assert!(map.is_empty(), "{}: setting the default must not leave an override", d.key);
        }
    }

    #[test]
    fn color_settings_accept_irssi_codes_names_hex_and_indexes() {
        for ok in ["", "%Y", "%m", "red", "Light_Red", "#ff8800", "208"] {
            assert!(is_valid_color(ok), "{ok:?} should be valid");
        }
        for bad in ["%", "%Z", "%YY", "notacolor", "#12", "300"] {
            assert!(!is_valid_color(bad), "{bad:?} should be invalid");
        }
        let mut map = BTreeMap::new();
        assert!(set(&mut map, "hilight_color", "%Y").is_ok());
        assert!(set(&mut map, "hilight_color", "%Z").is_err());
    }

    #[test]
    fn keys_are_sorted_and_unique() {
        let keys: Vec<&str> = DEFS.iter().map(|d| d.key).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn timestamp_format_validates_strftime_and_is_stored_only_when_changed() {
        let _g = test_lock();
        let mut map = BTreeMap::new();
        assert!(set(&mut map, "timestamp_format", "%H:%M").is_ok());
        assert_eq!(map.get("timestamp_format").map(String::as_str), Some("%H:%M"));
        publish(&map);
        assert_eq!(timestamp_format(), "%H:%M");
        assert!(set(&mut map, "timestamp_format", "%Q!").is_err(), "unknown strftime item");
        assert!(set(&mut map, "timestamp_format", "").is_err());
        assert_eq!(map.get("timestamp_format").map(String::as_str), Some("%H:%M"), "a rejected value changes nothing");
        assert!(set(&mut map, "timestamp_format", "%H:%M:%S").is_ok());
        assert!(map.is_empty(), "back to the default drops the override");
        publish(&map);
        assert_eq!(timestamp_format(), "%H:%M:%S");
    }

    #[test]
    fn unknown_keys_are_refused_and_dropped_on_publish() {
        let _g = test_lock();
        let mut map = BTreeMap::new();
        assert!(set(&mut map, "no_such_key", "1").is_err());
        map.insert("no_such_key".to_string(), "1".to_string());
        publish(&map);
        assert_eq!(get("timestamp_format"), "%H:%M:%S");
    }
}
