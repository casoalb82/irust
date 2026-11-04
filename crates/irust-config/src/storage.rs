//! Shared TOML-file storage for the per-feature stores in this crate
//! (aliases, keybinds, hilights, ...): a `ProjectDirs` data-dir lookup,
//! read + parse with a startup warning on malformed input, and a
//! create-dir + write-pretty-TOML save.

use std::fs;
use std::path::PathBuf;

use directories::ProjectDirs;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub(crate) fn data_file_path(filename: &str) -> Option<PathBuf> {
    ProjectDirs::from("", "", "irust").map(|dirs| dirs.data_dir().join(filename))
}

pub(crate) fn load_toml<T: Default + DeserializeOwned>(filename: &str) -> T {
    let Some(path) = data_file_path(filename) else {
        return T::default();
    };
    let Ok(contents) = fs::read_to_string(&path) else {
        return T::default();
    };
    crate::warnings::parse_toml_or_warn::<T>(&contents, &path).unwrap_or_default()
}

pub(crate) fn save_toml<T: Serialize>(filename: &str, value: &T, restrict: bool) -> std::io::Result<()> {
    let Some(path) = data_file_path(filename) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let toml_str =
        toml::to_string_pretty(value).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&path, toml_str)?;
    if restrict {
        crate::perms::restrict_to_owner(&path);
    }
    Ok(())
}
