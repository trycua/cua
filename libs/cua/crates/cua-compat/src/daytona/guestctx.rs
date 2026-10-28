//! What a toolbox route needs to run something in the sandbox: its record,
//! its cua-spacesd, the user it acts as and the user cua-spacesd runs as.
#![allow(dead_code)]

use super::Daytona;
use super::toolbox::{self, run_collect, shell, shell_of, status_err, workdir};
use crate::state::Record;
use axum::body::Body;
use cua_spacesd_client::SpacesdClient;
use http::{HeaderMap, Response, StatusCode};
use std::collections::HashMap;
use std::time::Duration;

/// A live, authorized sandbox.
pub struct Guest {
    pub r: Record,
    pub c: SpacesdClient,
    /// The sandbox's user (Daytona's `daytona` by default).
    pub user: String,
    /// The user cua-spacesd runs as (see `guest.rs`).
    pub driver: String,
    /// The sandbox user's home (Daytona's working directory).
    pub home: String,
}

/// Output of [`Guest::run`].
pub struct Ran {
    pub code: i32,
    /// stdout and stderr in arrival order.
    pub all: String,
    pub stdout: String,
    pub stderr: String,
}

impl Guest {
    /// The caller's started sandbox `id` (activity resets its idle timer).
    pub async fn open(d: &Daytona, headers: &HeaderMap, id: &str) -> Result<Guest, Response<Body>> {
        let (r, c, user) = toolbox::sandbox(d, headers, id).await?;
        let driver = d.drivers.of(&r.id, &c).await;
        let home = workdir(d, &r, &user).await;
        Ok(Guest {
            r,
            c,
            user,
            driver,
            home,
        })
    }

    /// Resolves `p` against the sandbox user's home.
    pub fn path(&self, p: &str) -> String {
        crate::e2b::resolve_path(p, &self.home)
    }

    /// Runs `argv` as the sandbox user in `cwd` (default: home) to
    /// completion. `timeout`: killed at the limit (exit code -1).
    pub async fn run(
        &self,
        d: &Daytona,
        argv: Vec<String>,
        env: HashMap<String, String>,
        cwd: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<Ran, Response<Body>> {
        let _ = d;
        let (code, all, stdout, stderr) = run_collect(
            &self.c,
            argv,
            env,
            cwd.map(str::to_string).unwrap_or_else(|| self.home.clone()),
            &self.user,
            &self.driver,
            timeout,
        )
        .await
        .map_err(status_err)?;
        Ok(Ran {
            code,
            all,
            stdout,
            stderr,
        })
    }

    /// Runs a shell script (`zsh`, `bash` or `sh`: Daytona's pick).
    pub async fn sh(
        &self,
        d: &Daytona,
        script: &str,
        env: HashMap<String, String>,
        cwd: Option<&str>,
        timeout: Option<Duration>,
    ) -> Result<Ran, Response<Body>> {
        let sh = shell_of(d, &self.r, &self.c).await;
        self.run(d, shell(&sh, script), env, cwd, timeout).await
    }

    /// The sandbox's own environment (create-time `env`).
    pub fn env(&self) -> HashMap<String, String> {
        self.r.env_vars.clone().into_iter().collect()
    }
}

/// A 400 with the daemon code, for bad requests.
pub fn bad(code: &str, msg: impl Into<String>) -> Response<Body> {
    super::daemon_error(StatusCode::BAD_REQUEST, code, msg)
}
