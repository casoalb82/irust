
use std::collections::{BTreeMap, HashMap};
use std::sync::{OnceLock, RwLock};

/// How a value is parsed, validated and normalized. Mirrors irssi's
/// `settings_add_*` families.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    /// irssi `settings_add_time`: `1day`, `5min`, `100msecs`, bare
    /// number = seconds. Stored as the text the user typed.
    Time,
    Str,
    /// irssi `settings_add_level`: space-separated level names, each
    /// optionally prefixed with `-` (remove) or `+`.
    Level,
    /// irssi `settings_add_choice`: one of the listed words.
    Choice(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy)]
pub struct Spec {
    pub key: &'static str,
    pub kind: Kind,
    pub default: &'static str,
}

const fn spec(key: &'static str, kind: Kind, default: &'static str) -> Spec {
    Spec { key, kind, default }
}

/// Every key this module owns, sorted by name (the `/set` listing order).
pub const SPECS: &[Spec] = &[
    // completion (chat-completion.c, completion.c)
    spec("completion_auto", Kind::Bool, "false"),
    spec("completion_empty_line", Kind::Bool, "true"),
    spec("completion_keep_privates", Kind::Int, "10"),
    spec("completion_keep_publics", Kind::Int, "50"),
    spec("completion_keep_word", Kind::Bool, "true"),
    spec("completion_nicks_lowercase", Kind::Bool, "false"),
    spec("completion_nicks_match_case", Kind::Choice(&["never", "always", "auto"]), "auto"),
    spec("completion_strict", Kind::Bool, "false"),
    // history (command-history.c, gui-printtext.c, gui-readline.c, rawlog.c)
    spec("command_history_editable", Kind::Bool, "false"),
    spec("max_command_history", Kind::Int, "100"),
    spec("scroll_page_count", Kind::Str, "/2"),
    spec("scrollback_burst_remove", Kind::Int, "10"),
    spec("scrollback_max_age", Kind::Time, "0"),
    spec("scrollback_time", Kind::Time, "0"), // irssi: 1day; 0 keeps irust's strict line cap
    // flood (flood.c, autoignore.c, irc-servers.c, ctcp.c)
    spec("autoignore_level", Kind::Level, ""),
    spec("autoignore_time", Kind::Time, "5min"),
    spec("flood_max_msgs", Kind::Int, "4"),
    spec("flood_timecheck", Kind::Int, "8"),
    // log (fe-log.c, log.c, log-away.c)
    spec("autolog", Kind::Bool, "true"), // irssi: false; irust has always logged, keep that
    spec("autolog_colors", Kind::Bool, "false"),
    spec("autolog_level", Kind::Level, "all -crap -clientcrap -ctcps"),
    spec("autolog_only_saved_channels", Kind::Bool, "false"),
    spec("autolog_path", Kind::Str, ""), // irssi: ~/irclogs/$tag/$0.log; empty keeps irust's own <logs>/<net>/<name>-<date>.log layout
    spec("awaylog_colors", Kind::Bool, "true"),
    spec("awaylog_file", Kind::Str, ""), // empty = away.log in irust's data dir (irssi: ~/.irssi/away.log)
    spec("awaylog_level", Kind::Level, "msgs hilight"),
    spec("log_close_string", Kind::Str, "--- Log closed %a %b %d %H:%M:%S %Y"),
    spec("log_create_mode", Kind::Int, "600"), // irssi: 644; irust logs may hold credentials, so owner-only
    spec("log_open_string", Kind::Str, "--- Log opened %a %b %d %H:%M:%S %Y"),
    spec("log_timestamp", Kind::Str, "[%H:%M:%S] "), // irssi: "%H:%M "; keeps irust's existing line format
];

/// Default `awaylog_file` when the setting is empty: `away.log` in
/// irust's data directory (irssi uses `~/.irssi/away.log`).
pub fn default_awaylog_file() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("", "", "irust").map(|d| d.data_dir().join("away.log"))
}

pub fn spec_for(key: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.key == key)
}

static VALUES: OnceLock<RwLock<HashMap<&'static str, String>>> = OnceLock::new();

fn values() -> &'static RwLock<HashMap<&'static str, String>> {
    VALUES.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Current text for `key`: the published value, else its default.
/// Panics on a key that is not in `SPECS` -- callers name keys as
/// literals, so a typo is a bug to find in tests, not a runtime case.
pub fn raw(key: &str) -> String {
    let spec = spec_for(key).unwrap_or_else(|| panic!("unknown history/log setting '{key}'"));
    values()
        .read()
        .ok()
        .and_then(|m| m.get(spec.key).cloned())
        .unwrap_or_else(|| spec.default.to_string())
}

pub fn get_bool(key: &str) -> bool {
    parse_bool(&raw(key)).unwrap_or_else(|| parse_bool(spec_for(key).map_or("false", |s| s.default)).unwrap_or(false))
}

pub fn get_int(key: &str) -> i64 {
    raw(key)
        .trim()
        .parse()
        .unwrap_or_else(|_| spec_for(key).and_then(|s| s.default.parse().ok()).unwrap_or(0))
}

/// Milliseconds, as irssi's `settings_get_time`.
pub fn get_time_ms(key: &str) -> i64 {
    parse_time_ms(&raw(key))
        .or_else(|| spec_for(key).and_then(|s| parse_time_ms(s.default)))
        .unwrap_or(0)
}

pub fn get_str(key: &str) -> String {
    raw(key)
}

/// irssi's `is_true` words plus `on`/`off`.
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

pub fn parse_time_ms(text: &str) -> Option<i64> {
    let text = text.trim_start();
    let (sign, mut rest) = match text.strip_prefix('-') {
        Some(r) => (-1i64, r),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let mut total: i64 = 0;
    let mut ok = true;
    let mut number: i64 = 0;
    let mut digits = false;
    loop {
        if rest.starts_with(|c: char| c.is_ascii_digit()) {
            let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            number = rest[..end].parse().ok()?;
            rest = &rest[end..];
            digits = true;
            continue;
        }
        if !digits {
            return None;
        }
        rest = rest.trim_start_matches(|c: char| c.is_ascii_punctuation() && c != '-');
        let end = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        let unit = rest[..end].to_ascii_lowercase();
        rest = rest[end..].trim_start();
        if unit.is_empty() {
            if !rest.is_empty() {
                return None;
            }
            total += number * 1000;
            break;
        }
        let is_prefix_of = |word: &str| word.starts_with(unit.as_str());
        if is_prefix_of("days") {
            if number > 24 {
                return None;
            }
            total += number * 86_400_000;
        } else if is_prefix_of("hours") {
            total += number * 3_600_000;
        } else if is_prefix_of("minutes") || is_prefix_of("mins") {
            total += number * 60_000;
        } else if is_prefix_of("seconds") || is_prefix_of("secs") {
            total += number * 1000;
        } else if is_prefix_of("milliseconds")
            || is_prefix_of("millisecs")
            || is_prefix_of("mseconds")
            || is_prefix_of("msecs")
        {
            total += number;
        } else {
            ok = false;
        }
        rest = rest.trim_start_matches(|c: char| c.is_ascii_punctuation() && c != '-');
        if rest.is_empty() {
            break;
        }
        digits = false;
    }
    if !ok || total > (1i64 << 31) {
        return None;
    }
    Some(total * sign)
}

/// irssi message levels (`core/levels.c`), in bit order.
pub const LEVEL_NAMES: &[&str] = &[
    "CRAP", "MSGS", "PUBLICS", "NOTICES", "PUBNOTICES", "SNOTES", "CTCPS", "ACTIONS", "JOINS", "PARTS", "QUITS",
    "KICKS", "MODES", "TOPICS", "WALLOPS", "INVITES", "NICKS", "DCC", "DCCMSGS", "CLIENTNOTICES", "CLIENTCRAP",
    "CLIENTERRORS", "HILIGHTS",
];

/// Extra pseudo-levels irssi resolves by exact name before prefix
/// matching (`NEVER`, `NO_ACT`, `NOHILIGHT`, `HIDDEN`), placed above the
/// 23 real ones.
const EXTRA_LEVELS: &[&str] = &["NEVER", "NO_ACT", "NOHILIGHT", "HIDDEN"];

/// irssi's `level_get`: `ALL`/`*` = every real level; then exact
/// pseudo-level names; then a name that is a prefix of exactly one real
/// level (a full-name match always wins, an ambiguous prefix is rejected).
fn level_bit(name: &str) -> Option<u32> {
    let up = name.to_ascii_uppercase();
    if up == "ALL" || up == "*" {
        return Some((1u32 << LEVEL_NAMES.len()) - 1);
    }
    if let Some(i) = EXTRA_LEVELS.iter().position(|l| *l == up) {
        return Some(1u32 << (LEVEL_NAMES.len() + i));
    }
    if up.is_empty() {
        return None;
    }
    let mut found = None;
    for (i, l) in LEVEL_NAMES.iter().enumerate() {
        if *l == up {
            return Some(1u32 << i);
        }
        if l.starts_with(up.as_str()) {
            if found.is_some() {
                return None;
            }
            found = Some(1u32 << i);
        }
    }
    found
}

/// `level2bits`: names add (or `+name`), `-name` removes, evaluated left
/// to right, so `"all -crap -clientcrap -ctcps"` is everything but those
/// three. `None` when any word is not a level.
pub fn level_bits(text: &str) -> Option<u32> {
    let mut bits = 0u32;
    for word in text.split_whitespace() {
        if let Some(name) = word.strip_prefix('-') {
            bits &= !level_bit(name)?;
        } else {
            bits |= level_bit(word.strip_prefix('+').unwrap_or(word))?;
        }
    }
    Some(bits)
}

pub fn get_level(key: &str) -> u32 {
    level_bits(&raw(key))
        .or_else(|| spec_for(key).and_then(|s| level_bits(s.default)))
        .unwrap_or(0)
}

/// C's `atof` on a prefix: the longest leading decimal number, else 0.
fn atof_prefix(text: &str) -> f64 {
    let t = text.trim_start();
    let mut end = 0;
    let mut seen_dot = false;
    for (i, c) in t.char_indices() {
        let ok = c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) || (c == '.' && !seen_dot);
        if !ok {
            break;
        }
        if c == '.' {
            seen_dot = true;
        }
        end = i + c.len_utf8();
    }
    t[..end].parse().unwrap_or(0.0)
}

/// Lines a PageUp/PageDown scrolls: irssi's `get_scroll_count`
/// (`fe-text/gui-readline.c`) for `scroll_page_count` against a view
/// `height` lines tall. `"/2"` is half the view, `"3"` is 3 lines,
/// `"-2"` is all but 2, `".5"` is half.
pub fn scroll_page_lines(setting: &str, height: usize) -> usize {
    let height = height as f64;
    let mut count = atof_prefix(setting.strip_prefix('/').unwrap_or(setting));
    if count == 0.0 {
        count = 1.0;
    } else if count < 0.0 {
        count += height;
    } else if count < 1.0 {
        count = 1.0 / count;
    }
    if setting.starts_with('/') || setting.starts_with('.') {
        count = height / count;
    }
    count as usize
}

/// Validates `value` for `key` and returns its normalized text.
/// Booleans normalize to `true`/`false`, choices to lowercase.
pub fn validate(key: &str, value: &str) -> anyhow::Result<String> {
    let spec = spec_for(key).ok_or_else(|| anyhow::anyhow!("unknown setting '{key}'"))?;
    let value = value.trim();
    match spec.kind {
        Kind::Bool => match parse_bool(value) {
            Some(b) => Ok(b.to_string()),
            None => anyhow::bail!("invalid value '{value}' for {key} -- use on/off (or true/false)"),
        },
        Kind::Int => match value.parse::<i64>() {
            Ok(n) => Ok(n.to_string()),
            Err(_) => anyhow::bail!("invalid value '{value}' for {key} -- must be a whole number"),
        },
        Kind::Time => match parse_time_ms(value) {
            Some(_) => Ok(value.to_string()),
            None => anyhow::bail!("invalid value '{value}' for {key} -- use a time like 5min, 1day, 100msecs"),
        },
        Kind::Str => Ok(value.to_string()),
        Kind::Level => match level_bits(value) {
            Some(_) => Ok(value.to_string()),
            None => anyhow::bail!("invalid value '{value}' for {key} -- unknown message level"),
        },
        Kind::Choice(options) => {
            let lower = value.to_ascii_lowercase();
            if options.contains(&lower.as_str()) {
                Ok(lower)
            } else {
                anyhow::bail!("invalid value '{value}' for {key} -- one of: {}", options.join(", "))
            }
        }
    }
}

/// Publishes a persisted map (`Settings::history_log`) to the live
/// global. Keys not in `SPECS` (an older or hand-edited file) are
/// ignored; keys absent from the map fall back to their defaults.
pub fn publish(saved: &BTreeMap<String, String>) {
    let mut next = HashMap::new();
    for spec in SPECS {
        if let Some(v) = saved.get(spec.key) {
            if validate(spec.key, v).is_ok() {
                next.insert(spec.key, v.clone());
            }
        }
    }
    if let Ok(mut m) = values().write() {
        *m = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_sorted_within_category_and_unique() {
        let mut keys: Vec<_> = SPECS.iter().map(|s| s.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), SPECS.len(), "duplicate key in SPECS");
    }

    #[test]
    fn every_default_validates_against_its_own_kind() {
        for s in SPECS {
            validate(s.key, s.default).unwrap_or_else(|e| panic!("{}: default '{}' invalid: {e}", s.key, s.default));
        }
    }

    #[test]
    fn time_grammar_matches_irssi() {
        assert_eq!(parse_time_ms("0"), Some(0));
        assert_eq!(parse_time_ms("5min"), Some(300_000));
        assert_eq!(parse_time_ms("1day"), Some(86_400_000));
        assert_eq!(parse_time_ms("100msecs"), Some(100));
        assert_eq!(parse_time_ms("30"), Some(30_000), "bare number is seconds");
        assert_eq!(parse_time_ms("1h30m"), Some(5_400_000));
        assert_eq!(parse_time_ms("25days"), None, "irssi rejects more than 24 days");
        assert_eq!(parse_time_ms("soon"), None);
        assert_eq!(parse_time_ms("-5s"), Some(-5000));
    }

    #[test]
    fn level_grammar_matches_irssi() {
        let all = level_bits("all").unwrap();
        let default = level_bits("all -crap -clientcrap -ctcps").unwrap();
        assert_eq!(all & !default, level_bit("crap").unwrap() | level_bit("clientcrap").unwrap() | level_bit("ctcps").unwrap());
        let msgs_hilight = level_bit("msgs").unwrap() | level_bit("hilights").unwrap();
        assert_eq!(level_bits("msgs hilight"), Some(msgs_hilight), "a unique prefix resolves");
        assert_eq!(level_bits("msgs hilights"), Some(msgs_hilight));
        assert_eq!(level_bit("c"), None, "ambiguous prefix (crap, ctcps, clientcrap...) is rejected");
        assert_eq!(level_bit("pub"), None, "publics vs pubnotices is ambiguous");
        assert_eq!(level_bit("public"), Some(level_bit("publics").unwrap()));
        assert_eq!(level_bits(""), Some(0));
    }

    #[test]
    fn scroll_page_count_matches_irssi_get_scroll_count() {
        assert_eq!(scroll_page_lines("/2", 40), 20, "the default: half a page");
        assert_eq!(scroll_page_lines("/4", 40), 10);
        assert_eq!(scroll_page_lines("5", 40), 5, "a plain number of lines");
        assert_eq!(scroll_page_lines("-2", 40), 38, "negative: all but that many");
        assert_eq!(scroll_page_lines("0", 40), 1, "zero counts as one line");
        assert_eq!(scroll_page_lines("garbage", 40), 1);
        assert_eq!(scroll_page_lines(".5", 40), 20, "leading dot: 1/0.5 = 2, then height/2");
    }

    #[test]
    fn validate_normalizes_and_rejects() {
        assert_eq!(validate("completion_auto", "ON").unwrap(), "true");
        assert!(validate("completion_auto", "maybe").is_err());
        assert!(validate("max_command_history", "many").is_err());
        assert_eq!(validate("completion_nicks_match_case", "ALWAYS").unwrap(), "always");
        assert!(validate("completion_nicks_match_case", "sometimes").is_err());
        assert!(validate("autoignore_level", "notalevel").is_err());
        assert!(validate("nope", "1").is_err());
    }
}
