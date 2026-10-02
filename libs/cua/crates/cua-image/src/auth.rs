//! Registry credentials: env → docker config (`auths` / credential helpers) →
//! anonymous.
//!
//! * `CUA_REGISTRY_USERNAME` + `CUA_REGISTRY_PASSWORD` (optionally scoped with
//!   `CUA_REGISTRY_HOST`).
//! * `GITHUB_TOKEN` for `ghcr.io` (user `GITHUB_ACTOR`, else `x-access-token`).
//! * `$DOCKER_CONFIG/config.json` or `~/.docker/config.json`: inline `auths`,
//!   then `credHelpers`/`credsStore` via `docker-credential-<helper> get`
//!   (disable helpers with `CUA_REGISTRY_CRED_HELPERS=0`).
//! * Private ECR (`<account>.dkr.ecr.<region>.amazonaws.com`, as Fleet's
//!   private images): `aws ecr get-login-password --region <region>` with
//!   the caller's AWS CLI credentials, when `aws` is installed (disable with
//!   `CUA_REGISTRY_AWS_CLI=0`). Read-only; nothing is written to the docker
//!   config.

use std::path::PathBuf;

use base64::Engine;
use oci_client::secrets::RegistryAuth;

/// Where a credential came from (for logs; never contains the secret).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthSource {
    /// Credentials passed with the request (`RegistrySecret`).
    Explicit,
    Env,
    GithubToken,
    DockerConfig,
    CredentialHelper(String),
    AwsCli,
    Anonymous,
}

/// Resolve credentials for `registry` (a host like `ghcr.io`).
pub async fn resolve(registry: &str) -> (RegistryAuth, AuthSource) {
    if let (Ok(u), Ok(p)) = (
        std::env::var("CUA_REGISTRY_USERNAME"),
        std::env::var("CUA_REGISTRY_PASSWORD"),
    ) {
        let scoped = std::env::var("CUA_REGISTRY_HOST").ok();
        if scoped.as_deref().is_none_or(|h| h == registry) {
            return (RegistryAuth::Basic(u, p), AuthSource::Env);
        }
    }
    if registry == "ghcr.io"
        && let Ok(tok) = std::env::var("GITHUB_TOKEN")
    {
        let user = std::env::var("GITHUB_ACTOR").unwrap_or_else(|_| "x-access-token".into());
        return (RegistryAuth::Basic(user, tok), AuthSource::GithubToken);
    }
    if let Some(cfg) = docker_config() {
        if let Some((u, p)) = inline_auth(&cfg, registry) {
            return (RegistryAuth::Basic(u, p), AuthSource::DockerConfig);
        }
        if std::env::var("CUA_REGISTRY_CRED_HELPERS").as_deref() != Ok("0")
            && let Some(helper) = helper_for(&cfg, registry)
            && let Some((u, p)) = run_helper(&helper, registry).await
        {
            return (
                RegistryAuth::Basic(u, p),
                AuthSource::CredentialHelper(helper),
            );
        }
    }
    if let Some(region) = ecr_region(registry)
        && std::env::var("CUA_REGISTRY_AWS_CLI").as_deref() != Ok("0")
        && let Some(p) = aws_ecr_password(region).await
    {
        return (RegistryAuth::Basic("AWS".into(), p), AuthSource::AwsCli);
    }
    (RegistryAuth::Anonymous, AuthSource::Anonymous)
}

/// Region of a private ECR host (`<acct>.dkr.ecr.<region>.amazonaws.com`).
pub fn ecr_region(registry: &str) -> Option<&str> {
    let rest = registry.split_once(".dkr.ecr.")?.1;
    let region = rest.strip_suffix(".amazonaws.com")?;
    (!region.is_empty() && !region.contains('.')).then_some(region)
}

/// A private ECR login token for `region` from the caller's AWS CLI
/// credentials (`aws ecr get-login-password`); the user is `AWS`. `None`
/// when the CLI is missing or refuses.
pub async fn aws_ecr_login(region: &str) -> Option<String> {
    aws_ecr_password(region).await
}

/// ECR tokens last 12 h; reuse one for 6 h per region.
async fn aws_ecr_password(region: &str) -> Option<String> {
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, (String, Instant)>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((p, at)) = cache.lock().ok()?.get(region)
        && at.elapsed() < Duration::from_secs(6 * 3600)
    {
        return Some(p.clone());
    }
    let p = aws_ecr_password_uncached(region).await?;
    cache
        .lock()
        .ok()?
        .insert(region.to_string(), (p.clone(), Instant::now()));
    Some(p)
}

async fn aws_ecr_password_uncached(region: &str) -> Option<String> {
    let aws = cua_vmm::host::which("aws")?;
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new(aws)
            .args(["ecr", "get-login-password", "--region", region])
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let p = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !p.is_empty()).then_some(p)
}

fn docker_config() -> Option<serde_json::Value> {
    let dir = std::env::var_os("DOCKER_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| cua_vmm::host::home_dir().join(".docker"));
    serde_json::from_slice(&std::fs::read(dir.join("config.json")).ok()?).ok()
}

/// Keys docker uses for a registry in `auths`.
fn auth_keys(registry: &str) -> Vec<String> {
    let mut keys = vec![
        registry.to_string(),
        format!("https://{registry}"),
        format!("https://{registry}/v2/"),
    ];
    if matches!(
        registry,
        "docker.io" | "registry-1.docker.io" | "index.docker.io"
    ) {
        keys.push("https://index.docker.io/v1/".into());
    }
    keys
}

/// `(username, password)` from `auths.<registry>.auth` (base64 `user:pass`).
pub fn inline_auth(cfg: &serde_json::Value, registry: &str) -> Option<(String, String)> {
    let auths = cfg.get("auths")?.as_object()?;
    for k in auth_keys(registry) {
        let Some(entry) = auths.get(&k) else { continue };
        if let Some(b64) = entry
            .get("auth")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            let raw = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
            let s = String::from_utf8(raw).ok()?;
            let (u, p) = s.split_once(':')?;
            return Some((u.to_string(), p.to_string()));
        }
        if let (Some(u), Some(p)) = (
            entry.get("username").and_then(|v| v.as_str()),
            entry.get("password").and_then(|v| v.as_str()),
        ) {
            return Some((u.to_string(), p.to_string()));
        }
    }
    None
}

/// Credential helper configured for `registry` (`credHelpers` wins over `credsStore`).
pub fn helper_for(cfg: &serde_json::Value, registry: &str) -> Option<String> {
    if let Some(h) = cfg
        .get("credHelpers")
        .and_then(|m| m.get(registry))
        .and_then(|v| v.as_str())
    {
        return Some(h.to_string());
    }
    cfg.get("credsStore")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn run_helper(helper: &str, registry: &str) -> Option<(String, String)> {
    let bin = cua_vmm::host::which(&format!("docker-credential-{helper}"))?;
    let out = cua_vmm::PrefixExec {
        program: bin.display().to_string(),
        prefix: vec![],
    };
    // PrefixExec appends the script as one argument; the helper wants `get`
    // plus the registry on stdin.
    let res = cua_vmm::GuestExec::exec(
        &out,
        cua_vmm::ExecRequest::sh("get")
            .stdin(registry.as_bytes().to_vec())
            .timeout(std::time::Duration::from_secs(20)),
    )
    .await
    .ok()?;
    if !res.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&res.stdout).ok()?;
    let user = v.get("Username")?.as_str()?.to_string();
    let secret = v.get("Secret")?.as_str()?.to_string();
    Some((user, secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_auths_decode_for_all_key_spellings() {
        let cfg = serde_json::json!({
            "auths": {
                "ghcr.io": {"auth": base64::engine::general_purpose::STANDARD.encode("me:tok")},
                "https://index.docker.io/v1/": {"auth": base64::engine::general_purpose::STANDARD.encode("hub:pw")},
                "https://reg.example.com": {"username": "u", "password": "p"}
            }
        });
        assert_eq!(
            inline_auth(&cfg, "ghcr.io"),
            Some(("me".into(), "tok".into()))
        );
        assert_eq!(
            inline_auth(&cfg, "registry-1.docker.io"),
            Some(("hub".into(), "pw".into()))
        );
        assert_eq!(
            inline_auth(&cfg, "reg.example.com"),
            Some(("u".into(), "p".into()))
        );
        assert_eq!(inline_auth(&cfg, "public.ecr.aws"), None);
    }

    #[test]
    fn ecr_hosts_yield_their_region() {
        assert_eq!(
            ecr_region("123456789012.dkr.ecr.us-west-2.amazonaws.com"),
            Some("us-west-2")
        );
        assert_eq!(ecr_region("public.ecr.aws"), None);
        assert_eq!(ecr_region("ghcr.io"), None);
    }

    #[test]
    fn helper_selection_prefers_per_registry() {
        let cfg = serde_json::json!({"credsStore": "desktop", "credHelpers": {"123.dkr.ecr.us-east-1.amazonaws.com": "ecr-login"}});
        assert_eq!(
            helper_for(&cfg, "123.dkr.ecr.us-east-1.amazonaws.com").as_deref(),
            Some("ecr-login")
        );
        assert_eq!(helper_for(&cfg, "ghcr.io").as_deref(), Some("desktop"));
        assert_eq!(
            helper_for(&serde_json::json!({"credsStore": ""}), "x"),
            None
        );
    }
}
