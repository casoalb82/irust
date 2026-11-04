use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct AutobanEntry {
    pub pattern: String,
    #[serde(default)]
    pub is_regex: bool,
    /// Optional per-entry ban/kick reason. `None` falls back to a
    /// generic "spam" reason at execution time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Accepts either a bare string (the common case -- just a phrase to
/// match) or a full `{pattern=..., ...}` table, same untagged-enum
/// migration trick `HilightEntry`/`IgnoreEntry` already use.
#[derive(Deserialize)]
#[serde(untagged)]
enum PatternOrEntry {
    Pattern(String),
    Entry(AutobanEntry),
}

impl From<PatternOrEntry> for AutobanEntry {
    fn from(p: PatternOrEntry) -> Self {
        match p {
            PatternOrEntry::Pattern(pattern) => AutobanEntry {
                pattern,
                ..Default::default()
            },
            PatternOrEntry::Entry(e) => e,
        }
    }
}

#[derive(Deserialize, Default)]
struct AutobanFileRaw {
    #[serde(default)]
    patterns: Vec<PatternOrEntry>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct AutobanFile {
    patterns: Vec<AutobanEntry>,
}

const AUTOBAN_FILE: &str = "autoban.toml";

pub fn load_autoban() -> Vec<AutobanEntry> {
    crate::storage::load_toml::<AutobanFileRaw>(AUTOBAN_FILE)
        .patterns
        .into_iter()
        .map(AutobanEntry::from)
        .collect()
}

/// Overwrites the autoban file with the current list. Best-effort: a
/// save failure (disk full, permissions...) shouldn't crash the client.
pub fn save_autoban(entries: &[AutobanEntry]) -> std::io::Result<()> {
    let file = AutobanFile {
        patterns: entries.to_vec(),
    };
    crate::storage::save_toml(AUTOBAN_FILE, &file, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips_to_an_empty_list() {
        let parsed: AutobanFileRaw = toml::from_str("").unwrap();
        assert!(parsed.patterns.is_empty());
    }

    #[test]
    fn bare_string_entries_parse_as_substring_patterns() {
        let parsed: AutobanFileRaw = toml::from_str(
            r#"
            patterns = ["Hello everyone! I am"]
            "#,
        )
        .unwrap();
        let entries: Vec<AutobanEntry> = parsed.patterns.into_iter().map(AutobanEntry::from).collect();
        assert_eq!(
            entries,
            vec![AutobanEntry {
                pattern: "Hello everyone! I am".to_string(),
                ..Default::default()
            }]
        );
    }

    #[test]
    fn full_table_entries_keep_their_own_flags() {
        let parsed: AutobanFileRaw = toml::from_str(
            r#"
            [[patterns]]
            pattern = "webcam.*\\.mpg"
            is_regex = true
            reason = "spam"
            "#,
        )
        .unwrap();
        let entries: Vec<AutobanEntry> = parsed.patterns.into_iter().map(AutobanEntry::from).collect();
        assert_eq!(
            entries,
            vec![AutobanEntry {
                pattern: "webcam.*\\.mpg".to_string(),
                is_regex: true,
                reason: Some("spam".to_string()),
            }]
        );
    }
}
