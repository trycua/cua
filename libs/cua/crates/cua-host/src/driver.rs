//! Finding (or downloading) the cua-spacesd binary the service runs.
//!
//! Order: an explicit path, `CUA_SPACESD_BIN` (or the older `CUA_GUESTD_BIN` / `CUA_ENV_DRIVER_BIN`), a binary next to the
//! current executable (the app bundle / CLI install ships one), else the
//! release artifact:
//!
//! ```text
//! <base>/cua-spacesd-<os>-<arch>.tar.gz          (the binary at the archive root)
//! <base>/cua-spacesd-<os>-<arch>.tar.gz.sha256   (required; the download fails without it)
//! base = $CUA_SPACESD_DOWNLOAD_BASE
//!      | https://github.com/<repo>/releases/download/cua-spacesd-v<version>
//! repo = the GitHub repository this build was released from
//!        (`CUA_RELEASE_REPOSITORY` at build time; default trycua/cua)
//! version = $CUA_SPACESD_VERSION | the released driver version (libs/cua-spacesd/VERSION)
//! os = linux | macos | windows, arch = x86_64 | aarch64
//! ```

use crate::{Error, Result};
use sha2::Digest as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// The cua-spacesd release this build downloads by default (the driver
/// is versioned and released on its own, `cua-spacesd-v<version>`).
pub const SPACESD_VERSION: &str = include_str!("../../../../cua-spacesd/VERSION");

/// Binary file name for this OS.
pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "cua-spacesd.exe"
    } else {
        "cua-spacesd"
    }
}

/// `<os>-<arch>` of this build, as used in artifact names.
pub fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// The artifact name for this platform.
pub fn artifact_name() -> String {
    format!("cua-spacesd-{}.tar.gz", platform())
}

/// `CUA_SPACESD_<name>`, else the older `CUA_GUESTD_<name>` or
/// `CUA_ENV_DRIVER_<name>` (the new name wins). Empty values count as unset.
fn spacesd_env(name: &str) -> Option<std::ffi::OsString> {
    [
        format!("CUA_SPACESD_{name}"),
        format!("CUA_GUESTD_{name}"),
        format!("CUA_ENV_DRIVER_{name}"),
    ]
    .iter()
    .find_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()))
}

/// The GitHub repository whose releases this build downloads from. Release
/// workflows set `CUA_RELEASE_REPOSITORY` to `github.repository` at build
/// time, so a staging build fetches its own repository's cua-spacesd.
pub const RELEASE_REPOSITORY: &str = match option_env!("CUA_RELEASE_REPOSITORY") {
    Some(r) if !r.is_empty() => r,
    _ => "trycua/cua",
};

/// Release download base (see the module docs).
pub fn download_base() -> String {
    if let Some(b) = spacesd_env("DOWNLOAD_BASE")
        .and_then(|v| v.into_string().ok())
        .filter(|v| !v.trim().is_empty())
    {
        return b.trim().trim_end_matches('/').to_string();
    }
    let version = spacesd_env("VERSION")
        .and_then(|v| v.into_string().ok())
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| SPACESD_VERSION.trim().to_string());
    format!(
        "https://github.com/{RELEASE_REPOSITORY}/releases/download/cua-spacesd-v{}",
        version.trim_start_matches('v')
    )
}

/// Where the binary comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverSource {
    /// A local file.
    Local(PathBuf),
    /// The release artifact (to download).
    Download(String),
}

/// Resolves the source without touching the network.
pub fn locate(explicit: Option<&Path>) -> Result<DriverSource> {
    if let Some(p) = explicit {
        return if p.is_file() {
            Ok(DriverSource::Local(p.to_path_buf()))
        } else {
            Err(Error::InvalidArgument(format!(
                "cua-spacesd binary {} does not exist",
                p.display()
            )))
        };
    }
    if let Some(p) = spacesd_env("BIN") {
        let p = PathBuf::from(p);
        return if p.is_file() {
            Ok(DriverSource::Local(p))
        } else {
            Err(Error::InvalidArgument(format!(
                "CUA_SPACESD_BIN={} does not exist",
                p.display()
            )))
        };
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        // Next to the CLI, or in a macOS bundle's Contents/MacOS or
        // Contents/Resources.
        for candidate in [
            dir.join(binary_name()),
            dir.join("../Resources").join(binary_name()),
        ] {
            if candidate.is_file() {
                return Ok(DriverSource::Local(candidate));
            }
        }
    }
    Ok(DriverSource::Download(format!(
        "{}/{}",
        download_base(),
        artifact_name()
    )))
}

/// Puts the driver at `dest` (copying a local binary or downloading and
/// verifying the release artifact) and makes it executable.
pub async fn install_to(source: &DriverSource, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dest.with_extension("partial");
    match source {
        DriverSource::Local(p) => {
            if same_file(p, dest) {
                return Ok(());
            }
            std::fs::copy(p, &tmp)?;
        }
        DriverSource::Download(url) => {
            let bytes = download_verified(url).await?;
            let binary = extract_binary(&bytes)?;
            std::fs::write(&tmp, binary)?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Upper bound for a driver archive (the binary is tens of MiB).
const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;

/// Whether `url` may be fetched: HTTPS, or plain HTTP to a loopback host
/// (a local mirror or test server). Anything else is refused.
fn allowed_download_url(url: &url::Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => match url.host() {
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
}

/// Downloads `url` and checks it against the published `<url>.sha256`.
/// Fails closed: a missing or mismatched checksum, a non-HTTPS URL, or a
/// redirect to one is an error.
pub async fn download_verified(url: &str) -> Result<Vec<u8>> {
    let parsed = url::Url::parse(url).map_err(|e| Error::Download(format!("{url}: {e}")))?;
    if !allowed_download_url(&parsed) {
        return Err(Error::Download(format!(
            "{url}: refusing a non-HTTPS download"
        )));
    }
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if allowed_download_url(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("refusing a redirect to a non-HTTPS URL")
            }
        }))
        .build()
        .map_err(|e| Error::Download(e.to_string()))?;
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| Error::Download(format!("{url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::Download(format!("{url}: HTTP {}", resp.status())));
    }
    if resp
        .content_length()
        .is_some_and(|n| n as usize > MAX_ARCHIVE_BYTES)
    {
        return Err(Error::Download(format!("{url}: archive too large")));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| Error::Download(format!("{url}: {e}")))?;
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(Error::Download(format!("{url}: archive too large")));
    }
    let sum_url = format!("{url}.sha256");
    let r = http
        .get(&sum_url)
        .send()
        .await
        .map_err(|e| Error::Download(format!("{sum_url}: {e}")))?;
    if !r.status().is_success() {
        return Err(Error::Download(format!(
            "{sum_url}: HTTP {} (a published checksum is required)",
            r.status()
        )));
    }
    let text = r
        .text()
        .await
        .map_err(|e| Error::Download(format!("{sum_url}: {e}")))?;
    let expected = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
    if expected.len() != 64 || expected != actual {
        return Err(Error::Download(format!(
            "{url}: sha256 mismatch (expected {expected}, got {actual})"
        )));
    }
    Ok(bytes.to_vec())
}

/// Finds the driver binary in a `.tar.gz` (at the root or in a directory).
pub fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    let gz = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(gz);
    for entry in tar
        .entries()
        .map_err(|e| Error::Download(format!("archive: {e}")))?
    {
        let mut entry = entry.map_err(|e| Error::Download(format!("archive: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| Error::Download(format!("archive: {e}")))?
            .into_owned();
        if path.file_name().and_then(|n| n.to_str()) == Some(binary_name())
            && entry.header().entry_type().is_file()
        {
            let mut out = Vec::new();
            entry
                .by_ref()
                .take(MAX_ARCHIVE_BYTES as u64)
                .read_to_end(&mut out)
                .map_err(|e| Error::Download(format!("archive: {e}")))?;
            return Ok(out);
        }
    }
    Err(Error::Download(format!("archive has no {}", binary_name())))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn tar_gz(name: &str, content: &[u8]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, name, content).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn downloads_the_released_driver_version_by_default() {
        let v = SPACESD_VERSION.trim();
        assert!(
            v.split('.').count() == 3 && v.split('.').all(|p| p.parse::<u32>().is_ok()),
            "{v:?}"
        );
        if spacesd_env("DOWNLOAD_BASE").is_none() && spacesd_env("VERSION").is_none() {
            assert!(download_base().ends_with(&format!("/cua-spacesd-v{v}")));
        }
    }

    #[test]
    fn artifact_names_follow_the_scheme() {
        let name = artifact_name();
        assert!(name.starts_with("cua-spacesd-"));
        assert!(name.ends_with(".tar.gz"));
        assert!(
            ["linux-", "macos-", "windows-"]
                .iter()
                .any(|p| name.contains(p))
        );
    }

    #[test]
    fn extracts_the_binary_from_a_subdirectory() {
        let archive = tar_gz(&format!("dist/{}", binary_name()), b"#!bin");
        assert_eq!(extract_binary(&archive).unwrap(), b"#!bin");
        let other = tar_gz("README", b"x");
        assert!(extract_binary(&other).is_err());
    }

    #[test]
    fn explicit_paths_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("drv");
        assert!(locate(Some(&bin)).is_err());
        std::fs::write(&bin, b"x").unwrap();
        assert_eq!(locate(Some(&bin)).unwrap(), DriverSource::Local(bin));
    }

    #[tokio::test]
    async fn downloads_refuse_non_https_urls() {
        for url in [
            "http://example.com/a.tar.gz",
            "ftp://example.com/a.tar.gz",
            "file:///etc/passwd",
            "http://10.0.0.1/a.tar.gz",
        ] {
            let err = download_verified(url).await.unwrap_err();
            assert!(err.to_string().contains("non-HTTPS"), "{url}: {err}");
        }
        for ok in [
            "https://github.com/a",
            "http://127.0.0.1:1/a",
            "http://localhost/a",
            "http://[::1]/a",
        ] {
            assert!(allowed_download_url(&url::Url::parse(ok).unwrap()), "{ok}");
        }
    }

    #[tokio::test]
    async fn downloads_refuse_redirects_off_https() {
        use axum::{Router, response::Redirect, routing::get};
        let app = Router::new().route(
            "/a.tar.gz",
            get(|| async { Redirect::temporary("http://example.invalid/a.tar.gz") }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let err = download_verified(&format!("http://{addr}/a.tar.gz"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("redirect"), "{err}");
    }

    async fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
        use axum::{Router, routing::get};
        let mut app = Router::new();
        for (path, body) in files {
            app = app.route(path, get(move || async move { body.clone() }));
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn downloads_verify_the_published_checksum() {
        let archive = tar_gz(binary_name(), b"driver-bytes");
        let good = hex::encode(sha2::Sha256::digest(&archive));
        let base = serve(vec![
            ("/ok/a.tar.gz", archive.clone()),
            (
                "/ok/a.tar.gz.sha256",
                format!("{good}  a.tar.gz\n").into_bytes(),
            ),
            ("/bad/a.tar.gz", archive.clone()),
            ("/bad/a.tar.gz.sha256", b"00ff\n".to_vec()),
            ("/nosum/a.tar.gz", archive.clone()),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("bin").join(binary_name());
        install_to(
            &DriverSource::Download(format!("{base}/ok/a.tar.gz")),
            &dest,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"driver-bytes");
        let err = install_to(
            &DriverSource::Download(format!("{base}/bad/a.tar.gz")),
            &dest,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("sha256 mismatch"), "{err}");
        let nosum = install_to(
            &DriverSource::Download(format!("{base}/nosum/a.tar.gz")),
            &dest,
        )
        .await
        .unwrap_err();
        assert!(
            nosum.to_string().contains("checksum is required"),
            "{nosum}"
        );
        let missing = install_to(
            &DriverSource::Download(format!("{base}/none.tar.gz")),
            &dest,
        )
        .await
        .unwrap_err();
        assert!(missing.to_string().contains("404"), "{missing}");
    }
}
