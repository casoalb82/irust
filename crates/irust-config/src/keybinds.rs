use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct KeybindFile {
    #[serde(default)]
    binds: HashMap<String, String>,
}

const KEYBINDS_FILE: &str = "keybinds.toml";

/// Loads persisted `/bind` overrides (canonical key-name -> canonical
/// action-name, see `irust_core::keybind`). A missing or unparseable
/// file just means no overrides yet -- irust-tui falls back to its
/// built-in defaults for anything not present here.
pub fn load_keybinds() -> HashMap<String, String> {
    crate::storage::load_toml::<KeybindFile>(KEYBINDS_FILE).binds
}

/// Overwrites the keybinds file with the current map. Best-effort: a
/// save failure (disk full, permissions...) shouldn't crash the client.
pub fn save_keybinds(binds: &HashMap<String, String>) -> std::io::Result<()> {
    let file = KeybindFile { binds: binds.clone() };
    crate::storage::save_toml(KEYBINDS_FILE, &file, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips_to_an_empty_map() {
        let parsed: KeybindFile = toml::from_str("").unwrap();
        assert!(parsed.binds.is_empty());
    }

    #[test]
    fn file_with_entries_parses_correctly() {
        let parsed: KeybindFile = toml::from_str(
            r#"
            [binds]
            "ctrl-a" = "history_up"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.binds.get("ctrl-a"), Some(&"history_up".to_string()));
    }
}
