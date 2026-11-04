use serde::{Deserialize, Serialize};

/// One `/ignore` entry: a mask (a bare nick, or a `nick!user@host`-
/// style glob) plus which levels it applies to. Empty `levels` means
/// "ignore everything from this mask" -- irssi's own default when no
/// level is given, and what every `/ignore` added before levels
/// existed here already meant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct IgnoreEntry {
    pub mask: String,
    #[serde(default)]
    pub levels: Vec<String>,
    #[serde(default)]
    pub is_regex: bool,
    #[serde(default)]
    pub is_except: bool,
    #[serde(default)]
    pub is_replies: bool,
    #[serde(default)]
    pub expires_at: Option<i64>,
}

pub fn parse_time_interval(input: &str) -> Option<u64> {
    let mut chars = input.trim().char_indices().peekable();
    let mut total: u64 = 0;
    let mut saw_any_group = false;
    loop {
        // Skip whitespace/punctuation between groups (mirrors irssi's
        // own "skip punctuation" pass between digit/unit pairs).
        while matches!(chars.peek(), Some((_, c)) if c.is_whitespace()) {
            chars.next();
        }
        let Some(&(digit_start, c)) = chars.peek() else {
            break;
        };
        if !c.is_ascii_digit() {
            return None;
        }
        let mut digit_end = digit_start;
        while let Some(&(idx, c)) = chars.peek() {
            if c.is_ascii_digit() {
                digit_end = idx + c.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let number: u64 = input[digit_start..digit_end].parse().ok()?;
        let unit_start = digit_end;
        let mut unit_end = unit_start;
        while let Some(&(idx, c)) = chars.peek() {
            if c.is_ascii_alphabetic() {
                unit_end = idx + c.len_utf8();
                chars.next();
            } else {
                break;
            }
        }
        let unit = &input[unit_start..unit_end];
        let seconds = if unit.is_empty() {
            number
        } else {
            let lower = unit.to_ascii_lowercase();
            if !lower.is_empty() && "days".starts_with(&lower) {
                number.checked_mul(86400)?
            } else if !lower.is_empty() && "hours".starts_with(&lower) {
                number.checked_mul(3600)?
            } else if !lower.is_empty() && ("minutes".starts_with(&lower) || "mins".starts_with(&lower)) {
                number.checked_mul(60)?
            } else if !lower.is_empty() && ("seconds".starts_with(&lower) || "secs".starts_with(&lower)) {
                number
            } else {
                return None;
            }
        };
        total = total.checked_add(seconds)?;
        saw_any_group = true;
    }
    saw_any_group.then_some(total)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum MaskOrEntry {
    Mask(String),
    Entry(IgnoreEntry),
}

impl From<MaskOrEntry> for IgnoreEntry {
    fn from(m: MaskOrEntry) -> Self {
        match m {
            MaskOrEntry::Mask(mask) => IgnoreEntry {
                mask,
                levels: Vec::new(),
                is_regex: false,
                is_except: false,
                is_replies: false,
                expires_at: None,
            },
            MaskOrEntry::Entry(e) => e,
        }
    }
}

#[derive(Deserialize, Default)]
struct IgnoreFileRaw {
    #[serde(default)]
    masks: Vec<MaskOrEntry>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct IgnoreFile {
    masks: Vec<IgnoreEntry>,
}

const IGNORES_FILE: &str = "ignores.toml";

/// Loads the persisted `/ignore` list (order matters -- `/unignore
/// <id>` removes by 1-based position, irssi's own indexing). A missing
/// or unparseable file just means none defined yet.
pub fn load_ignores() -> Vec<IgnoreEntry> {
    crate::storage::load_toml::<IgnoreFileRaw>(IGNORES_FILE)
        .masks
        .into_iter()
        .map(IgnoreEntry::from)
        .collect()
}

/// Overwrites the ignores file with the current list. Best-effort: a
/// save failure (disk full, permissions...) shouldn't crash the client.
pub fn save_ignores(entries: &[IgnoreEntry]) -> std::io::Result<()> {
    let file = IgnoreFile {
        masks: entries.to_vec(),
    };
    crate::storage::save_toml(IGNORES_FILE, &file, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips_to_an_empty_list() {
        let parsed: IgnoreFileRaw = toml::from_str("").unwrap();
        assert!(parsed.masks.is_empty());
    }

    #[test]
    fn file_with_entries_parses_correctly_and_preserves_order() {
        let parsed: IgnoreFileRaw = toml::from_str(
            r#"
            masks = ["BadNick", "*!*@spammer.example"]
            "#,
        )
        .unwrap();
        let entries: Vec<IgnoreEntry> = parsed.masks.into_iter().map(IgnoreEntry::from).collect();
        assert_eq!(
            entries,
            vec![
                IgnoreEntry {
                    mask: "BadNick".to_string(),
                    ..Default::default()
                },
                IgnoreEntry {
                    mask: "*!*@spammer.example".to_string(),
                    ..Default::default()
                },
            ]
        );
    }

    /// A file saved before `/ignore` levels existed (bare strings) must
    /// still load correctly, as entries with no level restriction --
    /// no migration step, no data loss.
    #[test]
    fn old_bare_string_entries_load_as_unrestricted() {
        let parsed: IgnoreFileRaw = toml::from_str(r#"masks = ["BadNick"]"#).unwrap();
        let entries: Vec<IgnoreEntry> = parsed.masks.into_iter().map(IgnoreEntry::from).collect();
        assert_eq!(
            entries,
            vec![IgnoreEntry {
                mask: "BadNick".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn entries_with_explicit_levels_parse_correctly() {
        let parsed: IgnoreFileRaw = toml::from_str(
            r#"
            [[masks]]
            mask = "BadNick"
            levels = ["notices", "ctcps"]
            "#,
        )
        .unwrap();
        let entries: Vec<IgnoreEntry> = parsed.masks.into_iter().map(IgnoreEntry::from).collect();
        assert_eq!(
            entries,
            vec![IgnoreEntry {
                mask: "BadNick".to_string(),
                levels: vec!["notices".to_string(), "ctcps".to_string()],
                ..Default::default()
            }]
        );
    }

    #[test]
    fn entries_saved_before_is_regex_existed_load_as_glob() {
        let parsed: IgnoreFileRaw = toml::from_str(
            r#"
            [[masks]]
            mask = "BadNick"
            levels = ["notices"]
            "#,
        )
        .unwrap();
        let entries: Vec<IgnoreEntry> = parsed.masks.into_iter().map(IgnoreEntry::from).collect();
        assert!(!entries[0].is_regex);
    }

    #[test]
    fn parse_time_interval_handles_irssi_style_durations() {
        assert_eq!(parse_time_interval("5m"), Some(300));
        assert_eq!(parse_time_interval("1h"), Some(3600));
        assert_eq!(parse_time_interval("2d"), Some(172800));
        assert_eq!(parse_time_interval("90"), Some(90));
        assert_eq!(parse_time_interval("1h30m"), Some(5400));
        assert_eq!(parse_time_interval("30s"), Some(30));
        assert_eq!(parse_time_interval("2days"), Some(172800));
        assert_eq!(parse_time_interval("1 hour"), None);
        assert_eq!(parse_time_interval("1hour"), Some(3600));
        assert_eq!(parse_time_interval(""), None);
        assert_eq!(parse_time_interval("abc"), None);
        assert_eq!(parse_time_interval("5x"), None);
    }

    #[test]
    fn expires_at_round_trips_through_save_and_load() {
        let entries = vec![
            IgnoreEntry {
                mask: "BadNick".to_string(),
                expires_at: Some(1_800_000_000),
                ..Default::default()
            },
            IgnoreEntry {
                mask: "OtherNick".to_string(),
                expires_at: None,
                ..Default::default()
            },
        ];
        let toml_str = toml::to_string_pretty(&IgnoreFile { masks: entries.clone() }).unwrap();
        let parsed: IgnoreFileRaw = toml::from_str(&toml_str).unwrap();
        let loaded: Vec<IgnoreEntry> = parsed.masks.into_iter().map(IgnoreEntry::from).collect();
        assert_eq!(loaded, entries);
    }

    #[test]
    fn entries_saved_before_expires_at_existed_load_as_no_expiry() {
        let parsed: IgnoreFileRaw = toml::from_str(
            r#"
            [[masks]]
            mask = "BadNick"
            levels = ["notices"]
            "#,
        )
        .unwrap();
        let entries: Vec<IgnoreEntry> = parsed.masks.into_iter().map(IgnoreEntry::from).collect();
        assert_eq!(entries[0].expires_at, None);
    }

    #[test]
    fn a_regex_entry_round_trips_through_save_and_load() {
        let entries = vec![IgnoreEntry {
            mask: r"^Bad.*\d+$".to_string(),
            levels: vec!["msgs".to_string()],
            is_regex: true,
            ..Default::default()
        }];
        let toml_str = toml::to_string_pretty(&IgnoreFile { masks: entries.clone() }).unwrap();
        let parsed: IgnoreFileRaw = toml::from_str(&toml_str).unwrap();
        let loaded: Vec<IgnoreEntry> = parsed.masks.into_iter().map(IgnoreEntry::from).collect();
        assert_eq!(loaded, entries);
    }
}
