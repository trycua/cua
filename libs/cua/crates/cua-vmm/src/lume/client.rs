//! Typed HTTP client for `lume serve` (default `http://127.0.0.1:7777`).
//!
//! Routes mirror `libs/lume/src/Server/Server.swift`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Result, VmmError};

/// `GET /lume/vms[/:name]` row (`VMDetails.swift`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmDetails {
    pub name: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub cpu_count: u32,
    #[serde(default)]
    pub memory_size: u64,
    #[serde(default)]
    pub disk_size: Value,
    #[serde(default)]
    pub display: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub provisioning_operation: Option<String>,
    #[serde(default)]
    pub vnc_url: Option<String>,
    #[serde(default, alias = "ip_address")]
    pub ip_address: Option<String>,
    #[serde(default, alias = "mac_address")]
    pub mac_address: Option<String>,
    #[serde(default)]
    pub ssh_available: Option<bool>,
    #[serde(default)]
    pub location_name: Option<String>,
    #[serde(default)]
    pub download_progress: Option<f64>,
    /// Bytes of the pull done so far (Lume that reports bytes; it also
    /// cancels pulls, `POST /lume/pull/cancel`).
    #[serde(default)]
    pub downloaded_bytes: Option<u64>,
    /// Bytes the pull downloads in all.
    #[serde(default)]
    pub total_bytes: Option<u64>,
    /// Lume's smoothed download rate.
    #[serde(default)]
    pub bytes_per_second: Option<f64>,
}

impl VmDetails {
    /// A usable guest IP (lume reports `unknown`/`0.0.0.0` while booting).
    pub fn ip(&self) -> Option<&str> {
        self.ip_address
            .as_deref()
            .filter(|ip| !ip.is_empty() && *ip != "unknown" && !ip.starts_with("0.0.0.0"))
    }
}

/// Parsed `vnc://:PASSWORD@HOST:PORT`.
pub fn parse_vnc_url(url: &str) -> Option<(String, u16, Option<String>)> {
    let rest = url.strip_prefix("vnc://")?;
    let (auth, hostport) = match rest.rsplit_once('@') {
        Some((a, h)) => (Some(a), h),
        None => (None, rest),
    };
    let (host, port) = hostport.rsplit_once(':')?;
    let password = auth
        .map(|a| a.trim_start_matches(':').to_string())
        .filter(|p| !p.is_empty());
    Some((
        host.to_string(),
        port.trim_end_matches('/').parse().ok()?,
        password,
    ))
}

/// `POST /lume/vms` body (`CreateVMRequest`).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateVm {
    pub name: String,
    pub os: String,
    pub cpu: u32,
    pub memory: String,
    pub disk_size: String,
    pub display: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipsw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
}

/// `PATCH /lume/vms/:name` body (`SetVMRequest`, what `lume set` sends).
/// Unset fields keep the VM's current value. The VM must be stopped.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetVm {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<u32>,
    /// Size with a unit, e.g. `4096MB`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
    /// Grow-only disk size with a unit, e.g. `200GB`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_size: Option<String>,
    /// Skip lume's pre-resize disk backup (macOS disk growth only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_backup: Option<bool>,
}

impl SetVm {
    pub fn is_empty(&self) -> bool {
        self.cpu.is_none() && self.memory.is_none() && self.disk_size.is_none()
    }
}

/// Total disk size in bytes from a `VmDetails::disk_size` value
/// (`{"allocated":…,"total":…}`).
pub fn disk_total_bytes(disk_size: &Value) -> Option<u64> {
    disk_size.get("total").and_then(Value::as_u64)
}

/// `POST /lume/vms/:name/run` body (`RunVMRequest`).
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunVm {
    pub no_display: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shared_directories: Vec<SharedDir>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_path: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedDir {
    pub host_path: String,
    pub read_only: bool,
}

/// `POST /lume/pull[/start]` body. Splits `registry/org/image:tag` the same
/// way `cua_sandbox/runtime/lume.py` does.
pub fn pull_body(reference: &str, name: &str) -> Value {
    let parts: Vec<&str> = reference.split('/').collect();
    if parts.len() >= 3 && parts[0].contains('.') {
        json!({
            "image": parts[2..].join("/"),
            "name": name,
            "registry": parts[0],
            "organization": parts[1],
        })
    } else if parts.len() == 2 {
        json!({ "image": parts[1], "name": name, "organization": parts[0] })
    } else {
        json!({ "image": reference, "name": name })
    }
}

/// Client for one `lume serve` instance.
#[derive(Clone)]
pub struct LumeClient {
    http: reqwest::Client,
    base: String,
}

impl LumeClient {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            // lume's HTTP server closes connections after each response
            // without saying so; reusing a pooled connection then fails with
            // "error sending request". One connection per request.
            http: reqwest::Client::builder()
                .pool_max_idle_per_host(0)
                .connect_timeout(Duration::from_secs(3))
                .build()
                .expect("reqwest client"),
            base: base.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let text = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or(text);
        if message.to_ascii_lowercase().contains("not found") {
            // "Virtual machine not found: <name>" → the name.
            let name = message
                .rsplit_once(": ")
                .map(|(_, n)| n.trim().to_string())
                .unwrap_or(message);
            return Err(VmmError::NotFound(name));
        }
        Err(VmmError::Lume {
            status: status.as_u16(),
            message,
        })
    }

    fn net_err_ref(&self, e: &reqwest::Error) -> String {
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(e);
        while let Some(s) = src {
            msg.push_str(&format!(": {s}"));
            src = s.source();
        }
        msg
    }

    fn net_err(&self, e: reqwest::Error) -> VmmError {
        if e.is_connect() {
            VmmError::missing(
                format!("lume serve at {}", self.base),
                "not reachable; start it with `lume serve` (cua starts it automatically when the binary is installed)",
            )
        } else {
            VmmError::other(format!("lume request failed: {}", self.net_err_ref(&e)))
        }
    }

    /// Whether the server answers at all.
    pub async fn reachable(&self) -> bool {
        self.http
            .get(self.url("/lume/host/status"))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .is_ok()
    }

    /// `GET /lume/host/status`.
    pub async fn host_status(&self) -> Result<Value> {
        let r = self
            .http
            .get(self.url("/lume/host/status"))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        Self::check(r)
            .await?
            .json()
            .await
            .map_err(|e| self.net_err(e))
    }

    pub async fn list(&self) -> Result<Vec<VmDetails>> {
        let r = self
            .http
            .get(self.url("/lume/vms"))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        let v: Value = Self::check(r)
            .await?
            .json()
            .await
            .map_err(|e| self.net_err(e))?;
        let arr = if v.is_array() {
            v
        } else {
            v.get("vms").cloned().unwrap_or(Value::Array(vec![]))
        };
        Ok(serde_json::from_value(arr)?)
    }

    /// `GET /lume/vms/:name`; `Ok(None)` when it does not exist.
    pub async fn get(&self, name: &str) -> Result<Option<VmDetails>> {
        let r = self
            .http
            .get(self.url(&format!("/lume/vms/{name}")))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        match Self::check(r).await {
            Ok(r) => Ok(Some(r.json().await.map_err(|e| self.net_err(e))?)),
            Err(VmmError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn create(&self, req: &CreateVm) -> Result<()> {
        let r = self
            .http
            .post(self.url("/lume/vms"))
            .json(req)
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        Self::check(r).await.map(|_| ())
    }

    pub async fn clone_vm(&self, name: &str, new_name: &str) -> Result<()> {
        let r = self
            .http
            .post(self.url("/lume/vms/clone"))
            .json(&json!({ "name": name, "newName": new_name }))
            .timeout(Duration::from_secs(600))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        Self::check(r).await.map(|_| ())
    }

    /// `PATCH /lume/vms/:name` (`lume set`). Growing a macOS disk can take
    /// minutes, hence the long timeout.
    pub async fn set(&self, name: &str, req: &SetVm) -> Result<()> {
        let r = self
            .http
            .patch(self.url(&format!("/lume/vms/{name}")))
            .json(req)
            .timeout(Duration::from_secs(1800))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        Self::check(r).await.map(|_| ())
    }

    pub async fn run(&self, name: &str, req: &RunVm) -> Result<()> {
        let r = self
            .http
            .post(self.url(&format!("/lume/vms/{name}/run")))
            .json(req)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        Self::check(r).await.map(|_| ())
    }

    pub async fn stop(&self, name: &str) -> Result<()> {
        self.stop_with(name, 4, Duration::from_secs(120)).await
    }

    /// The stop before a delete: the user already chose to delete, so a
    /// hung `lume serve` gets two short tries (about a minute at worst)
    /// instead of [`Self::stop`]'s eight minutes. Lume's own delete stops a
    /// VM it still runs.
    pub async fn stop_for_delete(&self, name: &str) -> Result<()> {
        self.stop_with(name, 2, Duration::from_secs(30)).await
    }

    async fn stop_with(&self, name: &str, attempts: u32, timeout: Duration) -> Result<()> {
        // lume serve occasionally drops the connection on the first stop
        // right after boot; the request is idempotent, so retry a few times.
        let mut last = None;
        for attempt in 0..attempts {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            match self
                .http
                .post(self.url(&format!("/lume/vms/{name}/stop")))
                .json(&json!({}))
                .timeout(timeout)
                .send()
                .await
            {
                Ok(r) => return Self::check(r).await.map(|_| ()),
                Err(e) if e.is_connect() => return Err(self.net_err(e)),
                Err(e) => {
                    tracing::debug!(vm = name, attempt, "lume stop: {}", self.net_err_ref(&e));
                    last = Some(e);
                }
            }
            if let Ok(Some(vm)) = self.get(name).await
                && vm.status == "stopped"
            {
                return Ok(());
            }
        }
        Err(self.net_err(last.expect("retried")))
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        let r = self
            .http
            .delete(self.url(&format!("/lume/vms/{name}")))
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        Self::check(r).await?;
        super::lease::remove_resize_guards(&crate::host::home_dir(), name);
        Ok(())
    }

    /// `GET /lume/config/locations` → `(name, path)` with `~` expanded.
    pub async fn locations(&self) -> Result<Vec<(String, std::path::PathBuf)>> {
        let r = self
            .http
            .get(self.url("/lume/config/locations"))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        let v: Value = Self::check(r)
            .await?
            .json()
            .await
            .map_err(|e| self.net_err(e))?;
        let home = crate::host::home_dir();
        Ok(v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| {
                let name = l.get("name")?.as_str()?.to_string();
                let path = l.get("path")?.as_str()?;
                let path = match path.strip_prefix("~/") {
                    Some(rest) => home.join(rest),
                    None => std::path::PathBuf::from(path),
                };
                Some((name, path))
            })
            .collect())
    }

    /// Pull an image into a stopped VM named `name`. Uses the async
    /// `/lume/pull/start` + polling where available and falls back to the
    /// blocking `/lume/pull` (older lume). Retries lume's spurious
    /// "Invalid request body" 400 on fresh connections.
    pub async fn pull(&self, reference: &str, name: &str, timeout: Duration) -> Result<()> {
        // Another create in this or another process is pulling it already:
        // wait for that pull instead of starting a second one.
        if let Ok(Some(vm)) = self.get(name).await
            && vm.status == "pulling"
        {
            return self.wait_pulled(name, timeout).await;
        }
        let body = pull_body(reference, name);
        let mut start = None;
        for attempt in 0..3 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            match self
                .http
                .post(self.url("/lume/pull/start"))
                .json(&body)
                .send()
                .await
            {
                Ok(r) if r.status() == 400 => {
                    let text = r.text().await.unwrap_or_default();
                    if text.contains("Invalid request body") {
                        continue;
                    }
                    return Err(VmmError::Lume {
                        status: 400,
                        message: text,
                    });
                }
                Ok(r) => {
                    start = Some(r);
                    break;
                }
                Err(e) if e.is_connect() => return Err(self.net_err(e)),
                Err(_) => break, // older lume drops the connection: fall back
            }
        }
        match start {
            Some(r) if r.status() == 404 => {}
            Some(r) => {
                Self::check(r).await?;
                return self.wait_pulled(name, timeout).await;
            }
            None => {}
        }
        let r = self
            .http
            .post(self.url("/lume/pull"))
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        Self::check(r).await.map(|_| ())
    }

    /// Waits for the async pull of `name` to end, reporting its progress.
    /// It fails after `idle` without movement (a new percent or new bytes),
    /// never because a slow download that still moves takes long. Dropping
    /// the wait (a cancelled create) cancels the pull on a Lume that can
    /// ([`LumeClient::cancel_pull`]), once no other wait in this process
    /// still needs it.
    async fn wait_pulled(&self, name: &str, idle: Duration) -> Result<()> {
        let mut guard = PullGuard::new(self.clone(), name);
        let mut moved_at = tokio::time::Instant::now();
        let mut last: Option<(Option<f64>, Option<u64>)> = None;
        let mut meter = crate::progress::Meter::new();
        loop {
            // A Lume that counts bytes shows a stall much sooner.
            let budget = if guard.cancellable {
                idle.min(PULL_IDLE_WITH_BYTES)
            } else {
                idle
            };
            if moved_at.elapsed() >= budget {
                return Err(VmmError::Timeout {
                    name: name.into(),
                    secs: budget.as_secs(),
                    detail: "lume pull made no progress".into(),
                });
            }
            match self.get(name).await {
                Ok(Some(vm)) => {
                    let now = (vm.download_progress, vm.downloaded_bytes);
                    if last != Some(now) {
                        moved_at = tokio::time::Instant::now();
                        last = Some(now);
                    }
                    if vm.downloaded_bytes.is_some() {
                        guard.cancellable = true;
                    }
                    match vm.status.as_str() {
                        "pulling" | "provisioning" => {
                            if let Some(p) = pull_progress(&vm, &mut meter) {
                                crate::progress::report(p);
                            }
                        }
                        s if s.to_ascii_lowercase().contains("error") => {
                            guard.done();
                            return Err(VmmError::Lume {
                                status: 500,
                                message: format!("pull of '{name}' failed: {s}"),
                            });
                        }
                        _ => {
                            guard.done();
                            return Ok(());
                        }
                    }
                }
                // Lume answers a failed async pull with a 400 naming it.
                Err(VmmError::Lume {
                    status: 400,
                    message,
                }) if message.contains("Pull failed") => {
                    guard.done();
                    return Err(VmmError::Lume {
                        status: 500,
                        message,
                    });
                }
                _ => {}
            }
            tokio::time::sleep(PULL_POLL).await;
        }
    }

    /// `POST /lume/pull/cancel`: stops the async pull of `name`; Lume
    /// removes its partial files and keeps the layers it finished, so a
    /// later pull resumes. `Ok(false)` when nothing was pulling.
    pub async fn cancel_pull(&self, name: &str) -> Result<bool> {
        let r = self
            .http
            .post(self.url("/lume/pull/cancel"))
            .json(&json!({ "name": name }))
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| self.net_err(e))?;
        match Self::check(r).await {
            Ok(_) => Ok(true),
            Err(VmmError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

/// How often an async pull is polled (Lume answers from memory).
const PULL_POLL: Duration = Duration::from_millis(250);
/// How long a pull whose bytes Lume counts may go without a new byte.
const PULL_IDLE_WITH_BYTES: Duration = Duration::from_secs(10 * 60);

/// The progress report for a VM that is pulling: bytes when Lume counts
/// them (through `meter`, a few a second), else its percent.
fn pull_progress(
    vm: &VmDetails,
    meter: &mut crate::progress::Meter,
) -> Option<crate::progress::Progress> {
    use crate::progress::{Phase, Progress};
    // lume reports percent (0-100).
    let fraction = vm
        .download_progress
        .map(|p| if p > 1.0 { p / 100.0 } else { p });
    match (vm.downloaded_bytes, vm.total_bytes.filter(|t| *t > 0)) {
        (Some(done), Some(total)) => {
            let mut t = meter.sample(done, total)?;
            // Lume's own rate, when it has one, is over its whole transfer.
            if let Some(r) = vm.bytes_per_second.filter(|r| r.is_finite() && *r > 0.0) {
                t.per_second = Some(r);
            }
            Some(
                Progress::phase(Phase::Pulling)
                    .fraction(done as f64 / total as f64)
                    .bytes(t),
            )
        }
        _ => fraction.map(|f| Progress::phase(Phase::Pulling).fraction(f)),
    }
}

/// Waits on a pull in this process, per VM name: a cancelled wait cancels
/// Lume's pull only when it was the last one waiting.
static PULL_WAITERS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Cancels the pull when the wait is dropped before it ended.
struct PullGuard {
    client: LumeClient,
    name: String,
    /// The Lume reports bytes, so it has `POST /lume/pull/cancel`.
    cancellable: bool,
    finished: bool,
}

impl PullGuard {
    fn new(client: LumeClient, name: &str) -> Self {
        PULL_WAITERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(name.to_string());
        Self {
            client,
            name: name.to_string(),
            cancellable: false,
            finished: false,
        }
    }

    fn done(&mut self) {
        self.finished = true;
    }
}

impl Drop for PullGuard {
    fn drop(&mut self) {
        let last = {
            let mut w = PULL_WAITERS.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = w.iter().position(|n| n == &self.name) {
                w.remove(i);
            }
            !w.contains(&self.name)
        };
        if self.finished || !last {
            return;
        }
        if !self.cancellable {
            tracing::info!(
                vm = %self.name,
                "this lume cannot cancel a pull; it finishes in the background as a reusable base"
            );
            return;
        }
        let client = self.client.clone();
        let name = self.name.clone();
        crate::cleanup::spawn(async move {
            match client.cancel_pull(&name).await {
                Ok(true) => {
                    // Nothing was made: the SDK's record of the base goes too.
                    super::owned::OwnedVms::default().forget(&name);
                    tracing::info!(vm = %name, "cancelled the lume pull")
                }
                Ok(false) => {}
                Err(e) => tracing::warn!(vm = %name, error = %e, "could not cancel the lume pull"),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp CUA_HOME for this test binary (a cancelled pull forgets its
    /// owned-VM record there, never under the real `~/.cua`).
    fn temp_home() {
        static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        HOME.get_or_init(|| {
            let d = tempfile::tempdir().unwrap();
            // SAFETY: set once, before any test here reads CUA_HOME.
            unsafe {
                std::env::set_var("CUA_HOME", d.path().join(".cua"));
            }
            d
        });
    }

    /// A scripted `lume serve`: each `GET /lume/vms/<vm>` answers the next
    /// body of `gets` (the last one repeats); records every request line.
    async fn scripted_lume(
        gets: Vec<(u16, String)>,
    ) -> (LumeClient, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        temp_home();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        let gets = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from(
            gets,
        )));
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let log = log.clone();
                let gets = gets.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let line = req.lines().next().unwrap_or("").to_string();
                    log.lock().unwrap().push(line.clone());
                    let (status, body) = if line.starts_with("GET /lume/vms/") {
                        let mut g = gets.lock().unwrap();
                        if g.len() > 1 {
                            g.pop_front().unwrap()
                        } else {
                            g.front().cloned().unwrap()
                        }
                    } else if line.starts_with("POST /lume/pull/cancel") {
                        (200, r#"{"message":"Pull cancelled"}"#.to_string())
                    } else {
                        (202, "{}".to_string())
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (LumeClient::new(url), seen)
    }

    fn pulling(pct: f64, bytes: Option<(u64, u64)>) -> (u16, String) {
        let extra = bytes
            .map(|(d, t)| {
                format!(r#","downloadedBytes":{d},"totalBytes":{t},"bytesPerSecond":1000.0"#)
            })
            .unwrap_or_default();
        (
            200,
            format!(r#"{{"name":"b","status":"pulling","downloadProgress":{pct}{extra}}}"#),
        )
    }

    fn stopped() -> (u16, String) {
        (200, r#"{"name":"b","status":"stopped"}"#.to_string())
    }

    fn heard() -> (
        crate::progress::Sink,
        std::sync::Arc<std::sync::Mutex<Vec<crate::progress::Progress>>>,
    ) {
        let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let h = heard.clone();
        (
            std::sync::Arc::new(move |p: &crate::progress::Progress| {
                h.lock().unwrap().push(p.clone())
            }),
            heard,
        )
    }

    #[tokio::test]
    async fn a_pull_reports_lumes_bytes() {
        let mut gets = vec![];
        for i in 0..6u64 {
            gets.push(pulling(i as f64 * 10.0, Some((i * 1_000_000, 10_000_000))));
        }
        gets.push(stopped());
        let (client, _) = scripted_lume(gets).await;
        let (sink, heard) = heard();
        crate::progress::scope(sink, client.wait_pulled("b1", Duration::from_secs(30)))
            .await
            .unwrap();
        let bytes: Vec<u64> = heard
            .lock()
            .unwrap()
            .iter()
            .filter_map(|p| p.bytes.map(|b| b.done))
            .collect();
        // 250 ms apart: every sample passes the meter's throttle.
        assert_eq!(
            bytes,
            [0, 1_000_000, 2_000_000, 3_000_000, 4_000_000, 5_000_000]
        );
        let last = heard.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last.bytes.unwrap().total, 10_000_000);
        assert_eq!(last.bytes.unwrap().per_second, Some(1000.0));
        assert_eq!(last.fraction, Some(0.5));
    }

    #[tokio::test]
    async fn a_dropped_wait_cancels_a_lume_that_can_cancel() {
        for (vm, bytes, cancels) in [("b2a", Some((5u64, 10u64)), 1), ("b2b", None, 0)] {
            let (client, seen) = scripted_lume(vec![pulling(50.0, bytes)]).await;
            let wait = client.wait_pulled(vm, Duration::from_secs(30));
            // Cut off mid-pull, as a cancelled create drops its future.
            let _ = tokio::time::timeout(Duration::from_millis(600), wait).await;
            assert!(crate::cleanup::settle(Duration::from_secs(5)).await);
            let n = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("POST /lume/pull/cancel"))
                .count();
            assert_eq!(n, cancels, "bytes {bytes:?}: {:?}", seen.lock().unwrap());
        }
    }

    #[tokio::test]
    async fn a_cancelled_pull_forgets_the_bases_record() {
        let (client, _) = scripted_lume(vec![pulling(50.0, Some((5, 10)))]).await;
        let owned = super::super::owned::OwnedVms::default();
        owned.mark("b5", super::super::owned::OwnedKind::Base, Some("r"));
        let _ = tokio::time::timeout(
            Duration::from_millis(600),
            client.wait_pulled("b5", Duration::from_secs(30)),
        )
        .await;
        assert!(crate::cleanup::settle(Duration::from_secs(5)).await);
        assert!(
            !owned.list().iter().any(|r| r.name == "b5"),
            "nothing was made, so no record is left"
        );
    }

    #[tokio::test]
    async fn a_finished_or_shared_wait_does_not_cancel() {
        let (client, seen) = scripted_lume(vec![pulling(50.0, Some((5, 10))), stopped()]).await;
        client
            .wait_pulled("b3a", Duration::from_secs(30))
            .await
            .unwrap();
        // Two waits on one pull: dropping one leaves the other's pull alone.
        let (client, seen2) = scripted_lume(vec![pulling(50.0, Some((5, 10)))]).await;
        let other = tokio::spawn({
            let c = client.clone();
            async move { c.wait_pulled("b3b", Duration::from_secs(30)).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = tokio::time::timeout(
            Duration::from_millis(600),
            client.wait_pulled("b3b", Duration::from_secs(30)),
        )
        .await;
        assert!(crate::cleanup::settle(Duration::from_secs(5)).await);
        other.abort();
        let _ = other.await;
        assert!(crate::cleanup::settle(Duration::from_secs(5)).await);
        let cancels = |s: &std::sync::Mutex<Vec<String>>| {
            s.lock()
                .unwrap()
                .iter()
                .filter(|l| l.starts_with("POST /lume/pull/cancel"))
                .count()
        };
        assert_eq!(cancels(&seen), 0, "a finished pull is not cancelled");
        assert_eq!(cancels(&seen2), 1, "only the last wait cancels");
    }

    #[tokio::test]
    async fn a_pull_fails_on_no_movement_and_on_lumes_error() {
        let (client, _) = scripted_lume(vec![pulling(10.0, Some((5, 10)))]).await;
        let err = client
            .wait_pulled("b4a", Duration::from_millis(700))
            .await
            .unwrap_err();
        assert!(matches!(err, VmmError::Timeout { .. }), "{err}");
        // Slow but moving: a new byte every poll keeps it alive past the
        // idle budget.
        let gets = (0..8u64)
            .map(|i| pulling(10.0, Some((i, 100))))
            .chain([stopped()])
            .collect();
        let (client, _) = scripted_lume(gets).await;
        client
            .wait_pulled("b4b", Duration::from_millis(700))
            .await
            .expect("a moving pull outlives the idle budget");
        let (client, _) = scripted_lume(vec![(
            400,
            r#"{"message":"Pull failed for 'b': disk full"}"#.to_string(),
        )])
        .await;
        let err = client
            .wait_pulled("b4c", Duration::from_secs(30))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("disk full"), "{err}");
    }

    #[test]
    fn pull_body_splits_registry_and_org() {
        let b = pull_body("ghcr.io/trycua/macos-sequoia-cua:latest", "cua-base-x");
        assert_eq!(b["registry"], "ghcr.io");
        assert_eq!(b["organization"], "trycua");
        assert_eq!(b["image"], "macos-sequoia-cua:latest");
        let b = pull_body("trycua/macos:latest", "n");
        assert_eq!(b["organization"], "trycua");
        assert!(b.get("registry").is_none());
        assert_eq!(pull_body("macos:1", "n")["image"], "macos:1");
    }

    #[test]
    fn parses_vnc_urls() {
        assert_eq!(
            parse_vnc_url("vnc://:s3cret@127.0.0.1:59001"),
            Some(("127.0.0.1".into(), 59001, Some("s3cret".into())))
        );
        assert_eq!(
            parse_vnc_url("vnc://localhost:5900"),
            Some(("localhost".into(), 5900, None))
        );
        assert_eq!(parse_vnc_url("http://x"), None);
    }

    #[test]
    fn vm_details_decode_and_ip_filtering() {
        let v: VmDetails = serde_json::from_str(
            r#"{"name":"a","os":"linux","cpuCount":2,"memorySize":4294967296,"diskSize":{"allocated":1,"total":2},
                "display":"1024x768","status":"running","vncUrl":"vnc://:p@127.0.0.1:5901","ipAddress":"unknown",
                "locationName":"home","sshAvailable":false}"#,
        )
        .unwrap();
        assert_eq!(v.ip(), None);
        let v = VmDetails {
            ip_address: Some("192.168.64.5".into()),
            ..v
        };
        assert_eq!(v.ip(), Some("192.168.64.5"));
    }
}
