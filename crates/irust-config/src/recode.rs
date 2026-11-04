//! `/recode`'s persisted per-target/network charset overrides -- irssi
//! parity (`src/core/recode.c`'s own config storage under
//! a `conversions` node, keyed the same three ways: `"<tag>/<target>"`,
//! `"<target>"`, or bare `"<tag>"`). Same `save_*`/`load_*` TOML-file
//! convention every sibling module in this crate already follows.

use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, OnceLock, RwLock};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
struct RecodeFile {
    #[serde(default)]
    conversions: HashMap<String, String>,
}

fn recode_path() -> Option<std::path::PathBuf> {
    ProjectDirs::from("", "", "irust").map(|dirs| dirs.data_dir().join("recode.toml"))
}

/// Loads every persisted `/recode` override, keyed exactly as
/// `/recode add` stored them (`"<tag>/<target>"`, `"<target>"`, or
/// `"<tag>"`). A missing or unparseable file just means none defined
/// yet.
pub fn load_recode() -> HashMap<String, String> {
    let Some(path) = recode_path() else {
        return HashMap::new();
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return HashMap::new();
    };
    crate::warnings::parse_toml_or_warn::<RecodeFile>(&contents, &path)
        .map(|f| f.conversions)
        .unwrap_or_default()
}

/// Overwrites the recode file with the current set of overrides.
/// Best-effort, same as every sibling `save_*` in this crate.
pub fn save_recode(conversions: &HashMap<String, String>) -> std::io::Result<()> {
    let Some(path) = recode_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = RecodeFile {
        conversions: conversions.clone(),
    };
    let toml_str =
        toml::to_string_pretty(&file).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(path, toml_str)
}

fn recode_cache() -> &'static RwLock<Arc<HashMap<String, String>>> {
    static CACHE: OnceLock<RwLock<Arc<HashMap<String, String>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(Arc::new(load_recode())))
}

fn cached_conversions() -> Arc<HashMap<String, String>> {
    recode_cache().read().unwrap().clone()
}

pub fn update_cache_entry(key: String, charset: String) {
    let cache = recode_cache();
    let mut guard = cache.write().unwrap();
    let mut updated = (**guard).clone();
    updated.insert(key, charset);
    *guard = Arc::new(updated);
}

/// `/recode remove`'s live-cache write -- removes one entry, leaving
/// every other override untouched.
pub fn remove_cache_entry(key: &str) {
    let cache = recode_cache();
    let mut guard = cache.write().unwrap();
    let mut updated = (**guard).clone();
    updated.remove(key);
    *guard = Arc::new(updated);
}

pub fn resolve(tag: &str, target: Option<&str>) -> Option<String> {
    let map = cached_conversions();
    if let Some(target) = target {
        let tag_target = format!("{tag}/{target}");
        if let Some(charset) = map.get(&tag_target) {
            return Some(charset.clone());
        }
        if let Some(charset) = map.get(target) {
            return Some(charset.clone());
        }
    }
    map.get(tag).cloned()
}

pub fn is_valid_charset(name: &str) -> bool {
    encoding_rs::Encoding::for_label(name.as_bytes()).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_has_no_conversions() {
        let parsed: RecodeFile = toml::from_str("").unwrap();
        assert!(parsed.conversions.is_empty());
    }

    #[test]
    fn save_recode_toml_shape_round_trips() {
        let conversions = HashMap::from([
            ("chatnet/#legacy".to_string(), "windows-1251".to_string()),
            ("libera".to_string(), "ISO-8859-1".to_string()),
        ]);
        let file = RecodeFile {
            conversions: conversions.clone(),
        };
        let toml_str = toml::to_string_pretty(&file).unwrap();
        let parsed: RecodeFile = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.conversions, conversions);
    }

    #[test]
    fn resolve_prefers_tag_slash_target_over_target_over_tag() {
        let key_tag_target = "resolve_test_tag_zzz/#resolve_test_chan_zzz".to_string();
        let key_target = "#resolve_test_chan_zzz".to_string();
        let key_tag = "resolve_test_tag_zzz".to_string();
        update_cache_entry(key_target.clone(), "iso-8859-1".to_string());
        update_cache_entry(key_tag.clone(), "koi8-r".to_string());
        assert_eq!(
            resolve("resolve_test_tag_zzz", Some("#resolve_test_chan_zzz")),
            Some("iso-8859-1".to_string()),
            "target-only beats tag-only when both exist but tag/target doesn't"
        );
        update_cache_entry(key_tag_target.clone(), "windows-1251".to_string());
        assert_eq!(
            resolve("resolve_test_tag_zzz", Some("#resolve_test_chan_zzz")),
            Some("windows-1251".to_string()),
            "tag/target beats both once it exists"
        );
        assert_eq!(
            resolve("resolve_test_tag_zzz", Some("#some_other_chan_zzz")),
            Some("koi8-r".to_string()),
            "a different target on the same tag falls through to the tag-only entry"
        );
        assert_eq!(resolve("resolve_test_tag_zzz", None), Some("koi8-r".to_string()));
        remove_cache_entry(&key_tag_target);
        remove_cache_entry(&key_target);
        remove_cache_entry(&key_tag);
    }

    #[test]
    fn resolve_is_none_with_nothing_configured() {
        assert_eq!(
            resolve("resolve_test_tag_with_nothing_configured_zzz", Some("#chan")),
            None
        );
    }

    #[test]
    fn is_valid_charset_accepts_real_labels_and_rejects_garbage() {
        assert!(is_valid_charset("UTF-8"));
        assert!(is_valid_charset("windows-1251"));
        assert!(is_valid_charset("cp1251"));
        assert!(is_valid_charset("ISO-8859-1"));
        assert!(!is_valid_charset("not-a-real-charset"));
        assert!(!is_valid_charset(""));
    }
}
