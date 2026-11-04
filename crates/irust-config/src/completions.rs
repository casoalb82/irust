use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct CompletionFile {
    #[serde(default)]
    completions: HashMap<String, String>,
}

const COMPLETIONS_FILE: &str = "completions.toml";

pub fn load_completions() -> HashMap<String, String> {
    crate::storage::load_toml::<CompletionFile>(COMPLETIONS_FILE).completions
}

/// Overwrites the completions file with the current map. Best-effort: a
/// save failure (disk full, permissions...) shouldn't crash the client.
pub fn save_completions(completions: &HashMap<String, String>) -> std::io::Result<()> {
    let file = CompletionFile {
        completions: completions.clone(),
    };
    crate::storage::save_toml(COMPLETIONS_FILE, &file, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips_to_an_empty_map() {
        let parsed: CompletionFile = toml::from_str("").unwrap();
        assert!(parsed.completions.is_empty());
    }

    #[test]
    fn file_with_entries_parses_correctly() {
        let parsed: CompletionFile = toml::from_str(
            r#"
            [completions]
            brb = "be right back"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.completions.get("brb"), Some(&"be right back".to_string()));
    }
}
