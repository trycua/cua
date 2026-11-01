//! envd: the `process.Process` and `filesystem.Filesystem` Connect
//! services and `/health`, mapped onto the sandbox's cua-spacesd
//! (`cua.env.v1` ProcessService and FilesystemService). `/files` is in
//! [`super::files`].

use super::pb::{self as epb, b64};
use super::{E2b, HostSandbox, request_user, resolve_path};
use super::{fsops, watch};
use crate::connect::{self, Code, RpcError};
use crate::state::Record;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Extension, Router};
use cua_spacesd_client::pb;
use cua_spacesd_client::transport::Transport;
use futures_util::{Stream, StreamExt};
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Value, json};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

type Rpc = Pin<Box<dyn Stream<Item = Result<Value, RpcError>> + Send>>;

pub fn routes(e2b: Arc<E2b>) -> Router {
    Router::new()
        .route("/process.Process/{method}", post(process))
        .route("/filesystem.Filesystem/{method}", post(filesystem))
        .route("/files", get(super::files::read).post(super::files::write))
        .route("/health", get(health))
        .with_state(e2b)
}

fn host_id(h: &Option<Extension<HostSandbox>>) -> Option<&str> {
    h.as_ref().map(|Extension(HostSandbox(id))| id.as_str())
}

/// `GET /health`: envd's health (a sandbox request) or the API's.
async fn health(
    State(e): State<Arc<E2b>>,
    host: Option<Extension<HostSandbox>>,
    headers: HeaderMap,
) -> Response<Body> {
    if headers.get("e2b-sandbox-id").is_none() && host.is_none() {
        return Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("Health check successful"))
            .expect("response");
    }
    let r = match e.envd_record_with(&headers, host_id(&host), false).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match e.spacesd(&r).await {
        Ok(c) => match tokio::time::timeout(Duration::from_secs(5), c.health()).await {
            Ok(Ok(_)) => Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .expect("response"),
            _ => unreachable_sandbox(&e, &r).await,
        },
        Err(_) => unreachable_sandbox(&e, &r).await,
    }
}

/// A sandbox whose spacesd cannot be reached: gone (502, the record is
/// dropped) or unhealthy (502 too; envd's edge answers the same way).
async fn unreachable_sandbox(e: &E2b, r: &Record) -> Response<Body> {
    e.forget(&r.id).await;
    if e.check_gone(r).await {
        return super::not_found_edge(&r.id);
    }
    super::edge_error(
        StatusCode::BAD_GATEWAY,
        "The sandbox is not responding",
        &r.id,
    )
}

fn selector(s: Option<epb::ProcessSelector>) -> Result<pb::ProcessSelector, RpcError> {
    use pb::process_selector::Selector;
    let s = s.ok_or_else(|| RpcError::invalid("process selector is required"))?;
    let sel = match (s.pid, s.tag) {
        (Some(pid), _) if pid != 0 => Selector::Pid(pid),
        (_, Some(tag)) if !tag.is_empty() => Selector::Tag(tag),
        // pid 0 is a pid like any other that no process has.
        (Some(0), _) => return Err(RpcError::not_found("process with pid 0 not found")),
        _ => return Err(RpcError::invalid("process selector needs a pid or a tag")),
    };
    Ok(pb::ProcessSelector {
        selector: Some(sel),
    })
}

/// What the pid or tag of a selector says, for messages.
fn selector_label(s: &Option<epb::ProcessSelector>) -> String {
    match s {
        Some(epb::ProcessSelector { pid: Some(p), .. }) if *p != 0 => format!("pid {p}"),
        Some(epb::ProcessSelector { tag: Some(t), .. }) => format!("tag {t}"),
        _ => "pid 0".to_string(),
    }
}

/// envd's answer for a process that is not (or no longer) running: not
/// found. cua-spacesd keeps exited processes and says "already exited" or
/// "stdin is closed" for them.
fn process_error(status: tonic::Status, label: &str) -> RpcError {
    let m = status.message();
    let gone = status.code() == tonic::Code::NotFound
        || m.contains("No such process")
        || (status.code() == tonic::Code::FailedPrecondition
            && (m.contains("exited") || m.contains("stdin is closed")));
    if gone {
        RpcError::not_found(format!("process with {label} not found"))
    } else {
        RpcError::from(status)
    }
}

fn keepalive(headers: &HeaderMap) -> Option<pbjson_types::Duration> {
    headers
        .get("keepalive-ping-interval")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|s| *s > 0)
        .map(|s| pbjson_types::Duration {
            seconds: s,
            nanos: 0,
        })
}

/// A spacesd process event as envd's `{"event": ...}` message.
fn event_json(ev: pb::ProcessEvent) -> Option<Value> {
    use pb::process_data::Output;
    use pb::process_event::Event;
    Some(match ev.event? {
        Event::Start(s) => json!({"event": {"start": {"pid": s.pid}}}),
        Event::Data(d) => match d.output? {
            Output::Stdout(b) => json!({"event": {"data": {"stdout": b64(&b)}}}),
            Output::Stderr(b) => json!({"event": {"data": {"stderr": b64(&b)}}}),
            Output::Pty(b) => json!({"event": {"data": {"pty": b64(&b)}}}),
        },
        Event::End(e) => epb::end_event(e.exit_code, e.signal, e.signal_number, &e.error, e.lost),
        Event::Keepalive(_) => json!({"event": {"keepalive": {}}}),
    })
}

/// Bounds a stream by the call's `Connect-Timeout-Ms`, ending it with
/// `deadline_exceeded` (the process keeps running, as with envd).
fn with_deadline(s: Rpc, timeout: Option<Duration>) -> Rpc {
    let Some(t) = timeout else { return s };
    let deadline = Instant::now() + t;
    Box::pin(futures_util::stream::unfold(
        (s, false),
        move |(mut s, done)| async move {
            if done {
                return None;
            }
            match tokio::time::timeout_at(deadline, s.next()).await {
                Ok(Some(item)) => Some((item, (s, false))),
                Ok(None) => None,
                Err(_) => Some((
                    Err(RpcError::new(
                        Code::DeadlineExceeded,
                        "the call's deadline was exceeded",
                    )),
                    (s, true),
                )),
            }
        },
    ))
}

/// A broken stream to the guest (the sandbox was killed or lost mid-call):
/// envd's edge ends the stream with this message, which the SDKs turn into
/// "sandbox killed or reached its end of life" after a health probe. A
/// transport error text such as "h2 protocol error" never reaches a client.
pub(crate) fn stream_error(status: tonic::Status, sandbox: &str) -> RpcError {
    let m = status.message();
    let transport = matches!(
        status.code(),
        tonic::Code::Unavailable
            | tonic::Code::Unknown
            | tonic::Code::Internal
            | tonic::Code::Cancelled
            | tonic::Code::Aborted
    ) && (m.contains("h2")
        || m.contains("transport")
        || m.contains("connection")
        || m.contains("broken pipe")
        || m.contains("reset")
        || m.is_empty());
    if transport {
        // UNAVAILABLE with envd's own text: the SDKs then probe the
        // sandbox's health and raise "sandbox killed" when it is gone.
        RpcError::new(
            Code::Unavailable,
            format!(
                "the connection to sandbox {sandbox} ended before the stream completed: The sandbox was killed or reached its end of life while the request was in flight"
            ),
        )
    } else {
        RpcError::from(status)
    }
}

fn process_stream(
    s: tonic::Streaming<impl ProcessEventMsg + Send + 'static>,
    sandbox: String,
) -> Rpc {
    Box::pin(s.filter_map(move |item| {
        let sandbox = sandbox.clone();
        async move {
            match item {
                Ok(m) => m.into_event().and_then(event_json).map(Ok),
                Err(status) => Some(Err(stream_error(status, &sandbox))),
            }
        }
    }))
}

trait ProcessEventMsg {
    fn into_event(self) -> Option<pb::ProcessEvent>;
}
impl ProcessEventMsg for pb::StartProcessResponse {
    fn into_event(self) -> Option<pb::ProcessEvent> {
        self.event
    }
}
impl ProcessEventMsg for pb::ConnectProcessResponse {
    fn into_event(self) -> Option<pb::ProcessEvent> {
        self.event
    }
}

/// `POST /process.Process/<method>`.
async fn process(
    state: State<Arc<E2b>>,
    Path(method): Path<String>,
    host: Option<Extension<HostSandbox>>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let ty = format!("process.{method}Request");
    connect::with_request_type(ty, process_call(state, Path(method), host, headers, body)).await
}

async fn process_call(
    State(e): State<Arc<E2b>>,
    Path(method): Path<String>,
    host: Option<Extension<HostSandbox>>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let streaming = connect::is_streaming(&headers);
    let fail = |err: RpcError| {
        if streaming {
            connect::streaming_error(err)
        } else {
            err.into_response()
        }
    };
    let r = match e.envd_record(&headers, host_id(&host)).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let c = match e.spacesd(&r).await {
        Ok(c) => c,
        Err(_) => return unreachable_sandbox(&e, &r).await,
    };
    let timeout = connect::timeout(&headers);
    match method.as_str() {
        "Start" => {
            let req: epb::StartRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let user = request_user(&headers, super::default_user_of(&e, &r));
            if let Err(err) = e.check_user(&r, &c, &user).await {
                return fail(err);
            }
            let home = e.home(&r, &user).await;
            let cfg = req.process.unwrap_or_default();
            if cfg.cmd.is_empty() {
                return fail(RpcError::invalid("process.cmd is required"));
            }
            let mut env = r.env_vars.clone();
            env.extend(cfg.envs);
            // Relative cwd and `~` are under the request user's home; no cwd
            // at all is the sandbox's default workdir, the default user's
            // home (E2B's `root` starts in /home/user too).
            let explicit_cwd = cfg.cwd.as_ref().is_some_and(|c| !c.is_empty());
            let cwd = match cfg.cwd.filter(|c| !c.is_empty()) {
                Some(c) => resolve_path(&c, &home),
                None => match r.extra.get("workdir").and_then(Value::as_str) {
                    // A built template's WORKDIR.
                    Some(w) => w.to_string(),
                    None => e.home(&r, &e.cfg.default_user).await,
                },
            };
            let driver = e.drivers.of(&r.id, &c).await;
            // envd refuses a working directory that is missing, or that the
            // user cannot enter, before it starts anything.
            if explicit_cwd {
                let checked = crate::guest::run_as(
                    &c,
                    &driver,
                    &user,
                    "[ -d \"$1\" ] || exit 44; [ -x \"$1\" ] || exit 13",
                    &[&cwd],
                )
                .await;
                match checked.map(|o| o.status.code) {
                    Ok(Some(44)) => {
                        return fail(RpcError::invalid(format!("cwd '{cwd}' does not exist")));
                    }
                    Ok(Some(13)) => {
                        let line = std::iter::once(cfg.cmd.as_str())
                            .chain(cfg.args.iter().map(String::as_str))
                            .collect::<Vec<_>>()
                            .join(" ");
                        return fail(RpcError::invalid(format!(
                            "error starting process '{line}': fork/exec /bin/sh: permission denied"
                        )));
                    }
                    _ => {}
                }
            }
            let (command, args, user, cwd) =
                crate::guest::as_user_in(&driver, &user, cfg.cmd, cfg.args, cwd);
            let start = pb::StartProcessRequest {
                config: Some(pb::ProcessConfig {
                    command,
                    args,
                    env: env.into_iter().collect(),
                    cwd,
                    user,
                    timeout: None,
                }),
                pty: req.pty.map(|p| {
                    let size = p.size.unwrap_or_default();
                    pb::PtyConfig {
                        size: Some(pb::PtySize {
                            cols: size.cols,
                            rows: size.rows,
                            ..Default::default()
                        }),
                        term: "xterm-256color".into(),
                    }
                }),
                tag: req.tag.unwrap_or_default(),
                stdin: req.stdin.unwrap_or(false),
                keepalive_interval: keepalive(&headers),
                kill_on_disconnect: false,
                ..Default::default()
            };
            match c.process().start_process(start).await {
                Ok(s) => connect::streaming(with_deadline(
                    process_stream(s.into_inner(), r.id.clone()),
                    timeout,
                )),
                Err(status) => fail(status.into()),
            }
        }
        "Connect" => {
            let req: epb::ConnectRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let label = selector_label(&req.process);
            let sel = match selector(req.process) {
                Ok(s) => s,
                Err(err) => return fail(err),
            };
            let call = pb::ConnectProcessRequest {
                process: Some(sel),
                replay_bytes: 0,
                replay_from_offset: None,
                keepalive_interval: keepalive(&headers),
            };
            match c.process().connect_process(call).await {
                Ok(s) => connect::streaming(with_deadline(
                    process_stream(s.into_inner(), r.id.clone()),
                    timeout,
                )),
                Err(status) => fail(process_error(status, &label)),
            }
        }
        "List" => connect::unary(list_processes(&c).await),
        "SendInput" => {
            let req: epb::SendInputRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let sel = req.process.clone();
            let out = send_input(&c, req.process, req.input).await;
            connect::unary(match out {
                Ok(v) => Ok(v),
                Err(err) => Err(stdin_gone(&c, err, &sel).await),
            })
        }
        "SendSignal" => {
            let req: epb::SendSignalRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let signal = match epb::signal_number(&req.signal) {
                9 => pb::Signal::Kill,
                15 => pb::Signal::Term,
                _ => return fail(RpcError::invalid("unsupported signal")),
            };
            let pid = req.process.as_ref().and_then(|p| p.pid).filter(|p| *p != 0);
            let label = selector_label(&req.process);
            let call = match selector(req.process) {
                Ok(sel) => pb::SignalProcessRequest {
                    process: Some(sel),
                    signal: signal as i32,
                    process_group: false,
                },
                Err(err) => return fail(err),
            };
            let sent = c.process().signal_process(call).await;
            // A command run as another user is owned by that user (its
            // `sudo` parent by root), which the driver may not signal: root
            // signals its group, then the process itself.
            let sent = match (sent, pid) {
                (Err(err), Some(pid)) if err.to_string().contains("Operation not permitted") => {
                    let driver = e.drivers.of(&r.id, &c).await;
                    let sig = if signal == pb::Signal::Kill {
                        "KILL"
                    } else {
                        "TERM"
                    };
                    let out = crate::guest::run_as(
                        &c,
                        &driver,
                        "root",
                        "a=1; kill -s \"$1\" -- \"-$2\" 2>/dev/null && a=0; kill -s \"$1\" \"$2\" 2>/dev/null && a=0; exit $a",
                        &[sig, &pid.to_string()],
                    )
                    .await;
                    match out {
                        Ok(o) if o.success() => Ok(()),
                        _ => Err(err),
                    }
                }
                (sent, _) => sent.map(|_| ()),
            };
            // The signal is delivered; envd's list no longer shows the
            // process by the time the call returns. Wait for it to go.
            if sent.is_ok() {
                wait_gone(&c, pid).await;
            }
            connect::unary(
                sent.map(|_| json!({}))
                    .map_err(|status| process_error(status, &label)),
            )
        }
        "CloseStdin" => {
            let req: epb::SelectorRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let label = selector_label(&req.process);
            let pid = req.process.as_ref().and_then(|p| p.pid).filter(|p| *p != 0);
            let call = match selector(req.process) {
                Ok(sel) => pb::CloseStdinRequest { process: Some(sel) },
                Err(err) => return fail(err),
            };
            if let Some(pid) = pid
                && !pid_running(&c, pid).await
            {
                return fail(RpcError::not_found(format!(
                    "process with {label} not found"
                )));
            }
            connect::unary(
                c.process()
                    .close_stdin(call)
                    .await
                    .map(|_| json!({}))
                    .map_err(|status| process_error(status, &label)),
            )
        }
        "Update" => {
            let req: epb::UpdateRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let label = selector_label(&req.process);
            let sel = match selector(req.process) {
                Ok(s) => s,
                Err(err) => return fail(err),
            };
            let Some(size) = req.pty.and_then(|p| p.size) else {
                return connect::unary(Ok(json!({})));
            };
            let call = pb::ResizePtyRequest {
                process: Some(sel),
                size: Some(pb::PtySize {
                    cols: size.cols,
                    rows: size.rows,
                    ..Default::default()
                }),
            };
            connect::unary(
                c.process()
                    .resize_pty(call)
                    .await
                    .map(|_| json!({}))
                    .map_err(|status| process_error(status, &label)),
            )
        }
        "StreamInput" => stream_input(c, body).await,
        other => fail(RpcError::new(
            Code::Unimplemented,
            format!("process.Process/{other} is not a known method"),
        )),
    }
}

/// Whether `pid` is a running process.
async fn pid_running(c: &cua_spacesd_client::SpacesdClient, pid: u32) -> bool {
    c.process()
        .list_processes(pb::ListProcessesRequest::default())
        .await
        .map(|r| {
            r.into_inner()
                .processes
                .iter()
                .any(|p| p.pid == pid && p.state != pb::ProcessState::Exited as i32)
        })
        .unwrap_or(false)
}

/// Input for a process that cannot take it: not found when it is gone,
/// envd's "stdin not enabled or closed" when it runs without stdin.
async fn stdin_gone(
    c: &cua_spacesd_client::SpacesdClient,
    err: RpcError,
    sel: &Option<epb::ProcessSelector>,
) -> RpcError {
    let label = selector_label(sel);
    let stdin_closed = err.message.contains("stdin is closed");
    let exited = err.message.contains("exited");
    if err.code == Code::NotFound || exited {
        return RpcError::not_found(format!("process with {label} not found"));
    }
    if err.code == Code::FailedPrecondition && stdin_closed {
        let running = match sel.as_ref().and_then(|s| s.pid).filter(|p| *p != 0) {
            Some(pid) => pid_running(c, pid).await,
            None => true,
        };
        return if running {
            RpcError::internal("error writing to stdin: stdin not enabled or closed")
        } else {
            RpcError::not_found(format!("process with {label} not found"))
        };
    }
    err
}

/// Waits (briefly) until `pid` is no longer a running process.
async fn wait_gone(c: &cua_spacesd_client::SpacesdClient, pid: Option<u32>) {
    let Some(pid) = pid else { return };
    for _ in 0..120 {
        let running = c
            .process()
            .list_processes(pb::ListProcessesRequest::default())
            .await
            .map(|r| {
                r.into_inner()
                    .processes
                    .iter()
                    .any(|p| p.pid == pid && p.state != pb::ProcessState::Exited as i32)
            })
            .unwrap_or(false);
        if !running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn list_processes(c: &cua_spacesd_client::SpacesdClient) -> Result<Value, RpcError> {
    let resp = c
        .process()
        .list_processes(pb::ListProcessesRequest {
            include_exited: false,
            tag_prefix: String::new(),
        })
        .await?
        .into_inner();
    let processes: Vec<Value> = resp
        .processes
        .into_iter()
        .filter(|p| p.state != pb::ProcessState::Exited as i32)
        .map(|p| {
            let cfg = p.config.unwrap_or_default();
            let (cmd, args, cwd) = crate::guest::unwrap_switch(&cfg.command, &cfg.args);
            let mut v = json!({
                "config": {"cmd": cmd, "args": args, "envs": cfg.env, "cwd": cwd.unwrap_or(cfg.cwd)},
                "pid": p.pid,
            });
            if !p.tag.is_empty() {
                v["tag"] = json!(p.tag);
            }
            v
        })
        .collect();
    Ok(json!({"processes": processes}))
}

fn input(i: Option<epb::ProcessInput>) -> Result<pb::ProcessInput, RpcError> {
    use pb::process_input::Input;
    let i = i.ok_or_else(|| RpcError::invalid("input is required"))?;
    let input = match (i.stdin, i.pty) {
        (Some(d), _) => Input::Stdin(d),
        (None, Some(d)) => Input::Pty(d),
        _ => return Err(RpcError::invalid("input needs stdin or pty data")),
    };
    Ok(pb::ProcessInput { input: Some(input) })
}

async fn send_input(
    c: &cua_spacesd_client::SpacesdClient,
    sel: Option<epb::ProcessSelector>,
    i: Option<epb::ProcessInput>,
) -> Result<Value, RpcError> {
    let call = pb::SendInputRequest {
        process: Some(selector(sel)?),
        input: Some(input(i)?),
        sequence: 0,
        writer_id: String::new(),
    };
    c.process().send_input(call).await?;
    Ok(json!({}))
}

/// `StreamInput` (client stream): forwarded as a client stream on native
/// gRPC, else as ordered `SendInput` calls (gRPC-Web has no client
/// streams).
async fn stream_input(c: cua_spacesd_client::SpacesdClient, body: Body) -> Response<Body> {
    use pb::stream_input_request::Message;
    let mut incoming = Box::pin(connect::client_stream::<epb::StreamInputRequest>(body));
    let first = match incoming.next().await {
        Some(Ok(m)) => m,
        Some(Err(err)) => return connect::streaming_error(err),
        None => return connect::streaming_error(RpcError::invalid("empty input stream")),
    };
    let Some(start) = first.start else {
        return connect::streaming_error(RpcError::invalid(
            "the first StreamInput message must be start",
        ));
    };
    let sel = match selector(start.process) {
        Ok(s) => s,
        Err(err) => return connect::streaming_error(err),
    };
    if c.transport() == Transport::Native {
        let (tx, rx) = tokio::sync::mpsc::channel::<pb::StreamInputRequest>(16);
        let _ = tx
            .send(pb::StreamInputRequest {
                message: Some(Message::Process(sel)),
            })
            .await;
        let pump = tokio::spawn(async move {
            while let Some(m) = incoming.next().await {
                let m = m?;
                let msg = if let Some(d) = m.data {
                    Message::Input(input(d.input)?)
                } else if m.keepalive.is_some() {
                    Message::Keepalive(pb::KeepAlive {})
                } else {
                    continue;
                };
                if tx
                    .send(pb::StreamInputRequest { message: Some(msg) })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok::<_, RpcError>(())
        });
        let res = c
            .process()
            .stream_input(tokio_stream::wrappers::ReceiverStream::new(rx))
            .await;
        let pumped = pump
            .await
            .unwrap_or_else(|e| Err(RpcError::internal(e.to_string())));
        let out = match (res, pumped) {
            (Err(status), _) => Err(RpcError::from(status)),
            (Ok(_), Err(err)) => Err(err),
            (Ok(_), Ok(())) => Ok(json!({})),
        };
        return connect::streaming(futures_util::stream::iter([out]));
    }
    let mut out = Ok(json!({}));
    while let Some(m) = incoming.next().await {
        let r = match m {
            Ok(m) => match m.data {
                Some(d) => {
                    let sel = epb::ProcessSelector {
                        pid: match &sel.selector {
                            Some(pb::process_selector::Selector::Pid(p)) => Some(*p),
                            _ => None,
                        },
                        tag: match &sel.selector {
                            Some(pb::process_selector::Selector::Tag(t)) => Some(t.clone()),
                            _ => None,
                        },
                    };
                    send_input(&c, Some(sel), d.input).await.map(|_| ())
                }
                None => Ok(()),
            },
            Err(err) => Err(err),
        };
        if let Err(err) = r {
            out = Err(err);
            break;
        }
    }
    connect::streaming(futures_util::stream::iter([out]))
}

/// `POST /filesystem.Filesystem/<method>`.
async fn filesystem(
    state: State<Arc<E2b>>,
    Path(method): Path<String>,
    host: Option<Extension<HostSandbox>>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let ty = format!("filesystem.{method}Request");
    connect::with_request_type(
        ty,
        filesystem_call(state, Path(method), host, headers, body),
    )
    .await
}

async fn filesystem_call(
    State(e): State<Arc<E2b>>,
    Path(method): Path<String>,
    host: Option<Extension<HostSandbox>>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let streaming = connect::is_streaming(&headers);
    let fail = |err: RpcError| {
        if streaming {
            connect::streaming_error(err)
        } else {
            err.into_response()
        }
    };
    let r = match e.envd_record(&headers, host_id(&host)).await {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let c = match e.spacesd(&r).await {
        Ok(c) => c,
        Err(_) => return unreachable_sandbox(&e, &r).await,
    };
    let user = request_user(&headers, super::default_user_of(&e, &r));
    if let Err(err) = e.check_user(&r, &c, &user).await {
        return fail(err);
    }
    let timeout = connect::timeout(&headers);
    let driver = e.drivers.of(&r.id, &c).await;
    let fs = fsops::Fs {
        c: &c,
        driver: &driver,
    };
    match method.as_str() {
        "Stat" | "ListDir" | "MakeDir" | "Remove" | "WatchDir" | "CreateWatcher" => {
            let req: epb::PathRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let home = e.home(&r, &user).await;
            let path = resolve_path(&req.path, &home);
            match method.as_str() {
                "Stat" => {
                    let out = fs.stat(&path).await.map(|en| fsops::stat_json(&en));
                    connect::unary(with_metadata(&e, &r, &c, out).await)
                }
                "ListDir" => {
                    let out = fs
                        .list(&path, req.depth.max(1))
                        .await
                        .map(|es| fsops::list_json(&es));
                    connect::unary(with_metadata(&e, &r, &c, out).await)
                }
                "MakeDir" => connect::unary(
                    fs.make_dir(&path, &user)
                        .await
                        .map(|en| fsops::stat_json(&en)),
                ),
                "Remove" => connect::unary(fs.remove(&path).await.map(|()| json!({}))),
                "WatchDir" | "CreateWatcher" => {
                    // envd's checks first: the path exists and is a directory.
                    match fs.stat(&path).await {
                        Err(err) if err.code == Code::NotFound => {
                            return fail(RpcError::not_found(format!(
                                "path {path} not found: stat {path}: no such file or directory"
                            )));
                        }
                        Err(err) => return fail(err),
                        Ok(en) => {
                            let is_dir = if en.kind == 'l' {
                                fs.stat(&format!("{path}/"))
                                    .await
                                    .is_ok_and(|d| d.kind == 'd')
                            } else {
                                en.kind == 'd'
                            };
                            if !is_dir {
                                return fail(RpcError::invalid(format!(
                                    "path {path} not a directory: %!w(<nil>)"
                                )));
                            }
                        }
                    }
                    let items = match watch_items(&c, &driver, &path, req.recursive).await {
                        Ok(i) => i,
                        Err(err) => return fail(err),
                    };
                    if method == "CreateWatcher" {
                        let id = crate::auth::new_token();
                        let entry = req.include_entry.then(|| (c.clone(), driver.clone()));
                        let w = watch::Watcher::spawn(path, items, entry);
                        e.watchers.lock().await.insert(id.clone(), w);
                        return connect::unary(Ok(json!({"watcherId": id})));
                    }
                    let include_entry = req.include_entry;
                    let fsc = c.clone();
                    let drv = driver.clone();
                    let events: Rpc = Box::pin(items.then(move |item| {
                        let fsc = fsc.clone();
                        let drv = drv.clone();
                        async move {
                            match item? {
                                watch::WatchItem::Started => Ok(json!({"start": {}})),
                                watch::WatchItem::Keepalive => Ok(json!({"keepalive": {}})),
                                watch::WatchItem::Event { name, kind, path } => {
                                    let mut f = json!({"name": name, "type": kind});
                                    // envd 0.6.3: the entry, when asked for and
                                    // the path still exists.
                                    if include_entry && kind != "EVENT_TYPE_REMOVE" {
                                        let fs = fsops::Fs {
                                            c: &fsc,
                                            driver: &drv,
                                        };
                                        if let Ok(en) = fs.stat(&path).await {
                                            f["entry"] = en.json();
                                        }
                                    }
                                    Ok(json!({"filesystem": f}))
                                }
                            }
                        }
                    }));
                    connect::streaming(with_deadline(events, timeout))
                }
                _ => unreachable!(),
            }
        }
        "Move" => {
            let req: epb::MoveRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let home = e.home(&r, &user).await;
            let (source, destination) = (
                resolve_path(&req.source, &home),
                resolve_path(&req.destination, &home),
            );
            let out = fs
                .rename(&source, &destination, &user)
                .await
                .map(|en| fsops::stat_json(&en));
            connect::unary(with_metadata(&e, &r, &c, out).await)
        }
        "GetWatcherEvents" | "RemoveWatcher" => {
            let req: epb::WatcherRequest = match connect::read_request(&headers, body).await {
                Ok(r) => r,
                Err(err) => return fail(err),
            };
            let mut ws = e.watchers.lock().await;
            if method == "RemoveWatcher" {
                return connect::unary(match ws.remove(&req.watcher_id) {
                    Some(w) => {
                        w.stop();
                        Ok(json!({}))
                    }
                    None => Err(RpcError::not_found(format!(
                        "watcher with id {} not found",
                        req.watcher_id
                    ))),
                });
            }
            let mut events = ws.get(&req.watcher_id).map(|w| w.drain());
            if events.as_ref().is_some_and(Vec::is_empty) {
                // envd's events are in-process and instant; ours come from
                // the guest's inotify reader a few ms later, so a poll right
                // after a change waits a moment.
                drop(ws);
                tokio::time::sleep(Duration::from_millis(300)).await;
                events = e
                    .watchers
                    .lock()
                    .await
                    .get(&req.watcher_id)
                    .map(|w| w.drain());
            }
            connect::unary(match events {
                Some(events) => Ok(json!({"events": events})),
                None => Err(RpcError::not_found(format!(
                    "watcher with id {} not found",
                    req.watcher_id
                ))),
            })
        }
        other => fail(RpcError::new(
            Code::Unimplemented,
            format!("filesystem.Filesystem/{other} is not a known method"),
        )),
    }
}

/// The events of a watch: the guest inotify reader (envd's event
/// semantics), or cua-spacesd's own watch where the guest has no `python3`.
async fn watch_items(
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    path: &str,
    recursive: bool,
) -> Result<watch::Items, RpcError> {
    match watch::start(c, driver, path, recursive).await {
        Ok(items) => Ok(items),
        Err(err) => {
            tracing::debug!("compat: guest watch reader unavailable ({err}); using cua-spacesd's");
            let call = pb::WatchDirRequest {
                path: path.to_string(),
                recursive,
                keepalive_interval: None,
                ..Default::default()
            };
            let s = c.filesystem().watch_dir(call).await?.into_inner();
            let base = path.to_string();
            Ok(Box::pin(s.filter_map(move |item| {
                let base = base.clone();
                async move {
                    use pb::watch_dir_response::Message;
                    match item {
                        Ok(m) => match m.message? {
                            Message::Started(_) => Some(Ok(watch::WatchItem::Started)),
                            Message::Event(ev) => {
                                let f = epb::fs_event(&ev, &base);
                                Some(Ok(watch::WatchItem::Event {
                                    name: f["name"].as_str().unwrap_or_default().to_string(),
                                    kind: epb::event_type_name(ev.r#type),
                                    path: ev.path,
                                }))
                            }
                            Message::Keepalive(_) => Some(Ok(watch::WatchItem::Keepalive)),
                            Message::Overflow(_) => None,
                        },
                        Err(status) => Some(Err(RpcError::from(status))),
                    }
                }
            })))
        }
    }
}

/// Adds E2B file metadata (`user.e2b.*` xattrs) to the entries of a Stat,
/// ListDir or Move answer, in one guest command. Metadata can be set from
/// inside the sandbox too (`setxattr`), so it is always read.
async fn with_metadata(
    e: &E2b,
    r: &Record,
    c: &cua_spacesd_client::SpacesdClient,
    v: Result<Value, RpcError>,
) -> Result<Value, RpcError> {
    let mut v = v?;
    let paths: Vec<String> = v
        .get("entry")
        .into_iter()
        .chain(
            v.get("entries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
        .filter_map(|x| x.get("path").and_then(Value::as_str).map(str::to_string))
        .collect();
    let driver = e.drivers.of(&r.id, c).await;
    let found = crate::guest::get_metadata(c, &driver, &paths).await;
    let add = |en: &mut Value| {
        if let Some(m) = en
            .get("path")
            .and_then(Value::as_str)
            .and_then(|p| found.get(p))
        {
            en["metadata"] = json!(m);
        }
    };
    if let Some(en) = v.get_mut("entry").filter(|x| x.is_object()) {
        add(en);
    }
    if let Some(Value::Array(list)) = v.get_mut("entries") {
        list.iter_mut().for_each(add);
    }
    Ok(v)
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn transport_errors_become_the_edge_message() {
        let e = stream_error(
            tonic::Status::internal("h2 protocol error: error reading a body from connection"),
            "abc",
        );
        assert_eq!(e.code, Code::Unavailable);
        assert_eq!(
            e.message,
            "the connection to sandbox abc ended before the stream completed: The sandbox was killed or reached its end of life while the request was in flight"
        );
        // Real errors from the guest pass through.
        let e = stream_error(tonic::Status::not_found("no such process"), "abc");
        assert_eq!(e.code, Code::NotFound);
    }
}
