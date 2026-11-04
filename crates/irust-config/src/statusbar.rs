use std::fs;
use std::sync::{OnceLock, RwLock};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Item {
    Clock,
    User,
    Window,
    Lag,
    Act,
}

impl Item {
    pub const ALL: [Item; 5] = [Item::Clock, Item::User, Item::Window, Item::Lag, Item::Act];

    pub fn name(&self) -> &'static str {
        match self {
            Item::Clock => "clock",
            Item::User => "user",
            Item::Window => "window",
            Item::Lag => "lag",
            Item::Act => "act",
        }
    }

    pub fn parse(s: &str) -> Option<Item> {
        match s.to_lowercase().as_str() {
            "clock" => Some(Item::Clock),
            "user" => Some(Item::User),
            "window" => Some(Item::Window),
            "lag" => Some(Item::Lag),
            "act" => Some(Item::Act),
            _ => None,
        }
    }

    /// Which side an item lands on when `/statusbar add` doesn't give an
    /// explicit `-alignment` -- matches its side in today's hardcoded
    /// layout, so an unqualified `add` never looks like a random choice.
    fn default_alignment(&self) -> Alignment {
        match self {
            Item::Lag | Item::Act => Alignment::Right,
            Item::Clock | Item::User | Item::Window => Alignment::Left,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Alignment {
    Left,
    Right,
}

impl Alignment {
    pub fn parse(s: &str) -> Option<Alignment> {
        match s.to_lowercase().as_str() {
            "left" => Some(Alignment::Left),
            "right" => Some(Alignment::Right),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Alignment::Left => "left",
            Alignment::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Entry {
    pub item: Item,
    pub alignment: Alignment,
}

pub fn default_entries() -> Vec<Entry> {
    vec![
        Entry {
            item: Item::Clock,
            alignment: Alignment::Left,
        },
        Entry {
            item: Item::User,
            alignment: Alignment::Left,
        },
        Entry {
            item: Item::Window,
            alignment: Alignment::Left,
        },
        Entry {
            item: Item::Lag,
            alignment: Alignment::Right,
        },
        Entry {
            item: Item::Act,
            alignment: Alignment::Right,
        },
    ]
}

static ITEMS: OnceLock<RwLock<Vec<Entry>>> = OnceLock::new();

fn cell() -> &'static RwLock<Vec<Entry>> {
    ITEMS.get_or_init(|| RwLock::new(default_entries()))
}

/// Current layout, as `irust-tui` sees it. Before anything ever calls
/// `load_statusbar()` (e.g. every existing `buflist.rs` test), this
/// returns `default_entries()` -- so the default rendering stays exactly
/// what it was before this file existed, with zero setup required.
pub fn items() -> Vec<Entry> {
    cell().read().unwrap().clone()
}

fn set_items_global(entries: Vec<Entry>) {
    *cell().write().unwrap() = entries;
}

fn statusbar_path() -> Option<std::path::PathBuf> {
    ProjectDirs::from("", "", "irust").map(|dirs| dirs.data_dir().join("statusbar.toml"))
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Statusbar {
    #[serde(default = "default_entries")]
    entries: Vec<Entry>,
}

impl Default for Statusbar {
    fn default() -> Self {
        Statusbar {
            entries: default_entries(),
        }
    }
}

impl Statusbar {
    /// Publishes the current layout to the live global -- call after
    /// every mutation, same convention as `Settings::apply()`.
    fn apply(&self) {
        set_items_global(self.entries.clone());
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// `/statusbar list` -- current order, split by side (irssi shows
    /// its statusbars this way too: which items, which side, in order).
    pub fn list_text(&self) -> String {
        let side = |wanted: Alignment| -> String {
            let names: Vec<&str> = self
                .entries
                .iter()
                .filter(|e| e.alignment == wanted)
                .map(|e| e.item.name())
                .collect();
            if names.is_empty() {
                "(none)".to_string()
            } else {
                names.join(", ")
            }
        };
        format!("left: {} | right: {}", side(Alignment::Left), side(Alignment::Right))
    }

    pub fn add(
        &mut self,
        name: &str,
        before: Option<&str>,
        after: Option<&str>,
        alignment: Option<&str>,
    ) -> anyhow::Result<()> {
        let item = Item::parse(name).ok_or_else(|| anyhow::anyhow!(unknown_item_message(name)))?;
        if self.entries.iter().any(|e| e.item == item) {
            anyhow::bail!("'{}' is already in the statusbar", item.name());
        }
        let alignment = match alignment {
            Some(a) => Alignment::parse(a)
                .ok_or_else(|| anyhow::anyhow!("invalid alignment '{a}' -- expected left or right"))?,
            None => item.default_alignment(),
        };
        let entry = Entry { item, alignment };

        if let Some(before_name) = before {
            let before_item =
                Item::parse(before_name).ok_or_else(|| anyhow::anyhow!(unknown_item_message(before_name)))?;
            let pos = self
                .entries
                .iter()
                .position(|e| e.item == before_item)
                .ok_or_else(|| anyhow::anyhow!("'{}' is not in the statusbar", before_item.name()))?;
            self.entries.insert(pos, entry);
        } else if let Some(after_name) = after {
            let after_item =
                Item::parse(after_name).ok_or_else(|| anyhow::anyhow!(unknown_item_message(after_name)))?;
            let pos = self
                .entries
                .iter()
                .position(|e| e.item == after_item)
                .ok_or_else(|| anyhow::anyhow!("'{}' is not in the statusbar", after_item.name()))?;
            self.entries.insert(pos + 1, entry);
        } else {
            self.entries.push(entry);
        }

        self.apply();
        Ok(())
    }

    /// `/statusbar remove <item>`.
    pub fn remove(&mut self, name: &str) -> anyhow::Result<()> {
        let item = Item::parse(name).ok_or_else(|| anyhow::anyhow!(unknown_item_message(name)))?;
        let before_len = self.entries.len();
        self.entries.retain(|e| e.item != item);
        if self.entries.len() == before_len {
            anyhow::bail!("'{}' is not in the statusbar", item.name());
        }
        self.apply();
        Ok(())
    }

    /// `/statusbar reset` -- back to today's hardcoded default order.
    pub fn reset(&mut self) {
        self.entries = default_entries();
        self.apply();
    }
}

fn unknown_item_message(name: &str) -> String {
    format!(
        "unknown statusbar item '{name}' -- expected one of: {}",
        Item::ALL.iter().map(|i| i.name()).collect::<Vec<_>>().join(", ")
    )
}

pub fn load_statusbar() -> Statusbar {
    let sb: Statusbar = statusbar_path()
        .and_then(|path| fs::read_to_string(&path).ok().map(|contents| (path, contents)))
        .and_then(|(path, contents)| crate::warnings::parse_toml_or_warn(&contents, &path))
        .unwrap_or_default();
    sb.apply();
    sb
}

/// Overwrites the statusbar file with the current layout. Best-effort: a
/// save failure (disk full, permissions...) shouldn't crash the client.
pub fn save_statusbar(sb: &Statusbar) -> std::io::Result<()> {
    let Some(path) = statusbar_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(sb).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(path, toml_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    static STATUSBAR_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock_statusbar() -> std::sync::MutexGuard<'static, ()> {
        STATUSBAR_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn default_order_matches_todays_hardcoded_layout() {
        let _guard = lock_statusbar();
        let sb = Statusbar::default();
        let names: Vec<&str> = sb.entries().iter().map(|e| e.item.name()).collect();
        assert_eq!(names, vec!["clock", "user", "window", "lag", "act"]);
        assert_eq!(sb.entries()[0].alignment, Alignment::Left);
        assert_eq!(
            sb.entries()[3].alignment,
            Alignment::Right,
            "lag must default to the right side"
        );
    }

    #[test]
    fn empty_file_round_trips_to_the_default_layout() {
        let _guard = lock_statusbar();
        let parsed: Statusbar = toml::from_str("").unwrap();
        assert_eq!(parsed, Statusbar::default());
    }

    #[test]
    fn add_appends_on_its_default_side_when_no_position_given() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("act").unwrap();
        assert!(sb.add("act", None, None, None).is_ok());
        let names: Vec<&str> = sb.entries().iter().map(|e| e.item.name()).collect();
        assert_eq!(names, vec!["clock", "user", "window", "lag", "act"]);
    }

    #[test]
    fn add_rejects_an_unknown_item() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        let err = sb.add("nonexistent", None, None, None).unwrap_err().to_string();
        assert!(err.contains("nonexistent"), "got: {err}");
    }

    #[test]
    fn add_rejects_a_duplicate() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        let err = sb.add("clock", None, None, None).unwrap_err().to_string();
        assert!(err.contains("already"), "got: {err}");
    }

    #[test]
    fn add_before_inserts_at_the_right_position() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("window").unwrap();
        assert!(sb.add("window", Some("user"), None, None).is_ok());
        let names: Vec<&str> = sb.entries().iter().map(|e| e.item.name()).collect();
        assert_eq!(names, vec!["clock", "window", "user", "lag", "act"]);
    }

    #[test]
    fn add_after_inserts_at_the_right_position() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("window").unwrap();
        assert!(sb.add("window", None, Some("user"), None).is_ok());
        let names: Vec<&str> = sb.entries().iter().map(|e| e.item.name()).collect();
        assert_eq!(names, vec!["clock", "user", "window", "lag", "act"]);
    }

    #[test]
    fn add_rejects_an_unknown_before_target() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("window").unwrap();
        let err = sb
            .add("window", Some("nonexistent"), None, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("nonexistent"), "got: {err}");
    }

    #[test]
    fn add_respects_an_explicit_alignment_override() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("clock").unwrap();
        assert!(sb.add("clock", None, None, Some("right")).is_ok());
        let entry = sb.entries().iter().find(|e| e.item == Item::Clock).unwrap();
        assert_eq!(entry.alignment, Alignment::Right);
    }

    #[test]
    fn add_rejects_an_invalid_alignment() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("clock").unwrap();
        let err = sb.add("clock", None, None, Some("sideways")).unwrap_err().to_string();
        assert!(err.contains("sideways"), "got: {err}");
    }

    #[test]
    fn remove_rejects_an_item_thats_not_present() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("act").unwrap();
        let err = sb.remove("act").unwrap_err().to_string();
        assert!(err.contains("not in the statusbar"), "got: {err}");
    }

    #[test]
    fn reset_restores_the_default_order_after_changes() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("act").unwrap();
        sb.remove("lag").unwrap();
        sb.reset();
        assert_eq!(sb, Statusbar::default());
    }

    #[test]
    fn list_text_splits_by_side() {
        let _guard = lock_statusbar();
        let sb = Statusbar::default();
        assert_eq!(sb.list_text(), "left: clock, user, window | right: lag, act");
    }

    #[test]
    fn mutating_a_statusbar_publishes_to_the_live_global() {
        let _guard = lock_statusbar();
        let mut sb = Statusbar::default();
        sb.remove("act").unwrap();
        let names: Vec<&str> = items().iter().map(|e| e.item.name()).collect();
        assert!(
            !names.contains(&"act"),
            "removing an item must publish live, got: {names:?}"
        );
        sb.reset();
        let names: Vec<&str> = items().iter().map(|e| e.item.name()).collect();
        assert!(names.contains(&"act"), "reset must publish live too, got: {names:?}");
    }
}
