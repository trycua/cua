// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The first-run "Command line" step: put the app's bundled `cua` CLI on
//! PATH. Both Spaces shells run this (the Tauri app through its
//! `installer_*` commands, the SwiftUI app through the cua SDK's
//! `AppCliInstaller`), so the plan they show and the files they write are
//! the same.
//!
//! [`CliInstaller::plan`] shows the exact target path before anything is
//! written; [`CliInstaller::install`] writes only after the user consents.
//! Paths and environment are injected, so tests run against temp dirs and
//! fake binaries only.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Marker line written above a PATH edit (same text as install.sh).
pub const PATH_MARKER: &str = "# Added by the cua installer";

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// File name of the CLI on this OS.
pub fn cli_file_name() -> &'static str {
    if cfg!(windows) { "cua.exe" } else { "cua" }
}

/// The sidecar `cua` next to the app executable, when the bundle has one.
pub fn bundled_cli(exe: &Path) -> Option<PathBuf> {
    let candidate = exe.parent()?.join(cli_file_name());
    candidate.is_file().then_some(candidate)
}

/// Where the GUI installs the CLI: `~/.local/bin` on macOS/Linux (the same
/// place install.sh uses), `%LOCALAPPDATA%\Programs\cua\bin` on Windows.
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
    /// Another `cua` found earlier on PATH (it would shadow `target`).
    pub shadowed_by: Option<String>,
    /// `bin_dir` is already on PATH.
    pub on_path: bool,
    /// Shell profile (or "user PATH" on Windows) the PATH change would edit.
    pub path_profile: Option<String>,
    /// The line that would be added to `path_profile`.
    pub path_line: Option<String>,
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
pub struct CliInstaller {
    pub bundled: Option<PathBuf>,
    pub bin_dir: PathBuf,
    /// Value of PATH used for the on-path / shadow checks.
    pub path_env: String,
    /// Shell profile edited by `modify_path` (non-Windows).
    pub profile: PathBuf,
    /// Symlink rather than copy (stable app location on macOS).
    pub prefer_symlink: bool,
}

impl CliInstaller {
    /// The real machine: sidecar next to `exe`, `$HOME`, `$PATH`, `$SHELL`.
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
            prefer_symlink: bundled.as_deref().is_some_and(stable_app_location),
            bundled,
            bin_dir: default_bin_dir(&home, local.as_deref()),
            path_env: std::env::var("PATH").unwrap_or_default(),
            profile: profile_for_shell(&home, shell.as_deref(), xdg.as_deref()),
        }
    }

    /// The file the CLI is written to.
    pub fn target(&self) -> PathBuf {
        self.bin_dir.join(cli_file_name())
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
        let up_to_date = match (&self.bundled, installed) {
            (Some(src), true) => same_file(src, &target),
            _ => false,
        };
        let on_path = path_contains(&self.path_env, &self.bin_dir);
        let shadowed_by = first_on_path(&self.path_env, cli_file_name())
            .filter(|found| {
                found != &target && !self.bundled.as_ref().is_some_and(|b| same_file(b, found))
            })
            .filter(|_| on_path_before(&self.path_env, &self.bin_dir))
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
        CliInstallPlan {
            source: self.bundled.as_ref().map(|p| p.display().to_string()),
            target: target.display().to_string(),
            bin_dir: self.bin_dir.display().to_string(),
            method: self.bundled.as_ref().map(|_| self.method()),
            installed,
            up_to_date,
            installed_version: if installed {
                cli_version(&target).await
            } else {
                None
            },
            bundled_version: match &self.bundled {
                Some(src) => cli_version(src).await,
                None => None,
            },
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
        std::fs::create_dir_all(&self.bin_dir)
            .map_err(|e| format!("cannot create {}: {e}", self.bin_dir.display()))?;
        let staging = self.bin_dir.join(format!(".{}.cua-new", cli_file_name()));
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

#[cfg(windows)]
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

#[cfg(not(windows))]
fn add_to_windows_user_path(_dir: &Path) -> Result<(), String> {
    Err("not Windows".into())
}

/// App locations that survive relaunches (not a mounted dmg or App
/// Translocation), where a symlink into the bundle stays valid.
pub fn stable_app_location(cli: &Path) -> bool {
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

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) if x == y => true,
        _ => match (std::fs::read(a), std::fs::read(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        },
    }
}

/// The first `name` on a PATH-style string.
pub fn first_on_path(path_env: &str, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path_env)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Whether some *other* PATH entry with a `cua` comes before `bin_dir`
/// (or `bin_dir` is not on PATH at all).
fn on_path_before(path_env: &str, bin_dir: &Path) -> bool {
    for dir in std::env::split_paths(path_env) {
        if dir == bin_dir {
            return false;
        }
        if dir.join(cli_file_name()).is_file() {
            return true;
        }
    }
    true
}

/// `ETXTBSY` ("text file busy") on Linux and macOS.
const ETXTBSY: i32 = 26;

/// `cua --version`, first line, or `None` if it does not run.
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

#[cfg(test)]
mod tests {
    use super::*;

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
            bundled: bundled_cli(&exe),
            bin_dir: default_bin_dir(&home, None),
            path_env: "/usr/bin:/bin".into(),
            profile: profile_for_shell(&home, Some("/bin/zsh"), None),
            prefer_symlink: false,
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
        // Without a bundled CLI there is nothing to install.
        let none = CliInstaller {
            bundled: None,
            ..cli
        };
        assert!(none.install(&CliInstallRequest::default()).await.is_err());
    }
}
