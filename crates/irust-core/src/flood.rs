
use std::collections::{HashMap, VecDeque};

use irust_config::settings_history_log as cfg;
use irust_config::IgnoreEntry;

#[derive(Debug, Default)]
struct Item {
    level: &'static str,
    target: String,
    times: VecDeque<i64>,
}

#[derive(Debug, Default)]
pub struct FloodTracker {
    /// Keyed by lowercased nick.
    nicks: HashMap<String, Vec<Item>>,
}

impl FloodTracker {
    /// irssi's `flood_newmsg`: record one message and say whether the
    /// sender is now flooding. `level` is an irssi level name
    /// (`publics`, `msgs`, `notices`, `ctcps`).
    pub fn note(&mut self, nick: &str, level: &'static str, target: &str, now: i64, timecheck: i64, max_msgs: i64) -> bool {
        // Cheap upkeep in place of irssi's 5-second sweep: forget senders
        // that have gone quiet once the table gets large.
        if self.nicks.len() > 256 {
            self.prune(now, timecheck);
        }
        let items = self.nicks.entry(nick.to_ascii_lowercase()).or_default();
        let pos = items.iter().position(|i| i.level == level && i.target.eq_ignore_ascii_case(target));
        match pos {
            Some(p) => {
                let item = &mut items[p];
                while item.times.front().is_some_and(|t| now - *t >= timecheck) {
                    item.times.pop_front();
                }
                item.times.push_back(now);
                item.times.len() as i64 > max_msgs
            }
            None => {
                items.push(Item { level, target: target.to_string(), times: VecDeque::from([now]) });
                false
            }
        }
    }

    /// `flood_timeout`: drop counters whose messages are all older than
    /// `timecheck`, and senders with none left.
    pub fn prune(&mut self, now: i64, timecheck: i64) {
        self.nicks.retain(|_, items| {
            items.retain_mut(|i| {
                i.times.retain(|t| now - *t < timecheck);
                !i.times.is_empty()
            });
            !items.is_empty()
        });
    }

    pub fn clear(&mut self) {
        self.nicks.clear();
    }
}

/// `sig_flood`: if `level` is one `autoignore_level` covers, add an
/// ignore for `mask` lasting `autoignore_time`, or widen the existing
/// automatic one (its levels union, its expiry restarts).
pub fn autoignore(list: &mut Vec<IgnoreEntry>, mask: &str, level: &'static str, now: i64) {
    let Some(want) = cfg::level_bits(level) else { return };
    if cfg::get_level("autoignore_level") & want == 0 {
        return;
    }
    let expires_at = now + cfg::get_time_ms("autoignore_time") / 1000;
    match list.iter_mut().find(|e| e.mask.eq_ignore_ascii_case(mask)) {
        Some(existing) => {
            if !existing.levels.iter().any(|l| l == level) {
                existing.levels.push(level.to_string());
            }
            existing.expires_at = Some(expires_at);
        }
        None => list.push(IgnoreEntry {
            mask: mask.to_string(),
            levels: vec![level.to_string()],
            expires_at: Some(expires_at),
            ..IgnoreEntry::default()
        }),
    }
}

/// Automatic ignores still in force at `now`.
pub fn live_entries(list: &mut Vec<IgnoreEntry>, now: i64) {
    list.retain(|e| e.expires_at.is_none_or(|t| t > now));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn more_than_max_messages_inside_the_window_is_a_flood() {
        let mut t = FloodTracker::default();
        // max 4 in 8s: the 5th message inside the window floods.
        for s in 0..4 {
            assert!(!t.note("Bob", "publics", "#c", 100 + s, 8, 4), "message {s}");
        }
        assert!(t.note("bob", "publics", "#C", 104, 8, 4), "nick and target compare case-insensitively");
    }

    #[test]
    fn old_messages_age_out_of_the_window() {
        let mut t = FloodTracker::default();
        for s in 0..4 {
            t.note("bob", "msgs", "me", s, 8, 4);
        }
        // 20s later the earlier four no longer count.
        assert!(!t.note("bob", "msgs", "me", 20, 8, 4));
        assert!(!t.note("bob", "msgs", "me", 21, 8, 4));
    }

    #[test]
    fn levels_and_targets_are_counted_separately() {
        let mut t = FloodTracker::default();
        for s in 0..4 {
            t.note("bob", "publics", "#a", s, 8, 4);
            t.note("bob", "publics", "#b", s, 8, 4);
            t.note("bob", "notices", "#a", s, 8, 4);
        }
        assert!(!t.note("bob", "publics", "#c", 5, 8, 4), "a fresh (level, target) starts at one");
        assert!(t.note("bob", "publics", "#a", 5, 8, 4));
    }

    #[test]
    fn prune_forgets_quiet_senders() {
        let mut t = FloodTracker::default();
        t.note("bob", "msgs", "me", 0, 8, 4);
        t.prune(100, 8);
        assert!(t.nicks.is_empty());
    }

    #[test]
    fn autoignore_only_covers_the_configured_levels_and_extends() {
        let mut settings = irust_config::settings::Settings::default();
        let mut list = Vec::new();
        autoignore(&mut list, "bob!u@h", "publics", 1000);
        assert!(list.is_empty(), "autoignore_level is empty by default: nothing is ignored");

        settings.set("autoignore_level", "publics notices").unwrap();
        settings.set("autoignore_time", "10min").unwrap();
        autoignore(&mut list, "bob!u@h", "publics", 1000);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].levels, ["publics"]);
        assert_eq!(list[0].expires_at, Some(1600));
        autoignore(&mut list, "BOB!u@h", "notices", 1100);
        assert_eq!(list.len(), 1, "the same mask is updated, not duplicated");
        assert_eq!(list[0].levels, ["publics", "notices"]);
        assert_eq!(list[0].expires_at, Some(1700), "the expiry restarts");
        autoignore(&mut list, "bob!u@h", "msgs", 1100);
        assert_eq!(list[0].levels.len(), 2, "msgs is not an autoignore level");

        live_entries(&mut list, 1699);
        assert_eq!(list.len(), 1);
        live_entries(&mut list, 1700);
        assert!(list.is_empty(), "expired");
        settings.set("autoignore_level", "").unwrap();
        settings.set("autoignore_time", "5min").unwrap();
    }
}
