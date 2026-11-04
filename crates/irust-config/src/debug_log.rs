use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::time::Instant;

static LOG: OnceLock<(Instant, Sender<String>)> = OnceLock::new();

pub fn init(path: &Path) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    crate::perms::restrict_to_owner(path);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            let _ = writeln!(file, "{line}");
        }
    });
    let _ = LOG.set((Instant::now(), tx));
    Ok(())
}

pub fn enabled() -> bool {
    LOG.get().is_some()
}

/// Queues one timestamped line: `[seconds.millis] SOURCE tag: text`.
/// Silently does nothing if `init()` was never called -- callers never
/// need to guard on `enabled()` first.
pub fn line(source: &str, tag: &str, text: &str) {
    let Some((start, tx)) = LOG.get() else { return };
    let elapsed = start.elapsed();
    let formatted = format!(
        "[{:>6}.{:03}] {source:<4} {tag}: {text}",
        elapsed.as_secs(),
        elapsed.subsec_millis()
    );
    let _ = tx.send(formatted);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn init_restricts_a_freshly_created_file_to_owner_only() {
        let path = std::env::temp_dir().join(format!("irust-debuglog-test-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        init(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode, 0o600);
    }
}
