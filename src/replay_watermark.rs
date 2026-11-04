
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use irust_proto::RawMessage;

/// A line this much older than "now" counts as playback rather than live,
/// whatever the clocks say (guards against clock skew between the bouncer
/// and this machine).
const REPLAY_AGE_SECS: i64 = 60;
/// Minimum time between two writes of the watermark file.
const SAVE_EVERY: Duration = Duration::from_secs(5);

struct Inner {
    marks: HashMap<String, i64>,
    path: Option<PathBuf>,
    dirty: bool,
    last_save: Instant,
}

fn inner() -> &'static Mutex<Inner> {
    static INNER: OnceLock<Mutex<Inner>> = OnceLock::new();
    INNER.get_or_init(|| {
        let path = directories::ProjectDirs::from("", "", "irust").map(|d| d.data_dir().join("replay_watermark.txt"));
        let marks = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| parse_marks(&t))
            .unwrap_or_default();
        Mutex::new(Inner {
            marks,
            path,
            dirty: false,
            last_save: Instant::now(),
        })
    })
}

fn now_epoch() -> i64 {
    chrono::Utc::now().timestamp()
}

/// `tag<TAB>epoch` per line; anything malformed is skipped.
fn parse_marks(text: &str) -> HashMap<String, i64> {
    text.lines()
        .filter_map(|l| {
            let (tag, epoch) = l.split_once('\t')?;
            Some((tag.to_string(), epoch.trim().parse().ok()?))
        })
        .collect()
}

fn format_marks(marks: &HashMap<String, i64>) -> String {
    let mut lines: Vec<String> = marks.iter().map(|(t, e)| format!("{t}\t{e}")).collect();
    lines.sort();
    lines.join("\n") + "\n"
}

fn save_locked(inner: &mut Inner) {
    inner.dirty = false;
    inner.last_save = Instant::now();
    let Some(path) = inner.path.clone() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, format_marks(&inner.marks)).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// The cutoff for a connection that is starting now: when this network was
/// last alive. `None` on a first run -- nothing is filtered then.
pub fn begin(tag: &str) -> Option<i64> {
    inner().lock().unwrap().marks.get(tag).copied()
}

/// Records that the connection to `tag` is alive right now.
pub fn touch(tag: &str) {
    let mut inner = inner().lock().unwrap();
    inner.marks.insert(tag.to_string(), now_epoch());
    inner.dirty = true;
    if inner.last_save.elapsed() >= SAVE_EVERY {
        save_locked(&mut inner);
    }
}

/// Writes the watermark now if it changed since the last write. Called every
/// lag-ping tick and on exit, so a quiet session still records when it ended.
pub fn flush() {
    let mut inner = inner().lock().unwrap();
    if inner.dirty {
        save_locked(&mut inner);
    }
}

fn is_channel_target(target: &str) -> bool {
    target
        .trim_start_matches(['@', '%', '+', '~'])
        .starts_with(['#', '&', '!'])
}

fn server_epoch(msg: &RawMessage) -> Option<i64> {
    let raw = msg.tags.get("time")?.as_deref()?;
    Some(chrono::DateTime::parse_from_rfc3339(raw).ok()?.timestamp())
}

/// `true` for a replayed PRIVATE line (a PRIVMSG/NOTICE that is not a channel
/// message) the user has already seen: stamped at or before `cutoff` and
/// older than a minute.
pub fn is_stale_private(cutoff: Option<i64>, msg: &RawMessage) -> bool {
    is_stale_private_at(cutoff, msg, now_epoch())
}

fn is_stale_private_at(cutoff: Option<i64>, msg: &RawMessage, now: i64) -> bool {
    let Some(cutoff) = cutoff else { return false };
    if msg.command != "PRIVMSG" && msg.command != "NOTICE" {
        return false;
    }
    let Some(target) = msg.params.first() else { return false };
    if is_channel_target(target) {
        return false;
    }
    match server_epoch(msg) {
        Some(t) => t <= cutoff && now - t > REPLAY_AGE_SECS,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irust_proto::parse;

    const NOW: i64 = 1_800_000_000;

    fn line(time_offset: i64, rest: &str) -> RawMessage {
        let t = chrono::DateTime::from_timestamp(NOW + time_offset, 0)
            .unwrap()
            .format("%Y-%m-%dT%H:%M:%S.000Z");
        parse(&format!("@time={t} {rest}")).unwrap()
    }

    #[test]
    fn old_private_lines_before_the_cutoff_are_dropped() {
        let old_pm = line(-7200, ":K9!k@h NOTICE abcX :Authenticated");
        let own_cmd = line(-7200, ":abcX!u@h PRIVMSG K9 :auth #chatnet secret");
        let cutoff = Some(NOW - 60);
        assert!(is_stale_private_at(cutoff, &old_pm, NOW));
        assert!(is_stale_private_at(cutoff, &own_cmd, NOW));
    }

    #[test]
    fn what_arrived_while_away_is_kept() {
        let missed = line(-300, ":Bob!b@h PRIVMSG abcX :ping me back");
        assert!(!is_stale_private_at(Some(NOW - 3600), &missed, NOW), "newer than the cutoff");
    }

    #[test]
    fn channel_lines_are_never_touched() {
        let chan = line(-7200, ":Bob!b@h PRIVMSG #italy :ciao");
        let statusmsg = line(-7200, ":Bob!b@h NOTICE @#italy :ops only");
        assert!(!is_stale_private_at(Some(NOW), &chan, NOW));
        assert!(!is_stale_private_at(Some(NOW), &statusmsg, NOW));
    }

    #[test]
    fn live_lines_and_first_runs_are_never_dropped() {
        let live = line(-5, ":Bob!b@h PRIVMSG abcX :hi");
        assert!(!is_stale_private_at(Some(NOW + 100), &live, NOW), "under a minute old = live, even with clock skew");
        let old = line(-7200, ":Bob!b@h PRIVMSG abcX :hi");
        assert!(!is_stale_private_at(None, &old, NOW), "no watermark yet: filter nothing");
        let untagged = parse(":Bob!b@h PRIVMSG abcX :hi").unwrap();
        assert!(!is_stale_private_at(Some(NOW), &untagged, NOW), "no server-time: cannot tell, keep");
        let other = line(-7200, ":srv 001 abcX :Welcome");
        assert!(!is_stale_private_at(Some(NOW), &other, NOW), "only PRIVMSG/NOTICE");
    }

    #[test]
    fn watermark_file_round_trips() {
        let mut m = HashMap::new();
        m.insert("chatnet".to_string(), 1_700_000_000);
        m.insert("libera".to_string(), 1_700_000_500);
        assert_eq!(parse_marks(&format_marks(&m)), m);
        assert!(parse_marks("garbage\nfoo\tbar\n").is_empty());
    }
}
