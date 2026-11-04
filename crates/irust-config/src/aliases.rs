use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct AliasFile {
    #[serde(default)]
    aliases: HashMap<String, String>,
}

const ALIASES_FILE: &str = "aliases.toml";

pub fn load_aliases() -> HashMap<String, String> {
    crate::storage::load_toml::<AliasFile>(ALIASES_FILE).aliases
}

/// `true` if the aliases file doesn't exist yet -- the one-time seeding
/// signal, distinct from "exists but is empty" (the user deliberately
/// cleared every alias, which must stay empty on the next start).
pub fn aliases_file_missing() -> bool {
    match crate::storage::data_file_path(ALIASES_FILE) {
        Some(path) => !path.exists(),
        None => true,
    }
}

/// Overwrites the aliases file with the current alias map. Best-effort:
/// a save failure (disk full, permissions...) shouldn't crash the
/// client, so errors are returned for the caller to decide whether to
/// surface anything, not panicked on.
pub fn save_aliases(aliases: &HashMap<String, String>) -> std::io::Result<()> {
    let file = AliasFile {
        aliases: aliases.clone(),
    };
    // An alias template is free-form command text (e.g. a stored
    // NickServ identify command) -- same credential-exposure class as
    // config.toml/debug.log, so this store gets the same restricted
    // permissions.
    crate::storage::save_toml(ALIASES_FILE, &file, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_round_trips_to_an_empty_map() {
        let parsed: AliasFile = toml::from_str("").unwrap();
        assert!(parsed.aliases.is_empty());
    }

    #[test]
    fn file_with_entries_parses_correctly() {
        let parsed: AliasFile = toml::from_str(
            r#"
            [aliases]
            j = "join"
            umode = "mode $N"
            "#,
        )
        .unwrap();
        assert_eq!(parsed.aliases.get("j"), Some(&"join".to_string()));
        assert_eq!(parsed.aliases.get("umode"), Some(&"mode $N".to_string()));
    }
}
