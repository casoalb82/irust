use serde::{Deserialize, Serialize};

/// One `/hilight` entry: a pattern plus how/where it applies. Matches
/// `irust_config::IgnoreEntry`'s own shape closely (regex vs glob-style
/// mask matching, same reasoning).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct HilightEntry {
    pub pattern: String,
    /// `/hilight -regexp` -- `pattern` is a regex instead of a literal
    /// word/mask.
    #[serde(default)]
    pub is_regex: bool,
    /// `/hilight -mask` -- match against the sender's `nick!user@host`
    /// (glob, unless `is_regex` too) instead of the message text
    /// (word-boundary match, unless `is_regex`).
    #[serde(default)]
    pub is_mask: bool,
    /// `/hilight -channels #a,#b` -- restrict to these channels only
    /// (case-insensitive). Empty means every channel/query, matching
    /// every entry added before this flag existed.
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actcolor: Option<String>,
    #[serde(default)]
    pub priority: i32,
}

/// Accepts either a bare string (every `/hilight` saved before flags
/// existed) or a full `{pattern=..., ...}` table -- same untagged-enum
/// migration trick `irust_config::IgnoreEntry` already uses, so an
/// existing `hilights.toml` keeps loading unchanged.
#[derive(Deserialize)]
#[serde(untagged)]
enum PatternOrEntry {
    Pattern(String),
    Entry(HilightEntry),
}

impl From<PatternOrEntry> for HilightEntry {
    fn from(p: PatternOrEntry) -> Self {
        match p {
            PatternOrEntry::Pattern(pattern) => HilightEntry {
                pattern,
                ..Default::default()
            },
            PatternOrEntry::Entry(e) => e,
        }
    }
}

#[derive(Deserialize, Default)]
struct HilightFileRaw {
    #[serde(default)]
    patterns: Vec<PatternOrEntry>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct HilightFile {
    patterns: Vec<HilightEntry>,
}

const HILIGHTS_FILE: &str = "hilights.toml";

/// Loads the persisted `/hilight` list (order matters -- `/dehilight
/// <id>` removes by 1-based position, same convention as `/unignore`). A
/// missing or unparseable file just means none defined yet -- own-nick
/// mentions still highlight regardless, this list is purely additive
/// (see `client::handle_message_with_highlight`'s doc comment).
pub fn load_hilights() -> Vec<HilightEntry> {
    crate::storage::load_toml::<HilightFileRaw>(HILIGHTS_FILE)
        .patterns
        .into_iter()
        .map(HilightEntry::from)
        .collect()
}

/// Overwrites the hilights file with the current list. Best-effort: a
/// save failure (disk full, permissions...) shouldn't crash the client.
pub fn save_hilights(entries: &[HilightEntry]) -> std::io::Result<()> {
    let file = HilightFile {
        patterns: entries.to_vec(),
    };
    crate::storage::save_toml(HILIGHTS_FILE, &file, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips_to_an_empty_list() {
        let parsed: HilightFileRaw = toml::from_str("").unwrap();
        assert!(parsed.patterns.is_empty());
    }

    #[test]
    fn bare_string_entries_load_as_a_plain_word_pattern() {
        let parsed: HilightFileRaw = toml::from_str(
            r#"
            patterns = ["urgent", "outage"]
            "#,
        )
        .unwrap();
        let entries: Vec<HilightEntry> = parsed.patterns.into_iter().map(HilightEntry::from).collect();
        assert_eq!(
            entries,
            vec![
                HilightEntry {
                    pattern: "urgent".to_string(),
                    ..Default::default()
                },
                HilightEntry {
                    pattern: "outage".to_string(),
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn full_entries_with_flags_round_trip() {
        let toml_str = r##"
            [[patterns]]
            pattern = "^Bad.*Bot$"
            is_regex = true
            is_mask = true
            channels = ["#chatnet"]
        "##;
        let parsed: HilightFileRaw = toml::from_str(toml_str).unwrap();
        let entries: Vec<HilightEntry> = parsed.patterns.into_iter().map(HilightEntry::from).collect();
        assert_eq!(
            entries,
            vec![HilightEntry {
                pattern: "^Bad.*Bot$".to_string(),
                is_regex: true,
                is_mask: true,
                channels: vec!["#chatnet".to_string()],
                color: None,
                actcolor: None,
                priority: 0,
            }]
        );
    }

    #[test]
    fn save_and_load_round_trips_through_disk_format() {
        let entries = vec![
            HilightEntry {
                pattern: "urgent".to_string(),
                ..Default::default()
            },
            HilightEntry {
                pattern: "spammer".to_string(),
                is_mask: true,
                channels: vec!["#chatnet".to_string(), "#routing".to_string()],
                ..Default::default()
            },
        ];
        let file = HilightFile {
            patterns: entries.clone(),
        };
        let toml_str = toml::to_string_pretty(&file).unwrap();
        let parsed: HilightFileRaw = toml::from_str(&toml_str).unwrap();
        let round_tripped: Vec<HilightEntry> = parsed.patterns.into_iter().map(HilightEntry::from).collect();
        assert_eq!(round_tripped, entries);
    }
}
