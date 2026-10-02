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
//!
//! When the base is the default (no `CUA_SPACESD_DOWNLOAD_BASE` or
//! `CUA_SPACESD_VERSION`) and the pinned release or its artifact is missing
//! (a release that never published, or is still a draft), [`install_to`]
//! falls back to the newest published, non-draft cua-spacesd release on the
//! same major.minor, else the closest newer one on the same major, and logs
//! that it did. The fallback artifact passes the same checksum check, against
//! the `.sha256` published in the release it actually uses.

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

/// `CUA_SPACESD_DOWNLOAD_BASE`, when set.
fn download_base_override() -> Option<String> {
    spacesd_env("DOWNLOAD_BASE")
        .and_then(|v| v.into_string().ok())
        .filter(|v| !v.trim().is_empty())
        .map(|b| b.trim().trim_end_matches('/').to_string())
}

/// `CUA_SPACESD_VERSION`, when set.
fn version_override() -> Option<String> {
    spacesd_env("VERSION")
        .and_then(|v| v.into_string().ok())
        .filter(|v| !v.trim().is_empty())
        .map(|v| v.trim().trim_start_matches('v').to_string())
}

/// `https://github.com/<repo>/releases/download/cua-spacesd-v<version>`.
fn release_base(version: &str) -> String {
    format!("https://github.com/{RELEASE_REPOSITORY}/releases/download/cua-spacesd-v{version}")
}

/// Release download base (see the module docs).
pub fn download_base() -> String {
    if let Some(b) = download_base_override() {
        return b;
    }
    let version = version_override().unwrap_or_else(|| SPACESD_VERSION.trim().to_string());
    release_base(&version)
}

/// The GitHub API listing of [`RELEASE_REPOSITORY`]'s releases (where the
/// fallback looks for a published cua-spacesd).
pub fn releases_api() -> String {
    format!("https://api.github.com/repos/{RELEASE_REPOSITORY}/releases")
}

/// Where the binary comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DriverSource {
    /// A local file.
    Local(PathBuf),
    /// An artifact URL to download as is (an explicit download base or
    /// version: no fallback).
    Download(String),
    /// The pinned cua-spacesd release built into this binary: `url` is its
    /// artifact; if that release is missing or unpublished, the newest
    /// compatible published release in `releases_api` is used instead.
    Release {
        /// The pinned version (`libs/cua-spacesd/VERSION`).
        version: String,
        /// The pinned release's artifact URL.
        url: String,
        /// The GitHub releases API listing to fall back on.
        releases_api: String,
    },
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
    let url = format!("{}/{}", download_base(), artifact_name());
    if download_base_override().is_some() || version_override().is_some() {
        return Ok(DriverSource::Download(url));
    }
    Ok(DriverSource::Release {
        version: SPACESD_VERSION.trim().to_string(),
        url,
        releases_api: releases_api(),
    })
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
        DriverSource::Release {
            version,
            url,
            releases_api,
        } => {
            let (_used, bytes) = download_release(version, url, releases_api).await?;
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

/// Why a download failed: `missing` when the artifact or its checksum is
/// not there (HTTP 404/410: a release that never published or is still a
/// draft), which the release fallback treats as "try another release".
#[derive(Debug)]
struct FetchError {
    missing: bool,
    message: String,
}

impl FetchError {
    fn other(message: String) -> Self {
        Self {
            missing: false,
            message,
        }
    }
}

impl From<FetchError> for Error {
    fn from(e: FetchError) -> Self {
        Error::Download(e.message)
    }
}

fn is_missing(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE
}

fn http_client(timeout: std::time::Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(timeout)
        .user_agent(concat!("cua-host/", env!("CARGO_PKG_VERSION")))
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
        .map_err(|e| Error::Download(e.to_string()))
}

/// Downloads `url` and checks it against the published `<url>.sha256`.
/// Fails closed: a missing or mismatched checksum, a non-HTTPS URL, or a
/// redirect to one is an error.
pub async fn download_verified(url: &str) -> Result<Vec<u8>> {
    Ok(fetch_verified(url).await?)
}

async fn fetch_verified(url: &str) -> std::result::Result<Vec<u8>, FetchError> {
    let parsed = url::Url::parse(url).map_err(|e| FetchError::other(format!("{url}: {e}")))?;
    if !allowed_download_url(&parsed) {
        return Err(FetchError::other(format!(
            "{url}: refusing a non-HTTPS download"
        )));
    }
    let http = http_client(std::time::Duration::from_secs(600))
        .map_err(|e| FetchError::other(e.to_string()))?;
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| FetchError::other(format!("{url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(FetchError {
            missing: is_missing(resp.status()),
            message: format!("{url}: HTTP {}", resp.status()),
        });
    }
    if resp
        .content_length()
        .is_some_and(|n| n as usize > MAX_ARCHIVE_BYTES)
    {
        return Err(FetchError::other(format!("{url}: archive too large")));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| FetchError::other(format!("{url}: {e}")))?;
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err(FetchError::other(format!("{url}: archive too large")));
    }
    let sum_url = format!("{url}.sha256");
    let r = http
        .get(&sum_url)
        .send()
        .await
        .map_err(|e| FetchError::other(format!("{sum_url}: {e}")))?;
    if !r.status().is_success() {
        return Err(FetchError {
            missing: is_missing(r.status()),
            message: format!(
                "{sum_url}: HTTP {} (a published checksum is required)",
                r.status()
            ),
        });
    }
    let text = r
        .text()
        .await
        .map_err(|e| FetchError::other(format!("{sum_url}: {e}")))?;
    let expected = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let actual = hex::encode(sha2::Sha256::digest(&bytes));
    if expected.len() != 64 || expected != actual {
        return Err(FetchError::other(format!(
            "{url}: sha256 mismatch (expected {expected}, got {actual})"
        )));
    }
    Ok(bytes.to_vec())
}

/// `major.minor.patch` (no prerelease or build suffix).
fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim().trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// A published cua-spacesd release that carries this platform's artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Candidate {
    version: (u64, u64, u64),
    url: String,
}

impl Candidate {
    fn version_string(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }
}

/// The releases in a GitHub releases listing that may stand in for
/// `pinned`, best first: published (not draft, not prerelease)
/// `cua-spacesd-v<x.y.z>` releases other than `pinned` with both
/// `artifact` and `artifact.sha256` attached under the release's own tag.
/// Same major.minor first (newest patch first), then newer releases on the
/// same major (closest first).
fn fallback_candidates(
    listing: &serde_json::Value,
    pinned: (u64, u64, u64),
    artifact: &str,
) -> Vec<Candidate> {
    let mut same_minor = Vec::new();
    let mut newer = Vec::new();
    for release in listing.as_array().into_iter().flatten() {
        if release["draft"].as_bool() != Some(false)
            || release["prerelease"].as_bool() != Some(false)
        {
            continue;
        }
        let Some(tag) = release["tag_name"].as_str() else {
            continue;
        };
        let Some(version) = tag.strip_prefix("cua-spacesd-v").and_then(parse_version) else {
            continue;
        };
        if version == pinned {
            continue;
        }
        let asset_url = |name: &str| {
            release["assets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["name"].as_str() == Some(name))
                .find_map(|a| a["browser_download_url"].as_str())
                .map(str::to_string)
        };
        let (Some(url), Some(sum)) = (
            asset_url(artifact),
            asset_url(&format!("{artifact}.sha256")),
        ) else {
            continue;
        };
        // Identity: the artifact and its checksum come from this release's
        // own tag (download_verified fetches `<url>.sha256`).
        let suffix = format!("/{tag}/{artifact}");
        if !url.ends_with(&suffix) || sum != format!("{url}.sha256") {
            continue;
        }
        let candidate = Candidate { version, url };
        if (version.0, version.1) == (pinned.0, pinned.1) {
            same_minor.push(candidate);
        } else if version.0 == pinned.0 && version > pinned {
            newer.push(candidate);
        }
    }
    same_minor.sort_by_key(|c| std::cmp::Reverse(c.version));
    newer.sort_by_key(|c| c.version);
    same_minor.extend(newer);
    same_minor
}

/// The error when neither the pinned release nor a compatible fallback is
/// available: a plain sentence first, the raw cause last.
fn unavailable(pinned: &str, detail: &str) -> Error {
    Error::Download(format!(
        "cua-spacesd {pinned} (the Cua host service this version of Cua needs) is not \
         available to download, and no compatible published release was found. Check \
         your internet connection and try again later, or update Cua. Details: {detail}"
    ))
}

/// Downloads the pinned release's artifact at `url`, else (when that
/// release or artifact is missing) the best compatible published release
/// from `releases_api`. Returns the version used and the verified archive.
async fn download_release(
    pinned: &str,
    url: &str,
    releases_api: &str,
) -> Result<(String, Vec<u8>)> {
    let pinned_err = match fetch_verified(url).await {
        Ok(bytes) => return Ok((pinned.to_string(), bytes)),
        Err(e) if e.missing => e.message,
        Err(e) => return Err(e.into()),
    };
    let Some(pinned_version) = parse_version(pinned) else {
        return Err(unavailable(pinned, &pinned_err));
    };
    tracing::warn!(
        pinned,
        error = %pinned_err,
        "cua-spacesd {pinned} is not published; looking for a compatible published release"
    );
    let listing = list_releases(releases_api)
        .await
        .map_err(|e| unavailable(pinned, &format!("{pinned_err}; listing releases: {e}")))?;
    let mut last = pinned_err;
    for candidate in fallback_candidates(&listing, pinned_version, &artifact_name()) {
        let used = candidate.version_string();
        match fetch_verified(&candidate.url).await {
            Ok(bytes) => {
                tracing::warn!(
                    pinned,
                    used = %used,
                    url = %candidate.url,
                    "cua-spacesd {pinned} is not published; using cua-spacesd {used} instead"
                );
                return Ok((used, bytes));
            }
            Err(e) if e.missing => last = e.message,
            Err(e) => return Err(e.into()),
        }
    }
    Err(unavailable(pinned, &last))
}

/// `GET <api>?per_page=100` (the first two pages): the releases listing.
async fn list_releases(api: &str) -> std::result::Result<serde_json::Value, String> {
    let parsed = url::Url::parse(api).map_err(|e| format!("{api}: {e}"))?;
    if !allowed_download_url(&parsed) {
        return Err(format!("{api}: refusing a non-HTTPS URL"));
    }
    let http = http_client(std::time::Duration::from_secs(30)).map_err(|e| e.to_string())?;
    let mut all = Vec::new();
    for page in 1..=2 {
        let resp = http
            .get(api)
            .query(&[("per_page", "100"), ("page", &page.to_string())])
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| format!("{api}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{api}: HTTP {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| format!("{api}: {e}"))?;
        let Some(items) = body.as_array() else {
            return Err(format!("{api}: not a release listing"));
        };
        let done = items.len() < 100;
        all.extend(items.iter().cloned());
        if done {
            break;
        }
    }
    Ok(serde_json::Value::Array(all))
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

    /// A fake GitHub: `/releases` lists `releases` (tag, draft, prerelease,
    /// whether its artifact is attached, archive bytes, checksum override),
    /// and `/download/<tag>/<artifact>[.sha256]` serves the attached ones.
    struct FakeRelease {
        tag: &'static str,
        draft: bool,
        prerelease: bool,
        attached: bool,
        payload: &'static [u8],
        bad_sum: bool,
    }

    fn published(tag: &'static str, payload: &'static [u8]) -> FakeRelease {
        FakeRelease {
            tag,
            draft: false,
            prerelease: false,
            attached: true,
            payload,
            bad_sum: false,
        }
    }

    async fn fake_github(releases: Vec<FakeRelease>) -> String {
        use axum::{Json, Router, routing::get};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let artifact = artifact_name();
        let mut listing = Vec::new();
        let mut app = Router::new();
        for r in releases {
            let mut assets = Vec::new();
            if r.attached && !r.draft {
                let archive = tar_gz(binary_name(), r.payload);
                let sum = if r.bad_sum {
                    "0".repeat(64)
                } else {
                    hex::encode(sha2::Sha256::digest(&archive))
                };
                let path = format!("/download/{}/{artifact}", r.tag);
                for (name, url) in [
                    (artifact.clone(), format!("{base}{path}")),
                    (format!("{artifact}.sha256"), format!("{base}{path}.sha256")),
                ] {
                    assets.push(serde_json::json!({"name": name, "browser_download_url": url}));
                }
                let a = archive.clone();
                app = app
                    .route(&path, get(move || async move { a.clone() }))
                    .route(
                        &format!("{path}.sha256"),
                        get(move || async move { format!("{sum}  x\n") }),
                    );
            }
            listing.push(serde_json::json!({
                "tag_name": r.tag,
                "draft": r.draft,
                "prerelease": r.prerelease,
                "assets": assets,
            }));
        }
        let listing = serde_json::Value::Array(listing);
        app = app.route(
            "/releases",
            get(move || async move { Json(listing.clone()) }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    fn pinned_url(base: &str, version: &str) -> String {
        format!("{base}/download/cua-spacesd-v{version}/{}", artifact_name())
    }

    async fn install_release(base: &str, version: &str) -> Result<(String, Vec<u8>)> {
        let (used, archive) = download_release(
            version,
            &pinned_url(base, version),
            &format!("{base}/releases"),
        )
        .await?;
        Ok((used, extract_binary(&archive)?))
    }

    #[tokio::test]
    async fn release_uses_the_pinned_version_when_published() {
        let base = fake_github(vec![
            published("cua-spacesd-v0.2.2", b"v0.2.2"),
            published("cua-spacesd-v0.2.0", b"v0.2.0"),
        ])
        .await;
        let (used, bin) = install_release(&base, "0.2.0").await.unwrap();
        assert_eq!((used.as_str(), bin.as_slice()), ("0.2.0", &b"v0.2.0"[..]));
    }

    #[tokio::test]
    async fn release_falls_back_to_the_newest_published_patch() {
        // The 0.3.0 incident: 0.2.0 stayed an empty draft.
        let base = fake_github(vec![
            FakeRelease {
                draft: true,
                ..published("cua-spacesd-v0.2.3", b"draft")
            },
            FakeRelease {
                prerelease: true,
                ..published("cua-spacesd-v0.2.4-rc.1", b"rc")
            },
            published("cua-spacesd-v0.3.0", b"v0.3.0"),
            published("cua-spacesd-v0.2.2", b"v0.2.2"),
            published("cua-spacesd-v0.2.1", b"v0.2.1"),
            published("cua-driver-v9.9.9", b"other"),
            FakeRelease {
                draft: true,
                ..published("cua-spacesd-v0.2.0", b"v0.2.0")
            },
        ])
        .await;
        let (used, bin) = install_release(&base, "0.2.0").await.unwrap();
        assert_eq!((used.as_str(), bin.as_slice()), ("0.2.2", &b"v0.2.2"[..]));
    }

    #[tokio::test]
    async fn release_falls_back_to_the_closest_newer_release_on_the_same_major() {
        let base = fake_github(vec![
            published("cua-spacesd-v1.0.0", b"v1.0.0"),
            published("cua-spacesd-v0.4.0", b"v0.4.0"),
            published("cua-spacesd-v0.3.1", b"v0.3.1"),
            published("cua-spacesd-v0.1.9", b"v0.1.9"),
        ])
        .await;
        let (used, bin) = install_release(&base, "0.2.0").await.unwrap();
        assert_eq!((used.as_str(), bin.as_slice()), ("0.3.1", &b"v0.3.1"[..]));
    }

    #[tokio::test]
    async fn release_fallback_still_verifies_the_checksum_of_the_release_it_uses() {
        let base = fake_github(vec![
            FakeRelease {
                bad_sum: true,
                ..published("cua-spacesd-v0.2.2", b"tampered")
            },
            published("cua-spacesd-v0.2.1", b"v0.2.1"),
        ])
        .await;
        let err = install_release(&base, "0.2.0").await.unwrap_err();
        assert!(err.to_string().contains("sha256 mismatch"), "{err}");
    }

    #[tokio::test]
    async fn release_with_nothing_published_is_a_plain_error() {
        let base = fake_github(vec![
            FakeRelease {
                draft: true,
                ..published("cua-spacesd-v0.2.0", b"v0.2.0")
            },
            published("cua-spacesd-v1.0.0", b"v1.0.0"),
        ])
        .await;
        let err = install_release(&base, "0.2.0")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("download: cua-spacesd 0.2.0 (the Cua host service"),
            "{err}"
        );
        assert!(err.contains("no compatible published release"), "{err}");
        // A dead releases API reads the same way.
        let err = download_release(
            "0.2.0",
            &pinned_url(&base, "0.2.0"),
            &format!("{base}/no-such-api"),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("is not available to download"), "{err}");
    }

    #[tokio::test]
    async fn release_source_installs_through_install_to() {
        let base = fake_github(vec![published("cua-spacesd-v0.2.2", b"v0.2.2")]).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("bin").join(binary_name());
        install_to(
            &DriverSource::Release {
                version: "0.2.0".into(),
                url: pinned_url(&base, "0.2.0"),
                releases_api: format!("{base}/releases"),
            },
            &dest,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"v0.2.2");
    }

    #[test]
    fn fallback_candidates_require_the_artifact_under_the_release_tag() {
        let a = artifact_name();
        let listing = serde_json::json!([
            {"tag_name": "cua-spacesd-v0.2.5", "draft": false, "prerelease": false, "assets": [
                {"name": a, "browser_download_url": format!("https://x/cua-spacesd-v0.2.9/{a}")},
                {"name": format!("{a}.sha256"), "browser_download_url": format!("https://x/cua-spacesd-v0.2.9/{a}.sha256")},
            ]},
            {"tag_name": "cua-spacesd-v0.2.4", "draft": false, "prerelease": false, "assets": [
                {"name": a, "browser_download_url": format!("https://x/cua-spacesd-v0.2.4/{a}")},
            ]},
            {"tag_name": "cua-spacesd-v0.2.3", "draft": false, "prerelease": false, "assets": [
                {"name": a, "browser_download_url": format!("https://x/cua-spacesd-v0.2.3/{a}")},
                {"name": format!("{a}.sha256"), "browser_download_url": format!("https://x/cua-spacesd-v0.2.3/{a}.sha256")},
            ]},
        ]);
        let got = fallback_candidates(&listing, (0, 2, 0), &a);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].version_string(), "0.2.3");
        assert_eq!(parse_version("v0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse_version("0.2.0-rc.1"), None);
    }
}
