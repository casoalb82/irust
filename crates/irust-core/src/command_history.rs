
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CommandHistory {
    entries: Vec<String>,
    /// Index of the entry currently shown, `None` while the input is the
    /// live line.
    pos: Option<usize>,
    /// irssi's `redo`: set when the shown entry was just removed to make
    /// room, so the next Down keeps the position `add` moved it to
    /// instead of advancing once more.
    redo: bool,
}

impl CommandHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// `command_history_add`: skip when identical to the newest entry;
    /// when `max` (>= 1) entries are held, drop the oldest first.
    /// `max < 1` means unlimited, as in irssi.
    pub fn add(&mut self, text: &str, max: i64) {
        if self.entries.last().is_some_and(|last| last == text) {
            return;
        }
        if max >= 1 && self.entries.len() as i64 >= max {
            self.entries.remove(0);
            match self.pos {
                // The shown entry itself was dropped: move to whatever
                // is now first and remember not to advance again.
                Some(0) => {
                    self.pos = if self.entries.is_empty() { None } else { Some(0) };
                    self.redo = true;
                }
                Some(i) => self.pos = Some(i - 1),
                None => {}
            }
        }
        self.entries.push(text.to_string());
    }

    /// A line was sent: record it and go back to the live line
    /// (`gui-readline.c`'s send path: `command_history_add` then
    /// `command_history_clear_pos`).
    pub fn record_sent(&mut self, text: &str, max: i64) {
        if !text.is_empty() {
            self.add(text, max);
        }
        self.pos = None;
    }

    /// Up. `text` is the current input; returns the new input.
    pub fn prev(&mut self, text: &str, max: i64, editable: bool) -> String {
        let before = self.pos;
        self.redo = false;
        self.pos = match before {
            // No wrap past the oldest entry.
            Some(i) => Some(i.saturating_sub(1)),
            None => self.entries.len().checked_sub(1),
        };
        self.save_edited(before, text, max, editable);
        match self.pos {
            Some(i) => self.entries[i].clone(),
            None => text.to_string(),
        }
    }

    /// Down. `text` is the current input; returns the new input.
    pub fn next(&mut self, text: &str, max: i64, editable: bool) -> String {
        let before = self.pos;
        if !self.redo {
            if let Some(i) = before {
                self.pos = if i + 1 < self.entries.len() { Some(i + 1) } else { None };
            }
        }
        self.redo = false;
        self.save_edited(before, text, max, editable);
        match self.pos {
            Some(i) => self.entries[i].clone(),
            None => String::new(),
        }
    }

    /// The "save the old entry to history" step shared by `prev`/`next`:
    /// non-empty input that differs from the entry it was recalled from
    /// is either written over that entry (`editable`) or added.
    fn save_edited(&mut self, before: Option<usize>, text: &str, max: i64, editable: bool) {
        if text.is_empty() {
            return;
        }
        let unchanged = before.is_some_and(|i| self.entries.get(i).is_some_and(|e| e == text));
        if unchanged {
            return;
        }
        match before {
            Some(i) if editable && i < self.entries.len() => self.entries[i] = text.to_string(),
            _ => self.add(text, max),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(lines: &[&str]) -> CommandHistory {
        let mut h = CommandHistory::new();
        for l in lines {
            h.record_sent(l, 100);
        }
        h
    }

    #[test]
    fn up_walks_back_and_stops_at_the_oldest() {
        let mut h = filled(&["one", "two", "three"]);
        assert_eq!(h.prev("", 100, false), "three");
        assert_eq!(h.prev("three", 100, false), "two");
        assert_eq!(h.prev("two", 100, false), "one");
        assert_eq!(h.prev("one", 100, false), "one", "no wrap past the first entry");
    }

    #[test]
    fn down_past_the_newest_returns_to_an_empty_live_line() {
        let mut h = filled(&["one", "two"]);
        h.prev("", 100, false);
        h.prev("two", 100, false);
        assert_eq!(h.next("one", 100, false), "two");
        assert_eq!(h.next("two", 100, false), "");
    }

    #[test]
    fn unsent_text_is_stored_in_history_when_you_press_up() {
        let mut h = filled(&["old"]);
        assert_eq!(h.prev("half typed", 100, false), "old");
        assert_eq!(h.entries(), ["old", "half typed"], "the draft became the newest entry");
        assert_eq!(h.next("old", 100, false), "half typed", "Down brings it back");
    }

    #[test]
    fn an_edited_recalled_line_is_appended_unless_editable() {
        let mut h = filled(&["one", "two"]);
        h.prev("", 100, false); // showing "two"
        h.prev("two changed", 100, false); // move up, leaving an edit behind
        assert_eq!(h.entries(), ["one", "two", "two changed"]);

        let mut e = filled(&["one", "two"]);
        e.prev("", 100, true);
        e.prev("two changed", 100, true);
        assert_eq!(e.entries(), ["one", "two changed"], "editable rewrites the entry in place");
    }

    #[test]
    fn sending_the_same_line_twice_stores_it_once() {
        let h = filled(&["a", "a", "b", "b", "a"]);
        assert_eq!(h.entries(), ["a", "b", "a"]);
    }

    #[test]
    fn the_cap_drops_the_oldest_entry() {
        let mut h = CommandHistory::new();
        for l in ["1", "2", "3", "4"] {
            h.record_sent(l, 3);
        }
        assert_eq!(h.entries(), ["2", "3", "4"]);
    }

    #[test]
    fn a_cap_below_one_means_unlimited() {
        let mut h = CommandHistory::new();
        for i in 0..250 {
            h.record_sent(&format!("l{i}"), 0);
        }
        assert_eq!(h.entries().len(), 250);
    }

    #[test]
    fn dropping_the_shown_entry_keeps_the_position_without_skipping() {
        let mut h = CommandHistory::new();
        for l in ["1", "2", "3"] {
            h.record_sent(l, 3);
        }
        h.prev("", 3, false);
        h.prev("3", 3, false);
        h.prev("2", 3, false); // showing "1", the oldest
        // A draft is stored while the oldest is shown: the cap drops "1".
        assert_eq!(h.prev("draft", 3, false), "2", "position moved to what is now first");
        assert_eq!(h.next("2", 3, false), "2", "redo: Down does not skip past it");
    }

    #[test]
    fn sending_returns_to_the_live_line() {
        let mut h = filled(&["one", "two"]);
        h.prev("", 100, false);
        h.record_sent("three", 100);
        assert_eq!(h.prev("", 100, false), "three");
    }

    #[test]
    fn empty_history_up_leaves_the_text_alone() {
        let mut h = CommandHistory::new();
        assert_eq!(h.prev("typing", 100, false), "typing");
        assert_eq!(h.entries(), ["typing"]);
    }
}
