//! What every contrib provider shares: credentials (environment first,
//! then `cua auth provider set`), a JSON API client that never logs
//! secrets, template cache keys and bounded polling.

use cua_sandbox_core::{Error, ProviderImage, Result};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};

/// A secret string: `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value.
    pub fn new(v: impl Into<String>) -> Self {
        Self(v.into())
    }

    /// The value (only for building request headers).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Where `cua auth provider set` keeps provider keys:
/// `$CUA_HOME/contrib/credentials.json` (owner-only), `{provider: {VAR: value}}`.
#[derive(Clone, Debug)]
pub struct CredentialStore {
    path: PathBuf,
}

impl Default for CredentialStore {
    fn default() -> Self {
        Self::at(
            cua_auth::cua_home()
                .join("contrib")
                .join("credentials.json"),
        )
    }
}

type StoreMap = BTreeMap<String, BTreeMap<String, String>>;

impl CredentialStore {
    /// A store at `path` (tests).
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn read(&self) -> StoreMap {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write(&self, m: &StoreMap) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
        restrict(&tmp)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// The stored value of `var` for `provider`.
    pub fn get(&self, provider: &str, var: &str) -> Option<Secret> {
        self.read()
            .get(provider)
            .and_then(|m| m.get(var))
            .filter(|v| !v.is_empty())
            .map(|v| Secret::new(v.clone()))
    }

    /// Stores `var` for `provider`.
    pub fn set(&self, provider: &str, var: &str, value: &Secret) -> Result<()> {
        let mut m = self.read();
        m.entry(provider.to_string())
            .or_default()
            .insert(var.to_string(), value.expose().to_string());
        self.write(&m)
    }

    /// Removes everything stored for `provider`; whether anything was.
    pub fn remove(&self, provider: &str) -> Result<bool> {
        let mut m = self.read();
        let had = m.remove(provider).is_some();
        if had {
            self.write(&m)?;
        }
        Ok(had)
    }

    /// The variable names stored per provider (never the values).
    pub fn names(&self) -> BTreeMap<String, Vec<String>> {
        self.read()
            .into_iter()
            .map(|(p, m)| (p, m.into_keys().collect()))
            .collect()
    }
}

#[cfg(unix)]
fn restrict(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

/// Where a credential came from (for `cua auth provider ls`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    /// An environment variable.
    Env,
    /// `cua auth provider set`.
    Store,
}

/// The first of `vars` set in the environment, else in `store`, with the
/// variable it came from.
pub fn credential<'v>(
    store: &CredentialStore,
    provider: &str,
    vars: &[&'v str],
) -> Option<(Secret, &'v str, CredentialSource)> {
    for v in vars {
        if let Ok(value) = std::env::var(v)
            && !value.trim().is_empty()
        {
            return Some((Secret::new(value.trim()), *v, CredentialSource::Env));
        }
    }
    for v in vars {
        if let Some(s) = store.get(provider, v) {
            return Some((s, *v, CredentialSource::Store));
        }
    }
    None
}

/// [`Error::ContribNotConfigured`] naming how to configure `provider`.
pub fn not_configured(provider: &str, vars: &[&str]) -> Error {
    Error::ContribNotConfigured(format!(
        "{provider} is not configured: set {} (or run `cua auth provider set {provider}`)",
        vars.join(" or ")
    ))
}

/// Base URL override from the environment (tests and self-hosted
/// deployments), else `default`.
pub fn base_url(var: &str, default: &str) -> String {
    std::env::var(var)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.trim_end_matches('/').to_string())
}

/// A JSON API with fixed auth headers. Errors carry the status and the
/// server's message, never a header value.
#[derive(Clone)]
pub struct Api {
    provider: &'static str,
    base: String,
    headers: Vec<(&'static str, Secret)>,
    http: reqwest::Client,
    /// The credential variable (named in 401/403 errors).
    credential_var: &'static str,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api")
            .field("provider", &self.provider)
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

/// A raw HTTP answer.
#[derive(Clone, Debug)]
pub struct Answer {
    /// Status.
    pub status: u16,
    /// Body.
    pub body: Vec<u8>,
}

impl Api {
    /// An API at `base` sending `headers` (values are secrets).
    pub fn new(
        provider: &'static str,
        base: impl Into<String>,
        headers: Vec<(&'static str, Secret)>,
        credential_var: &'static str,
    ) -> Self {
        cua_sandbox_core::cua_spacesd_client::transport::ensure_crypto_provider();
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(120))
            .user_agent(concat!("cua-contrib/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            provider,
            base: base.into().trim_end_matches('/').to_string(),
            headers,
            http,
            credential_var,
        }
    }

    /// The base URL.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// A request to `path` (joined to the base) or an absolute URL.
    pub async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<Answer> {
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{}", self.base, path)
        };
        let mut req = self.http.request(method.clone(), &url);
        for (k, v) in &self.headers {
            let mut value = reqwest::header::HeaderValue::from_str(v.expose()).map_err(|_| {
                Error::InvalidArgument(format!("{}: bad credential", self.provider))
            })?;
            value.set_sensitive(true);
            req = req.header(*k, value);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.map_err(|e| {
            Error::Http(format!(
                "{} {method} {}: {}",
                self.provider,
                redact_path(path),
                e.without_url()
            ))
        })?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| Error::Http(format!("{}: {}", self.provider, e.without_url())))?
            .to_vec();
        Ok(Answer { status, body })
    }

    /// A JSON request; non-2xx answers become typed errors.
    pub async fn json<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let body = body.map(serde_json::to_value).transpose()?;
        let a = self.send(method.clone(), path, body).await?;
        self.check(&method, path, &a)?;
        let bytes = if a.body.is_empty() {
            b"null".as_slice()
        } else {
            &a.body
        };
        serde_json::from_slice(bytes).map_err(|e| {
            Error::Http(format!(
                "{} {method} {}: unexpected response ({e})",
                self.provider,
                redact_path(path)
            ))
        })
    }

    /// A request whose answer body is ignored (2xx, or 404 when
    /// `missing_ok`).
    pub async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
        missing_ok: bool,
    ) -> Result<()> {
        let body = body.map(serde_json::to_value).transpose()?;
        let a = self.send(method.clone(), path, body).await?;
        if missing_ok && a.status == 404 {
            return Ok(());
        }
        self.check(&method, path, &a)
    }

    /// Maps a non-2xx answer to an error.
    pub fn check(&self, method: &reqwest::Method, path: &str, a: &Answer) -> Result<()> {
        if (200..300).contains(&a.status) {
            return Ok(());
        }
        let msg = server_message(&a.body);
        let what = format!("{} {method} {}", self.provider, redact_path(path));
        Err(match a.status {
            401 | 403 => Error::ContribNotConfigured(format!(
                "{what}: the {} API refused the credentials in {} ({}): {msg}",
                self.provider, self.credential_var, a.status
            )),
            404 => Error::NotFound(format!("{what}: {msg}")),
            400 | 409 | 422 => Error::InvalidArgument(format!("{what}: {msg}")),
            429 => Error::Http(format!("{what}: rate limited: {msg}")),
            s => Error::Http(format!("{what}: HTTP {s}: {msg}")),
        })
    }
}

/// Paths may carry ids but never secrets; query strings are dropped.
fn redact_path(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

/// The server's error message (`message`, `error`, `detail`), else a
/// prefix of the body.
pub fn server_message(body: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) {
        for k in ["message", "error", "detail", "msg"] {
            match v.get(k) {
                Some(serde_json::Value::String(s)) => return s.clone(),
                Some(o @ serde_json::Value::Object(_)) => {
                    if let Some(m) = o.get("message").and_then(|m| m.as_str()) {
                        return m.to_string();
                    }
                }
                _ => {}
            }
        }
    }
    let text = String::from_utf8_lossy(body);
    text.chars().take(300).collect()
}

/// A template / snapshot cache key: `cua-<16 hex>` over the pinned image
/// (digest when known) and every build input that changes the artifact.
/// Two creates with the same inputs reuse one build; a moved tag (new
/// digest) builds again.
pub fn template_key(image: &ProviderImage, inputs: &[(&str, String)]) -> String {
    let mut h = Sha256::new();
    h.update(b"cua-contrib-template-v1\n");
    let pinned = if image.digest.is_empty() {
        &image.pinned_ref
    } else {
        &image.digest
    };
    h.update(pinned.as_bytes());
    h.update(b"\n");
    h.update(image.kind.as_str().as_bytes());
    h.update(b"\n");
    h.update(image.arch.as_bytes());
    for (k, v) in inputs {
        h.update(b"\n");
        h.update(k.as_bytes());
        h.update(b"=");
        h.update(v.as_bytes());
    }
    let hex: String = h
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("cua-{hex}")
}

/// Polls `f` every `interval` until it returns `Some`, for at most
/// `timeout` and `max_polls` attempts (both bound the loop).
pub async fn poll<T, F, Fut>(
    what: &str,
    timeout: Duration,
    interval: Duration,
    max_polls: u32,
    mut f: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>>>,
{
    let deadline = Instant::now() + timeout;
    for _ in 0..max_polls.max(1) {
        if let Some(v) = f().await? {
            return Ok(v);
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(interval).await;
    }
    Err(Error::Timeout(format!("{what} within {timeout:?}")))
}

/// Shell-quotes one argument for `sh -c`.
pub fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@,+".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// An argv as one `sh -c` line.
pub fn sh_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| sh_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_sandbox_core::RunKind;

    fn image(digest: &str) -> ProviderImage {
        ProviderImage {
            reference: "ghcr.io/trycua/linux:24.04".into(),
            pinned_ref: format!("ghcr.io/trycua/linux@{digest}"),
            digest: digest.into(),
            kind: RunKind::Container,
            arch: "amd64".into(),
            spacesd: Some(true),
        }
    }

    #[test]
    fn template_key_follows_digest_and_inputs() {
        let a = template_key(&image("sha256:aa"), &[("cpu", "2".into())]);
        assert_eq!(a, template_key(&image("sha256:aa"), &[("cpu", "2".into())]));
        assert_ne!(a, template_key(&image("sha256:bb"), &[("cpu", "2".into())]));
        assert_ne!(a, template_key(&image("sha256:aa"), &[("cpu", "4".into())]));
        assert!(a.starts_with("cua-") && a.len() == 20, "{a}");
    }

    #[test]
    fn secrets_never_debug() {
        let s = Secret::new("e2b_live_value");
        assert!(!format!("{s:?}").contains("live"));
    }

    #[test]
    fn store_round_trip_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::at(dir.path().join("c.json"));
        store.set("e2b", "E2B_API_KEY", &Secret::new("k1")).unwrap();
        assert_eq!(store.get("e2b", "E2B_API_KEY").unwrap().expose(), "k1");
        assert_eq!(store.names()["e2b"], vec!["E2B_API_KEY".to_string()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(store.remove("e2b").unwrap());
        assert!(store.get("e2b", "E2B_API_KEY").is_none());
    }

    #[test]
    fn quoting() {
        assert_eq!(
            sh_join(&["echo".into(), "a b".into(), "it's".into()]),
            "echo 'a b' 'it'\\''s'"
        );
    }

    #[test]
    fn server_messages() {
        assert_eq!(server_message(br#"{"message":"nope"}"#), "nope");
        assert_eq!(server_message(br#"{"error":{"message":"deep"}}"#), "deep");
        assert_eq!(server_message(b"plain"), "plain");
    }
}
