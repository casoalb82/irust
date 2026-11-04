
#[cfg(unix)]
pub fn restrict_to_owner(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
pub fn restrict_to_owner(_path: &std::path::Path) {}

/// Best-effort `chmod` of `path` to `mode` (e.g. `0o644`).
#[cfg(unix)]
pub fn set_file_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
pub fn set_file_mode(_path: &std::path::Path, _mode: u32) {}

/// Best-effort `chmod 700` on a directory `path`. Unlike a file, a
/// directory needs its execute bit for the owner to still traverse/stat
/// entries by name -- `0o600` on a directory would lock the owner out
/// too, not just other users.
#[cfg(unix)]
pub fn restrict_dir_to_owner(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
pub fn restrict_dir_to_owner(_path: &std::path::Path) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn restrict_to_owner_leaves_only_the_owner_able_to_read_it() {
        let path = std::env::temp_dir().join(format!("irust-perms-test-{}.tmp", std::process::id()));
        std::fs::write(&path, "irrelevant").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o664)).unwrap();
        restrict_to_owner(&path);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_file(&path);
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn restrict_dir_to_owner_keeps_the_execute_bit_for_the_owner() {
        let path = std::env::temp_dir().join(format!("irust-perms-dir-test-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o775)).unwrap();
        restrict_dir_to_owner(&path);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_dir_all(&path);
        assert_eq!(mode, 0o700);
    }
}
