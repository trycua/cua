//! Build identity of the running process: which source revision it was built
//! from and the sha256 of the executable actually running.
//!
//! CI uses this to prove a test ran against the code under test and not a
//! stale binary (one bundled in an image, or installed on the runner):
//! `health_report` carries it as `build`, and the cua doctors compare it
//! with `--expect cua-driver=<sha256|git|version>`.
//!
//! - `git_sha` is embedded by the *binary* crate's build script and handed
//!   over with [`init`] at startup, so a new commit relinks only the binary,
//!   not this crate and everything that depends on it.
//! - `exe_sha256` hashes the running image (`/proc/self/exe` on Linux, which
//!   still names the old inode after the file on disk was replaced), so a
//!   process that was never restarted after an upgrade reports its real,
//!   stale identity.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

static GIT_SHA: OnceLock<String> = OnceLock::new();
static EXE_SHA256: OnceLock<Option<String>> = OnceLock::new();

/// Records the source revision this binary was built from (a 40-hex git sha,
/// or empty when unknown). Call once, first thing in `main`; later calls are
/// ignored.
pub fn init(git_sha: &str) {
    let _ = GIT_SHA.set(normalize_sha(git_sha));
}

/// The source revision passed to [`init`], empty when unknown.
pub fn git_sha() -> &'static str {
    GIT_SHA.get().map(String::as_str).unwrap_or("")
}

/// Lowercases a hex sha; anything that is not 7 to 64 hex digits becomes
/// empty (an unknown revision, never a wrong one).
pub fn normalize_sha(value: &str) -> String {
    let v = value.trim().to_ascii_lowercase();
    if (7..=64).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_hexdigit()) {
        v
    } else {
        String::new()
    }
}

/// sha256 (lowercase hex) of the running executable, computed once.
pub fn exe_sha256() -> Option<String> {
    EXE_SHA256
        .get_or_init(|| {
            #[cfg(target_os = "linux")]
            let path = std::path::PathBuf::from("/proc/self/exe");
            #[cfg(not(target_os = "linux"))]
            let path = std::env::current_exe().ok()?;
            sha256_file(&path).ok()
        })
        .clone()
}

/// sha256 (lowercase hex) of a file, streamed.
pub fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The `build` block of `health_report` and `cua-driver doctor --json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildInfo {
    /// Crate version (semver).
    pub version: String,
    /// Source revision, empty when the build did not record one.
    pub git_sha: String,
    /// sha256 of the running executable, empty when unreadable.
    pub exe_sha256: String,
}

/// The running process's build identity.
pub fn current() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        git_sha: git_sha().to_owned(),
        exe_sha256: exe_sha256().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keeps_only_hex_shas() {
        assert_eq!(normalize_sha(" ABCDEF1 "), "abcdef1");
        assert_eq!(normalize_sha(&"a".repeat(40)), "a".repeat(40));
        assert_eq!(normalize_sha("unknown"), "");
        assert_eq!(normalize_sha("abc"), "");
        assert_eq!(normalize_sha(&"a".repeat(65)), "");
    }

    #[test]
    fn exe_hash_is_stable_hex() {
        let a = exe_sha256().expect("the test binary is readable");
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(exe_sha256().unwrap(), a);
    }

    #[test]
    fn sha256_file_matches_a_known_digest() {
        let dir = std::env::temp_dir().join(format!("cua-build-info-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("abc");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
