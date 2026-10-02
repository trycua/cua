//! `SpacesdClient`: the env module (cua-spacesd, `cua.env.v1`).

use super::run;
use crate::json;
use crate::types::*;
use crate::{CuaError, Result};
use cua_spacesd_client::pb;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

impl SpacesdCommand {
    fn to_env(&self) -> cua_spacesd_client::Command {
        let mut c = cua_spacesd_client::Command::new(self.program.clone()).args(self.args.clone());
        for (k, v) in &self.env {
            c = c.env(k.clone(), v.clone());
        }
        if let Some(d) = &self.cwd {
            c = c.cwd(d.clone());
        }
        if let Some(u) = &self.user {
            c = c.user(u.clone());
        }
        if let Some(t) = self.timeout_ms {
            c = c.timeout(super::millis(t));
        }
        if let Some(t) = &self.tag {
            c = c.tag(t.clone());
        }
        c = c.stdin(self.stdin);
        if let Some(p) = self.pty {
            c = c.pty(p.cols, p.rows);
        }
        c
    }
}

impl From<cua_spacesd_client::ExitStatus> for ExitInfo {
    fn from(s: cua_spacesd_client::ExitStatus) -> Self {
        ExitInfo {
            success: s.success(),
            code: s.code,
            signal: s.signal.map(|x| enum_suffix(format!("{x:?}"))),
            timed_out: s.timed_out,
            error: s.error,
        }
    }
}

impl From<cua_spacesd_client::Output> for ProcessOutput {
    fn from(o: cua_spacesd_client::Output) -> Self {
        ProcessOutput {
            exit: o.status.into(),
            stdout: o.stdout,
            stderr: o.stderr,
            pty: o.pty,
        }
    }
}

impl From<cua_spacesd_client::ProcessEvent> for ProcessEvent {
    fn from(e: cua_spacesd_client::ProcessEvent) -> Self {
        use cua_spacesd_client::ProcessEvent as E;
        let (kind, offset, data, exit) = match e {
            E::Stdout { offset, data } => (ProcessEventKind::Stdout, offset, data.to_vec(), None),
            E::Stderr { offset, data } => (ProcessEventKind::Stderr, offset, data.to_vec(), None),
            E::Pty { offset, data } => (ProcessEventKind::Pty, offset, data.to_vec(), None),
            E::Exit(s) => (ProcessEventKind::Exit, 0, vec![], Some(s.into())),
        };
        ProcessEvent {
            kind,
            offset,
            data,
            exit,
        }
    }
}

/// Scrollback replay when attaching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ReplayMode {
    /// Live output only.
    None,
    /// Everything retained.
    All,
    /// The last `n` bytes.
    LastBytes {
        /// Byte count.
        n: u64,
    },
    /// From this combined-output offset.
    FromOffset {
        /// Offset.
        offset: u64,
    },
}

/// A guest process, from `spawn` or `attach`.
#[derive(uniffi::Object)]
pub struct SpacesdProcess {
    client: cua_spacesd_client::SpacesdClient,
    pid: u32,
    tag: Option<String>,
    handle: Arc<tokio::sync::Mutex<Option<cua_spacesd_client::ProcessHandle>>>,
    writer_id: String,
    sequence: AtomicU64,
}

impl SpacesdProcess {
    fn new(
        client: cua_spacesd_client::SpacesdClient,
        h: cua_spacesd_client::ProcessHandle,
    ) -> Self {
        SpacesdProcess {
            client,
            pid: h.pid(),
            tag: h.tag().map(str::to_string),
            handle: Arc::new(tokio::sync::Mutex::new(Some(h))),
            writer_id: format!("cua-sdk-{:016x}", rand_u64()),
            sequence: AtomicU64::new(0),
        }
    }

    fn selector(&self) -> pb::ProcessSelector {
        pb::ProcessSelector {
            selector: Some(match &self.tag {
                Some(t) => pb::process_selector::Selector::Tag(t.clone()),
                None => pb::process_selector::Selector::Pid(self.pid),
            }),
        }
    }
}

fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    h.finish()
}

fn parse_signal(name: &str) -> Result<pb::Signal> {
    let upper = name.trim().trim_start_matches("SIG").to_ascii_uppercase();
    let upper = upper.trim_start_matches("SIGNAL_");
    pb::Signal::from_str_name(&format!("SIGNAL_{upper}"))
        .filter(|s| *s != pb::Signal::Unspecified)
        .ok_or_else(|| CuaError::InvalidArgument(format!("unknown signal {name:?}")))
}

#[uniffi::export]
impl SpacesdProcess {
    /// Guest pid.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Tag, when set.
    pub fn tag(&self) -> Option<String> {
        self.tag.clone()
    }

    /// Next output chunk or the exit; `None` after the exit was returned.
    /// Reconnects transparently from the last seen offset.
    pub async fn next_event(&self) -> Result<Option<ProcessEvent>> {
        let handle = self.handle.clone();
        run(async move {
            let mut guard = handle.lock().await;
            let Some(h) = guard.as_mut() else {
                return Ok(None);
            };
            Ok(h.next_event().await?.map(Into::into))
        })
        .await
    }

    /// Collects the remaining output until exit.
    pub async fn wait(&self) -> Result<ProcessOutput> {
        let handle = self.handle.clone();
        run(async move {
            let h = handle
                .lock()
                .await
                .take()
                .ok_or_else(|| CuaError::Closed("process output already consumed".into()))?;
            Ok(h.wait().await?.into())
        })
        .await
    }

    /// Writes stdin (PTY input for PTY processes), exactly once across
    /// retries.
    pub async fn write_stdin(&self, data: Vec<u8>) -> Result<()> {
        self.send_input(pb::process_input::Input::Stdin(data)).await
    }

    /// Writes PTY keystrokes.
    pub async fn write_pty(&self, data: Vec<u8>) -> Result<()> {
        self.send_input(pb::process_input::Input::Pty(data)).await
    }

    /// Closes stdin.
    pub async fn close_stdin(&self) -> Result<()> {
        let (client, process) = (self.client.clone(), Some(self.selector()));
        run(async move {
            client
                .process()
                .close_stdin(pb::CloseStdinRequest { process })
                .await?;
            Ok(())
        })
        .await
    }

    /// Sends a signal by name (`term`, `kill`, `SIGINT`, ...).
    pub async fn signal(&self, signal: String) -> Result<()> {
        let sig = parse_signal(&signal)?;
        let (client, process) = (self.client.clone(), Some(self.selector()));
        run(async move {
            client
                .process()
                .signal_process(pb::SignalProcessRequest {
                    process,
                    signal: sig as i32,
                    process_group: false,
                })
                .await?;
            Ok(())
        })
        .await
    }

    /// SIGKILL.
    pub async fn kill(&self) -> Result<()> {
        self.signal("kill".into()).await
    }

    /// Resizes the PTY.
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        let (client, process) = (self.client.clone(), Some(self.selector()));
        run(async move {
            client
                .process()
                .resize_pty(pb::ResizePtyRequest {
                    process,
                    size: Some(pb::PtySize {
                        cols,
                        rows,
                        pixel_width: 0,
                        pixel_height: 0,
                    }),
                })
                .await?;
            Ok(())
        })
        .await
    }

    /// Stops receiving output without affecting the process.
    pub async fn detach(&self) {
        let handle = self.handle.clone();
        let _ = run(async move {
            if let Some(h) = handle.lock().await.take() {
                h.detach();
            }
            Ok(())
        })
        .await;
    }
}

impl SpacesdProcess {
    async fn send_input(&self, input: pb::process_input::Input) -> Result<()> {
        let seq = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let req = pb::SendInputRequest {
            process: Some(self.selector()),
            input: Some(pb::ProcessInput { input: Some(input) }),
            sequence: seq,
            writer_id: self.writer_id.clone(),
        };
        let client = self.client.clone();
        run(async move {
            client
                .retry_policy()
                .run(|_| {
                    let req = req.clone();
                    let client = client.clone();
                    async move {
                        client.process().send_input(req).await?;
                        Ok(())
                    }
                })
                .await?;
            Ok(())
        })
        .await
    }
}

impl UploadOptions {
    fn to_env(&self) -> cua_spacesd_client::UploadOptions {
        cua_spacesd_client::UploadOptions {
            mode: if self.create_new {
                pb::WriteMode::CreateNew
            } else if self.append {
                pb::WriteMode::Append
            } else {
                pb::WriteMode::Overwrite
            },
            permissions: self.permissions,
            create_parents: self.create_parents,
            ..Default::default()
        }
    }
}

/// One HTTP header (duplicates are kept in order).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesdHttpHeader {
    /// Header name.
    pub name: String,
    /// Header value.
    pub value: String,
}

/// A plain-HTTP request to the spacesd port (for example MCP at `/mcp`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesdHttpRequest {
    /// Method, for example `POST`.
    pub method: String,
    /// Absolute path under the endpoint, for example `/mcp`. Never a URL.
    pub path: String,
    /// Extra headers. Credentials (`authorization`, the env and Fleet claim
    /// headers) are the client's and cannot be set here.
    #[uniffi(default = [])]
    pub headers: Vec<SpacesdHttpHeader>,
    /// Request body (empty for none).
    pub body: Vec<u8>,
    /// Whole-request timeout; 30 s when unset.
    #[uniffi(default = None)]
    pub timeout_ms: Option<u32>,
    /// Response body cap; 16 MiB when unset.
    #[uniffi(default = None)]
    pub max_response_bytes: Option<u64>,
}

/// The spacesd's HTTP answer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesdHttpResponse {
    /// Status code.
    pub status: u16,
    /// Response headers, in order.
    pub headers: Vec<SpacesdHttpHeader>,
    /// Response body.
    pub body: Vec<u8>,
}

/// Re-mints the WebSocket upgrade headers when a socket is opened. Fleet
/// access tokens are short-lived (about five minutes), so a bearer taken when
/// the client was created makes the gateway refuse (302) every media socket
/// opened later on a long-lived client.
pub(crate) type WsHeaderRefresh = Arc<
    dyn Fn() -> futures_util::future::BoxFuture<'static, Result<Vec<(String, String)>>>
        + Send
        + Sync,
>;

/// Client for one cua-spacesd.
#[derive(uniffi::Object)]
pub struct SpacesdClient {
    pub(crate) client: cua_spacesd_client::SpacesdClient,
    /// Extra headers for WebSocket upgrades on the same route, as of
    /// creation.
    #[cfg_attr(not(feature = "media"), allow(dead_code))]
    pub(crate) ws_headers: Vec<(String, String)>,
    /// Fresh headers at use (cloud Spaces); `None` when they never expire.
    #[cfg_attr(not(feature = "media"), allow(dead_code))]
    pub(crate) ws_refresh: Option<WsHeaderRefresh>,
}

impl SpacesdClient {
    pub(crate) fn new(
        client: cua_spacesd_client::SpacesdClient,
        ws_headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            client,
            ws_headers,
            ws_refresh: None,
        }
    }

    /// A client whose WebSocket headers are re-minted by `refresh` each time
    /// a socket is opened (a cloud Space's Fleet bearer).
    pub(crate) fn with_ws_refresh(mut self, refresh: WsHeaderRefresh) -> Self {
        self.ws_refresh = Some(refresh);
        self
    }

    /// Rust hosts: headers WebSocket upgrades on this route need, as of
    /// creation. Prefer [`SpacesdClient::current_ws_headers`] for a socket
    /// opened later: a cloud Space's bearer expires within minutes.
    pub fn ws_headers(&self) -> Vec<(String, String)> {
        self.ws_headers.clone()
    }

    /// Rust hosts: headers a WebSocket upgrade opened now needs, with a
    /// fresh Fleet bearer for a cloud Space.
    pub async fn current_ws_headers(&self) -> Result<Vec<(String, String)>> {
        match &self.ws_refresh {
            // On the SDK runtime: the refresh may do Fleet I/O.
            Some(refresh) => run(refresh()).await,
            None => Ok(self.ws_headers.clone()),
        }
    }

    /// Rust hosts: the underlying `cua_spacesd_client` client.
    pub fn inner(&self) -> &cua_spacesd_client::SpacesdClient {
        &self.client
    }
}

macro_rules! env_call {
    ($self:ident, |$c:ident| $body:expr) => {{
        let $c = $self.client.clone();
        run(async move { $body }).await
    }};
}

#[uniffi::export]
impl SpacesdClient {
    /// Endpoint URL.
    pub fn endpoint(&self) -> String {
        self.client.endpoint().to_string()
    }

    /// `grpc` or `grpc-web`.
    pub fn transport(&self) -> String {
        match self.client.transport() {
            cua_spacesd_client::Transport::Native => "grpc".into(),
            cua_spacesd_client::Transport::GrpcWeb => "grpc-web".into(),
        }
    }

    /// `GetCapabilities` (cached).
    pub async fn capabilities(&self) -> Result<SpacesdCapabilities> {
        env_call!(self, |c| Ok(c.capabilities().await?.into()))
    }

    /// Whether the guest supports `feature`.
    pub async fn has_feature(&self, feature: String) -> Result<bool> {
        env_call!(self, |c| Ok(c.has_feature(&feature).await?))
    }

    /// `Health` as proto3 JSON.
    pub async fn health(&self) -> Result<String> {
        env_call!(self, |c| Ok(serde_json::to_string(&c.health().await?)?))
    }

    /// Runs the image self-test (`SystemService.Diagnose`, the checks of
    /// `cua-spacesd doctor`) and returns the report as JSON (schema version
    /// 1, `libs/cua/proto/diagnose-report.schema.json`). `options_json` is a
    /// proto3-JSON `DiagnoseOptions` (`"{}"` for the defaults: every check,
    /// read-only). Falls back to `DiagnoseOnce` when the stream is cut.
    pub async fn diagnose(&self, options_json: String) -> Result<String> {
        let options: cua_proto::env::v1::DiagnoseOptions =
            serde_json::from_str(if options_json.trim().is_empty() {
                "{}"
            } else {
                &options_json
            })
            .map_err(|e| crate::CuaError::InvalidArgument(format!("DiagnoseOptions: {e}")))?;
        env_call!(self, |c| Ok(c
            .diagnose_report(options, |_| {})
            .await?
            .to_json()))
    }

    // ------------------------------------------------------------ process

    /// Runs a command to completion.
    pub async fn run(&self, command: SpacesdCommand) -> Result<ProcessOutput> {
        let cmd = command.to_env();
        env_call!(self, |c| Ok(c.run(cmd).await?.into()))
    }

    /// Runs `line` with `/bin/sh -c` to completion.
    pub async fn sh(&self, line: String, timeout_ms: Option<u32>) -> Result<ProcessOutput> {
        let mut cmd = cua_spacesd_client::Command::shell(line);
        if let Some(t) = timeout_ms {
            cmd = cmd.timeout(super::millis(t));
        }
        env_call!(self, |c| Ok(c.run(cmd).await?.into()))
    }

    /// Starts a command and returns its handle.
    pub async fn spawn(&self, command: SpacesdCommand) -> Result<Arc<SpacesdProcess>> {
        let cmd = command.to_env();
        env_call!(self, |c| {
            let h = c.spawn(cmd).await?;
            Ok(Arc::new(SpacesdProcess::new(c, h)))
        })
    }

    /// Attaches to a running process by pid or tag.
    pub async fn attach(
        &self,
        pid: Option<u32>,
        tag: Option<String>,
        replay: ReplayMode,
    ) -> Result<Arc<SpacesdProcess>> {
        let sel = match (pid, tag) {
            (Some(p), None) => cua_spacesd_client::ProcessRef::Pid(p),
            (None, Some(t)) => cua_spacesd_client::ProcessRef::Tag(t),
            _ => {
                return Err(CuaError::InvalidArgument(
                    "attach needs exactly one of pid or tag".into(),
                ));
            }
        };
        let replay = match replay {
            ReplayMode::None => cua_spacesd_client::Replay::None,
            ReplayMode::All => cua_spacesd_client::Replay::All,
            ReplayMode::LastBytes { n } => cua_spacesd_client::Replay::LastBytes(n),
            ReplayMode::FromOffset { offset } => cua_spacesd_client::Replay::FromOffset(offset),
        };
        env_call!(self, |c| {
            let h = c.attach(sel, replay).await?;
            Ok(Arc::new(SpacesdProcess::new(c, h)))
        })
    }

    /// Guest processes as proto3 JSON (`[ProcessInfo]`).
    pub async fn list_processes(&self, include_exited: bool) -> Result<String> {
        env_call!(self, |c| {
            Ok(serde_json::to_string(
                &c.list_processes(include_exited).await?,
            )?)
        })
    }

    // --------------------------------------------------------- filesystem

    /// Uploads bytes to `path` (chunked, resumable, SHA-256 verified).
    pub async fn upload(
        &self,
        path: String,
        data: Vec<u8>,
        options: Option<UploadOptions>,
    ) -> Result<TransferResult> {
        let o = options.unwrap_or_default().to_env();
        env_call!(self, |c| {
            let r = c.upload(&path, data, o).await?;
            Ok(TransferResult {
                size: r.size,
                sha256: r.sha256,
                resumes: r.resumes,
            })
        })
    }

    /// Uploads a local file to `path`.
    pub async fn upload_file(
        &self,
        local_path: String,
        path: String,
        options: Option<UploadOptions>,
    ) -> Result<TransferResult> {
        let o = options.unwrap_or_default().to_env();
        env_call!(self, |c| {
            let r = c.upload(&path, PathBuf::from(local_path), o).await?;
            Ok(TransferResult {
                size: r.size,
                sha256: r.sha256,
                resumes: r.resumes,
            })
        })
    }

    /// Downloads `path` into memory.
    pub async fn download(&self, path: String) -> Result<Vec<u8>> {
        env_call!(self, |c| Ok(c.download(&path).await?.to_vec()))
    }

    /// Downloads `path` to a local file.
    pub async fn download_file(&self, path: String, local_path: String) -> Result<TransferResult> {
        env_call!(self, |c| {
            let r = c
                .download_to_file(&path, std::path::Path::new(&local_path), Default::default())
                .await?;
            Ok(TransferResult {
                size: r.size,
                sha256: r.sha256,
                resumes: r.resumes,
            })
        })
    }

    /// `Stat`.
    pub async fn stat(&self, path: String) -> Result<FileEntry> {
        env_call!(self, |c| Ok(c.stat(&path).await?.into()))
    }

    /// `ListDir` (depth 1 = direct children).
    pub async fn list_dir(&self, path: String, depth: u32) -> Result<Vec<FileEntry>> {
        env_call!(self, |c| Ok(c
            .list_dir(&path, depth)
            .await?
            .into_iter()
            .map(Into::into)
            .collect()))
    }

    /// `MakeDir` (with parents).
    pub async fn make_dir(&self, path: String) -> Result<FileEntry> {
        env_call!(self, |c| Ok(c.make_dir(&path).await?.into()))
    }

    /// `Remove`.
    pub async fn remove(&self, path: String, recursive: bool) -> Result<()> {
        env_call!(self, |c| Ok(c.remove(&path, recursive).await?))
    }

    // ------------------------------------------------------------ computer

    /// Captures a display.
    pub async fn screenshot(&self, options: Option<ScreenshotOptions>) -> Result<Screenshot> {
        let opts = screenshot_request(options);
        env_call!(self, |c| Ok(screenshot_reply(c.screenshot(opts).await?)))
    }

    /// Left click.
    pub async fn click(&self, x: f64, y: f64) -> Result<()> {
        env_call!(self, |c| {
            c.click(x, y).await?;
            Ok(())
        })
    }

    /// Double click.
    pub async fn double_click(&self, x: f64, y: f64) -> Result<()> {
        env_call!(self, |c| {
            c.double_click(x, y).await?;
            Ok(())
        })
    }

    /// Right click.
    pub async fn right_click(&self, x: f64, y: f64) -> Result<()> {
        env_call!(self, |c| {
            c.right_click(x, y).await?;
            Ok(())
        })
    }

    /// Moves the pointer.
    pub async fn move_to(&self, x: f64, y: f64) -> Result<()> {
        env_call!(self, |c| {
            c.move_to(x, y).await?;
            Ok(())
        })
    }

    /// Scrolls by line deltas.
    pub async fn scroll(&self, dx: f64, dy: f64) -> Result<()> {
        env_call!(self, |c| {
            c.scroll(dx, dy).await?;
            Ok(())
        })
    }

    /// Drags with the left button.
    pub async fn drag(&self, from_x: f64, from_y: f64, to_x: f64, to_y: f64) -> Result<()> {
        env_call!(self, |c| {
            c.drag((from_x, from_y), (to_x, to_y)).await?;
            Ok(())
        })
    }

    /// Full `ComputerService.Pointer` from proto3 JSON.
    pub async fn pointer_json(&self, request_json: String) -> Result<String> {
        self.call_json("/cua.env.v1.ComputerService/Pointer".into(), request_json)
            .await
    }

    /// Types text.
    pub async fn type_text(&self, text: String) -> Result<()> {
        env_call!(self, |c| {
            c.type_text(&text).await?;
            Ok(())
        })
    }

    /// Presses one key (`enter`, `KEY_ESCAPE`, `a`, ...).
    pub async fn press(&self, key: String) -> Result<()> {
        cua_spacesd_client::KeySpec::parse(&key)?;
        env_call!(self, |c| {
            c.press(key.as_str()).await?;
            Ok(())
        })
    }

    /// Presses a chord (`["ctrl", "c"]`).
    pub async fn hotkey(&self, keys: Vec<String>) -> Result<()> {
        env_call!(self, |c| {
            let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
            c.hotkey(&refs).await?;
            Ok(())
        })
    }

    /// Full `ComputerService.Keyboard` from proto3 JSON.
    pub async fn keyboard_json(&self, request_json: String) -> Result<String> {
        self.call_json("/cua.env.v1.ComputerService/Keyboard".into(), request_json)
            .await
    }

    /// Clipboard text.
    pub async fn get_clipboard(&self) -> Result<Option<String>> {
        env_call!(self, |c| Ok(c.get_clipboard().await?))
    }

    /// Sets clipboard text; returns the clipboard generation.
    pub async fn set_clipboard(&self, text: String) -> Result<u64> {
        env_call!(self, |c| Ok(c.set_clipboard(&text).await?))
    }

    /// Pointer position.
    pub async fn cursor_position(&self) -> Result<Point> {
        env_call!(self, |c| {
            let (x, y) = c.cursor_position().await?;
            Ok(Point { x, y })
        })
    }

    /// Displays as proto3 JSON (`[Display]`).
    pub async fn displays(&self) -> Result<String> {
        env_call!(self, |c| Ok(serde_json::to_string(&c.displays().await?)?))
    }

    // --------------------------------------------------------------- http

    /// Sends one plain-HTTP request to the spacesd port with this
    /// client's endpoint and credentials (through the Fleet gateway when the
    /// endpoint is a Fleet service). Used for streamable-HTTP MCP at `/mcp`,
    /// for example by typed cua-driver clients. Not retried.
    pub async fn http(&self, request: SpacesdHttpRequest) -> Result<SpacesdHttpResponse> {
        let call = cua_spacesd_client::HttpCall {
            method: request.method,
            path: request.path,
            headers: request
                .headers
                .into_iter()
                .map(|h| (h.name, h.value))
                .collect(),
            body: request.body,
            timeout: request
                .timeout_ms
                .map(|ms| std::time::Duration::from_millis(u64::from(ms))),
            max_response_bytes: request
                .max_response_bytes
                .map(|n| usize::try_from(n).unwrap_or(usize::MAX)),
        };
        env_call!(self, |c| {
            let reply = c.http(call).await?;
            Ok(SpacesdHttpResponse {
                status: reply.status,
                headers: reply
                    .headers
                    .into_iter()
                    .map(|(name, value)| SpacesdHttpHeader { name, value })
                    .collect(),
                body: reply.body,
            })
        })
    }

    // ------------------------------------------------------------- escape

    /// Calls any unary `cua.env.v1` RPC with a proto3-JSON request and
    /// returns the proto3-JSON response. `method` is the full path
    /// (`/cua.env.v1.WindowsService/ListWindows`) or `Service/Method`.
    /// Covers windows, accessibility, driver tools, teleport, tunnels and
    /// every other RPC without a typed method.
    pub async fn call_json(&self, method: String, request_json: String) -> Result<String> {
        let path = json::normalize(&method)?;
        env_call!(self, |c| json::call_unary(
            c.channel(),
            &path,
            &request_json
        )
        .await)
    }

    /// Calls a server-streaming `cua.env.v1` RPC and collects at most
    /// `max_messages` responses (or until the stream ends or `timeout_ms`
    /// elapses).
    pub async fn call_json_stream(
        &self,
        method: String,
        request_json: String,
        max_messages: u32,
        timeout_ms: u32,
    ) -> Result<Vec<String>> {
        let path = json::normalize(&method)?;
        env_call!(self, |c| {
            json::call_stream(
                c.channel(),
                &path,
                &request_json,
                max_messages,
                super::millis(timeout_ms),
            )
            .await
        })
    }

    /// Methods reachable through `call_json` / `call_json_stream`.
    pub fn json_methods(&self) -> Vec<String> {
        json::JSON_UNARY_METHODS
            .iter()
            .chain(json::JSON_STREAM_METHODS)
            .map(|s| s.to_string())
            .collect()
    }
}

#[cfg(feature = "media")]
#[uniffi::export]
impl SpacesdClient {
    /// Opens a media session and starts delivering encoded video frames and
    /// control events to `frames`. Decoding stays in the host language.
    pub async fn open_media(
        &self,
        options: MediaOpenOptions,
        frames: Arc<dyn super::FrameSink>,
    ) -> Result<Arc<super::MediaSession>> {
        let c = self.client.clone();
        let headers = self.current_ws_headers().await?;
        run(async move { super::media::open(c, headers, options, frames, None).await }).await
    }

    /// [`SpacesdClient::open_media`] that also delivers audio packets to
    /// `audio` (set `options.audio` to negotiate audio tracks).
    ///
    /// Two methods instead of an optional sink: optional callback objects
    /// do not lower in every binding generator.
    pub async fn open_media_with_audio(
        &self,
        options: MediaOpenOptions,
        frames: Arc<dyn super::FrameSink>,
        audio: Arc<dyn super::AudioSink>,
    ) -> Result<Arc<super::MediaSession>> {
        let c = self.client.clone();
        let headers = self.current_ws_headers().await?;
        run(async move { super::media::open(c, headers, options, frames, Some(audio)).await }).await
    }
}

/// The spacesd request for SDK screenshot options (PNG, full size and the
/// primary display when unset).
pub(crate) fn screenshot_request(
    options: Option<ScreenshotOptions>,
) -> cua_spacesd_client::ScreenshotOptions {
    let o = options.unwrap_or(ScreenshotOptions {
        display: None,
        format: None,
        quality: None,
        max_dimension: None,
        include_cursor: false,
    });
    cua_spacesd_client::ScreenshotOptions {
        display: o.display,
        format: o
            .format
            .map(ImageFormat::to_pb)
            .unwrap_or(pb::ImageFormat::Png),
        quality: o.quality.unwrap_or(0),
        max_dimension: o.max_dimension.unwrap_or(0),
        include_cursor: o.include_cursor,
    }
}

/// The SDK record for a spacesd screenshot.
pub(crate) fn screenshot_reply(s: cua_spacesd_client::Screenshot) -> Screenshot {
    Screenshot {
        image: s.image.to_vec(),
        format: ImageFormat::from_pb(s.format),
        width: s.width,
        height: s.height,
        scale: s.scale,
        screenshot_id: s.screenshot_id,
    }
}

#[cfg(all(test, feature = "media"))]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    struct Discard;

    impl super::super::FrameSink for Discard {
        fn on_frame(&self, _: super::super::VideoFrame) {}
        fn on_event(&self, _: super::super::MediaEvent) {}
    }

    fn options() -> MediaOpenOptions {
        MediaOpenOptions {
            display: None,
            window_handle: None,
            max_fps: 0,
            max_dimension: 0,
            audio: false,
            disable_video: false,
            request_json: None,
        }
    }

    /// A long-lived client to a cloud Space: its Fleet bearer expires while
    /// the client is held, so every media socket must carry the bearer
    /// current when the socket opens, never the one from creation.
    #[tokio::test]
    async fn media_sockets_carry_the_bearer_current_at_open() {
        let env = cua_daemon::fixtures::start_env(Some("env-token"), None).await;
        let mut o = cua_spacesd_client::ConnectOptions::parse(&env.url)
            .unwrap()
            .transport(cua_spacesd_client::TransportPreference::Native)
            .probe(false);
        o.token = Some("env-token".into());
        let inner = cua_spacesd_client::SpacesdClient::connect(o).await.unwrap();

        // The bearer source: each refresh stands for an expired token
        // replaced by a new one.
        let minted = Arc::new(AtomicU64::new(0));
        let counter = minted.clone();
        let refresh: WsHeaderRefresh = Arc::new(move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                Ok(vec![
                    ("authorization".into(), format!("Bearer fresh-{n}")),
                    ("x-cua-fleet-claim".into(), "claim-1".into()),
                ])
            })
        });
        let stale = vec![("authorization".to_string(), "Bearer stale".to_string())];
        let client = SpacesdClient::new(inner, stale.clone()).with_ws_refresh(refresh);
        assert_eq!(client.ws_headers(), stale, "the creation snapshot is kept");

        let seen = Arc::new(Mutex::new(Vec::new()));
        for _ in 0..2 {
            let before = env.media.lock().unwrap().attaches;
            let session = client
                .open_media(options(), Arc::new(Discard))
                .await
                .unwrap();
            for _ in 0..250 {
                if env.media.lock().unwrap().attaches > before {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let log = env.media.lock().unwrap();
            assert!(log.attaches > before, "media socket never attached");
            seen.lock()
                .unwrap()
                .push((log.last_authorization.clone(), log.last_claim.clone()));
            drop(log);
            drop(session);
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec![
                (Some("Bearer fresh-1".into()), Some("claim-1".into())),
                (Some("Bearer fresh-2".into()), Some("claim-1".into())),
            ]
        );
        assert_eq!(minted.load(Ordering::SeqCst), 2, "one refresh per socket");

        // Without a refresher (local and daemon routes) the headers are
        // fixed.
        let fixed = SpacesdClient::new(client.client.clone(), stale.clone());
        assert_eq!(fixed.current_ws_headers().await.unwrap(), stale);
    }
}
