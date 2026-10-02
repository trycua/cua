//! Built-in Lume: a pinned, signed Lume that cua downloads into its own
//! directory the first time a macOS Space needs one, so creating a macOS
//! Space never asks for Homebrew, a terminal or an installer.
//!
//! * **Which Lume.** [`LumeSource`] (`cua config set runtime.lume`, the
//!   apps' Settings): `auto` (the default) uses a Lume already on this Mac
//!   and the built-in one otherwise; `builtin` always the built-in one;
//!   `system` only the one on this Mac, and never downloads anything.
//! * **Pinned and verified.** [`VERSION`] from the trycua/cua release, its
//!   SHA-256 checked before anything is unpacked, then `codesign --verify`
//!   and the Developer ID team ([`TEAM_ID`]) checked before it is used. A
//!   new app (and cua) release moves the pin.
//! * **Cua's own directory.** `$CUA_HOME/runtimes/lume/<version>`
//!   ([`dir`]); [`remove`] deletes it. A user's own Lume is never touched.
//! * **Lazy.** Nothing is downloaded until a create needs it
//!   ([`super::LumeRuntime::ensure_serving`]), with the download reported as
//!   the create's progress.

use std::path::{Path, PathBuf};

use crate::error::{Result, VmmError};
use crate::host;

/// The pinned Lume release.
pub const VERSION: &str = "0.6.0";
/// Its Apple silicon tarball (`lume` launcher + signed `lume.app`).
pub const URL: &str =
    "https://github.com/trycua/cua/releases/download/lume-v0.6.0/lume-0.6.0-darwin-arm64.tar.gz";
/// The tarball's SHA-256 (the release manifest's).
pub const SHA256: &str = "4d25c7c36ebd3fdf0e2f97f9e7a2c4ff2d0538ba9eb2d536f6dbb542d9d504a7";
/// The tarball's size, for progress and the disk check.
pub const SIZE: u64 = 6_114_070;
/// Cua AI, Inc.'s Developer ID team: the only signer accepted.
pub const TEAM_ID: &str = "YCK386LBJ7";

/// The setting that picks the Lume: `runtime.lume` in
/// `$CUA_HOME/config.toml`, overridden by `CUA_RUNTIME_LUME`.
pub const SETTING_ENV: &str = "CUA_RUNTIME_LUME";

/// Which Lume macOS Spaces run on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LumeSource {
    /// This Mac's Lume when there is one, else the built-in one.
    #[default]
    Auto,
    /// Always the built-in one.
    Builtin,
    /// Only this Mac's own Lume (nothing is downloaded).
    System,
}

impl LumeSource {
    /// `auto`, `builtin` or `system`.
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Self::Auto),
            "builtin" | "built-in" => Some(Self::Builtin),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    /// The setting's word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Builtin => "builtin",
            Self::System => "system",
        }
    }

    /// The setting now: the environment, else the config file, else auto.
    /// Read on every use, so a change in Settings applies at once.
    pub fn current() -> Self {
        if let Some(s) = std::env::var(SETTING_ENV)
            .ok()
            .and_then(|v| Self::parse(&v))
        {
            return s;
        }
        Self::from_config(&host::cua_home().join("config.toml"))
    }

    /// `[runtime] lume = "..."` in `path` (auto when absent or unreadable).
    pub fn from_config(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| t.parse::<toml_edit::DocumentMut>().ok())
            .and_then(|d| {
                d.get("runtime")?
                    .get("lume")?
                    .as_str()
                    .and_then(Self::parse)
            })
            .unwrap_or_default()
    }

    /// Whether the built-in Lume may be used (and downloaded).
    pub fn allows_builtin(self) -> bool {
        self != Self::System
    }
}

/// Where the built-in Lume lives.
pub fn dir() -> PathBuf {
    root().join(VERSION)
}

/// `$CUA_HOME/runtimes/lume`: every built-in Lume version.
pub fn root() -> PathBuf {
    host::cua_home().join("runtimes").join("lume")
}

/// The built-in Lume's executable, when it is installed and was verified.
pub fn installed() -> Option<PathBuf> {
    let d = dir();
    let bin = d.join("lume.app/Contents/MacOS/lume");
    (bin.is_file() && d.join(VERIFIED).is_file()).then_some(bin)
}

/// Written last, once the unpacked Lume passed every check.
const VERIFIED: &str = ".verified";

/// The Lume to run for `source`: this Mac's own (`system`, or `auto` when
/// it has one), else the built-in one when installed.
pub fn resolve(source: LumeSource) -> Option<PathBuf> {
    let system = || host::which("lume").filter(|p| !p.starts_with(root()));
    match source {
        LumeSource::System => system(),
        LumeSource::Builtin => installed(),
        LumeSource::Auto => system().or_else(installed),
    }
}

/// Deletes every built-in Lume (`$CUA_HOME/runtimes/lume`). A user's own
/// Lume is elsewhere and stays.
pub fn remove() -> Result<()> {
    match std::fs::remove_dir_all(root()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// One install at a time in this process (a second create waits for it).
static INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Installs the built-in Lume when it is not there yet; its executable.
/// The download is reported as the running create's progress.
#[cfg(feature = "lume")]
pub async fn ensure() -> Result<PathBuf> {
    if let Some(bin) = installed() {
        return Ok(bin);
    }
    let _one = INSTALL.lock().await;
    if let Some(bin) = installed() {
        return Ok(bin);
    }
    if !(cfg!(target_os = "macos") && cfg!(target_arch = "aarch64")) {
        return Err(VmmError::missing(
            "Lume",
            "macOS Spaces need a Mac with Apple silicon",
        ));
    }
    std::fs::create_dir_all(root())?;
    crate::disk::ensure_space(&root(), SIZE * 5, "set up the built-in Lume")?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(root())?;
    let tarball = staging.path().join("lume.tar.gz");
    download(URL, &tarball).await?;
    let got = sha256_file(&tarball)?;
    if got != SHA256 {
        return Err(VmmError::other(format!(
            "the built-in Lume download did not match its checksum (got {got}); try again"
        )));
    }
    let unpacked = staging.path().join("lume");
    std::fs::create_dir_all(&unpacked)?;
    host::run(
        "/usr/bin/tar",
        &[
            "-xzf",
            &tarball.to_string_lossy(),
            "-C",
            &unpacked.to_string_lossy(),
        ],
    )
    .await?;
    verify_signature(&unpacked.join("lume.app")).await?;
    std::fs::write(unpacked.join(VERIFIED), VERSION)?;
    let target = dir();
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&unpacked, &target)?;
    tracing::info!(version = VERSION, dir = %target.display(), "installed the built-in Lume");
    installed().ok_or_else(|| VmmError::other("the built-in Lume did not install"))
}

/// Streams `url` to `to`, reporting bytes as create progress.
#[cfg(feature = "lume")]
async fn download(url: &str, to: &Path) -> Result<()> {
    use crate::progress::{Meter, Phase, Progress, report};
    use tokio::io::AsyncWriteExt as _;
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| VmmError::other(format!("download the built-in Lume: {e}")))?;
    let mut res = http
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| VmmError::other(format!("download the built-in Lume: {e}")))?;
    let total = res.content_length().unwrap_or(SIZE);
    let mut file = tokio::fs::File::create(to).await?;
    let mut meter = Meter::new();
    let mut done = 0u64;
    let detail = format!("Setting up Lume {VERSION} for macOS Spaces");
    report(Progress::phase(Phase::Preparing).detail(detail.clone()));
    while let Some(chunk) = res
        .chunk()
        .await
        .map_err(|e| VmmError::other(format!("download the built-in Lume: {e}")))?
    {
        if done + chunk.len() as u64 > SIZE * 4 {
            return Err(VmmError::other("the built-in Lume download is too large"));
        }
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        if let Some(t) = meter.sample_at(std::time::Instant::now(), done, total) {
            report(
                Progress::phase(Phase::Preparing)
                    .detail(detail.clone())
                    .bytes(t)
                    .fraction(done as f64 / total.max(1) as f64),
            );
        }
    }
    file.flush().await?;
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// `codesign --verify --strict` passes and the signer is [`TEAM_ID`].
async fn verify_signature(app: &Path) -> Result<()> {
    let app = app.to_string_lossy();
    host::run(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", &app],
    )
    .await
    .map_err(|e| VmmError::other(format!("the built-in Lume is not validly signed: {e}")))?;
    let out = tokio::process::Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2", &app])
        .output()
        .await?;
    // codesign -d prints to stderr.
    let text = String::from_utf8_lossy(&out.stderr);
    if !team_matches(&text) {
        return Err(VmmError::other(
            "the built-in Lume is not signed by Cua AI, Inc.",
        ));
    }
    Ok(())
}

fn team_matches(codesign_display: &str) -> bool {
    codesign_display
        .lines()
        .any(|l| l.trim() == format!("TeamIdentifier={TEAM_ID}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_parses_and_defaults_to_auto() {
        assert_eq!(LumeSource::parse("Built-in"), Some(LumeSource::Builtin));
        assert_eq!(LumeSource::parse("system"), Some(LumeSource::System));
        assert_eq!(LumeSource::parse("nope"), None);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("config.toml");
        assert_eq!(LumeSource::from_config(&p), LumeSource::Auto);
        std::fs::write(
            &p,
            "[default]\non = \"local\"\n\n[runtime]\nlume = \"system\"\n",
        )
        .unwrap();
        assert_eq!(LumeSource::from_config(&p), LumeSource::System);
        assert!(!LumeSource::System.allows_builtin());
        assert!(LumeSource::Auto.allows_builtin());
    }

    #[test]
    fn only_cua_s_team_is_accepted() {
        assert!(team_matches(
            "Identifier=com.trycua.lume\nTeamIdentifier=YCK386LBJ7\n"
        ));
        assert!(!team_matches("TeamIdentifier=ABCDE12345\n"));
        assert!(!team_matches("TeamIdentifier=not set\n"));
    }

    #[test]
    fn the_pin_is_a_sha256_and_a_release_url() {
        assert_eq!(SHA256.len(), 64);
        assert!(URL.contains(&format!(
            "lume-v{VERSION}/lume-{VERSION}-darwin-arm64.tar.gz"
        )));
    }

    /// Opt-in (network): downloads the pinned Lume into a temporary
    /// `CUA_HOME`, verifies it, and finds it as `builtin` and `auto`.
    /// `CUA_HOME=<tmp> cargo test -p cua-vmm --lib builtin -- --ignored`.
    #[cfg(all(feature = "lume", target_os = "macos", target_arch = "aarch64"))]
    #[tokio::test]
    #[ignore = "downloads the pinned Lume release (network)"]
    async fn downloads_and_verifies_the_pinned_lume() {
        let home = std::env::var("CUA_HOME").expect("set CUA_HOME to a temporary directory");
        assert!(!home.is_empty() && !home.ends_with("/.cua"), "never the real cua home");
        let bin = ensure().await.unwrap();
        assert_eq!(installed(), Some(bin.clone()));
        assert_eq!(resolve(LumeSource::Builtin), Some(bin.clone()));
        let out = std::process::Command::new(&bin).arg("--version").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains(VERSION));
        // Again: already there, nothing downloaded.
        assert_eq!(ensure().await.unwrap(), bin);
        remove().unwrap();
        assert!(installed().is_none());
    }
}
