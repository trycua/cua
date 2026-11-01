//! Running `git` for the cache: always unauthenticated, never interactive,
//! never reading the user's own git configuration, credentials or `~/.netrc`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// What a git run produced.
#[derive(Debug)]
pub struct Run {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// A git run that did not produce a result.
#[derive(Debug)]
pub enum GitError {
    /// No `git` on this machine.
    Missing,
    /// It did not finish in time (and was stopped).
    TimedOut,
    /// It could not be started.
    Spawn(String),
}

/// The `git` binary: `CUA_REPO_CACHE_GIT`, else the first on `PATH`.
pub fn binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUA_REPO_CACHE_GIT").filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
}

/// A `git` command that cannot use anything of the user's: a home with
/// nothing in it (so no `~/.netrc`, no credential helper, no config), no
/// system config, no prompts, `https` only, and a redirect followed only
/// from the first request.
pub fn command(bin: &Path, home: &Path, allow: &str) -> Command {
    let mut c = Command::new(bin);
    c.env_clear();
    for k in [
        "PATH",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "GIT_SSL_CAINFO",
        "GIT_SSL_CAPATH",
        "TMPDIR",
    ] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    c.env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("LC_ALL", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .env("SSH_ASKPASS", "true")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ALLOW_PROTOCOL", allow.replace(',', ":"))
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "core.askPass=",
            "-c",
            "protocol.allow=never",
            "-c",
            "http.followRedirects=initial",
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=30",
            "-c",
            "gc.auto=0",
            "-c",
            "core.fsmonitor=false",
        ])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    for p in allow.split(',') {
        c.arg("-c").arg(format!("protocol.{p}.allow=always"));
    }
    c
}

/// Runs `cmd` to the end within `limit`.
pub async fn run(mut cmd: Command, limit: Duration) -> Result<Run, GitError> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::Missing
        } else {
            GitError::Spawn(e.to_string())
        }
    })?;
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(o)) => Ok(Run {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }),
        Ok(Err(e)) => Err(GitError::Spawn(e.to_string())),
        // `kill_on_drop` stops it.
        Err(_) => Err(GitError::TimedOut),
    }
}
