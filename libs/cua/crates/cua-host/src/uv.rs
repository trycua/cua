//! `uv` for job members: a host installs a pinned release of astral's `uv`
//! for each guest OS and CPU its members run on (a macOS VM, a Linux
//! container, the machine itself) into a tools directory it mounts
//! read-only into their Spaces, first on their `PATH` (an image need not
//! carry uv). uv caches Python and each job's environment in the host's job
//! cache, so the second run of a job starts fast.
//!
//! The archive comes from astral's GitHub release of [`UV_VERSION`] for
//! the guest's target, and is installed only when its SHA-256 is the one
//! pinned here.

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// The uv release installed.
pub const UV_VERSION: &str = "0.12.17";

/// Where the release archives are (`CUA_UV_BASE_URL` overrides it, for a
/// mirror).
pub const UV_BASE_URL: &str = "https://github.com/astral-sh/uv/releases/download";

/// SHA-256 of `uv-<target>.tar.gz` of [`UV_VERSION`], per target.
const UV_SHA256: [(&str, &str); 4] = [
    (
        "aarch64-apple-darwin",
        "85f00cbdc6dd3e97eba4c31b4d014375a9fdfe8f570023b84e5102fc3456896b",
    ),
    (
        "x86_64-apple-darwin",
        "8dcf05a8c809bb3c471d2b614788ba27a6e41298fc8c31ac84b5f4339fd468e5",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "fa82fd8dde8e8eefdecada6aa0889666556cfceb690d06e0c3bca49eb3070a63",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "d636d1b678e9e7f367ecb22b46bd1cabbed234d6bc3b4d96365d2b507f72f86c",
    ),
];

/// This machine's uv release target, when uv has one pinned here.
pub fn uv_target() -> Option<&'static str> {
    uv_target_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The uv release target of a guest running `os` (`macos`, `linux`) on
/// `arch` (`aarch64`/`arm64`, `x86_64`/`amd64`), when uv has one pinned
/// here.
pub fn uv_target_for(os: &str, arch: &str) -> Option<&'static str> {
    let arm = matches!(arch, "aarch64" | "arm64");
    let x86 = matches!(arch, "x86_64" | "amd64");
    match os {
        "macos" if arm => Some("aarch64-apple-darwin"),
        "macos" if x86 => Some("x86_64-apple-darwin"),
        "linux" if x86 => Some("x86_64-unknown-linux-gnu"),
        "linux" if arm => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// The pinned SHA-256 of `target`'s archive.
pub fn uv_sha256(target: &str) -> Option<&'static str> {
    UV_SHA256
        .iter()
        .find(|(t, _)| *t == target)
        .map(|(_, h)| *h)
}

/// The file in a tools directory saying which uv release it holds (the
/// binary may be for another OS than this one's, so it is not asked).
const VERSION_FILE: &str = ".uv-version";

/// Installs the pinned uv for `target` into `dir` (`dir/uv`, `dir/uvx`)
/// unless that one is there already. Returns `dir/uv`.
pub async fn ensure_uv_in(dir: &Path, target: &str) -> Result<PathBuf> {
    let dest = dir.join("uv");
    let have = std::fs::read_to_string(dir.join(VERSION_FILE)).unwrap_or_default();
    if dest.is_file() && have.trim() == format!("{UV_VERSION} {target}") {
        return Ok(dest);
    }
    if uv_sha256(target).is_none() {
        return Err(Error::Download(format!(
            "no uv for {target} is pinned here"
        )));
    }
    let base = std::env::var("CUA_UV_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| UV_BASE_URL.to_string());
    let url = format!(
        "{}/{UV_VERSION}/uv-{target}.tar.gz",
        base.trim_end_matches('/')
    );
    let bytes = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| Error::Download(e.to_string()))?
        .get(&url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| Error::Download(format!("{url}: {e}")))?
        .bytes()
        .await
        .map_err(|e| Error::Download(format!("{url}: {e}")))?;
    let sha = uv_sha256(target).expect("pinned target");
    let dir = dir.to_path_buf();
    let target = target.to_string();
    tokio::task::spawn_blocking(move || install(&bytes, sha, &target, &dir))
        .await
        .map_err(|e| Error::Internal(e.to_string()))?
}

/// Checks `archive` against `sha256` and installs its `uv` (and `uvx`) into
/// `dir` (atomically: written aside, then renamed; readable and runnable by
/// every user, since members may run as other users).
pub fn install(archive: &[u8], sha256: &str, target: &str, dir: &Path) -> Result<PathBuf> {
    use sha2::Digest as _;
    use std::io::Read as _;
    let got = hex::encode(sha2::Sha256::digest(archive));
    if !got.eq_ignore_ascii_case(sha256) {
        return Err(Error::Download(format!(
            "uv {UV_VERSION} for {target}: SHA-256 {got} is not the pinned {sha256}; not installed"
        )));
    }
    let bin = dir.to_path_buf();
    std::fs::create_dir_all(&bin)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
    }
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut found = false;
    for entry in ar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !(name == "uv" || name == "uvx") || !entry.header().entry_type().is_file() {
            continue;
        }
        let mut data = Vec::new();
        entry.read_to_end(&mut data)?;
        let tmp = bin.join(format!(".{name}.{:08x}", rand::random::<u32>()));
        std::fs::write(&tmp, &data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp, bin.join(name))?;
        found |= name == "uv";
    }
    if !found {
        return Err(Error::Download(format!(
            "uv {UV_VERSION} for {target}: the archive has no uv"
        )));
    }
    std::fs::write(bin.join(VERSION_FILE), format!("{UV_VERSION} {target}\n"))?;
    Ok(bin.join("uv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(target: &str) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for name in ["uv", "uvx"] {
            let body = format!("#!/bin/sh\necho {name} {UV_VERSION}\n");
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, format!("uv-{target}/{name}"), body.as_bytes())
                .unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn every_supported_target_is_pinned() {
        for (t, h) in UV_SHA256 {
            assert_eq!(h.len(), 64, "{t}");
        }
        if let Some(t) = uv_target() {
            assert!(uv_sha256(t).is_some());
        }
    }

    #[test]
    fn an_archive_installs_only_with_the_pinned_hash() {
        use sha2::Digest as _;
        let home = tempfile::tempdir().unwrap();
        let tools = home.path().join("job-tools/x");
        let a = archive("x");
        let wrong = "0".repeat(64);
        let e = install(&a, &wrong, "x", &tools).unwrap_err();
        assert!(e.to_string().contains("not the pinned"), "{e}");
        assert!(!tools.join("uv").exists());
        let sha = hex::encode(sha2::Sha256::digest(&a));
        let p = install(&a, &sha, "x", &tools).unwrap();
        assert_eq!(p, tools.join("uv"));
        #[cfg(unix)]
        {
            let out = std::process::Command::new(&p).output().unwrap();
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim(),
                format!("uv {UV_VERSION}"),
                "the installed uv runs"
            );
        }
        assert!(tools.join("uvx").is_file());
        assert_eq!(
            std::fs::read_to_string(tools.join(".uv-version")).unwrap(),
            format!("{UV_VERSION} x\n")
        );
    }

    #[test]
    fn a_guests_target_is_named_by_its_os_and_cpu() {
        assert_eq!(
            uv_target_for("macos", "arm64"),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            uv_target_for("linux", "amd64"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            uv_target_for("linux", "aarch64"),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(uv_target_for("windows", "x86_64"), None);
    }
}
