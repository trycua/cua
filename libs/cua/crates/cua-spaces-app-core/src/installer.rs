// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The first-run "Command line" step: put the app's bundled `cua` CLI on
//! PATH. Both Spaces shells run this (the Tauri app through its
//! `installer_*` commands, the SwiftUI app through the cua SDK's
//! `AppCliInstaller`), so the plan they show and the files they write are
//! the same.
//!
//! What the CLI is installed as depends on the build's [`CuaChannel`]:
//! Stable installs `~/.local/bin/cua`; Local ("Cua Spaces Local", CUA-1776)
//! installs `~/.local/bin/cua-local` and never touches `cua`, so the two
//! apps keep their own commands. The bundled file is `cua` in both. Local
//! adds `~/.local/bin` to PATH the way Stable does, only when it is not on
//! PATH already and its line is not in the profile yet.
//!
//! [`CliInstaller::plan`] shows the exact target path before anything is
//! written; [`CliInstaller::install`] writes only after the user consents.
//! Paths and environment are injected, so tests run against temp dirs and
//! fake binaries only.

use std::path::{Path, PathBuf};
#[cfg(feature = "installer")]
use std::process::Stdio;
#[cfg(feature = "installer")]
use std::time::Duration;

/// The build channel that names the installed CLI (re-exported for the
/// shells that build a [`CliInstaller`]).
pub use cua_home::channel::CuaChannel;
use serde::{Deserialize, Serialize};

/// Marker line written above a PATH edit (same text as install.sh).
pub const PATH_MARKER: &str = "# Added by the cua installer";

#[cfg(feature = "installer")]
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// File name of the CLI the app bundles (its sidecar) on this OS. Every app
/// ships it as `cua`; what it is installed as depends on the channel
/// ([`installed_cli_file_name`]).
pub fn cli_file_name() -> &'static str {
    if cfg!(windows) { "cua.exe" } else { "cua" }
}

/// File name the CLI is installed under on this OS: `cua` for Stable,
/// `cua-local` for Local (`.exe` on Windows).
pub fn installed_cli_file_name(channel: CuaChannel) -> String {
    format!(
        "{}{}",
        channel.cli_name(),
        if cfg!(windows) { ".exe" } else { "" }
    )
}

/// The sidecar `cua` next to the app executable, when the bundle has one.
pub fn bundled_cli(exe: &Path) -> Option<PathBuf> {
    let candidate = exe.parent()?.join(cli_file_name());
    candidate.is_file().then_some(candidate)
}

/// Where the GUI installs the CLI: `~/.local/bin` on macOS/Linux (the same
/// place install.sh uses), `%LOCALAPPDATA%\Programs\cua\bin` on Windows.
/// Both channels share the folder (the file name tells them apart).
pub fn default_bin_dir(home: &Path, local_app_data: Option<&Path>) -> PathBuf {
    if cfg!(windows) {
        local_app_data
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("Programs")
            .join("cua")
            .join("bin")
    } else {
        home.join(".local").join("bin")
    }
}

/// The shell profile install.sh would edit for `shell` (`$SHELL`).
pub fn profile_for_shell(home: &Path, shell: Option<&str>, xdg_config: Option<&Path>) -> PathBuf {
    let name = shell.and_then(|s| s.rsplit('/').next()).unwrap_or_default();
    match name {
        "zsh" => home.join(".zshrc"),
        "bash" if cfg!(target_os = "macos") => home.join(".bash_profile"),
        "bash" => home.join(".bashrc"),
        "fish" => xdg_config
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(".config"))
            .join("fish")
            .join("config.fish"),
        _ => home.join(".profile"),
    }
}

/// The line that puts `dir` on PATH in `profile`.
pub fn path_line(dir: &Path, profile: &Path) -> String {
    if profile.extension().is_some_and(|e| e == "fish") {
        format!("fish_add_path \"{}\"", dir.display())
    } else {
        format!("export PATH=\"{}:$PATH\"", dir.display())
    }
}

/// Whether `dir` is an entry of a PATH-style string.
pub fn path_contains(path_env: &str, dir: &Path) -> bool {
    std::env::split_paths(path_env).any(|p| p == dir)
}

/// How the CLI lands at its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallMethod {
    /// A symlink into the app bundle: app updates update the CLI too.
    Symlink,
    /// A copy of the bundled binary.
    Copy,
}

/// What the "Install the cua CLI" step shows before asking for consent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallPlan {
    /// The bundled CLI, when this build carries one.
    pub source: Option<String>,
    /// Exact file the CLI is written to.
    pub target: String,
    /// Directory of `target`.
    pub bin_dir: String,
    /// `symlink` or `copy`.
    pub method: Option<InstallMethod>,
    /// Something already exists at `target`.
    pub installed: bool,
    /// `target` resolves to this app's bundled CLI (nothing to do).
    pub up_to_date: bool,
    /// `cua --version` of what is at `target`.
    pub installed_version: Option<String>,
    /// `cua --version` of the bundled CLI.
    pub bundled_version: Option<String>,
    /// Another CLI of the same name found earlier on PATH (it would shadow
    /// `target`).
    pub shadowed_by: Option<String>,
    /// `bin_dir` is already on PATH.
    pub on_path: bool,
    /// Shell profile (or "user PATH" on Windows) the PATH change would edit.
    pub path_profile: Option<String>,
    /// The line that would be added to `path_profile`.
    pub path_line: Option<String>,
    /// A working CLI of this channel already on this machine: `target`, else
    /// the first in the usual install folders or on PATH (a Finder-launched
    /// app's PATH lacks `~/.local/bin`, so the folders are searched too).
    /// Local looks for `cua-local` only: a Stable `cua` is not its CLI.
    #[serde(default)]
    pub found: Option<String>,
}

/// What the user agreed to in the CLI step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInstallRequest {
    /// Also add the bin dir to PATH (edits `path_profile`).
    #[serde(default)]
    pub modify_path: bool,
}

/// Installs the bundled CLI onto PATH. Paths and environment are injected.
#[derive(Clone, Debug)]
#[cfg(feature = "installer")]
pub struct CliInstaller {
    /// The build channel: names the installed CLI (`cua` or `cua-local`).
    pub channel: CuaChannel,
    pub bundled: Option<PathBuf>,
    pub bin_dir: PathBuf,
    /// Value of PATH used for the on-path / shadow checks.
    pub path_env: String,
    /// Shell profile edited by `modify_path` (non-Windows).
    pub profile: PathBuf,
    /// Symlink rather than copy (stable app location on macOS).
    pub prefer_symlink: bool,
    /// Other folders a `cua` is usually installed in (Homebrew, a root
    /// install.sh), searched with PATH for one already there.
    pub search_dirs: Vec<PathBuf>,
}

#[cfg(feature = "installer")]
impl CliInstaller {
    /// The real machine: sidecar next to `exe`, `$HOME`, `$PATH`, `$SHELL`,
    /// and the channel of this process ([`CuaChannel::current`]; the app
    /// sets `CUA_CHANNEL` before anything starts, and a Local app's own path
    /// says Local too).
    pub fn from_env(exe: &Path) -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        let shell = std::env::var("SHELL").ok();
        let bundled = bundled_cli(exe);
        Self {
            channel: CuaChannel::current(),
            prefer_symlink: bundled.as_deref().is_some_and(stable_app_location),
            bundled,
            bin_dir: default_bin_dir(&home, local.as_deref()),
            path_env: std::env::var("PATH").unwrap_or_default(),
            profile: profile_for_shell(&home, shell.as_deref(), xdg.as_deref()),
            search_dirs: known_bin_dirs(&home),
        }
    }

    /// A working `cua` CLI already here: `target` when it runs as one (its
    /// `--version` is `installed_version`), else the first in
    /// [`Self::search_dirs`] or on PATH.
    async fn find_installed(
        &self,
        target: &Path,
        installed_version: Option<&str>,
    ) -> Option<String> {
        if installed_version.is_some_and(is_cua_cli) {
            return Some(target.display().to_string());
        }
        let mut seen = vec![target.to_path_buf()];
        let dirs = self
            .search_dirs
            .iter()
            .cloned()
            .chain(std::env::split_paths(&self.path_env));
        for dir in dirs {
            let cli = dir.join(self.installed_name());
            if seen.contains(&cli) || !cli.is_file() {
                continue;
            }
            seen.push(cli.clone());
            if cli_version(&cli).await.as_deref().is_some_and(is_cua_cli) {
                return Some(cli.display().to_string());
            }
        }
        None
    }

    /// The file name the CLI is installed under: `cua` or `cua-local`.
    pub fn installed_name(&self) -> String {
        installed_cli_file_name(self.channel)
    }

    /// The file the CLI is written to.
    pub fn target(&self) -> PathBuf {
        self.bin_dir.join(self.installed_name())
    }

    fn method(&self) -> InstallMethod {
        if self.prefer_symlink && cfg!(unix) {
            InstallMethod::Symlink
        } else {
            InstallMethod::Copy
        }
    }

    pub async fn plan(&self) -> CliInstallPlan {
        let target = self.target();
        let installed = target.symlink_metadata().is_ok();
        let installed_version = if installed {
            cli_version(&target).await
        } else {
            None
        };
        let bundled_version = match &self.bundled {
            Some(src) => cli_version(src).await,
            None => None,
        };
        // Up to date when `target` is the bundled CLI and runs as it, or
        // when it is a working `cua` that the bundled one would not be
        // newer than (a CLI the user upgraded with install.sh is never
        // replaced by an older app's). Another `cua` there that is not a
        // working one (an old Python cua-cli from `uv tool`, say) or one
        // that no longer starts gets replaced.
        let up_to_date = match (&self.bundled, installed) {
            (Some(src), true) => {
                let runs = installed_version.as_deref().is_some_and(is_cua_cli);
                runs && (same_file(src, &target)
                    || keeps_installed(installed_version.as_deref(), bundled_version.as_deref()))
            }
            _ => false,
        };
        let on_path = path_contains(&self.path_env, &self.bin_dir);
        let shadowed_by = first_on_path(&self.path_env, &self.installed_name())
            .filter(|found| {
                found != &target && !self.bundled.as_ref().is_some_and(|b| same_file(b, found))
            })
            .filter(|_| on_path_before(&self.path_env, &self.bin_dir, &self.installed_name()))
            .map(|p| p.display().to_string());
        let (path_profile, line) = if on_path {
            (None, None)
        } else if cfg!(windows) {
            (
                Some("user PATH".to_string()),
                Some(self.bin_dir.display().to_string()),
            )
        } else {
            (
                Some(self.profile.display().to_string()),
                Some(path_line(&self.bin_dir, &self.profile)),
            )
        };
        let found = self
            .find_installed(&target, installed_version.as_deref())
            .await;
        CliInstallPlan {
            found,
            source: self.bundled.as_ref().map(|p| p.display().to_string()),
            target: target.display().to_string(),
            bin_dir: self.bin_dir.display().to_string(),
            method: self.bundled.as_ref().map(|_| self.method()),
            installed,
            up_to_date,
            installed_version,
            bundled_version,
            shadowed_by,
            on_path,
            path_profile,
            path_line: line,
        }
    }

    /// Writes the CLI to [`Self::target`] (atomically replacing what is
    /// there) and, when asked, adds the bin dir to PATH.
    pub async fn install(&self, request: &CliInstallRequest) -> Result<CliInstallPlan, String> {
        let source = self
            .bundled
            .as_ref()
            .ok_or("this build of Cua Spaces does not bundle the cua CLI")?;
        let target = self.target();
        // Never a downgrade: a working `cua` there that the bundled one is
        // not newer than stays as it is.
        if self.keeps_newer_installed(source, &target).await {
            return Ok(self.plan().await);
        }
        std::fs::create_dir_all(&self.bin_dir)
            .map_err(|e| format!("cannot create {}: {e}", self.bin_dir.display()))?;
        let staging = self
            .bin_dir
            .join(format!(".{}.cua-new", self.installed_name()));
        let _ = std::fs::remove_file(&staging);
        match self.method() {
            InstallMethod::Symlink => {
                #[cfg(unix)]
                std::os::unix::fs::symlink(source, &staging)
                    .map_err(|e| format!("cannot link {}: {e}", staging.display()))?;
            }
            InstallMethod::Copy => {
                std::fs::copy(source, &staging)
                    .map_err(|e| format!("cannot copy the CLI to {}: {e}", staging.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
                        .map_err(|e| {
                            format!("cannot mark {} executable: {e}", staging.display())
                        })?;
                }
            }
        }
        std::fs::rename(&staging, &target).map_err(|e| {
            let _ = std::fs::remove_file(&staging);
            format!("cannot install {}: {e}", target.display())
        })?;
        // What is there now must run as this CLI before the app shows it as
        // "the cua command".
        let version = cli_version(&target).await;
        if !version.as_deref().is_some_and(is_cua_cli) {
            return Err(format!(
                "installed {} but it does not run as the cua CLI (`cua --version` printed {})",
                target.display(),
                version
                    .as_deref()
                    .map_or("nothing".into(), |v| format!("{v:?}"))
            ));
        }
        if request.modify_path && !path_contains(&self.path_env, &self.bin_dir) {
            self.add_to_path()?;
        }
        let mut plan = self.plan().await;
        if request.modify_path {
            // The running app's PATH is unchanged; new shells pick it up.
            plan.on_path = true;
            plan.path_profile = None;
            plan.path_line = None;
        }
        Ok(plan)
    }

    /// The `cua` at `target` runs as the cua CLI and `source` would not
    /// upgrade it ([`keeps_installed`]).
    async fn keeps_newer_installed(&self, source: &Path, target: &Path) -> bool {
        if target.symlink_metadata().is_err() || same_file(source, target) {
            return false;
        }
        let installed = cli_version(target).await;
        installed.as_deref().is_some_and(is_cua_cli)
            && keeps_installed(installed.as_deref(), cli_version(source).await.as_deref())
    }

    fn add_to_path(&self) -> Result<(), String> {
        if cfg!(windows) {
            return add_to_windows_user_path(&self.bin_dir);
        }
        let line = path_line(&self.bin_dir, &self.profile);
        let existing = std::fs::read_to_string(&self.profile).unwrap_or_default();
        if existing.lines().any(|l| l.trim() == line) {
            return Ok(());
        }
        if let Some(parent) = self.profile.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let mut body = existing;
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&format!("\n{PATH_MARKER}\n{line}\n"));
        std::fs::write(&self.profile, body)
            .map_err(|e| format!("cannot update {}: {e}", self.profile.display()))
    }
}

#[cfg(all(windows, feature = "installer"))]
fn add_to_windows_user_path(dir: &Path) -> Result<(), String> {
    // HKCU\Environment Path through .NET, which also broadcasts the change.
    let script = format!(
        "$d='{}'; $p=[Environment]::GetEnvironmentVariable('Path','User'); \
         $parts=@(if($p){{$p -split ';' | ? {{ $_ }}}}); \
         if($parts -notcontains $d){{[Environment]::SetEnvironmentVariable('Path',(($parts+$d) -join ';'),'User')}}",
        dir.display().to_string().replace('\'', "''")
    );
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .map_err(|e| format!("cannot run powershell: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "could not update the user PATH".to_string())
}

#[cfg(all(not(windows), feature = "installer"))]
fn add_to_windows_user_path(_dir: &Path) -> Result<(), String> {
    Err("not Windows".into())
}

/// App locations that survive relaunches (not a mounted dmg or App
/// Translocation, not an AppImage's mount), where a symlink into the bundle
/// stays valid: app updates update the CLI too, and removing the app takes
/// it away, as dragging the Swift app to the Trash does.
pub fn stable_app_location(cli: &Path) -> bool {
    if cfg!(target_os = "linux") {
        return linux_package_location(cli);
    }
    if !cfg!(target_os = "macos") {
        return false;
    }
    let s = cli.display().to_string();
    let in_apps = s.starts_with("/Applications/")
        || std::env::var_os("HOME")
            .map(|h| s.starts_with(&format!("{}/Applications/", PathBuf::from(h).display())))
            .unwrap_or(false);
    in_apps && !s.contains("/AppTranslocation/")
}

/// A Linux package's install tree (the `.deb`'s `/opt/Cua Spaces`, or a
/// distribution's `/usr`): root-owned and replaced in place by upgrades.
pub fn linux_package_location(cli: &Path) -> bool {
    cli.is_absolute() && (cli.starts_with("/opt") || cli.starts_with("/usr"))
}

#[cfg(feature = "installer")]
fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) if x == y => true,
        _ => match (std::fs::read(a), std::fs::read(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        },
    }
}

/// Where a `cua` is usually installed besides the GUI's own target:
/// Homebrew and a root install.sh's `/usr/local/bin` (not on Windows).
pub fn known_bin_dirs(home: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        return vec![];
    }
    vec![
        home.join(".local").join("bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]
}

/// The first `name` on a PATH-style string.
pub fn first_on_path(path_env: &str, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path_env)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Whether some *other* PATH entry with a CLI file `name` comes before
/// `bin_dir` (or `bin_dir` is not on PATH at all).
#[cfg(feature = "installer")]
fn on_path_before(path_env: &str, bin_dir: &Path, name: &str) -> bool {
    for dir in std::env::split_paths(path_env) {
        if dir == bin_dir {
            return false;
        }
        if dir.join(name).is_file() {
            return true;
        }
    }
    true
}

/// Whether a `cua --version` line is the Rust cua CLI's: `cua <semver>`
/// (an optional suffix such as a build or commit may follow).
pub fn is_cua_cli(version_line: &str) -> bool {
    let mut words = version_line.split_whitespace();
    if words.next() != Some("cua") {
        return false;
    }
    let Some(v) = words.next() else {
        return false;
    };
    let core = v.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether the installed `cua` (its `--version` line) stays because the
/// bundled one (its line) would not be an upgrade: the same version or
/// newer installed, by semver, a dev build counting as older unless it is
/// the same build. A bundled CLI whose version cannot be read never
/// replaces a working one.
pub fn keeps_installed(installed: Option<&str>, bundled: Option<&str>) -> bool {
    let Some(installed) = installed.filter(|v| is_cua_cli(v)) else {
        return false;
    };
    match bundled {
        Some(bundled) => !cua_home::version::bundled_replaces(Some(installed), bundled),
        None => true,
    }
}

/// `ETXTBSY` ("text file busy") on Linux and macOS.
#[cfg(feature = "installer")]
const ETXTBSY: i32 = 26;

/// `cua --version`, first line, or `None` if it does not run.
#[cfg(feature = "installer")]
pub async fn cli_version(cli: &Path) -> Option<String> {
    // A file just written can be briefly "busy" to exec (ETXTBSY) while a
    // process forked elsewhere still holds its write handle: try again.
    let mut attempt = 0;
    let out = loop {
        let run = tokio::process::Command::new(cli)
            .arg("--version")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(VERSION_TIMEOUT, run).await.ok()? {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && attempt < 10 => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            r => break r.ok()?,
        }
    };
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

#[cfg(all(test, feature = "installer"))]
mod tests {
    use super::*;

    #[test]
    fn a_linux_package_links_its_cli_and_an_appimage_copies_it() {
        assert!(linux_package_location(Path::new(
            "/opt/Cua Spaces/resources/native/cua"
        )));
        assert!(linux_package_location(Path::new("/usr/lib/cua-spaces/cua")));
        // An AppImage mounts somewhere new each launch; its CLI is copied.
        assert!(!linux_package_location(Path::new(
            "/tmp/.mount_CuaSpaXYZ/resources/native/cua"
        )));
        assert!(!linux_package_location(Path::new(
            "/home/u/.local/share/cua-spaces/daemon/0.7.2-1/cua"
        )));
        assert!(!linux_package_location(Path::new("/optional/cua")));
        assert!(!linux_package_location(Path::new("opt/cua")));
    }

    #[test]
    fn profiles_and_path_lines() {
        let home = Path::new("/h");
        assert_eq!(
            profile_for_shell(home, Some("/bin/zsh"), None),
            home.join(".zshrc")
        );
        assert_eq!(profile_for_shell(home, None, None), home.join(".profile"));
        assert_eq!(
            profile_for_shell(home, Some("/usr/bin/fish"), Some(Path::new("/x"))),
            Path::new("/x/fish/config.fish")
        );
        assert_eq!(
            path_line(Path::new("/h/.local/bin"), Path::new("/h/.zshrc")),
            "export PATH=\"/h/.local/bin:$PATH\""
        );
        assert_eq!(
            path_line(Path::new("/b"), Path::new("/c/config.fish")),
            "fish_add_path \"/b\""
        );
    }

    #[test]
    fn only_the_rust_cli_version_counts_as_cua() {
        assert!(is_cua_cli("cua 0.4.0"));
        assert!(is_cua_cli("cua 1.2.3-rc.1 (abc123)"));
        assert!(!is_cua_cli("cua-cli 0.1.9"));
        assert!(!is_cua_cli("cua, version 0.1.9"));
        assert!(!is_cua_cli("cua 0.4"));
        assert!(!is_cua_cli("Traceback (most recent call last):"));
        assert!(!is_cua_cli(""));
    }

    #[test]
    fn path_contains_matches_whole_entries() {
        let env = std::env::join_paths(["/usr/bin", "/h/.local/bin"]).unwrap();
        let env = env.to_string_lossy();
        assert!(path_contains(&env, Path::new("/h/.local/bin")));
        assert!(!path_contains(&env, Path::new("/h/.local")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn plans_then_installs_into_a_temp_home_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("Cua Spaces.app/Contents/MacOS");
        std::fs::create_dir_all(&app).unwrap();
        let exe = app.join("CuaSpacesMac");
        std::fs::write(&exe, "").unwrap();
        let cua = app.join(cli_file_name());
        std::fs::write(&cua, "#!/bin/sh\necho \"cua 1.2.3\"\n").unwrap();
        std::fs::set_permissions(&cua, std::fs::Permissions::from_mode(0o755)).unwrap();
        let home = tmp.path().join("home");
        let cli = CliInstaller {
            channel: CuaChannel::Stable,
            bundled: bundled_cli(&exe),
            bin_dir: default_bin_dir(&home, None),
            path_env: "/usr/bin:/bin".into(),
            profile: profile_for_shell(&home, Some("/bin/zsh"), None),
            prefer_symlink: false,
            search_dirs: vec![],
        };
        let plan = cli.plan().await;
        assert_eq!(
            plan.target,
            home.join(".local/bin/cua").display().to_string()
        );
        assert_eq!(plan.bundled_version.as_deref(), Some("cua 1.2.3"));
        assert!(!plan.installed && !plan.on_path);
        assert!(!home.exists(), "planning writes nothing");
        let done = cli
            .install(&CliInstallRequest { modify_path: true })
            .await
            .unwrap();
        assert!(done.installed && done.up_to_date && done.on_path);
        let rc = std::fs::read_to_string(home.join(".zshrc")).unwrap();
        assert!(
            rc.contains(PATH_MARKER) && rc.contains(".local/bin"),
            "{rc}"
        );
        // A foreign `cua` at the target (an old Python cua-cli) is not up
        // to date: the next install replaces it.
        std::fs::remove_file(home.join(".local/bin/cua")).unwrap();
        std::fs::write(
            home.join(".local/bin/cua"),
            "#!/bin/sh\necho 'cua, version 0.1.9'\n",
        )
        .unwrap();
        std::fs::set_permissions(
            home.join(".local/bin/cua"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let plan = cli.plan().await;
        assert!(plan.installed && !plan.up_to_date, "{plan:?}");
        assert_eq!(plan.found, None, "an old Python cua-cli is not the cua CLI");
        // install.sh's own copy (the cua CLI, not the app's): found, so the
        // app says it is installed even though it would still offer its own.
        let elsewhere = tmp.path().join("brew/bin");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("cua"), "#!/bin/sh\necho 'cua 0.4.4'\n").unwrap();
        std::fs::set_permissions(
            elsewhere.join("cua"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let searching = CliInstaller {
            search_dirs: vec![elsewhere.clone()],
            ..cli.clone()
        };
        let plan = searching.plan().await;
        assert_eq!(
            plan.found,
            Some(elsewhere.join("cua").display().to_string()),
            "{plan:?}"
        );
        assert!(!plan.up_to_date);
        let done = cli.install(&CliInstallRequest::default()).await.unwrap();
        assert!(done.up_to_date, "{done:?}");
        assert_eq!(done.installed_version.as_deref(), Some("cua 1.2.3"));
        // A bundled CLI that does not run as cua is not reported installed
        // (and never replaces a working one).
        std::fs::write(&cua, "#!/bin/sh\nexit 1\n").unwrap();
        assert!(
            cli.install(&CliInstallRequest::default())
                .await
                .unwrap()
                .up_to_date
        );
        std::fs::remove_file(home.join(".local/bin/cua")).unwrap();
        assert!(cli.install(&CliInstallRequest::default()).await.is_err());
        // Without a bundled CLI there is nothing to install.
        let none = CliInstaller {
            bundled: None,
            ..cli
        };
        assert!(none.install(&CliInstallRequest::default()).await.is_err());
    }

    #[cfg(unix)]
    fn script(path: &Path, version: &str) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::remove_file(path);
        std::fs::write(path, format!("#!/bin/sh\necho \"{version}\"\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// UL2: an app bundling an older cua never replaces a newer one a user
    /// installed with install.sh; it upgrades an older one, and a dev build
    /// counts as older unless it is the same build.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_app_never_downgrades_the_installed_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("opt/Cua Spaces");
        std::fs::create_dir_all(&app).unwrap();
        let exe = app.join("cua-spaces");
        std::fs::write(&exe, "").unwrap();
        let bundled = app.join(cli_file_name());
        script(&bundled, "cua 0.6.0");
        let home = tmp.path().join("home");
        let cli = CliInstaller {
            channel: CuaChannel::Stable,
            bundled: bundled_cli(&exe),
            bin_dir: default_bin_dir(&home, None),
            path_env: "/usr/bin:/bin".into(),
            profile: profile_for_shell(&home, Some("/bin/zsh"), None),
            prefer_symlink: true,
            search_dirs: vec![],
        };
        let target = cli.target();
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let installed = |v: &str| script(&target, v);
        let content = || std::fs::read_to_string(&target).unwrap();

        // Newer CLI on disk (0.7.99), app bundles 0.6.0: left alone.
        script(&bundled, "cua 0.6.0");
        installed("cua 0.7.99");
        let before = content();
        let plan = cli.plan().await;
        assert!(plan.up_to_date, "{plan:?}");
        let done = cli.install(&CliInstallRequest::default()).await.unwrap();
        assert!(done.up_to_date);
        assert_eq!(content(), before);
        assert!(!target.symlink_metadata().unwrap().file_type().is_symlink());

        // The same version: left alone.
        script(&bundled, "cua 0.7.99");
        installed("cua 0.7.99 (other build)");
        let before = content();
        cli.install(&CliInstallRequest::default()).await.unwrap();
        assert_eq!(content(), before);

        // Older CLI on disk: the newer app upgrades it.
        script(&bundled, "cua 0.7.0");
        installed("cua 0.6.0");
        assert!(!cli.plan().await.up_to_date);
        let done = cli.install(&CliInstallRequest::default()).await.unwrap();
        assert!(done.up_to_date);
        assert!(target.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(done.installed_version.as_deref(), Some("cua 0.7.0"));

        // A dev build on disk is older than a release; a release on disk is
        // not replaced by a dev build.
        installed("cua 0.7.0-dev");
        assert!(!cli.plan().await.up_to_date);
        script(&bundled, "cua 0.7.0-dev");
        installed("cua 0.7.0");
        assert!(cli.plan().await.up_to_date);
        let before = content();
        cli.install(&CliInstallRequest::default()).await.unwrap();
        assert_eq!(content(), before);
        // The same dev build counts as the same.
        installed("cua 0.7.0-dev");
        assert!(cli.plan().await.up_to_date);

        // None there: installed.
        std::fs::remove_file(&target).unwrap();
        assert!(!cli.plan().await.up_to_date);
        assert!(
            cli.install(&CliInstallRequest::default())
                .await
                .unwrap()
                .up_to_date
        );
    }

    #[test]
    fn the_installed_name_follows_the_channel() {
        let ext = if cfg!(windows) { ".exe" } else { "" };
        assert_eq!(
            installed_cli_file_name(CuaChannel::Stable),
            format!("cua{ext}")
        );
        assert_eq!(
            installed_cli_file_name(CuaChannel::Local),
            format!("cua-local{ext}")
        );
        // Stable installs the sidecar's own name.
        assert_eq!(installed_cli_file_name(CuaChannel::Stable), cli_file_name());
        assert_ne!(
            installed_cli_file_name(CuaChannel::Stable),
            installed_cli_file_name(CuaChannel::Local)
        );
    }

    /// The Local app (a `Cua Spaces Local.app` bundling `cua`) beside a
    /// Stable `cua` in the same `~/.local/bin`.
    #[cfg(unix)]
    struct LocalRig {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        bin: PathBuf,
        local: CliInstaller,
        stable_cua: PathBuf,
        stable_before: Vec<u8>,
    }

    #[cfg(unix)]
    fn local_rig() -> LocalRig {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app = home.join("Applications/Cua Spaces Local.app/Contents/MacOS");
        std::fs::create_dir_all(&app).unwrap();
        let exe = app.join("CuaSpacesMac");
        std::fs::write(&exe, "").unwrap();
        // Every bundle ships its CLI as `cua`.
        script(&app.join(cli_file_name()), "cua 1.2.3");
        let bin = default_bin_dir(&home, None);
        std::fs::create_dir_all(&bin).unwrap();
        let stable_cua = bin.join("cua");
        script(&stable_cua, "cua 0.4.4");
        let stable_before = std::fs::read(&stable_cua).unwrap();
        let local = CliInstaller {
            channel: CuaChannel::Local,
            bundled: bundled_cli(&exe),
            bin_dir: bin.clone(),
            path_env: "/usr/bin:/bin".into(),
            profile: profile_for_shell(&home, Some("/bin/zsh"), None),
            prefer_symlink: false,
            search_dirs: vec![bin.clone()],
        };
        LocalRig {
            _tmp: tmp,
            home,
            bin,
            local,
            stable_cua,
            stable_before,
        }
    }

    /// CUA-1776: Local installs `cua-local` and never touches `cua`.
    #[cfg(unix)]
    #[tokio::test]
    async fn local_installs_cua_local_and_leaves_cua_alone() {
        let r = local_rig();
        let plan = r.local.plan().await;
        assert_eq!(plan.target, r.bin.join("cua-local").display().to_string());
        assert_eq!(plan.bundled_version.as_deref(), Some("cua 1.2.3"));
        assert!(!plan.installed && !plan.up_to_date, "{plan:?}");
        // Stable's `cua` is not Local's CLI, even in the folder searched.
        assert_eq!(plan.found, None, "{plan:?}");

        let done = r
            .local
            .install(&CliInstallRequest { modify_path: false })
            .await
            .unwrap();
        assert_eq!(done.target, r.bin.join("cua-local").display().to_string());
        assert!(done.installed && done.up_to_date, "{done:?}");
        assert_eq!(done.installed_version.as_deref(), Some("cua 1.2.3"));
        assert_eq!(
            done.found.as_deref(),
            Some(r.bin.join("cua-local").to_str().unwrap())
        );
        assert!(r.bin.join("cua-local").is_file());
        assert_eq!(
            std::fs::read(&r.stable_cua).unwrap(),
            r.stable_before,
            "Stable's cua is untouched"
        );
        // Nothing but the CLI was written: no profile, no staging file.
        assert!(!r.home.join(".zshrc").exists());
        let names: Vec<_> = std::fs::read_dir(&r.bin)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");

        // Stable's own installer still sees only its `cua` as its CLI.
        let stable = CliInstaller {
            channel: CuaChannel::Stable,
            ..r.local.clone()
        };
        let plan = stable.plan().await;
        assert_eq!(plan.target, r.stable_cua.display().to_string());
        assert_eq!(plan.installed_version.as_deref(), Some("cua 0.4.4"));
    }

    /// A Local app's CLI symlinks into its own app, like Stable's.
    #[cfg(unix)]
    #[tokio::test]
    async fn local_links_its_cli_into_its_app() {
        let r = local_rig();
        let local = CliInstaller {
            prefer_symlink: true,
            ..r.local.clone()
        };
        let done = local.install(&CliInstallRequest::default()).await.unwrap();
        let target = r.bin.join("cua-local");
        assert!(target.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(done.method, Some(InstallMethod::Symlink));
        assert!(
            std::fs::read_link(&target)
                .unwrap()
                .starts_with(r.home.join("Applications/Cua Spaces Local.app")),
        );
        assert_eq!(std::fs::read(&r.stable_cua).unwrap(), r.stable_before);
    }

    /// Local adds `~/.local/bin` to PATH the way Stable does: once, and not
    /// at all when it is on PATH already or its line is in the profile.
    #[cfg(unix)]
    #[tokio::test]
    async fn local_edits_the_shell_profile_once_and_only_when_needed() {
        let r = local_rig();
        let want = CliInstallRequest { modify_path: true };
        let line = path_line(&r.bin, &r.local.profile);
        let count = |profile: &Path, text: &str| {
            std::fs::read_to_string(profile)
                .unwrap()
                .matches(text)
                .count()
        };

        // Not on PATH: the profile gets the marker and the line.
        let plan = r.local.plan().await;
        assert!(!plan.on_path);
        assert_eq!(plan.path_line.as_deref(), Some(line.as_str()));
        let done = r.local.install(&want).await.unwrap();
        assert!(done.on_path && done.path_line.is_none(), "{done:?}");
        assert_eq!(count(&r.local.profile, PATH_MARKER), 1);
        assert_eq!(count(&r.local.profile, &line), 1);

        // Installing again (the PATH of this process is unchanged) adds no
        // second line.
        std::fs::remove_file(r.bin.join("cua-local")).unwrap();
        r.local.install(&want).await.unwrap();
        assert_eq!(count(&r.local.profile, PATH_MARKER), 1);
        assert_eq!(count(&r.local.profile, &line), 1);

        // Already on PATH: the profile is not touched, not even created.
        std::fs::remove_file(r.bin.join("cua-local")).unwrap();
        let on_path = CliInstaller {
            path_env: format!("/usr/bin:{}", r.bin.display()),
            profile: r.home.join(".profile-never-written"),
            ..r.local.clone()
        };
        let plan = on_path.plan().await;
        assert!(plan.on_path && plan.path_profile.is_none() && plan.path_line.is_none());
        on_path.install(&want).await.unwrap();
        assert!(!on_path.profile.exists());

        // Stable's app (or install.sh) already put the line in the profile:
        // Local leaves the file as it is.
        std::fs::remove_file(r.bin.join("cua-local")).unwrap();
        let shared = r.home.join(".zprofile");
        let body = format!("{PATH_MARKER}\n{line}\n");
        std::fs::write(&shared, &body).unwrap();
        let reuses = CliInstaller {
            profile: shared.clone(),
            ..r.local.clone()
        };
        reuses.install(&want).await.unwrap();
        assert_eq!(std::fs::read_to_string(&shared).unwrap(), body);
    }

    /// A `cua` ahead on PATH shadows Stable's CLI, not Local's; another
    /// `cua-local` ahead shadows Local's.
    #[cfg(unix)]
    #[tokio::test]
    async fn only_a_cli_of_the_same_name_shadows_the_target() {
        let r = local_rig();
        let other = r._tmp.path().join("brew/bin");
        std::fs::create_dir_all(&other).unwrap();
        script(&other.join("cua"), "cua 0.3.0");
        let path_env = format!("{}:{}", other.display(), r.bin.display());
        let local = CliInstaller {
            path_env: path_env.clone(),
            ..r.local.clone()
        };
        let stable = CliInstaller {
            channel: CuaChannel::Stable,
            path_env,
            ..r.local.clone()
        };
        assert_eq!(local.plan().await.shadowed_by, None);
        assert_eq!(
            stable.plan().await.shadowed_by.as_deref(),
            Some(other.join("cua").to_str().unwrap())
        );
        script(&other.join("cua-local"), "cua 0.3.0");
        assert_eq!(
            local.plan().await.shadowed_by.as_deref(),
            Some(other.join("cua-local").to_str().unwrap())
        );
    }

    #[test]
    fn keeps_installed_decides_by_version() {
        assert!(keeps_installed(Some("cua 0.7.99"), Some("cua 0.6.0")));
        assert!(keeps_installed(Some("cua 0.7.0"), Some("cua 0.7.0")));
        assert!(!keeps_installed(Some("cua 0.6.0"), Some("cua 0.7.0")));
        assert!(!keeps_installed(None, Some("cua 0.7.0")));
        assert!(!keeps_installed(
            Some("cua, version 0.1.9"),
            Some("cua 0.7.0")
        ));
        assert!(keeps_installed(Some("cua 0.7.0"), None));
    }
}
