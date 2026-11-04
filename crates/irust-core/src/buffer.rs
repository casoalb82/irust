use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

/// What kind of event a `BufferLine::System` line reports -- lets the UI
/// color join/leave lines distinctly (green/red) without
/// string-matching the rendered text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemKind {
    Join,
    /// Part, kick, quit, or a connection drop -- anything that means
    /// "gone," colored the same way.
    Leave,
    /// A channel topic being announced or changed.
    Topic,
    /// A `/whois` reply line -- plain (not the muted `Other` gray) so
    /// the embedded mIRC codes it carries (bold nick, gray brackets,
    /// irssi's own whois look) read against the normal foreground.
    Whois,
    Notice,
    Server,
    Warning,
    DayChange,
    Wallops,
    Other,
}

#[derive(Debug, Clone)]
pub enum BufferLine {
    /// A status/system message ("-- X joined", "-- topic: ...").
    /// `kind` drives its color; anything other than a join/leave event
    /// renders in the same neutral style as before.
    System {
        text: String,
        timestamp: String,
        time: Option<i64>,
        kind: SystemKind,
    },
    /// A channel PRIVMSG. `highlight` is fixed at arrival time (does
    /// this line mention our nick), not recomputed on later renders --
    /// our own outgoing messages are always `highlight: false`.
    Chat {
        nick: String,
        color_index: usize,
        rank: Option<char>,
        text: String,
        highlight: bool,
        highlight_color: Option<String>,
        timestamp: String,
        time: Option<i64>,
    },
    /// A `/me` (CTCP ACTION) -- rendered as `* nick text`, not `<nick> text`.
    Action {
        nick: String,
        color_index: usize,
        rank: Option<char>,
        text: String,
        highlight: bool,
        /// See `Chat::highlight_color`'s own doc comment.
        highlight_color: Option<String>,
        timestamp: String,
        time: Option<i64>,
    },
}

impl BufferLine {
    pub fn time(&self) -> Option<i64> {
        match self {
            BufferLine::System { time, .. } => *time,
            BufferLine::Chat { time, .. } => *time,
            BufferLine::Action { time, .. } => *time,
        }
    }
}

fn scrollback_drop_count(
    times: impl ExactSizeIterator<Item = Option<i64>> + Clone,
    lines: usize,
    burst: usize,
    keep_secs: i64,
    max_age_secs: i64,
    now: i64,
) -> usize {
    let total = times.len();
    let mut dropped = 0usize;
    if total >= lines + burst {
        let old_time = now - keep_secs + 1;
        for t in times.clone() {
            if total - dropped <= lines || lines == 0 || t.unwrap_or(0) >= old_time {
                break;
            }
            dropped += 1;
        }
    }
    if max_age_secs > 0 {
        let old_time = now - max_age_secs;
        for t in times.skip(dropped) {
            if t.unwrap_or(0) >= old_time {
                break;
            }
            dropped += 1;
        }
    }
    dropped
}

/// Compares everything except `timestamp` -- tests build expected
/// `BufferLine` values without caring about the exact wall-clock moment
/// a line was pushed, only its content.
impl PartialEq for BufferLine {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (BufferLine::System { text: a, kind: k1, .. }, BufferLine::System { text: b, kind: k2, .. }) => {
                a == b && k1 == k2
            }
            (
                BufferLine::Chat {
                    nick: n1,
                    color_index: c1,
                    rank: r1,
                    text: t1,
                    highlight: h1,
                    highlight_color: hc1,
                    ..
                },
                BufferLine::Chat {
                    nick: n2,
                    color_index: c2,
                    rank: r2,
                    text: t2,
                    highlight: h2,
                    highlight_color: hc2,
                    ..
                },
            ) => n1 == n2 && c1 == c2 && r1 == r2 && t1 == t2 && h1 == h2 && hc1 == hc2,
            (
                BufferLine::Action {
                    nick: n1,
                    color_index: c1,
                    rank: r1,
                    text: t1,
                    highlight: h1,
                    highlight_color: hc1,
                    ..
                },
                BufferLine::Action {
                    nick: n2,
                    color_index: c2,
                    rank: r2,
                    text: t2,
                    highlight: h2,
                    highlight_color: hc2,
                    ..
                },
            ) => n1 == n2 && c1 == c2 && r1 == r2 && t1 == t2 && h1 == h2 && hc1 == hc2,
            _ => false,
        }
    }
}
impl Eq for BufferLine {}

/// The current local time as `HH:MM:SS` -- shared by per-line timestamps
/// and (via `irust-tui`) the statusbar clock, so both agree.
pub fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// A line's timestamp text -- `/set timestamp_format` (irssi's `$Z`
/// expando). Separate from `now_hms`, which stays the fixed `HH:MM:SS`
/// the statusbar clock shows.
pub fn now_stamp() -> String {
    // An invalid hand-edited format falls back to the plain clock.
    format_stamp(&chrono::Local::now(), &irust_config::lookandfeel::timestamp_format()).unwrap_or_else(now_hms)
}

/// `at` rendered with the strftime `format`; `None` when the format is
/// invalid (chrono's `to_string` would panic on it).
fn format_stamp(at: &chrono::DateTime<chrono::Local>, format: &str) -> Option<String> {
    use std::fmt::Write as _;
    let mut out = String::new();
    write!(out, "{}", at.format(format)).ok()?;
    Some(out)
}

/// The current moment as real Unix-epoch seconds -- `now_hms`'s
/// companion for `BufferLine::time`, computed alongside it (never
/// derived FROM it -- `now_hms`'s string has no date in it at all).
pub fn now_epoch() -> i64 {
    chrono::Local::now().timestamp()
}

/// Today's local date -- `main.rs`'s per-connection clock tick polls
/// this every second to detect a local-midnight rollover (irssi's own
/// day-change marker, `ClientState::push_daychange_to_all_buffers`).
pub fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

/// A single scrollback buffer (a channel, a query, or later a status
/// window) -- a bounded in-memory line log, optionally also appended to
/// a file on disk (see `ClientState::enable_logging`).
pub struct Buffer {
    pub name: String,
    pub lines: VecDeque<BufferLine>,
    pub capacity: usize,
    log_path: Option<PathBuf>,
    logging_stopped: bool,
    /// The log file the `log_open_string` banner was last written to, so
    /// it is written once per opened file and `log_close_string` goes to
    /// the file being left when the path changes or logging stops.
    log_opened: Option<PathBuf>,
    /// Set on every push; cleared by `ClientState::mark_current_buffer_read`
    /// once per render frame for whichever buffer is currently active --
    /// so it settles to "true" only for buffers with activity you
    /// haven't actually looked at yet.
    pub has_unread: bool,
    pub has_unread_message: bool,
    pub has_unread_join: bool,
    /// Same idea, but only for a line that mentions us -- lets the UI
    /// distinguish "something happened" from "you were mentioned."
    pub has_unread_highlight: bool,
    pub highlight_actcolor: Option<String>,
    pub custom_name: Option<String>,
    cleared_before: usize,
}

impl Buffer {
    /// `capacity` is read from `/set scrollback_lines` at the moment
    /// this buffer is created (1000 unless that's ever been changed) --
    /// see `irust_config::settings::SCROLLBACK_LINES`'s own doc comment
    /// for why a later `/set` doesn't retroactively resize an
    /// already-open buffer.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            lines: VecDeque::new(),
            capacity: irust_config::settings::SCROLLBACK_LINES.load(std::sync::atomic::Ordering::Relaxed) as usize,
            log_path: None,
            logging_stopped: false,
            log_opened: None,
            has_unread: false,
            has_unread_message: false,
            has_unread_join: false,
            has_unread_highlight: false,
            highlight_actcolor: None,
            cleared_before: 0,
            custom_name: None,
        }
    }

    pub fn shown_name(&self) -> &str {
        self.custom_name.as_deref().unwrap_or(&self.name)
    }

    pub fn mark_read(&mut self) {
        self.has_unread = false;
        self.has_unread_message = false;
        self.has_unread_join = false;
        self.has_unread_highlight = false;
        self.highlight_actcolor = None;
    }

    /// `/clear` -- the live (unscrolled) view goes blank until new
    /// activity arrives; everything already here is still there for
    /// `/clear`'s own PageUp to reveal, same as irssi.
    pub fn clear_view(&mut self) {
        self.cleared_before = self.lines.len();
    }

    /// How many lines from the front of `lines` a `/clear` wants hidden
    /// from the live view right now -- `irust-tui`'s render layer is the
    /// only caller (`Buffer` itself doesn't know about scroll offsets).
    pub fn cleared_before(&self) -> usize {
        self.cleared_before
    }

    /// Every line pushed from now on also gets appended to `path`
    /// (created, and its parent directory created, on first write).
    pub fn set_log_path(&mut self, path: PathBuf) {
        if self.log_opened.as_deref() != Some(path.as_path()) {
            self.write_close_banner();
        }
        self.log_path = Some(path);
    }

    /// `log_close_string` into the file this buffer was logging to.
    fn write_close_banner(&mut self) {
        if let Some(prev) = self.log_opened.take() {
            append_log_text(&prev, &crate::log_format::banner("log_close_string", now_epoch()));
        }
    }

    /// Whether this buffer currently logs to disk.
    pub fn is_logging(&self) -> bool {
        self.log_path.is_some()
    }

    /// The path this buffer currently logs to, if any -- `/log` (no
    /// args)'s own status line wants to show it, not just on/off.
    pub fn log_path(&self) -> Option<&std::path::Path> {
        self.log_path.as_deref()
    }

    /// `/log stop [name|index]` -- turns logging off for just this
    /// buffer and remembers that it was an explicit stop (see
    /// `logging_stopped`'s own doc comment), so a later day-change
    /// rollover doesn't quietly turn it back on.
    pub fn stop_logging(&mut self) {
        self.write_close_banner();
        self.log_path = None;
        self.logging_stopped = true;
    }

    /// `/log start [name|index]` -- points this buffer's logging at
    /// `path` and clears the "explicitly stopped" flag, so rotation
    /// resumes carrying it forward like any other logging buffer.
    pub fn resume_logging(&mut self, path: PathBuf) {
        self.write_close_banner();
        self.log_path = Some(path);
        self.logging_stopped = false;
    }

    /// Whether `ClientState::rotate_logs` should skip this buffer when
    /// pointing every buffer at the new day's file -- true only right
    /// after an explicit `/log stop`.
    pub fn logging_stopped(&self) -> bool {
        self.logging_stopped
    }

    pub fn push_system(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Other);
    }

    pub fn push_system_join(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Join);
    }

    pub fn push_system_leave(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Leave);
    }

    pub fn push_system_topic(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Topic);
    }

    pub fn push_system_whois(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Whois);
    }

    pub fn push_system_notice(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Notice);
    }

    pub fn push_system_server(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Server);
    }

    pub fn push_system_warning(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Warning);
    }

    pub fn push_system_daychange(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::DayChange);
    }

    pub fn push_system_wallops(&mut self, line: impl Into<String>) {
        self.push_system_kind(line, SystemKind::Wallops);
    }

    fn push_system_kind(&mut self, line: impl Into<String>, kind: SystemKind) {
        self.push(BufferLine::System {
            text: line.into(),
            timestamp: now_stamp(),
            time: Some(now_epoch()),
            kind,
        });
    }

    pub fn push_system_before_whois_end(&mut self, line: impl Into<String>, after: usize) {
        let start = after.min(self.lines.len());
        let end_idx = self.lines.iter().enumerate().skip(start).find_map(|(i, l)| {
            matches!(l, BufferLine::System { kind: SystemKind::Whois, text, .. }
                if text == "-- End of WHOIS" || text == "-- End of WHOWAS")
            .then_some(i)
        });

        let len_before = self.lines.len();
        self.push_system(line);

        let Some(mut end_idx) = end_idx else { return };
        // `push` trims from the front once at capacity -- if that just
        // happened (net length unchanged despite pushing one), every
        // index computed above shifted down by one.
        if self.lines.len() == len_before {
            let Some(shifted) = end_idx.checked_sub(1) else {
                // The terminator itself was the line that got evicted --
                // nothing sensible left to splice before.
                return;
            };
            end_idx = shifted;
        }
        if let Some(moved) = self.lines.pop_back() {
            self.lines.insert(end_idx, moved);
        }
    }

    pub fn replace_last_chat_text(&mut self, nick: &str, old_text: &str, new_text: String) -> bool {
        if let Some(pos) = self.lines.iter().rposition(|l| {
            matches!(l, BufferLine::Chat { nick: n, text: t, .. } if n == nick && t == old_text)
        }) {
            if let Some(BufferLine::Chat { text, .. }) = self.lines.get_mut(pos) {
                *text = new_text;
                return true;
            }
        }
        false
    }

    pub fn push_chat(
        &mut self,
        nick: impl Into<String>,
        color_index: usize,
        rank: Option<char>,
        text: impl Into<String>,
        highlight: bool,
    ) {
        self.push_chat_at(nick, color_index, rank, text, highlight, None, None);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_chat_at(
        &mut self,
        nick: impl Into<String>,
        color_index: usize,
        rank: Option<char>,
        text: impl Into<String>,
        highlight: bool,
        timestamp: Option<String>,
        time: Option<i64>,
    ) {
        self.push_chat_at_colored(nick, color_index, rank, text, highlight, None, timestamp, time);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_chat_at_colored(
        &mut self,
        nick: impl Into<String>,
        color_index: usize,
        rank: Option<char>,
        text: impl Into<String>,
        highlight: bool,
        highlight_color: Option<String>,
        timestamp: Option<String>,
        time: Option<i64>,
    ) {
        self.push(BufferLine::Chat {
            nick: nick.into(),
            color_index,
            rank,
            text: text.into(),
            highlight,
            highlight_color,
            timestamp: timestamp.unwrap_or_else(now_stamp),
            time: Some(time.unwrap_or_else(now_epoch)),
        });
    }

    pub fn push_action(
        &mut self,
        nick: impl Into<String>,
        color_index: usize,
        rank: Option<char>,
        text: impl Into<String>,
        highlight: bool,
    ) {
        self.push_action_at(nick, color_index, rank, text, highlight, None, None);
    }

    /// See `push_chat_at`'s own doc comment -- same idea, for actions.
    #[allow(clippy::too_many_arguments)]
    pub fn push_action_at(
        &mut self,
        nick: impl Into<String>,
        color_index: usize,
        rank: Option<char>,
        text: impl Into<String>,
        highlight: bool,
        timestamp: Option<String>,
        time: Option<i64>,
    ) {
        self.push_action_at_colored(nick, color_index, rank, text, highlight, None, timestamp, time);
    }

    /// See `push_chat_at_colored`'s own doc comment -- same idea, for actions.
    #[allow(clippy::too_many_arguments)]
    pub fn push_action_at_colored(
        &mut self,
        nick: impl Into<String>,
        color_index: usize,
        rank: Option<char>,
        text: impl Into<String>,
        highlight: bool,
        highlight_color: Option<String>,
        timestamp: Option<String>,
        time: Option<i64>,
    ) {
        self.push(BufferLine::Action {
            nick: nick.into(),
            color_index,
            rank,
            text: text.into(),
            highlight,
            highlight_color,
            timestamp: timestamp.unwrap_or_else(now_stamp),
            time: Some(time.unwrap_or_else(now_epoch)),
        });
    }

    fn push(&mut self, line: BufferLine) {
        crate::log_format::awaylog_write(&line, !self.name.starts_with(['#', '&', '+', '!']));
        self.log_line(&line);
        match crate::activity::tier_of_line(&line, &self.name) {
            crate::activity::Tier::None => {}
            tier => {
                self.has_unread = true;
                if tier >= crate::activity::Tier::Msg {
                    self.has_unread_message = true;
                }
                if tier == crate::activity::Tier::Hilight {
                    self.has_unread_highlight = true;
                }
                if matches!(&line, BufferLine::System { kind: SystemKind::Join, .. }) {
                    self.has_unread_join = true;
                }
            }
        }
        self.lines.push_back(line);
        self.remove_old_lines(now_epoch());
    }

    /// irssi's `remove_old_lines` (`fe-text/gui-printtext.c`), run after
    /// every appended line, driven by `scrollback_lines` (this buffer's
    /// `capacity`), `scrollback_burst_remove`, `scrollback_time` and
    /// `scrollback_max_age`.
    fn remove_old_lines(&mut self, now: i64) {
        use irust_config::settings_history_log as cfg;
        let burst = cfg::get_int("scrollback_burst_remove").max(0) as usize;
        let keep_secs = cfg::get_time_ms("scrollback_time") / 1000;
        let max_age_secs = cfg::get_time_ms("scrollback_max_age") / 1000;
        let drop = scrollback_drop_count(
            self.lines.iter().map(BufferLine::time),
            self.capacity,
            burst,
            keep_secs,
            max_age_secs,
            now,
        );
        for _ in 0..drop {
            self.lines.pop_front();
            self.cleared_before = self.cleared_before.saturating_sub(1);
        }
    }

    fn log_line(&mut self, line: &BufferLine) {
        use irust_config::settings_history_log as cfg;
        let Some(path) = self.log_path.clone() else { return };

        // `autolog_level`: which kinds of line reach the file at all.
        let in_query = !self.name.starts_with(['#', '&', '+', '!']);
        if !crate::log_format::passes_level(line, in_query, cfg::get_level("autolog_level")) {
            return;
        }

        // First line into this file: irssi's `log_open_string` banner.
        if self.log_opened.as_deref() != Some(path.as_path()) {
            self.write_close_banner();
            append_log_text(&path, &crate::log_format::banner("log_open_string", now_epoch()));
            self.log_opened = Some(path.clone());
        }
        let text = crate::log_format::log_line_text(
            line,
            cfg::get_bool("autolog_colors"),
            &cfg::get_str("log_timestamp"),
        );
        append_log_text(&path, &text);
    }

    pub fn lastlog(&self, pattern: &str, is_regex: bool, case_sensitive: bool, count: Option<usize>) -> Vec<String> {
        let matches = |line: &BufferLine| -> bool {
            let text = searchable_text(line);
            if is_regex {
                crate::nicklist::matches_regex_cached(pattern, &text)
            } else if case_sensitive {
                text.contains(pattern)
            } else {
                text.to_lowercase().contains(&pattern.to_lowercase())
            }
        };
        let results: Vec<String> = self
            .lines
            .iter()
            .filter(|line| matches(line))
            .map(format_line)
            .collect();
        match count {
            Some(n) if results.len() > n => results[results.len() - n..].to_vec(),
            _ => results,
        }
    }

    pub fn first_index_at_or_after(&self, target: i64) -> Option<usize> {
        self.lines
            .iter()
            .position(|line| line.time().is_some_and(|t| t >= target))
    }
}

/// The single "nick + body" (or just body, for a system line) irssi-
/// style shape `/lastlog` matches against -- kept separate from
/// `format_line` since the two need slightly different content (this
/// one skips the timestamp, `format_line` includes it).
fn searchable_text(line: &BufferLine) -> String {
    match line {
        BufferLine::System { text, .. } => text.clone(),
        BufferLine::Chat { nick, text, .. } => format!("{nick} {text}"),
        BufferLine::Action { nick, text, .. } => format!("{nick} {text}"),
    }
}

/// Appends one line to a log file, creating it (and its directory) as
/// needed with `log_create_mode`. Best-effort: a logging failure (disk
/// full, permissions, ...) must never interrupt the chat session.
fn append_log_text(path: &std::path::Path, text: &str) {
    if let Some(parent) = path.parent() {
        // Only a directory created here is locked down: an existing one
        // (e.g. `$HOME` for a bare `~/$0.log` autolog_path) is the user's.
        let existed = parent.exists();
        let _ = std::fs::create_dir_all(parent);
        if !existed {
            irust_config::perms::restrict_dir_to_owner(parent);
        }
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    // `log_create_mode` is irssi's octal-digits-as-a-decimal-number
    // (644 means 0o644); an unparseable value keeps the owner-only mode.
    let digits = irust_config::settings_history_log::get_int("log_create_mode").to_string();
    let mode = u32::from_str_radix(&digits, 8).unwrap_or(0o600);
    irust_config::perms::set_file_mode(path, mode);
    let _ = writeln!(file, "{text}");
}

/// Shared by `Buffer::log_line` (writes this to disk) and
/// `Buffer::lastlog` (returns these as search results) -- one format,
/// so a `/lastlog` result always matches what's actually on disk.
fn format_line(line: &BufferLine) -> String {
    match line {
        // System text already carries its own "-- " prefix (see every
        // push_system call site) -- don't double it up.
        BufferLine::System { text, timestamp, .. } => format!("[{timestamp}] {text}"),
        BufferLine::Chat {
            nick, text, timestamp, ..
        } => format!("[{timestamp}] <{nick}> {text}"),
        BufferLine::Action {
            nick, text, timestamp, ..
        } => format!("[{timestamp}] * {nick} {text}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_invalid_timestamp_format_yields_none_instead_of_panicking() {
        let now = chrono::Local::now();
        assert!(format_stamp(&now, "%Q %!").is_none());
        assert_eq!(format_stamp(&now, "%H:%M:%S").map(|s| s.len()), Some(8));
    }

    use super::*;

    #[test]
    fn line_timestamps_follow_timestamp_format() {
        let stamp = irust_config::lookandfeel::with_test_overrides(&[("timestamp_format", "T%H")], || {
            let mut buf = Buffer::new("#a");
            buf.push_system("-- hello");
            match buf.lines.back().unwrap() {
                BufferLine::System { timestamp, .. } => timestamp.clone(),
                other => panic!("unexpected line {other:?}"),
            }
        });
        assert!(stamp.starts_with('T') && stamp.len() == 3, "got {stamp:?}");
        assert_eq!(now_hms().len(), 8, "the statusbar clock keeps HH:MM:SS regardless");
    }

    #[test]
    fn ip_result_inserts_before_a_trailing_end_of_whois() {
        let mut buf = Buffer::new("status");
        buf.push_system_whois("-- some whois field");
        buf.push_system_whois("-- End of WHOIS");

        buf.push_system_before_whois_end("-- 1.2.3.4 lookup result", 0);

        let texts: Vec<&str> = buf
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            texts,
            vec!["-- some whois field", "-- 1.2.3.4 lookup result", "-- End of WHOIS"]
        );
    }

    #[test]
    fn ip_result_appends_normally_when_nothing_ended_the_whois_yet() {
        let mut buf = Buffer::new("status");
        buf.push_system_whois("-- some whois field");

        buf.push_system_before_whois_end("-- 1.2.3.4 lookup result", 0);

        let texts: Vec<&str> = buf
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(texts, vec!["-- some whois field", "-- 1.2.3.4 lookup result"]);
    }

    #[test]
    fn ip_result_lands_before_its_own_terminator_not_a_later_unrelated_ones() {
        let mut buf = Buffer::new("status");
        let nick1_start = buf.lines.len();
        buf.push_system_whois("-- nick1 whois field");
        buf.push_system_whois("-- End of WHOIS"); // nick1's own terminator
        buf.push_system_whois("-- nick2 whois field");
        buf.push_system_whois("-- End of WHOIS"); // nick2's, unrelated

        buf.push_system_before_whois_end("-- 1.2.3.4 lookup result for nick1", nick1_start);

        let texts: Vec<&str> = buf
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                "-- nick1 whois field",
                "-- 1.2.3.4 lookup result for nick1",
                "-- End of WHOIS", // nick1's -- the result landed here
                "-- nick2 whois field",
                "-- End of WHOIS", // nick2's -- untouched
            ],
            "must land before nick1's OWN terminator, not nick2's later unrelated one"
        );
    }

    /// `after` must not let a search wander backwards into an earlier,
    /// already-closed exchange that happens to precede `start_len` --
    /// only a terminator at or after `start_len` counts.
    #[test]
    fn ip_result_ignores_an_earlier_terminator_before_its_own_start_len() {
        let mut buf = Buffer::new("status");
        buf.push_system_whois("-- earlier unrelated whois field");
        buf.push_system_whois("-- End of WHOIS"); // an earlier, closed exchange

        let this_start = buf.lines.len();
        buf.push_system_whois("-- nick1 whois field"); // still open, no terminator yet

        buf.push_system_before_whois_end("-- 1.2.3.4 lookup result for nick1", this_start);

        let texts: Vec<&str> = buf
            .lines
            .iter()
            .map(|l| match l {
                BufferLine::System { text, .. } => text.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            texts,
            vec![
                "-- earlier unrelated whois field",
                "-- End of WHOIS",
                "-- nick1 whois field",
                "-- 1.2.3.4 lookup result for nick1",
            ],
            "must fall back to a plain append, not the earlier exchange's terminator"
        );
    }

    #[test]
    fn lastlog_matches_case_insensitively_across_chat_action_and_system_lines() {
        let mut buf = Buffer::new("#chatnet");
        buf.push_chat("dana", 0, None, "check the ROUTING page", false);
        buf.push_action("sh", 0, None, "waves at Routing", false);
        buf.push_system("-- topic: unrelated");
        buf.push_chat("sh", 0, None, "nothing to see here", false);

        let results = buf.lastlog("routing", false, false, None);
        assert_eq!(results.len(), 2, "got: {results:?}");
        assert!(results[0].contains("<dana> check the ROUTING page"));
        assert!(results[1].contains("* sh waves at Routing"));
    }

    #[test]
    fn lastlog_with_no_matches_returns_empty() {
        let mut buf = Buffer::new("#chatnet");
        buf.push_chat("dana", 0, None, "hello", false);
        assert!(buf.lastlog("nonexistent", false, false, None).is_empty());
    }

    #[test]
    fn lastlog_results_match_the_on_disk_log_format() {
        let mut buf = Buffer::new("#chatnet");
        buf.push_chat("sh", 0, None, "findme", false);
        let results = buf.lastlog("findme", false, false, None);
        assert_eq!(results.len(), 1);
        // Same "[HH:MM:SS] <nick> text" shape `log_line` writes to disk.
        assert!(results[0].starts_with('['), "got: {:?}", results[0]);
        assert!(results[0].contains("] <sh> findme"), "got: {:?}", results[0]);
    }

    #[test]
    fn lastlog_case_sensitive_flag_rejects_a_different_case_match() {
        let mut buf = Buffer::new("#chatnet");
        buf.push_chat("dana", 0, None, "check the ROUTING page", false);
        assert_eq!(
            buf.lastlog("routing", false, false, None).len(),
            1,
            "case-insensitive by default"
        );
        assert!(
            buf.lastlog("routing", false, true, None).is_empty(),
            "-case must not match different casing"
        );
        assert_eq!(
            buf.lastlog("ROUTING", false, true, None).len(),
            1,
            "-case still matches the exact case"
        );
    }

    #[test]
    fn lastlog_regex_flag_matches_a_pattern_not_just_a_substring() {
        let mut buf = Buffer::new("#chatnet");
        buf.push_chat("sh", 0, None, "error 42 on build", false);
        buf.push_chat("sh", 0, None, "all good here", false);
        let results = buf.lastlog(r"error \d+", true, false, None);
        assert_eq!(results.len(), 1);
        assert!(results[0].contains("error 42 on build"));
    }

    #[test]
    fn lastlog_count_keeps_only_the_last_n_matches() {
        let mut buf = Buffer::new("#chatnet");
        for i in 1..=5 {
            buf.push_chat("sh", 0, None, format!("match {i}"), false);
        }
        let results = buf.lastlog("match", false, false, Some(2));
        assert_eq!(results.len(), 2);
        assert!(results[0].contains("match 4"), "got: {results:?}");
        assert!(results[1].contains("match 5"), "got: {results:?}");
    }

    #[test]
    fn lastlog_count_larger_than_matches_returns_everything() {
        let mut buf = Buffer::new("#chatnet");
        buf.push_chat("sh", 0, None, "one match", false);
        assert_eq!(buf.lastlog("match", false, false, Some(50)).len(), 1);
    }

    #[test]
    fn activity_tiers_distinguish_system_only_from_a_real_message() {
        let mut buf = Buffer::new("#chatnet");
        buf.push_system_join("-- dana joined #chatnet".to_string());
        assert!(buf.has_unread);
        assert!(!buf.has_unread_message, "a join is system-only, not a message");
        assert!(!buf.has_unread_highlight);

        buf.push_chat("dana", 0, None, "hey", false);
        assert!(buf.has_unread_message, "a real message should set the msg tier");
        assert!(!buf.has_unread_highlight, "not highlighted, so no hilight tier yet");

        buf.push_chat("dana", 0, None, "abcX check this out", true);
        assert!(buf.has_unread_highlight);

        buf.mark_read();
        assert!(!buf.has_unread);
        assert!(!buf.has_unread_message);
        assert!(!buf.has_unread_highlight);
    }

    #[test]
    fn has_unread_join_is_specific_to_a_join_line() {
        let mut buf = Buffer::new("#chatnet");
        buf.push_system_leave("-- dana left #chatnet".to_string());
        assert!(buf.has_unread, "a leave is still system-tier activity");
        assert!(!buf.has_unread_join, "a leave is not a join");

        buf.mark_read();
        buf.push_system_topic("-- topic changed".to_string());
        assert!(!buf.has_unread_join, "a topic change is not a join");

        buf.mark_read();
        buf.push_system_join("-- dana joined #chatnet".to_string());
        assert!(buf.has_unread_join);

        buf.mark_read();
        assert!(!buf.has_unread_join, "mark_read clears the join tier too");
    }

    /// `highlight_actcolor` is set by the caller in `client.rs` (this
    /// crate has no hilight-matching logic of its own -- see the
    /// field's own doc comment), so this only checks `mark_read`'s side:
    /// reading a highlighted buffer must clear its pending activity
    /// color too, not just the three `has_unread*` flags.
    #[test]
    fn mark_read_clears_the_pending_highlight_actcolor_too() {
        let mut buf = Buffer::new("#chatnet");
        buf.highlight_actcolor = Some("magenta".to_string());
        buf.mark_read();
        assert_eq!(buf.highlight_actcolor, None);
    }

    #[test]
    fn clear_view_hides_existing_lines_but_not_ones_pushed_after() {
        let mut buf = Buffer::new("#test");
        buf.push_system("-- before clear");
        buf.clear_view();
        assert_eq!(buf.cleared_before(), 1);

        buf.push_system("-- after clear");
        // Nothing was actually deleted -- both lines are still there for
        // scrollback to reach.
        assert_eq!(buf.lines.len(), 2);
        assert_eq!(
            buf.cleared_before(),
            1,
            "the marker itself doesn't move on new activity"
        );
    }

    #[test]
    fn cleared_before_stays_in_sync_when_capacity_evicts_old_lines() {
        let mut buf = Buffer::new("#test");
        buf.capacity = 3;
        buf.push_system("-- line 1");
        buf.push_system("-- line 2");
        buf.clear_view();
        assert_eq!(buf.cleared_before(), 2);

        // Push far past capacity (eviction happens in bursts of
        // `scrollback_burst_remove`) -- the marker shrinks in lockstep
        // with the evicted oldest lines and floors at 0.
        let mut last = buf.cleared_before();
        for n in 3..=40 {
            buf.push_system(format!("-- line {n}"));
            assert!(buf.cleared_before() <= last, "the marker only ever moves down");
            last = buf.cleared_before();
        }
        assert_eq!(buf.cleared_before(), 0, "both pre-clear lines are gone, must not underflow");
        assert!(buf.lines.len() < 3 + 10, "the burst limit bounds the overshoot");
    }

    #[test]
    fn a_logged_buffer_gets_open_and_close_banners_around_its_lines() {
        let dir = std::env::temp_dir().join(format!("irust-log-banner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("chan.log");
        let mut buf = Buffer::new("#banner");
        buf.set_log_path(path.clone());
        buf.push_chat("bob", 0, None, "hello", false);
        buf.push_system("-- something");
        buf.stop_logging();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("--- Log opened "), "first: {lines:?}");
        assert!(lines[1].ends_with("<bob> hello") && lines[1].starts_with('['), "chat line: {lines:?}");
        assert!(lines[2].ends_with("-- something"));
        assert!(lines[3].starts_with("--- Log closed "), "last: {lines:?}");
        assert_eq!(lines.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scrollback_drop_count_follows_irssi_remove_old_lines() {
        let t = |v: &[i64]| v.iter().map(|x| Some(*x)).collect::<Vec<_>>();
        let now = 10_000;
        // Under lines+burst nothing is removed, even beyond `lines`.
        assert_eq!(scrollback_drop_count(t(&[1; 12]).into_iter(), 10, 5, 0, 0, now), 0);
        // At lines+burst it trims back down to `lines`.
        assert_eq!(scrollback_drop_count(t(&[1; 15]).into_iter(), 10, 5, 0, 0, now), 5);
        // scrollback_time keeps lines younger than it, even over the cap.
        let mixed = t(&[1, 1, 9_990, 9_995, 9_999]);
        assert_eq!(scrollback_drop_count(mixed.clone().into_iter(), 1, 1, 100, 0, now), 2);
        // lines == 0: only the time rules apply.
        assert_eq!(scrollback_drop_count(mixed.clone().into_iter(), 0, 0, 100, 0, now), 0);
        // scrollback_max_age drops everything older than it.
        assert_eq!(scrollback_drop_count(mixed.into_iter(), 100, 10, 100, 9, now), 3);
    }

    /// `BufferLine::time`'s own tests -- live pushes and the
    /// `/scrollback goto <timestamp>` search itself.
    mod line_time {
        use super::*;

        #[test]
        fn live_pushes_of_every_kind_get_a_real_epoch() {
            let mut buf = Buffer::new("#test");
            let before = now_epoch();
            buf.push_system("-- something happened");
            buf.push_chat("abcX", 0, None, "hi", false);
            buf.push_action("abcX", 0, None, "waves", false);
            let after = now_epoch();

            for line in &buf.lines {
                let t = line.time().expect("a live push must always know its own time");
                assert!((before..=after).contains(&t), "expected {t} within [{before}, {after}]");
            }
        }

        #[test]
        fn an_explicit_time_override_is_used_instead_of_now() {
            let mut buf = Buffer::new("#test");
            let historical = 1_700_000_000_i64; // 2023-11-14, nowhere near "now"
            buf.push_chat_at_colored("abcX", 0, None, "hi", false, None, None, Some(historical));
            assert_eq!(buf.lines.back().unwrap().time(), Some(historical));
        }

        fn chat_at(buf: &mut Buffer, epoch: i64) {
            buf.push_chat_at_colored("abcX", 0, None, "hi", false, None, None, Some(epoch));
        }

        #[test]
        fn first_index_at_or_after_finds_the_first_qualifying_line() {
            let mut buf = Buffer::new("#test");
            chat_at(&mut buf, 100);
            chat_at(&mut buf, 200);
            chat_at(&mut buf, 300);
            assert_eq!(buf.first_index_at_or_after(150), Some(1));
            assert_eq!(
                buf.first_index_at_or_after(200),
                Some(1),
                "an exact match counts as at-or-after"
            );
            assert_eq!(buf.first_index_at_or_after(100), Some(0));
        }

        #[test]
        fn first_index_at_or_after_is_none_when_the_target_is_newer_than_everything() {
            let mut buf = Buffer::new("#test");
            chat_at(&mut buf, 100);
            chat_at(&mut buf, 200);
            assert_eq!(buf.first_index_at_or_after(1_000), None);
        }

        #[test]
        fn first_index_at_or_after_skips_lines_with_an_unknown_time() {
            let mut buf = Buffer::new("#test");
            buf.push_system("-- reloaded from the legacy log");
            if let Some(BufferLine::System { time, .. }) = buf.lines.back_mut() {
                *time = None;
            }
            chat_at(&mut buf, 500);
            assert_eq!(
                buf.first_index_at_or_after(0),
                Some(1),
                "the unknown-time line must be skipped, not treated as a match at position 0"
            );
        }
    }
}
