//! A read-only mirror of the job's repository, so `actions/checkout`
//! downloads only what is new.
//!
//! This is the CUA-2707 repo cache design applied to CI: a bare mirror
//! kept by the host, mounted read-only into the VM, and git told to use its
//! objects. The VM never talks to the cache's remote and never holds a
//! credential for it; the host fetches (public repositories with no
//! credential, private ones only with a per-repository credential the host
//! holds, which stays on the host).
//!
//! How the VM uses it: the runner's wrapper exports
//! `GIT_ALTERNATE_OBJECT_DIRECTORIES=<mirror>/repo.git/objects` for the one
//! job. Every git command of the job (the checkout action's included) then
//! finds the mirror's objects locally and, because git treats an
//! alternate's refs as commits it already has, the fetch from GitHub
//! sends only the difference. Nothing is rewritten and no URL is
//! redirected, so the checkout is always against GitHub's real tip: it
//! cannot be stale, and a missing or corrupt mirror only costs the
//! download the checkout would have done anyway (the wrapper checks the
//! mount first and leaves the variable unset when it is not there).
//!
//! The first job for a repository does not wait for the fill: the mirror
//! is filled in the background and serves the next job. A refresh
//! (`git fetch`) runs in the background at most once a minute per
//! repository, never on a job's path.
//!
//! When the public repo cache (`cua-repo-cache`, a remote helper that
//! also serves the clone from the mirror in about 0.25 s) lands here, it
//! replaces [`GitMirrors`] behind [`RepoMirrors`]; the VM side keeps
//! working.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use super::MirrorConfig;
use super::cachevol::{JobMeta, ScopeKey, Trust, scope_key, trust_of};

/// The name of the bare repository inside a mounted mirror folder.
pub const REPO_DIR: &str = "repo.git";

/// What to mirror, and for whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorRequest {
    /// `owner/name`.
    pub repo: String,
    /// The account the computer serves.
    pub account: String,
    /// Private repository: only mirrored with `credential_scope`.
    pub private: bool,
    /// Names the credential that may read a private repository (an
    /// installation id): part of the folder key, so another credential
    /// never shares the folder.
    pub credential_scope: Option<String>,
}

/// A mirror ready to mount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorRef {
    /// The folder to mount read-only (it holds [`REPO_DIR`]).
    pub folder: PathBuf,
}

/// Where mirrors come from.
#[async_trait]
pub trait RepoMirrors: Send + Sync {
    /// The mirror for `req` when one is ready now. Starts a fill or a
    /// refresh in the background when needed; never waits for the network.
    async fn ensure(
        &self,
        req: &MirrorRequest,
        credential: Option<&crate::Secret>,
    ) -> Option<MirrorRef>;
}

/// Whether and what to mirror for a job. `None`: nothing.
pub fn request_for(
    m: &JobMeta,
    cfg: &MirrorConfig,
    credential_scope: Option<&str>,
) -> Option<MirrorRequest> {
    if !cfg.enabled || !valid_repo(&m.repo) || m.account.is_empty() {
        return None;
    }
    let private = m.private.unwrap_or(true);
    if private {
        // A private repository only with its own credential, and never for
        // a job that runs a stranger's code.
        let scope = credential_scope?;
        if !cfg.private || matches!(trust_of(m), Trust::Fork | Trust::Untrusted | Trust::Unknown) {
            return None;
        }
        return Some(MirrorRequest {
            repo: m.repo.clone(),
            account: m.account.clone(),
            private: true,
            credential_scope: Some(scope.to_string()),
        });
    }
    Some(MirrorRequest {
        repo: m.repo.clone(),
        account: m.account.clone(),
        private: false,
        credential_scope: None,
    })
}

/// `owner/name` with the characters GitHub allows.
pub fn valid_repo(r: &str) -> bool {
    let Some((o, n)) = r.split_once('/') else {
        return false;
    };
    let ok = |s: &str, extra: &[char]| {
        !s.is_empty()
            && s.len() <= 100
            && s != "."
            && s != ".."
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
    };
    ok(o, &['-']) && ok(n, &['-', '_', '.'])
}

/// The folder key of a request: public mirrors are shared by every
/// account (they hold only public objects), private ones are per account
/// and credential scope.
pub fn folder_key(req: &MirrorRequest) -> String {
    if req.private {
        let key: ScopeKey = scope_key(
            &format!(
                "{}\0{}",
                req.account,
                req.credential_scope.as_deref().unwrap_or("")
            ),
            &req.repo,
        );
        format!("private-{}", key.as_str())
    } else {
        format!("public-{}", scope_key("public", &req.repo).as_str())
    }
}

/// Mirrors kept with the `git` command on the host.
pub struct GitMirrors {
    root: PathBuf,
    host: String,
    refresh_every: Duration,
    fill_timeout: Duration,
    state: Arc<std::sync::Mutex<MirrorState>>,
}

#[derive(Default)]
struct MirrorState {
    /// Folders being filled or refreshed.
    busy: std::collections::HashSet<String>,
    /// Last refresh start by folder.
    refreshed: HashMap<String, Instant>,
}

impl GitMirrors {
    /// Mirrors under `root` (`<ci home>/perf/mirrors`) of repositories on
    /// `host` (`github.com`).
    pub fn new(root: impl Into<PathBuf>, host: &str) -> Self {
        Self {
            root: root.into(),
            host: host.to_string(),
            refresh_every: Duration::from_secs(60),
            fill_timeout: Duration::from_secs(600),
            state: Arc::default(),
        }
    }

    fn folder(&self, req: &MirrorRequest) -> PathBuf {
        self.root.join(folder_key(req))
    }

    fn ready(folder: &Path) -> bool {
        folder.join(REPO_DIR).join("objects").is_dir() && folder.join(".verified").is_file()
    }
}

#[async_trait]
impl RepoMirrors for GitMirrors {
    async fn ensure(
        &self,
        req: &MirrorRequest,
        credential: Option<&crate::Secret>,
    ) -> Option<MirrorRef> {
        if !valid_repo(&req.repo) || (req.private && credential.is_none()) {
            return None;
        }
        let folder = self.folder(req);
        let key = folder_key(req);
        let ready = Self::ready(&folder);
        let due = {
            let mut st = self.state.lock().ok()?;
            let stale = st
                .refreshed
                .get(&key)
                .is_none_or(|t| t.elapsed() >= self.refresh_every);
            if st.busy.contains(&key) || (ready && !stale) {
                false
            } else {
                st.busy.insert(key.clone());
                st.refreshed.insert(key.clone(), Instant::now());
                true
            }
        };
        if due {
            let url = format!("https://{}/{}.git", self.host, req.repo);
            let token = credential.map(|c| c.expose().to_string());
            let (folder2, state, timeout) = (folder.clone(), self.state.clone(), self.fill_timeout);
            tokio::spawn(async move {
                let r = tokio::time::timeout(
                    timeout,
                    fill_or_refresh(&folder2, &url, token.as_deref()),
                )
                .await;
                if !matches!(r, Ok(Ok(()))) {
                    tracing::warn!(target: "cua_ci", "repo mirror {url}: {}", match r {
                        Ok(Err(e)) => e,
                        _ => "timed out".to_string(),
                    });
                }
                if let Ok(mut st) = state.lock() {
                    st.busy.remove(&key);
                }
            });
        }
        ready.then_some(MirrorRef { folder })
    }
}

/// Clones the mirror when missing, else fetches into it; verifies it
/// (`git fsck --connectivity-only`) before it is marked ready.
async fn fill_or_refresh(folder: &Path, url: &str, token: Option<&str>) -> Result<(), String> {
    tokio::fs::create_dir_all(folder)
        .await
        .map_err(|e| e.to_string())?;
    let repo = folder.join(REPO_DIR);
    let fresh = !repo.join("objects").is_dir();
    if fresh {
        let tmp = folder.join(".filling");
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        git(
            &["clone", "--mirror", "--quiet", url, &tmp.to_string_lossy()],
            None,
            token,
        )
        .await?;
        git(
            &["fsck", "--connectivity-only", "--no-dangling"],
            Some(&tmp),
            None,
        )
        .await?;
        tokio::fs::rename(&tmp, &repo)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::write(folder.join(".verified"), b"")
            .await
            .map_err(|e| e.to_string())?;
    } else {
        git(&["remote", "update", "--prune"], Some(&repo), token).await?;
    }
    Ok(())
}

async fn git(args: &[&str], cwd: Option<&Path>, token: Option<&str>) -> Result<(), String> {
    let mut c = tokio::process::Command::new("git");
    c.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "true")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    if let Some(t) = token {
        // The credential travels in the environment of this one process,
        // never on a command line and never into the mirror's config.
        use base64::Engine as _;
        let b = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{t}"));
        c.env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.extraheader")
            .env("GIT_CONFIG_VALUE_0", format!("AUTHORIZATION: basic {b}"));
    }
    let out = c.output().await.map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        // Git's message can echo a URL, never the header.
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests;
