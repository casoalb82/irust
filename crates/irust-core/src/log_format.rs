
use std::fmt::Write as _;
use std::path::PathBuf;

use chrono::{Local, NaiveDate, TimeZone};
use irust_config::settings_history_log as cfg;

use crate::buffer::{BufferLine, SystemKind};

pub fn line_level(line: &BufferLine, in_query: bool) -> Option<u32> {
    let bit = |name: &str| cfg::level_bits(name).unwrap_or(0);
    Some(match line {
        BufferLine::Chat { highlight, .. } => {
            (if in_query { bit("msgs") } else { bit("publics") }) | if *highlight { bit("hilights") } else { 0 }
        }
        BufferLine::Action { highlight, .. } => bit("actions") | if *highlight { bit("hilights") } else { 0 },
        BufferLine::System { kind, .. } => match kind {
            SystemKind::Join => bit("joins"),
            SystemKind::Leave => bit("parts") | bit("quits") | bit("kicks"),
            SystemKind::Topic => bit("topics"),
            SystemKind::Notice => bit("notices"),
            SystemKind::Server => bit("snotes"),
            SystemKind::Wallops => bit("wallops"),
            SystemKind::Warning => bit("clienterrors"),
            SystemKind::Whois | SystemKind::DayChange => bit("crap"),
            SystemKind::Other => return None,
        },
    })
}

/// `autolog_level` gate. An unclassified system line is logged whenever
/// the level is non-empty (it is not one specific irssi level).
pub fn passes_level(line: &BufferLine, in_query: bool, autolog_level: u32) -> bool {
    match line_level(line, in_query) {
        Some(bits) => bits & autolog_level != 0,
        None => autolog_level != 0,
    }
}

/// Removes mIRC formatting control codes (`autolog_colors` off): bold,
/// italic, underline, strike, monospace, reverse, reset, `^C fg[,bg]`
/// and `^D` hex colors.
pub fn strip_formatting(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\u{2}' | '\u{f}' | '\u{11}' | '\u{16}' | '\u{1d}' | '\u{1e}' | '\u{1f}' => i += 1,
            '\u{3}' => {
                i += 1;
                let digits = |i: &mut usize| {
                    let mut n = 0;
                    while n < 2 && *i < chars.len() && chars[*i].is_ascii_digit() {
                        *i += 1;
                        n += 1;
                    }
                    n
                };
                if digits(&mut i) > 0 && i + 1 < chars.len() && chars[i] == ',' && chars[i + 1].is_ascii_digit() {
                    i += 1;
                    digits(&mut i);
                }
            }
            '\u{4}' => {
                i += 1;
                let mut n = 0;
                while n < 6 && i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                    n += 1;
                }
                if n == 6 && i + 6 < chars.len() && chars[i] == ',' && chars[i + 1..=i + 6].iter().all(|c| c.is_ascii_hexdigit())
                {
                    i += 7;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `strftime` of `epoch` (local time). An invalid format falls back to the
/// format text itself rather than panicking.
pub fn strftime_epoch(fmt: &str, epoch: i64) -> String {
    let Some(dt) = Local.timestamp_opt(epoch, 0).single() else { return fmt.to_string() };
    let mut out = String::new();
    if write!(out, "{}", dt.format(fmt)).is_err() {
        return fmt.to_string();
    }
    out
}

/// One log line: `log_timestamp` prefix, then the text. `epoch` is the
/// line's real time; a line without one uses its stored `HH:MM:SS`.
pub fn log_line_text(line: &BufferLine, colors: bool, ts_fmt: &str) -> String {
    let (timestamp, time) = match line {
        BufferLine::System { timestamp, time, .. }
        | BufferLine::Chat { timestamp, time, .. }
        | BufferLine::Action { timestamp, time, .. } => (timestamp, *time),
    };
    let prefix = match time {
        Some(t) => strftime_epoch(ts_fmt, t),
        // Only a legacy line has no epoch: all that survives is HH:MM:SS.
        None => format!("{timestamp} "),
    };
    let body = match line {
        BufferLine::System { text, .. } => text.clone(),
        BufferLine::Chat { nick, text, .. } => format!("<{nick}> {text}"),
        BufferLine::Action { nick, text, .. } => format!("* {nick} {text}"),
    };
    let body = if colors { body } else { strip_formatting(&body) };
    format!("{prefix}{body}")
}

/// `log_open_string` / `log_close_string` banner for `epoch`.
pub fn banner(key: &str, epoch: i64) -> String {
    strftime_epoch(&cfg::get_str(key), epoch)
}

/// irssi's away log (`core/log-away.c`): while you are away, lines of the
/// `awaylog_level` kinds (default: private messages and highlights) from
/// every window are also appended to `awaylog_file`; when you come back
/// the new ones are shown.
struct AwayLog {
    path: PathBuf,
    /// File length when the away period began: only what follows is shown.
    start: u64,
    msgs: usize,
}

static AWAY: std::sync::Mutex<Option<AwayLog>> = std::sync::Mutex::new(None);

/// `awaylog_file` with `~` expanded; empty means `away.log` in irust's
/// data directory (irssi uses `~/.irssi/away.log`).
pub fn awaylog_file() -> Option<PathBuf> {
    let configured = cfg::get_str("awaylog_file");
    if configured.is_empty() {
        return cfg::default_awaylog_file();
    }
    match configured.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)),
        None => Some(PathBuf::from(configured)),
    }
}

/// Start collecting (`306`, we are now away). No-op when already
/// collecting, when `awaylog_file` is unusable or `awaylog_level` is empty.
pub fn awaylog_open() {
    // Unit tests drive `306` all the time; they must never write into the
    // developer's real away log.
    if cfg!(test) || cfg::get_level("awaylog_level") == 0 {
        return;
    }
    if let Some(path) = awaylog_file() {
        awaylog_open_at(path);
    }
}

fn awaylog_open_at(path: PathBuf) {
    let Ok(mut slot) = AWAY.lock() else { return };
    if slot.is_some() {
        return;
    }
    if let Some(parent) = path.parent() {
        let existed = parent.exists();
        let _ = std::fs::create_dir_all(parent);
        if !existed {
            irust_config::perms::restrict_dir_to_owner(parent);
        }
    }
    let start = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    *slot = Some(AwayLog { path, start, msgs: 0 });
}

/// Called for every line pushed into any buffer.
pub fn awaylog_write(line: &BufferLine, in_query: bool) {
    let Ok(mut slot) = AWAY.lock() else { return };
    let Some(away) = slot.as_mut() else { return };
    if !passes_level(line, in_query, cfg::get_level("awaylog_level")) {
        return;
    }
    let text = log_line_text(line, cfg::get_bool("awaylog_colors"), &cfg::get_str("log_timestamp"));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&away.path) {
        use std::io::Write as _;
        // Private messages and highlights: owner-only, like the chat logs.
        irust_config::perms::restrict_to_owner(&away.path);
        if writeln!(f, "{text}").is_ok() {
            away.msgs += 1;
        }
    }
}

/// Stop collecting (`305`, we are back): how many lines were logged and
/// the lines themselves, `None` when no away log was running.
pub fn awaylog_close() -> Option<(usize, Vec<String>)> {
    let away = AWAY.lock().ok()?.take()?;
    let bytes = std::fs::read(&away.path).unwrap_or_default();
    let from = (away.start as usize).min(bytes.len());
    let lines = String::from_utf8_lossy(&bytes[from..]).lines().map(str::to_string).collect();
    Some((away.msgs, lines))
}

/// Expands an `autolog_path` template: `~` at the start, `$tag` (the
/// network), `$0` (the window target) and `strftime` tokens for `date`.
/// A literal `%` in the tag or target is kept as text.
pub fn expand_autolog_path(template: &str, tag: &str, target: &str, date: NaiveDate) -> PathBuf {
    let escape = |v: &str| v.replace('%', "%%");
    let mut s = template.replace("$tag", &escape(tag)).replace("$0", &escape(target));
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            s = format!("{}/{rest}", home.to_string_lossy());
        }
    }
    let Some(noon) = date.and_hms_opt(12, 0, 0) else { return PathBuf::from(s) };
    let mut out = String::new();
    if write!(out, "{}", noon.format(&s)).is_err() {
        return PathBuf::from(s);
    }
    PathBuf::from(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(text: &str) -> BufferLine {
        BufferLine::Chat {
            nick: "bob".into(),
            color_index: 0,
            rank: None,
            text: text.into(),
            timestamp: "12:34:56".into(),
            time: Some(1_700_000_000),
            highlight: false,
            highlight_color: None,
        }
    }

    fn sys(kind: SystemKind) -> BufferLine {
        BufferLine::System { text: "-- x".into(), timestamp: "12:34:56".into(), time: Some(1_700_000_000), kind }
    }

    #[test]
    fn default_autolog_level_logs_chat_and_joins_but_not_crap() {
        let level = cfg::level_bits("all -crap -clientcrap -ctcps").unwrap();
        assert!(passes_level(&chat("hi"), false, level));
        assert!(passes_level(&chat("hi"), true, level));
        assert!(passes_level(&sys(SystemKind::Join), false, level));
        assert!(passes_level(&sys(SystemKind::Other), false, level), "unclassified lines are kept");
        assert!(!passes_level(&sys(SystemKind::Whois), false, level), "whois output is crap");
    }

    #[test]
    fn a_narrow_level_drops_everything_else() {
        let level = cfg::level_bits("msgs").unwrap();
        assert!(passes_level(&chat("psst"), true, level));
        assert!(!passes_level(&chat("hello channel"), false, level), "public chat is not msgs");
        assert!(!passes_level(&sys(SystemKind::Join), false, level));
        assert!(!passes_level(&sys(SystemKind::Other), false, 0), "empty level logs nothing");
    }

    #[test]
    fn strip_formatting_removes_mirc_codes_only() {
        assert_eq!(strip_formatting("\u{2}bold\u{2} plain"), "bold plain");
        assert_eq!(strip_formatting("\u{3}4,12red on blue\u{3} after"), "red on blue after");
        assert_eq!(strip_formatting("\u{3}4red\u{f}"), "red");
        assert_eq!(strip_formatting("a,b 12"), "a,b 12", "digits and commas outside a color code stay");
        assert_eq!(strip_formatting("\u{4}ff0000,00ff00x"), "x");
        assert_eq!(strip_formatting("plain"), "plain");
    }

    #[test]
    fn log_line_text_uses_the_configured_timestamp_and_colors_flag() {
        let l = chat("\u{2}hi\u{2}");
        let epoch = 1_700_000_000;
        let expect_ts = strftime_epoch("%H:%M ", epoch);
        assert_eq!(log_line_text(&l, false, "%H:%M "), format!("{expect_ts}<bob> hi"));
        assert_eq!(log_line_text(&l, true, "%H:%M "), format!("{expect_ts}<bob> \u{2}hi\u{2}"));
        let action = BufferLine::Action {
            nick: "bob".into(),
            color_index: 0,
            rank: None,
            text: "waves".into(),
            timestamp: "12:34:56".into(),
            time: Some(epoch),
            highlight: false,
            highlight_color: None,
        };
        assert_eq!(log_line_text(&action, false, "[%H:%M:%S] "), format!("{}* bob waves", strftime_epoch("[%H:%M:%S] ", epoch)));
    }

    #[test]
    fn a_legacy_line_without_an_epoch_keeps_its_stored_clock_time() {
        let mut l = chat("hi");
        if let BufferLine::Chat { time, .. } = &mut l {
            *time = None;
        }
        assert_eq!(log_line_text(&l, true, "%H:%M "), "12:34:56 <bob> hi");
    }

    #[test]
    fn away_log_collects_only_matching_lines_and_shows_what_is_new() {
        let dir = std::env::temp_dir().join(format!("irust-awaylog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("away.log");
        std::fs::write(&path, "left over from an earlier away\n").unwrap();

        // The collector is process-wide, so lines other tests push while it
        // is open can land in it too: only this test's marked lines count.
        awaylog_open_at(path.clone());
        awaylog_write(&chat("zzmark psst"), true); // a private message
        awaylog_write(&chat("zzmark public chatter"), false); // not msgs, not a hilight
        let mut hl = chat("zzmark abcX: look");
        if let BufferLine::Chat { highlight, .. } = &mut hl {
            *highlight = true;
        }
        awaylog_write(&hl, false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the away log holds private messages: owner-only");
        }
        let (n, lines) = awaylog_close().expect("an away log was open");
        let mine: Vec<&String> = lines.iter().filter(|l| l.contains("zzmark")).collect();
        assert_eq!(mine.len(), 2, "only the query message and the hilight: {lines:?}");
        assert!(mine[0].ends_with("<bob> zzmark psst") && mine[1].ends_with("<bob> zzmark abcX: look"));
        assert!(n >= 2);
        assert!(!lines.iter().any(|l| l.contains("left over")), "earlier file content is not shown");
        assert!(awaylog_close().is_none(), "closing twice finds nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn autolog_path_template_expands_tag_target_and_date() {
        let date = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        assert_eq!(
            expand_autolog_path("/var/irclogs/$tag/$0.%Y-%m-%d.log", "chatnet", "#irust", date),
            PathBuf::from("/var/irclogs/chatnet/#irust.2026-09-30.log")
        );
        assert_eq!(expand_autolog_path("/x/$0.log", "t", "a%b", date), PathBuf::from("/x/a%b.log"));
    }
}
