
use std::io::Write;
use std::sync::OnceLock;
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Where `/update` looks for the release manifest. Set `IRUST_UPDATE_URL` to enable it.
fn manifest_url() -> anyhow::Result<String> {
    match std::env::var("IRUST_UPDATE_URL") {
        Ok(u) if !u.trim().is_empty() => Ok(u),
        _ => anyhow::bail!("no update server configured (set IRUST_UPDATE_URL)"),
    }
}

/// This process's key into the manifest's per-target map. `x86` is
/// `std::env::consts::ARCH`'s name for 32-bit x86 (matches Rust's own
/// `i686-unknown-linux-gnu` target, not to be confused with 64-bit
/// `x86_64`). Matched on `(OS, ARCH)`, not `ARCH` alone -- `x86_64`
/// alone is ambiguous between Linux and macOS.
fn target_key() -> anyhow::Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-linux"),
        ("linux", "x86") => Ok("i686-linux"),
        ("macos", "x86_64") => Ok("x86_64-macos"),
        ("macos", "aarch64") => Ok("aarch64-macos"),
        (os, arch) => anyhow::bail!("no published irust builds for this architecture ({os}-{arch})"),
    }
}

struct Manifest {
    version: String,
    url: String,
    sha256: String,
}

/// Built once and reused for every call -- same reasoning as
/// `iplookup.rs`'s own `client()`.
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

pub enum UpdateOutcome {
    UpToDate(String),
    Updated { to: String },
    Failed(String),
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn cached_exe() -> anyhow::Result<std::path::PathBuf> {
    static EXE: OnceLock<std::path::PathBuf> = OnceLock::new();
    if let Some(p) = EXE.get() {
        return Ok(p.clone());
    }
    let p = std::env::current_exe().map_err(|e| anyhow::anyhow!("can't find current binary: {e}"))?;
    Ok(EXE.get_or_init(|| p).clone())
}

/// Reads and hashes the binary currently running -- `cached_exe()`
/// resolves the actual on-disk path of this process's executable, not
/// just the argv[0] string, so this reflects reality even if launched
/// via a relative path or a symlink.
fn hash_current_binary() -> anyhow::Result<String> {
    let path = cached_exe()?;
    let bytes = std::fs::read(&path).map_err(|e| anyhow::anyhow!("can't read current binary: {e}"))?;
    Ok(sha256_hex(&bytes))
}

async fn fetch_manifest() -> anyhow::Result<Manifest> {
    let key = target_key()?;
    let url = manifest_url()?;
    let resp = client()
        .get(url)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("couldn't reach update server: {e}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("update server returned {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| anyhow::anyhow!("bad manifest: {e}"))?;
    parse_manifest(&v, key)
}

/// Picks `key`'s `{version, url, sha256}` entry out of the per-target
/// manifest.
fn parse_manifest(v: &serde_json::Value, key: &str) -> anyhow::Result<Manifest> {
    let entry = v
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("manifest has no entry for this architecture ({key})"))?;
    let field = |field_key: &str| entry.get(field_key).and_then(|x| x.as_str()).map(str::to_string);
    Ok(Manifest {
        version: field("version").ok_or_else(|| anyhow::anyhow!("manifest[{key}] missing \"version\""))?,
        url: field("url").ok_or_else(|| anyhow::anyhow!("manifest[{key}] missing \"url\""))?,
        sha256: field("sha256").ok_or_else(|| anyhow::anyhow!("manifest[{key}] missing \"sha256\""))?,
    })
}

async fn download_and_verify(manifest: &Manifest) -> anyhow::Result<Vec<u8>> {
    let resp = client()
        .get(&manifest.url)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("download returned {}", resp.status());
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("download failed: {e}"))?
        .to_vec();

    if !sha256_hex(&bytes).eq_ignore_ascii_case(&manifest.sha256) {
        anyhow::bail!("checksum mismatch, refusing to install");
    }
    Ok(bytes)
}

fn install(bytes: &[u8]) -> anyhow::Result<()> {
    let current = cached_exe()?;
    let tmp = current.with_extension("update-tmp");

    (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    })()
    .map_err(|e| anyhow::anyhow!("can't write new binary: {e}"))?;

    std::fs::rename(&tmp, &current).map_err(|e| anyhow::anyhow!("can't install new binary: {e}"))
}

pub async fn check_and_update() -> UpdateOutcome {
    let manifest = match fetch_manifest().await {
        Ok(m) => m,
        Err(e) => return UpdateOutcome::Failed(e.to_string()),
    };

    let current_hash = match tokio::task::spawn_blocking(hash_current_binary).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return UpdateOutcome::Failed(e.to_string()),
        Err(e) => return UpdateOutcome::Failed(format!("hash task panicked: {e}")),
    };

    if current_hash.eq_ignore_ascii_case(&manifest.sha256) {
        return UpdateOutcome::UpToDate(manifest.version);
    }

    let bytes = match download_and_verify(&manifest).await {
        Ok(b) => b,
        Err(e) => return UpdateOutcome::Failed(e.to_string()),
    };

    match tokio::task::spawn_blocking(move || install(&bytes)).await {
        Ok(Ok(())) => UpdateOutcome::Updated { to: manifest.version },
        Ok(Err(e)) => UpdateOutcome::Failed(e.to_string()),
        Err(e) => UpdateOutcome::Failed(format!("install task panicked: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_formatting_matches_a_known_vector() {
        // Well-known SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn different_bytes_hash_differently() {
        assert_ne!(sha256_hex(b"v1"), sha256_hex(b"v2"));
    }

    #[test]
    fn hash_current_binary_succeeds_and_is_stable() {
        let a = hash_current_binary().unwrap();
        let b = hash_current_binary().unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64, "sha256 hex digest should always be 64 chars");
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    const NESTED: &str = r#"{
      "x86_64-linux": {"version": "r28", "url": "https://x/a", "sha256": "aa"},
      "i686-linux":   {"version": "r28", "url": "https://x/b", "sha256": "bb"}
    }"#;

    #[test]
    fn picks_the_entry_for_the_given_target() {
        let v: serde_json::Value = serde_json::from_str(NESTED).unwrap();
        let m = parse_manifest(&v, "i686-linux").unwrap();
        assert_eq!((m.url.as_str(), m.sha256.as_str()), ("https://x/b", "bb"));
        assert_eq!(parse_manifest(&v, "x86_64-linux").unwrap().sha256, "aa");
    }

    #[test]
    fn missing_target_or_field_is_an_error_not_a_fallback() {
        let v: serde_json::Value = serde_json::from_str(NESTED).unwrap();
        assert!(parse_manifest(&v, "aarch64-linux").is_err());
        let broken: serde_json::Value = serde_json::from_str(r#"{"x86_64-linux": {"url": "u"}}"#).unwrap();
        assert!(parse_manifest(&broken, "x86_64-linux").is_err());
    }
}
