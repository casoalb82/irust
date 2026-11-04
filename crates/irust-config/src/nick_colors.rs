use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct NickColorFile {
    #[serde(default)]
    colors: HashMap<String, usize>,
}

const NICK_COLORS_FILE: &str = "nick_colors.toml";

/// Loads persisted per-nick color overrides (`/color <nick> <index>`).
/// A missing or unparseable file just means no overrides yet -- same as
/// a fresh install, never an error.
pub fn load_nick_colors() -> HashMap<String, usize> {
    crate::storage::load_toml::<NickColorFile>(NICK_COLORS_FILE).colors
}

/// Overwrites the nick-colors file with the current override map.
/// Best-effort: a save failure (disk full, permissions...) shouldn't
/// crash the client, so errors are returned for the caller to decide
/// whether to surface anything, not panicked on.
pub fn save_nick_colors(colors: &HashMap<String, usize>) -> std::io::Result<()> {
    let file = NickColorFile { colors: colors.clone() };
    crate::storage::save_toml(NICK_COLORS_FILE, &file, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips_to_an_empty_map() {
        let parsed: NickColorFile = toml::from_str("").unwrap();
        assert!(parsed.colors.is_empty());
    }

    #[test]
    fn file_with_entries_parses_correctly() {
        let parsed: NickColorFile = toml::from_str(
            r#"
            [colors]
            k9 = 7
            someuser = 3
            "#,
        )
        .unwrap();
        assert_eq!(parsed.colors.get("k9"), Some(&7));
        assert_eq!(parsed.colors.get("someuser"), Some(&3));
    }
}
