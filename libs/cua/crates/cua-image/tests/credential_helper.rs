//! A pull never runs the user's docker credential helper unless the registry
//! refuses anonymous access. Docker Desktop's helper makes macOS ask "Cua
//! Spaces would like to access data from other apps", so a Space created
//! from a public image (`ghcr.io/trycua/linux:24.04`) must not start it.
//!
//! Hermetic: a loopback registry (one public repository, one that wants
//! Basic auth), a throwaway docker config with `"credsStore": "r7fake"` and a
//! fake `docker-credential-r7fake` first on `PATH` that logs each call. Its
//! own test binary, since it sets env vars.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine;
use cua_image::digest::sha256_bytes;
use cua_image::{ImageError, RegistryClient, auth};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The env vars below are process-wide: one test at a time.
static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const PUBLIC: &str = "pub/app";
const PRIVATE: &str = "team/app";
const BASIC: &str = "bot:s3cret";

struct Registry {
    host: String,
    /// `(path, sent credentials)` for every request, in order.
    requests: Arc<Mutex<Vec<(String, bool)>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Registry {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let manifest = serde_json::to_vec(&json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {"mediaType": "application/vnd.oci.image.config.v1+json",
                "digest": sha256_bytes(b"{}"), "size": 2},
            "layers": []
        }))
        .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let want = base64::engine::general_purpose::STANDARD.encode(BASIC);
        let server = tokio::spawn(async move {
            // Bounded: the test aborts this task; one request per connection.
            for _ in 0..200 {
                let Ok((mut s, _)) = listener.accept().await else {
                    return;
                };
                let (manifest, seen, want) = (manifest.clone(), seen.clone(), want.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 16 << 10];
                    let mut n = 0;
                    while n < buf.len() {
                        let Ok(k) = s.read(&mut buf[n..]).await else {
                            return;
                        };
                        if k == 0 {
                            return;
                        }
                        n += k;
                        if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                    let authed = req.lines().any(|l| {
                        l.to_ascii_lowercase().starts_with("authorization:") && l.contains(&want)
                    });
                    seen.lock().unwrap().push((path.clone(), authed));
                    let public = path.contains(&format!("/v2/{PUBLIC}/"));
                    let (status, headers, body): (&str, String, Vec<u8>) = if !authed && !public {
                        (
                            "401 Unauthorized",
                            "WWW-Authenticate: Basic realm=\"fake\"\r\n".into(),
                            br#"{"errors":[{"code":"UNAUTHORIZED","message":"auth required"}]}"#
                                .to_vec(),
                        )
                    } else if path == "/v2/" {
                        ("200 OK", String::new(), b"{}".to_vec())
                    } else if path.contains("/manifests/") {
                        (
                            "200 OK",
                            format!(
                                "Content-Type: application/vnd.oci.image.manifest.v1+json\r\n\
                                     Docker-Content-Digest: {}\r\n",
                                sha256_bytes(&manifest)
                            ),
                            manifest.clone(),
                        )
                    } else {
                        ("404 Not Found", String::new(), b"{}".to_vec())
                    };
                    let head = format!(
                        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = s.write_all(head.as_bytes()).await;
                    let _ = s.write_all(&body).await;
                });
            }
        });
        Self {
            host,
            requests,
            server,
        }
    }

    fn reference(&self, repo: &str) -> String {
        format!("{}/{repo}:1", self.host)
    }

    fn client(&self) -> RegistryClient {
        RegistryClient::new(vec![self.host.clone()])
    }

    /// Manifest requests that carried credentials / that did not.
    fn manifest_requests(&self) -> (usize, usize) {
        let all = self.requests.lock().unwrap();
        let manifests = all.iter().filter(|(p, _)| p.contains("/manifests/"));
        (
            manifests.clone().filter(|(_, a)| *a).count(),
            manifests.filter(|(_, a)| !*a).count(),
        )
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// A docker config with `credsStore: r7fake` (or `inline` credentials for
/// the registry) and a fake `docker-credential-r7fake` first on `PATH`.
struct Env {
    _dir: tempfile::TempDir,
    log: PathBuf,
}

impl Env {
    fn new(inline_for: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let helper = bin.join("docker-credential-r7fake");
        // The docker credential helper protocol: `get`, the registry on
        // stdin, `{"Username", "Secret"}` on stdout.
        std::fs::write(
            &helper,
            "#!/bin/sh\necho \"$1 $(cat)\" >> \"$R7_HELPER_LOG\"\n\
             printf '{\"ServerURL\":\"x\",\"Username\":\"bot\",\"Secret\":\"s3cret\"}'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut cfg = json!({"credsStore": "r7fake"});
        if let Some(host) = inline_for {
            cfg["auths"] =
                json!({host: {"auth": base64::engine::general_purpose::STANDARD.encode(BASIC)}});
        }
        std::fs::write(dir.path().join("config.json"), cfg.to_string()).unwrap();
        let log = dir.path().join("helper.log");
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&path));
        unsafe {
            std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
            std::env::set_var("DOCKER_CONFIG", dir.path());
            std::env::set_var("R7_HELPER_LOG", &log);
            std::env::remove_var("CUA_REGISTRY_CRED_HELPERS");
            for k in [
                "CUA_REGISTRY_USERNAME",
                "CUA_REGISTRY_PASSWORD",
                "CUA_REGISTRY_HOST",
                "GITHUB_TOKEN",
            ] {
                std::env::remove_var(k);
            }
        }
        Self { _dir: dir, log }
    }

    /// The registries the helper was asked for, one per call.
    fn helper_calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn is_manifest(m: &cua_image::manifest::Manifest) -> bool {
    matches!(m, cua_image::manifest::Manifest::Image(_))
}

#[tokio::test]
async fn a_public_image_never_runs_the_credential_helper() {
    let _guard = ENV.lock().await;
    let reg = Registry::start().await;
    let env = Env::new(None);
    let client = reg.client();

    let (m, digest) = client.manifest(&reg.reference(PUBLIC)).await.unwrap();
    assert!(is_manifest(&m));
    assert!(digest.starts_with("sha256:"));
    client.manifest_bytes(&reg.reference(PUBLIC)).await.unwrap();
    let (pinned, _, _) = client
        .resolve_platform(&reg.reference(PUBLIC), "arm64")
        .await
        .unwrap();
    assert!(pinned.starts_with(&format!("{}/{PUBLIC}@sha256:", reg.host)));

    assert_eq!(env.helper_calls(), Vec::<String>::new());
    let (with_creds, anonymous) = reg.manifest_requests();
    assert_eq!((with_creds, anonymous), (0, 3), "all anonymous");
}

#[tokio::test]
async fn the_old_pull_chain_ran_the_helper_for_the_same_public_image() {
    // What every pull did before: the whole chain, helper first. It is still
    // the chain pushes use, and what `resolve_quiet` leaves out.
    let _guard = ENV.lock().await;
    let reg = Registry::start().await;
    let env = Env::new(None);

    let (_, source) = auth::resolve_quiet(&reg.host).await;
    assert_eq!(source, auth::AuthSource::Anonymous);
    assert_eq!(env.helper_calls(), Vec::<String>::new());

    let (_, source) = auth::resolve(&reg.host).await;
    assert_eq!(source, auth::AuthSource::CredentialHelper("r7fake".into()));
    assert_eq!(env.helper_calls(), vec![format!("get {}", reg.host)]);
}

#[tokio::test]
async fn a_private_image_asks_the_helper_once_after_anonymous_access_is_refused() {
    let _guard = ENV.lock().await;
    let reg = Registry::start().await;
    let env = Env::new(None);
    let client = reg.client();
    let r = reg.reference(PRIVATE);

    // The refused anonymous try, then the helper's credentials.
    let (m, _) = client.manifest(&r).await.unwrap();
    assert!(is_manifest(&m));
    assert_eq!(env.helper_calls(), vec![format!("get {}", reg.host)]);
    assert_eq!(reg.manifest_requests(), (1, 1));

    // Remembered: no second refusal, no second helper call.
    client.manifest(&r).await.unwrap();
    client.manifest_bytes(&r).await.unwrap();
    assert_eq!(env.helper_calls().len(), 1);
    assert_eq!(reg.manifest_requests(), (3, 1));

    // A new client tries anonymously itself, but reuses the helper's answer.
    reg.client().manifest(&r).await.unwrap();
    assert_eq!(env.helper_calls().len(), 1);
    assert_eq!(reg.manifest_requests(), (4, 2));

    // The public repository on the same registry still goes out anonymously.
    client.manifest(&reg.reference(PUBLIC)).await.unwrap();
    assert_eq!(env.helper_calls().len(), 1);
    assert_eq!(reg.manifest_requests(), (4, 3));
}

#[tokio::test]
async fn a_private_image_without_helper_credentials_stays_unauthorized() {
    let _guard = ENV.lock().await;
    let reg = Registry::start().await;
    let env = Env::new(None);
    unsafe { std::env::set_var("CUA_REGISTRY_CRED_HELPERS", "0") };

    let e = reg
        .client()
        .manifest(&reg.reference(PRIVATE))
        .await
        .unwrap_err();
    assert!(matches!(e, ImageError::Unauthorized(_)), "{e:?}");
    assert_eq!(env.helper_calls(), Vec::<String>::new());
    unsafe { std::env::remove_var("CUA_REGISTRY_CRED_HELPERS") };
}

#[tokio::test]
async fn inline_credentials_are_used_at_once_without_the_helper() {
    let _guard = ENV.lock().await;
    let reg = Registry::start().await;
    let env = Env::new(Some(&reg.host));

    reg.client()
        .manifest(&reg.reference(PRIVATE))
        .await
        .unwrap();
    assert_eq!(env.helper_calls(), Vec::<String>::new());
    assert_eq!(reg.manifest_requests(), (1, 0));
}

/// The real `ghcr.io/trycua/linux:24.04`, the way a Space create resolves it,
/// with the fake helper configured. Needs the network:
/// `cargo test -p cua-image --test credential_helper -- --ignored`.
#[tokio::test]
#[ignore = "reads ghcr.io"]
async fn live_default_image_resolves_without_the_helper() {
    let _guard = ENV.lock().await;
    let env = Env::new(None);
    let got = cua_image::resolve::resolve(
        "ghcr.io/trycua/linux:24.04",
        cua_image::resolve::Backend::Container,
        "arm64",
    )
    .await
    .unwrap();
    assert!(got.pinned_ref.starts_with("ghcr.io/trycua/linux@sha256:"));
    assert_eq!(env.helper_calls(), Vec::<String>::new());
}
