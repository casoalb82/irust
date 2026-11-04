
use std::sync::Mutex;

static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(message: String) {
    // Poisoning can't actually happen here (nothing panics while
    // holding this lock), but never let a startup diagnostic itself be
    // the thing that panics -- worst case, silently drop the warning.
    if let Ok(mut warnings) = WARNINGS.lock() {
        warnings.push(message);
    }
}

/// Every startup warning recorded so far, in the order found. Empty in
/// the overwhelmingly common case (no file, or every file that exists
/// parsed fine).
pub fn startup_warnings() -> Vec<String> {
    WARNINGS.lock().map(|w| w.clone()).unwrap_or_default()
}

pub(crate) fn parse_toml_or_warn<T: serde::de::DeserializeOwned>(contents: &str, path: &std::path::Path) -> Option<T> {
    match toml::from_str(contents) {
        Ok(value) => Some(value),
        Err(err) => {
            record(format!(
                "-- warning: {} failed to parse ({err}) -- using defaults",
                path.display()
            ));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Simple {
        #[allow(dead_code)]
        n: u32,
    }

    #[test]
    fn parse_toml_or_warn_returns_some_on_valid_input() {
        let result: Option<Simple> = parse_toml_or_warn("n = 1", std::path::Path::new("test.toml"));
        assert!(result.is_some());
    }

    #[test]
    fn parse_toml_or_warn_returns_none_and_records_on_invalid_input() {
        let before = startup_warnings().len();
        let result: Option<Simple> =
            parse_toml_or_warn("not valid toml [[[", std::path::Path::new("test-invalid.toml"));
        assert!(result.is_none());
        let after = startup_warnings();
        assert_eq!(after.len(), before + 1);
        assert!(after.last().unwrap().contains("test-invalid.toml"));
    }
}
