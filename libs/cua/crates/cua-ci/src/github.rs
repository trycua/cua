//! The GitHub REST calls a pool makes.
//!
//! Auth is a GitHub App (its RS256 private key signs a short JWT, which buys
//! an installation token, cached until five minutes before it expires) or a
//! plain token. The App needs two permissions: organization "Self-hosted
//! runners: read and write" (repository "Administration: read and write"
//! for a repository pool) and repository "Actions: read".
//!
//! Calls: `POST {scope}/actions/runners/generate-jitconfig` (a just-in-time
//! runner configuration: the runner registers itself with it, takes one job
//! and is removed; no registration token ever reaches the Space),
//! `GET {scope}/actions/runners`, `DELETE {scope}/actions/runners/{id}`,
//! `GET /orgs/{org}/actions/runner-groups`, and the latest `actions/runner`
//! release (the tarballs and their SHA-256 digests).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Scope;
use crate::secret::Secret;
use crate::{Error, Result};

/// GitHub's REST API version header value.
const API_VERSION: &str = "2022-11-28";

/// An installation token is renewed this long before it expires.
const TOKEN_MARGIN_SECS: u64 = 300;

/// A GitHub App's RS256 private key.
pub struct AppKey {
    pair: ring::signature::RsaKeyPair,
}

impl std::fmt::Debug for AppKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AppKey([redacted])")
    }
}

impl AppKey {
    /// From the PEM file GitHub hands out (`BEGIN RSA PRIVATE KEY`, PKCS#1)
    /// or a PKCS#8 one (`BEGIN PRIVATE KEY`).
    pub fn from_pem(pem: &str) -> Result<Self> {
        let (label, der) = pem_body(pem).ok_or_else(|| {
            Error::Config(
                "the GitHub App private key is not a PEM file (use the .pem file GitHub \
                 generated for the App)"
                    .into(),
            )
        })?;
        let pair = match label.as_str() {
            "RSA PRIVATE KEY" => ring::signature::RsaKeyPair::from_der(&der),
            "PRIVATE KEY" => ring::signature::RsaKeyPair::from_pkcs8(&der),
            other => {
                return Err(Error::Config(format!(
                    "the GitHub App private key is a {other:?} PEM; GitHub App keys are RSA \
                     PRIVATE KEY"
                )));
            }
        }
        .map_err(|e| Error::Config(format!("the GitHub App private key is not usable: {e}")))?;
        Ok(Self { pair })
    }

    /// The public key (DER `RSAPublicKey`), to verify a JWT.
    pub fn public_key_der(&self) -> Vec<u8> {
        use ring::signature::KeyPair as _;
        self.pair.public_key().as_ref().to_vec()
    }

    /// A JWT for App `app_id`, valid from a minute before `now` for nine
    /// minutes (GitHub allows at most ten).
    pub fn jwt(&self, app_id: u64, now_secs: u64) -> Result<Secret> {
        let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
        let header = b64(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = b64(json!({
            "iat": now_secs.saturating_sub(60),
            "exp": now_secs + 540,
            "iss": app_id,
        })
        .to_string()
        .as_bytes());
        let signing_input = format!("{header}.{claims}");
        let mut sig = vec![0u8; self.pair.public().modulus_len()];
        self.pair
            .sign(
                &ring::signature::RSA_PKCS1_SHA256,
                &ring::rand::SystemRandom::new(),
                signing_input.as_bytes(),
                &mut sig,
            )
            .map_err(|_| Error::GitHub("signing the App JWT failed".into()))?;
        Ok(Secret::new(format!("{signing_input}.{}", b64(&sig))))
    }
}

fn pem_body(pem: &str) -> Option<(String, Vec<u8>)> {
    let begin = pem.find("-----BEGIN ")?;
    let rest = &pem[begin + 11..];
    let label_end = rest.find("-----")?;
    let label = rest[..label_end].to_string();
    let body_start = label_end + 5;
    let end_marker = format!("-----END {label}-----");
    let end = rest.find(&end_marker)?;
    let body: String = rest[body_start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    Some((label, der))
}

/// How a pool authenticates.
#[derive(Clone)]
pub enum Credentials {
    /// A GitHub App installation.
    App {
        /// The App's id.
        app_id: u64,
        /// The installation; looked up from the scope when `None`.
        installation_id: Option<u64>,
        /// The private key.
        key: Arc<AppKey>,
    },
    /// A token with the same rights.
    Token(Secret),
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credentials::App {
                app_id,
                installation_id,
                ..
            } => write!(
                f,
                "App(app_id={app_id}, installation_id={installation_id:?})"
            ),
            Credentials::Token(_) => f.write_str("Token([redacted])"),
        }
    }
}

/// A registered self-hosted runner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Runner {
    /// Runner id.
    pub id: u64,
    /// Runner name.
    pub name: String,
    /// `online` or `offline`.
    pub status: String,
    /// Running a job now.
    pub busy: bool,
    /// Label names.
    pub labels: Vec<String>,
}

/// A runner group of an organization.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerGroup {
    /// Group id (1 is `Default`).
    pub id: u64,
    /// Name.
    pub name: String,
    /// `all`, `selected` or `private`.
    pub visibility: String,
    /// Public repositories may use it.
    pub allows_public_repositories: bool,
    /// The organization's default group.
    pub default: bool,
}

/// A just-in-time runner configuration request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JitRequest {
    /// Runner name (unique in the scope).
    pub name: String,
    /// Runner group id (1 for a repository).
    pub runner_group_id: u64,
    /// Every label, `self-hosted` included.
    pub labels: Vec<String>,
    /// The work folder, relative to the runner directory.
    pub work_folder: String,
}

/// A just-in-time runner configuration.
#[derive(Clone, Debug)]
pub struct JitConfig {
    /// The runner it registers.
    pub runner_id: u64,
    /// Its name.
    pub runner_name: String,
    /// `encoded_jit_config`: start the runner with it. Secret: whoever holds
    /// it can take the runner's job.
    pub encoded: Secret,
}

/// What a runner delete did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// Removed.
    Deleted,
    /// It was already gone.
    NotFound,
    /// GitHub refused: the runner is running a job.
    Busy,
}

/// The runner calls the reconciler makes (the seam its tests fake).
#[async_trait]
pub trait RunnerApi: Send + Sync {
    /// The runner group id: `name` in an organization (default: its
    /// default group), always 1 for a repository.
    async fn runner_group_id(&self, name: Option<&str>) -> Result<u64>;
    /// A JIT configuration for a new runner.
    async fn generate_jit(&self, req: &JitRequest) -> Result<JitConfig>;
    /// Every self-hosted runner of the scope.
    async fn list_runners(&self) -> Result<Vec<Runner>>;
    /// Removes a runner.
    async fn delete_runner(&self, id: u64) -> Result<DeleteOutcome>;
}

/// One `actions/runner` tarball of a release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    /// File name (`actions-runner-osx-arm64-2.330.0.tar.gz`).
    pub name: String,
    /// Download URL.
    pub url: String,
    /// The platform (`osx-arm64`, `linux-x64`, `linux-arm64`).
    pub platform: String,
    /// SHA-256 (hex), from the asset's digest or the release notes.
    pub sha256: Option<String>,
}

/// The latest `actions/runner` release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerRelease {
    /// Version without the `v` (`2.330.0`).
    pub version: String,
    /// The runner tarballs.
    pub assets: Vec<ReleaseAsset>,
}

impl RunnerRelease {
    /// The tarball for `platform`.
    pub fn asset(&self, platform: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|a| a.platform == platform)
    }
}

/// A release from `GET /repos/actions/runner/releases/latest`: each
/// `actions-runner-<platform>-<version>.tar.gz` asset with its SHA-256,
/// from the asset's `digest` (`sha256:<hex>`) or, for older releases, the
/// `<!-- BEGIN SHA <platform> -->` markers of the release notes.
pub fn parse_release(v: &Value) -> Result<RunnerRelease> {
    let tag = v["tag_name"]
        .as_str()
        .ok_or_else(|| Error::GitHub("the actions/runner release has no tag".into()))?;
    let version = tag.trim_start_matches('v').to_string();
    let body = v["body"].as_str().unwrap_or_default();
    let mut assets = Vec::new();
    for a in v["assets"].as_array().into_iter().flatten() {
        let name = a["name"].as_str().unwrap_or_default();
        let Some(platform) = name
            .strip_prefix("actions-runner-")
            .and_then(|r| r.strip_suffix(&format!("-{version}.tar.gz")))
        else {
            continue;
        };
        let from_digest = a["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_ascii_lowercase);
        let from_notes = || {
            let begin = format!("<!-- BEGIN SHA {platform} -->");
            let end = format!("<!-- END SHA {platform} -->");
            let s = body.find(&begin)? + begin.len();
            let e = body[s..].find(&end)? + s;
            Some(body[s..e].trim().to_ascii_lowercase())
        };
        let sha256 = from_digest
            .or_else(from_notes)
            .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
        assets.push(ReleaseAsset {
            name: name.to_string(),
            url: a["browser_download_url"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            platform: platform.to_string(),
            sha256,
        });
    }
    Ok(RunnerRelease { version, assets })
}

/// The GitHub REST client of one pool.
pub struct GitHubClient {
    http: reqwest::Client,
    api: String,
    scope: Scope,
    creds: Credentials,
    installation: std::sync::Mutex<Option<u64>>,
    token: tokio::sync::Mutex<Option<(Secret, u64)>>,
    groups: std::sync::Mutex<HashMap<String, u64>>,
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("api", &self.api)
            .field("scope", &self.scope)
            .field("creds", &self.creds)
            .finish()
    }
}

/// How a call authenticates.
#[derive(Clone, Copy)]
enum Auth {
    /// The App JWT (installation lookup and token).
    Jwt,
    /// The installation token or the plain token.
    Token,
    /// Nothing (public data).
    None,
}

impl GitHubClient {
    /// A client for `scope` at `api_url` (`https://api.github.com`).
    pub fn new(api_url: &str, scope: Scope, creds: Credentials) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("cua-ci/", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| Error::GitHub(format!("HTTP client: {e}")))?;
        let installation = match &creds {
            Credentials::App {
                installation_id, ..
            } => *installation_id,
            Credentials::Token(_) => None,
        };
        Ok(Self {
            http,
            api: api_url.trim_end_matches('/').to_string(),
            scope,
            creds,
            installation: std::sync::Mutex::new(installation),
            token: tokio::sync::Mutex::new(None),
            groups: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// The scope.
    pub fn scope(&self) -> &Scope {
        &self.scope
    }

    fn now_secs() -> u64 {
        crate::unix_ms() / 1000
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        auth: Auth,
    ) -> Result<(u16, Value)> {
        let url = format!("{}/{}", self.api, path.trim_start_matches('/'));
        let mut retried = false;
        loop {
            let mut req = self
                .http
                .request(method.clone(), &url)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", API_VERSION);
            match auth {
                Auth::Jwt => {
                    let jwt = self.jwt()?;
                    req = req.bearer_auth(jwt.expose());
                }
                Auth::Token => {
                    // Boxed: buying a token may look up the installation,
                    // which is itself a call.
                    let t = Box::pin(self.access_token()).await?;
                    req = req.bearer_auth(t.expose());
                }
                Auth::None => {}
            }
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| Error::GitHub(format!("{method} {path}: {}", e.without_url())))?;
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            // An installation token GitHub no longer accepts (revoked,
            // clock skew): buy a fresh one once.
            if status == 401
                && matches!(auth, Auth::Token)
                && matches!(self.creds, Credentials::App { .. })
                && !retried
            {
                *self.token.lock().await = None;
                retried = true;
                continue;
            }
            return Ok((status, v));
        }
    }

    fn failure(method: &str, path: &str, status: u16, v: &Value) -> Error {
        let msg = v["message"].as_str().unwrap_or("no message");
        let hint = match status {
            401 => " (the credential is not accepted: check the App id and key, or the token)",
            403 | 404 => {
                " (check that the GitHub App is installed on this organization or repository \
                 with Self-hosted runners: read and write)"
            }
            _ => "",
        };
        Error::GitHub(format!("{method} {path}: {status} {msg}{hint}"))
    }

    fn jwt(&self) -> Result<Secret> {
        match &self.creds {
            Credentials::App { app_id, key, .. } => key.jwt(*app_id, Self::now_secs()),
            Credentials::Token(_) => Err(Error::GitHub("a token pool has no App JWT".into())),
        }
    }

    /// The App installation on the scope (looked up once).
    pub async fn installation_id(&self) -> Result<u64> {
        if let Some(id) = *self.installation.lock().expect("installation") {
            return Ok(id);
        }
        let path = format!("{}/installation", self.scope.api_prefix());
        let (status, v) = self
            .call(reqwest::Method::GET, &path, None, Auth::Jwt)
            .await?;
        if status != 200 {
            return Err(Self::failure("GET", &path, status, &v));
        }
        let id = v["id"]
            .as_u64()
            .ok_or_else(|| Error::GitHub(format!("GET {path}: no installation id")))?;
        *self.installation.lock().expect("installation") = Some(id);
        Ok(id)
    }

    /// The token calls use: the plain token, or an installation token
    /// (renewed five minutes before it expires).
    pub async fn access_token(&self) -> Result<Secret> {
        let (app_id, key) = match &self.creds {
            Credentials::Token(t) => return Ok(t.clone()),
            Credentials::App { app_id, key, .. } => (*app_id, key.clone()),
        };
        let mut cached = self.token.lock().await;
        if let Some((t, exp)) = cached.as_ref()
            && *exp > Self::now_secs() + TOKEN_MARGIN_SECS
        {
            return Ok(t.clone());
        }
        let installation = self.installation_id().await?;
        let path = format!("app/installations/{installation}/access_tokens");
        let url = format!("{}/{path}", self.api);
        let jwt = key.jwt(app_id, Self::now_secs())?;
        let resp = self
            .http
            .post(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .bearer_auth(jwt.expose())
            .send()
            .await
            .map_err(|e| Error::GitHub(format!("POST {path}: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        if status != 201 {
            return Err(Self::failure("POST", &path, status, &v));
        }
        let token = v["token"]
            .as_str()
            .ok_or_else(|| Error::GitHub(format!("POST {path}: no token")))?;
        let exp = v["expires_at"]
            .as_str()
            .and_then(|s| humantime::parse_rfc3339_weak(s.trim_end_matches('Z')).ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(Self::now_secs() + 3000);
        let t = Secret::new(token);
        *cached = Some((t.clone(), exp));
        Ok(t)
    }

    /// The organization's runner groups (an organization pool only).
    pub async fn runner_groups(&self) -> Result<Vec<RunnerGroup>> {
        let Scope::Org(org) = &self.scope else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        for page in 1..=50 {
            let path = format!("orgs/{org}/actions/runner-groups?per_page=100&page={page}");
            let (status, v) = self
                .call(reqwest::Method::GET, &path, None, Auth::Token)
                .await?;
            if status != 200 {
                return Err(Self::failure("GET", &path, status, &v));
            }
            let rows = v["runner_groups"].as_array().cloned().unwrap_or_default();
            let n = rows.len();
            for g in rows {
                out.push(RunnerGroup {
                    id: g["id"].as_u64().unwrap_or_default(),
                    name: g["name"].as_str().unwrap_or_default().to_string(),
                    visibility: g["visibility"].as_str().unwrap_or_default().to_string(),
                    allows_public_repositories: g["allows_public_repositories"]
                        .as_bool()
                        .unwrap_or(false),
                    default: g["default"].as_bool().unwrap_or(false),
                });
            }
            if n < 100 {
                break;
            }
        }
        Ok(out)
    }

    /// The latest `actions/runner` release (public data: asked without a
    /// credential, then with the pool's when the anonymous rate limit is
    /// spent).
    pub async fn latest_runner_release(&self) -> Result<RunnerRelease> {
        let path = "repos/actions/runner/releases/latest";
        let (mut status, mut v) = self
            .call(reqwest::Method::GET, path, None, Auth::None)
            .await?;
        if status == 403 || status == 429 {
            (status, v) = self
                .call(reqwest::Method::GET, path, None, Auth::Token)
                .await?;
        }
        if status != 200 {
            return Err(Self::failure("GET", path, status, &v));
        }
        parse_release(&v)
    }

    /// Downloads `url` (a release asset; redirects followed, no credential
    /// sent) to `dest`, returning the SHA-256 (hex) of what was written.
    pub async fn download(&self, url: &str, dest: &std::path::Path) -> Result<String> {
        use sha2::Digest as _;
        use tokio::io::AsyncWriteExt as _;
        let mut resp = self
            .http
            .get(url)
            .timeout(std::time::Duration::from_secs(900))
            .send()
            .await
            .map_err(|e| Error::GitHub(format!("download {url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(Error::GitHub(format!(
                "download {url}: {}",
                resp.status().as_u16()
            )));
        }
        let mut file = tokio::fs::File::create(dest).await?;
        let mut hash = sha2::Sha256::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| Error::GitHub(format!("download {url}: {e}")))?
        {
            hash.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        file.sync_all().await?;
        Ok(hex::encode(hash.finalize()))
    }
}

#[async_trait]
impl RunnerApi for GitHubClient {
    async fn runner_group_id(&self, name: Option<&str>) -> Result<u64> {
        if !self.scope.is_org() {
            return Ok(1);
        }
        let key = name.unwrap_or("").to_ascii_lowercase();
        if let Some(id) = self.groups.lock().expect("groups").get(&key) {
            return Ok(*id);
        }
        let groups = self.runner_groups().await?;
        let found = match name {
            None => groups.iter().find(|g| g.default),
            Some(n) => groups.iter().find(|g| g.name.eq_ignore_ascii_case(n)),
        };
        let id = match (found, name) {
            (Some(g), _) => g.id,
            (None, None) => 1,
            (None, Some(n)) => {
                let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
                return Err(Error::GitHub(format!(
                    "no runner group named {n:?} in {} (it has: {}); create it under the \
                     organization's Settings, Actions, Runner groups",
                    self.scope,
                    names.join(", ")
                )));
            }
        };
        self.groups.lock().expect("groups").insert(key, id);
        Ok(id)
    }

    async fn generate_jit(&self, req: &JitRequest) -> Result<JitConfig> {
        let path = format!(
            "{}/actions/runners/generate-jitconfig",
            self.scope.api_prefix()
        );
        let body = json!({
            "name": req.name,
            "runner_group_id": req.runner_group_id,
            "labels": req.labels,
            "work_folder": req.work_folder,
        });
        let (status, v) = self
            .call(reqwest::Method::POST, &path, Some(body), Auth::Token)
            .await?;
        if status != 201 {
            return Err(Self::failure("POST", &path, status, &v));
        }
        let encoded = v["encoded_jit_config"]
            .as_str()
            .ok_or_else(|| Error::GitHub(format!("POST {path}: no encoded_jit_config")))?;
        Ok(JitConfig {
            runner_id: v["runner"]["id"].as_u64().unwrap_or_default(),
            runner_name: v["runner"]["name"]
                .as_str()
                .unwrap_or(&req.name)
                .to_string(),
            encoded: Secret::new(encoded),
        })
    }

    async fn list_runners(&self) -> Result<Vec<Runner>> {
        let mut out = Vec::new();
        for page in 1..=100 {
            let path = format!(
                "{}/actions/runners?per_page=100&page={page}",
                self.scope.api_prefix()
            );
            let (status, v) = self
                .call(reqwest::Method::GET, &path, None, Auth::Token)
                .await?;
            if status != 200 {
                return Err(Self::failure("GET", &path, status, &v));
            }
            let rows = v["runners"].as_array().cloned().unwrap_or_default();
            let n = rows.len();
            for r in rows {
                out.push(Runner {
                    id: r["id"].as_u64().unwrap_or_default(),
                    name: r["name"].as_str().unwrap_or_default().to_string(),
                    status: r["status"].as_str().unwrap_or_default().to_string(),
                    busy: r["busy"].as_bool().unwrap_or(false),
                    labels: r["labels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|l| l["name"].as_str().map(str::to_string))
                        .collect(),
                });
            }
            if n < 100 {
                break;
            }
        }
        Ok(out)
    }

    async fn delete_runner(&self, id: u64) -> Result<DeleteOutcome> {
        let path = format!("{}/actions/runners/{id}", self.scope.api_prefix());
        let (status, v) = self
            .call(reqwest::Method::DELETE, &path, None, Auth::Token)
            .await?;
        match status {
            204 => Ok(DeleteOutcome::Deleted),
            404 => Ok(DeleteOutcome::NotFound),
            422 => Ok(DeleteOutcome::Busy),
            _ => Err(Self::failure("DELETE", &path, status, &v)),
        }
    }
}

/// GitHub when the pool has no usable credential (`cua ci disconnect --pool`
/// still deletes the Spaces): every call fails with the reason.
pub struct Unavailable(pub String);

#[async_trait]
impl RunnerApi for Unavailable {
    async fn runner_group_id(&self, _: Option<&str>) -> Result<u64> {
        Err(Error::GitHub(self.0.clone()))
    }
    async fn generate_jit(&self, _: &JitRequest) -> Result<JitConfig> {
        Err(Error::GitHub(self.0.clone()))
    }
    async fn list_runners(&self) -> Result<Vec<Runner>> {
        Err(Error::GitHub(self.0.clone()))
    }
    async fn delete_runner(&self, _: u64) -> Result<DeleteOutcome> {
        Err(Error::GitHub(self.0.clone()))
    }
}

#[async_trait]
impl crate::cache::ReleaseSource for Unavailable {
    async fn latest(&self) -> Result<RunnerRelease> {
        Err(Error::GitHub(self.0.clone()))
    }
    async fn download(&self, _: &str, _: &std::path::Path) -> Result<String> {
        Err(Error::GitHub(self.0.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{delete, get, post};
    use std::sync::atomic::{AtomicU32, Ordering};

    // A throwaway RSA key generated for these tests (it belongs to no App),
    // kept as base64 DER: the PEM is put together here, so the repository
    // holds no PEM private key for secret scanners to flag.
    const PKCS1_DER: &str = include_str!("../tests/fixtures/test-only-github-app-key.pkcs1.b64");
    const PKCS8_DER: &str = include_str!("../tests/fixtures/test-only-github-app-key.pkcs8.b64");

    fn pem(kind: &str, der_b64: &str) -> String {
        let label = format!("{kind}PRIVATE KEY");
        let body: String = der_b64.split_whitespace().collect();
        let lines: Vec<&str> = body
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            lines.join("\n")
        )
    }

    /// The key as GitHub hands it out (PKCS#1).
    pub(crate) fn pkcs1() -> String {
        pem("RSA ", PKCS1_DER)
    }

    fn pkcs8() -> String {
        pem("", PKCS8_DER)
    }

    fn verify_jwt(jwt: &str, public: &[u8]) -> Value {
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[2])
            .unwrap();
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            public,
        )
        .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
        .expect("signature verifies");
        let header: Value = serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[0])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(header, json!({"alg": "RS256", "typ": "JWT"}));
        serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(parts[1])
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn app_jwt_is_rs256_and_short_lived() {
        for pem in [pkcs1(), pkcs8()] {
            let key = AppKey::from_pem(&pem).unwrap();
            let jwt = key.jwt(4242, 1_700_000_000).unwrap();
            let claims = verify_jwt(jwt.expose(), &key.public_key_der());
            assert_eq!(claims["iss"], 4242);
            assert_eq!(claims["iat"], 1_700_000_000 - 60);
            assert_eq!(claims["exp"], 1_700_000_000 + 540);
        }
        assert_eq!(
            format!("{:?}", AppKey::from_pem(&pkcs1()).unwrap()),
            "AppKey([redacted])"
        );
        let e = AppKey::from_pem("not a key").unwrap_err().to_string();
        assert!(e.contains("not a PEM"), "{e}");
        let e = AppKey::from_pem("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----")
            .unwrap_err()
            .to_string();
        assert!(e.contains("CERTIFICATE"), "{e}");
    }

    #[test]
    fn release_assets_carry_their_digest() {
        let v = json!({
            "tag_name": "v2.338.2",
            "body": "notes\n<!-- BEGIN SHA linux-x64 -->ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789<!-- END SHA linux-x64 -->",
            "assets": [
                {"name": "actions-runner-osx-arm64-2.338.2.tar.gz",
                 "browser_download_url": "https://example.test/osx",
                 "digest": "sha256:25a065af409ffa317ab4d5a3b614dd15fc9c40f2172bc1b35a1b1258e919a388"},
                {"name": "actions-runner-linux-x64-2.338.2.tar.gz",
                 "browser_download_url": "https://example.test/linux"},
                {"name": "actions-runner-linux-arm64-2.338.2.tar.gz",
                 "browser_download_url": "https://example.test/linux-arm"},
                {"name": "actions-runner-win-x64-2.338.2.zip", "browser_download_url": "x"}
            ]
        });
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version, "2.338.2");
        assert_eq!(r.assets.len(), 3);
        assert_eq!(
            r.asset("osx-arm64").unwrap().sha256.as_deref(),
            Some("25a065af409ffa317ab4d5a3b614dd15fc9c40f2172bc1b35a1b1258e919a388")
        );
        assert_eq!(
            r.asset("linux-x64").unwrap().sha256.as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
        );
        // No digest and no notes entry: no hash, so the cache refuses it.
        assert_eq!(r.asset("linux-arm64").unwrap().sha256, None);
    }

    /// A loopback GitHub: checks the App JWT and the installation token.
    #[derive(Clone)]
    struct Fake {
        public: Arc<Vec<u8>>,
        token_calls: Arc<AtomicU32>,
        jit_bodies: Arc<std::sync::Mutex<Vec<Value>>>,
    }

    fn bearer(h: &HeaderMap) -> String {
        h.get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_string()
    }

    async fn serve(fake: Fake) -> String {
        let app = axum::Router::new()
            .route(
                "/orgs/acme/installation",
                get(|State(f): State<Fake>, h: HeaderMap| async move {
                    let claims = verify_jwt(&bearer(&h), &f.public);
                    assert_eq!(claims["iss"], 99);
                    axum::Json(json!({"id": 7}))
                }),
            )
            .route(
                "/app/installations/7/access_tokens",
                post(|State(f): State<Fake>, h: HeaderMap| async move {
                    verify_jwt(&bearer(&h), &f.public);
                    f.token_calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::CREATED,
                        axum::Json(json!({"token": "ghs_installation", "expires_at": "2999-01-01T00:00:00Z"})),
                    )
                }),
            )
            .route(
                "/orgs/acme/actions/runner-groups",
                get(|h: HeaderMap| async move {
                    assert_eq!(bearer(&h), "ghs_installation");
                    axum::Json(json!({"runner_groups": [
                        {"id": 1, "name": "Default", "default": true, "visibility": "all", "allows_public_repositories": false},
                        {"id": 5, "name": "cua-spaces", "default": false, "visibility": "selected", "allows_public_repositories": true}
                    ]}))
                }),
            )
            .route(
                "/orgs/acme/actions/runners",
                get(|Query(q): Query<HashMap<String, String>>, h: HeaderMap| async move {
                    assert_eq!(bearer(&h), "ghs_installation");
                    let page: u64 = q["page"].parse().unwrap();
                    let n = if page == 1 { 100 } else { 3 };
                    let runners: Vec<Value> = (0..n)
                        .map(|i| json!({"id": page * 1000 + i, "name": format!("r{page}-{i}"), "status": "online", "busy": i == 0, "labels": [{"name": "self-hosted"}]}))
                        .collect();
                    axum::Json(json!({"total_count": 103, "runners": runners}))
                }),
            )
            .route(
                "/orgs/acme/actions/runners/generate-jitconfig",
                post(|State(f): State<Fake>, h: HeaderMap, axum::Json(b): axum::Json<Value>| async move {
                    assert_eq!(bearer(&h), "ghs_installation");
                    f.jit_bodies.lock().unwrap().push(b.clone());
                    (
                        StatusCode::CREATED,
                        axum::Json(json!({"runner": {"id": 77, "name": b["name"]}, "encoded_jit_config": "JIT-SECRET"})),
                    )
                }),
            )
            .route(
                "/orgs/acme/actions/runners/{id}",
                delete(|Path(id): Path<u64>| async move {
                    match id {
                        1 => StatusCode::NO_CONTENT,
                        2 => StatusCode::UNPROCESSABLE_ENTITY,
                        _ => StatusCode::NOT_FOUND,
                    }
                }),
            )
            .with_state(fake);
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn app_client_end_to_end_against_a_loopback_github() {
        let key = Arc::new(AppKey::from_pem(&pkcs1()).unwrap());
        let fake = Fake {
            public: Arc::new(key.public_key_der()),
            token_calls: Arc::new(AtomicU32::new(0)),
            jit_bodies: Arc::new(std::sync::Mutex::new(vec![])),
        };
        let url = serve(fake.clone()).await;
        let gh = GitHubClient::new(
            &url,
            Scope::parse("org:acme").unwrap(),
            Credentials::App {
                app_id: 99,
                installation_id: None,
                key,
            },
        )
        .unwrap();
        assert_eq!(gh.installation_id().await.unwrap(), 7);
        assert_eq!(gh.runner_group_id(None).await.unwrap(), 1);
        assert_eq!(gh.runner_group_id(Some("CUA-spaces")).await.unwrap(), 5);
        let e = gh
            .runner_group_id(Some("nope"))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("Default, cua-spaces"), "{e}");
        let runners = gh.list_runners().await.unwrap();
        assert_eq!(runners.len(), 103, "both pages");
        assert!(runners[0].busy && !runners[1].busy);
        assert_eq!(runners[0].labels, ["self-hosted"]);
        let jit = gh
            .generate_jit(&JitRequest {
                name: "cua-acme-1234abcd".into(),
                runner_group_id: 5,
                labels: vec!["self-hosted".into(), "macOS".into()],
                work_folder: "_work".into(),
            })
            .await
            .unwrap();
        assert_eq!(jit.runner_id, 77);
        assert_eq!(jit.encoded.expose(), "JIT-SECRET");
        assert!(!format!("{jit:?}").contains("JIT-SECRET"));
        assert_eq!(
            fake.jit_bodies.lock().unwrap()[0],
            json!({"name": "cua-acme-1234abcd", "runner_group_id": 5, "labels": ["self-hosted", "macOS"], "work_folder": "_work"})
        );
        assert_eq!(gh.delete_runner(1).await.unwrap(), DeleteOutcome::Deleted);
        assert_eq!(gh.delete_runner(2).await.unwrap(), DeleteOutcome::Busy);
        assert_eq!(gh.delete_runner(3).await.unwrap(), DeleteOutcome::NotFound);
        // One installation token for every call: it is cached.
        assert_eq!(fake.token_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refusal_names_the_call_and_a_fix() {
        let app = axum::Router::new().route(
            "/repos/a/b/actions/runners",
            get(|| async {
                (
                    StatusCode::FORBIDDEN,
                    axum::Json(json!({"message": "Resource not accessible by integration"})),
                )
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let gh = GitHubClient::new(
            &format!("http://{addr}"),
            Scope::parse("repo:a/b").unwrap(),
            Credentials::Token(Secret::new("github_pat_SECRET")),
        )
        .unwrap();
        // A repository's runners are always in group 1.
        assert_eq!(gh.runner_group_id(Some("x")).await.unwrap(), 1);
        let e = gh.list_runners().await.unwrap_err().to_string();
        assert!(e.contains("GET repos/a/b/actions/runners"), "{e}");
        assert!(
            e.contains("403 Resource not accessible by integration"),
            "{e}"
        );
        assert!(e.contains("Self-hosted runners"), "{e}");
        assert!(!e.contains("github_pat_SECRET"));
        assert!(!format!("{gh:?}").contains("github_pat_SECRET"));
    }
}
