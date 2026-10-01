//! Hermetic harness for the `cua` binary: a temporary HOME, no ambient
//! credentials, loopback fakes only, and a hard timeout on every process.
#![allow(dead_code)]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

/// Variables that could leak real credentials or endpoints into a test.
const SCRUB: &[&str] = &[
    // CI sets telemetry opt-outs; each test decides telemetry itself.
    "DO_NOT_TRACK",
    "CUA_TELEMETRY_ENABLED",
    "CUA_TELEMETRY_DISABLED",
    "FLEETS_TOKEN",
    "CUA_CLIENT_ID",
    "CUA_CLIENT_SECRET",
    "CUA_TOKEN_URL",
    "CUA_FLEET_BASE_URL",
    "CUA_FLEET_NAMESPACE",
    "CUA_DAEMON",
    "CUA_DAEMON_TOKEN",
    "CUA_ENV_TOKEN",
    "CUA_SANDBOX",
    "CUA_MCP_PERMISSIONS",
    "CUA_OIDC_ISSUER",
    "CUA_OIDC_CLIENT_ID",
    "CUA_HOST_ENV_URL",
    "CUA_HOST_ENV_TOKEN",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "CUA_SNAPSHOT_MODEL",
];

/// A sandboxed home for one test.
pub struct Home {
    pub dir: tempfile::TempDir,
    pub env: HashMap<String, String>,
}

/// Process result.
#[derive(Debug)]
pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn ok(&self) -> &Self {
        assert_eq!(
            self.code, 0,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(self.stdout.trim()).unwrap_or_else(|e| {
            panic!("not JSON ({e}):\n{}\nstderr:\n{}", self.stdout, self.stderr)
        })
    }
}

impl Home {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut env = HashMap::new();
        env.insert("HOME".into(), dir.path().display().to_string());
        env.insert("USERPROFILE".into(), dir.path().display().to_string());
        env.insert(
            "CUA_HOME".into(),
            dir.path().join(".cua").display().to_string(),
        );
        // The SDK refuses writes to the real ~/.cua from this process tree.
        env.insert("CUA_TEST".into(), "1".into());
        // Never touch the OS keychain, never open a browser.
        env.insert("CUA_CREDENTIAL_STORE".into(), "file".into());
        env.insert("CUA_NO_BROWSER".into(), "1".into());
        env.insert("CUA_LOG".into(), "error".into());
        // Never read a real registry for the Fleet runtime/image rule.
        env.insert("CUA_FLEET_IMAGE_INSPECT".into(), "0".into());
        // Never send telemetry, even from a child that clears its
        // environment; tests that exercise it turn it on explicitly and keep
        // the no-network guard.
        env.insert("CUA_TELEMETRY".into(), "0".into());
        env.insert("CUA_TELEMETRY_FORBID_NETWORK".into(), "1".into());
        env.insert(
            "CUA_TELEMETRY_ENDPOINT".into(),
            "http://127.0.0.1:9/batch/".into(),
        );
        Self { dir, env }
    }

    pub fn set(&mut self, k: &str, v: impl Into<String>) -> &mut Self {
        self.env.insert(k.into(), v.into());
        self
    }

    pub fn cua_home(&self) -> PathBuf {
        self.dir.path().join(".cua")
    }

    fn command(&self, args: &[&str]) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_cua"));
        for k in SCRUB {
            c.env_remove(k);
        }
        c.envs(&self.env)
            .args(args)
            .current_dir(self.dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        c
    }

    /// Runs `cua <args>` (60 s budget).
    pub async fn run(&self, args: &[&str]) -> Out {
        let child = self.command(args).spawn().expect("spawn cua");
        let o = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .unwrap_or_else(|_| panic!("cua {args:?} timed out"))
            .unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into(),
            stderr: String::from_utf8_lossy(&o.stderr).into(),
        }
    }

    /// Spawns `cua <args>` with piped stdin for interactive protocols.
    pub fn spawn(&self, args: &[&str]) -> tokio::process::Child {
        let mut c = self.command(args);
        c.stdin(Stdio::piped());
        c.spawn().expect("spawn cua")
    }
}

/// One recorded HTTP request.
#[derive(Clone, Debug)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl Req {
    /// Form field.
    pub fn form(&self, k: &str) -> Option<String> {
        url::form_urlencoded::parse(self.body.as_bytes())
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.into_owned())
    }
}

type Handler = Arc<dyn Fn(&Req) -> (u16, serde_json::Value) + Send + Sync>;

/// A tiny loopback HTTP/1.1 server (one request per connection) for fake
/// OIDC, GitHub OIDC and LLM endpoints.
pub struct FakeHttp {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Req>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeHttp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeHttp {
    pub async fn start(
        handler: impl Fn(&Req) -> (u16, serde_json::Value) + Send + Sync + 'static,
    ) -> Self {
        let handler: Handler = Arc::new(handler);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let log = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else {
                    break;
                };
                let (handler, log) = (handler.clone(), log.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    // Headers (bounded).
                    let head_end = loop {
                        if buf.len() > 1 << 20 {
                            return;
                        }
                        match s.read(&mut tmp).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let mut lines = head.lines();
                    let mut first = lines.next().unwrap_or_default().split_whitespace();
                    let method = first.next().unwrap_or_default().to_string();
                    let path = first.next().unwrap_or_default().to_string();
                    let headers: HashMap<String, String> = lines
                        .filter_map(|l| l.split_once(':'))
                        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                        .collect();
                    let len: usize = headers
                        .get("content-length")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0)
                        .min(64 << 20);
                    while buf.len() < head_end + len {
                        match s.read(&mut tmp).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                    }
                    let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                    let req = Req {
                        method,
                        path,
                        headers,
                        body,
                    };
                    log.lock().unwrap().push(req.clone());
                    let (status, v) = handler(&req);
                    let body = v.to_string();
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = s.write_all(resp.as_bytes()).await;
                    let _ = s.shutdown().await;
                });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    pub fn requests(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }
}

/// An unsigned JWT with `claims` (the CLI only decodes it for display).
pub fn jwt(claims: serde_json::Value) -> String {
    use base64::Engine;
    let e = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    format!(
        "{}.{}.sig",
        e(br#"{"alg":"none"}"#),
        e(claims.to_string().as_bytes())
    )
}

pub fn read_json(p: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}
