//! Daytona's code interpreter: `GET /process/interpreter/execute` (a
//! WebSocket), and contexts at `/process/interpreter/context`.
//!
//! Each context is a Python worker process in the guest that keeps one
//! global namespace across executions (variables, imports, functions), as the
//! SDKs document (`daytona/_sync/code_interpreter.py`, `CodeInterpreter.ts`,
//! 0.225.0). The SDK sends `{code, contextId?, envs?, timeout?}` and reads
//! `{type: stdout|stderr, text}`, `{type: error, name, value, traceback}` and
//! `{type: control, text: completed|interrupted}` until the socket closes
//! (1000), or 4008 when the execution outlived its timeout.
//!
//! The worker is our own script, written to the guest at first use. Output of
//! the code (and of the processes it starts) is read from pipes that replace
//! file descriptors 1 and 2, so the protocol on the original stdout stays
//! clean.

use super::stdio::{Out, Stdio};
use super::toolbox::{error, sandbox, workdir};
use super::{Daytona, ok_json};
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::routing::{delete, get};
use cua_spacesd_client::pb;
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Close code the SDKs read as "timed out" (`WEBSOCKET_TIMEOUT_CODE`).
pub const TIMEOUT_CLOSE: u16 = 4008;
/// The default timeout of an execution, in seconds (the SDK's: 10 minutes).
const DEFAULT_TIMEOUT: u64 = 600;
/// Seconds an interrupted worker gets to report before it is killed.
const GRACE: Duration = Duration::from_secs(3);

/// The contexts of every sandbox.
#[derive(Default)]
pub struct Registry {
    ctxs: Mutex<HashMap<(String, String), Arc<Ctx>>>,
}

pub struct Ctx {
    id: String,
    cwd: String,
    created: String,
    default: bool,
    /// One execution at a time; holds the worker.
    worker: Mutex<Option<Arc<Stdio>>>,
}

impl Registry {
    pub async fn drop_sandbox(&self, sandbox_id: &str) {
        let removed: Vec<Arc<Ctx>> = {
            let mut m = self.ctxs.lock().await;
            let keys: Vec<_> = m.keys().filter(|(s, _)| s == sandbox_id).cloned().collect();
            keys.iter().filter_map(|k| m.remove(k)).collect()
        };
        for c in removed {
            if let Some(w) = c.worker.lock().await.take() {
                w.kill().await;
            }
        }
    }
}

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route(
            "/{id}/process/interpreter/context",
            get(list_contexts).post(create_context),
        )
        .route(
            "/{id}/process/interpreter/context/{cid}",
            delete(delete_context),
        )
        .route("/{id}/process/interpreter/execute", get(execute))
        .with_state(d)
}

fn rfc3339_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn ctx_json(c: &Ctx) -> Value {
    json!({
        "id": c.id,
        "cwd": c.cwd,
        "language": "python",
        "active": true,
        "createdAt": c.created,
    })
}

async fn create_context(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, _, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b: Value = match http_body_util::BodyExt::collect(body).await {
        Ok(c) => serde_json::from_slice(&c.to_bytes()).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    if let Some(l) = b.get("language").and_then(Value::as_str)
        && !l.is_empty()
        && !l.eq_ignore_ascii_case("python")
    {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "The code interpreter runs Python; {l} is not supported (use process.code_run for other languages)"
            ),
        );
    }
    let home = workdir(&d, &r, &user).await;
    let cwd = b
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(|c| crate::e2b::resolve_path(c, &home))
        .unwrap_or(home);
    let ctx = Arc::new(Ctx {
        id: super::api_uuid(),
        cwd,
        created: rfc3339_now(),
        default: false,
        worker: Mutex::new(None),
    });
    let out = ctx_json(&ctx);
    d.interp
        .ctxs
        .lock()
        .await
        .insert((r.id.clone(), ctx.id.clone()), ctx);
    ok_json(out)
}

async fn list_contexts(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, _, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let m = d.interp.ctxs.lock().await;
    let mut v: Vec<&Arc<Ctx>> = m
        .iter()
        .filter(|((s, _), c)| s == &r.id && !c.default)
        .map(|(_, c)| c)
        .collect();
    v.sort_by(|a, b| a.created.cmp(&b.created));
    ok_json(json!({"contexts": v.iter().map(|c| ctx_json(c)).collect::<Vec<_>>()}))
}

async fn delete_context(
    State(d): State<Arc<Daytona>>,
    Path((id, cid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, _, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    if cid == "default" {
        return error(
            StatusCode::BAD_REQUEST,
            "The default context cannot be deleted",
        );
    }
    let removed = d
        .interp
        .ctxs
        .lock()
        .await
        .remove(&(r.id.clone(), cid.clone()));
    match removed {
        Some(c) => {
            if let Some(w) = c.worker.lock().await.take() {
                w.kill().await;
            }
            ok_json(json!({"message": "Context deleted"}))
        }
        None => error(StatusCode::NOT_FOUND, format!("Context {cid} not found")),
    }
}

async fn execute(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Ok(ws) = ws else {
        return error(StatusCode::BAD_REQUEST, "a WebSocket upgrade is required").into_response();
    };
    let home = workdir(&d, &r, &user).await;
    ws.on_upgrade(move |sock| async move {
        run(sock, d, r, c, user, home).await;
    })
}

async fn run(
    mut sock: WebSocket,
    d: Arc<Daytona>,
    r: crate::state::Record,
    c: cua_spacesd_client::SpacesdClient,
    user: String,
    home: String,
) {
    let Some(Ok(Message::Text(first))) = sock.recv().await else {
        return;
    };
    let req: Value = serde_json::from_str(&first).unwrap_or(Value::Null);
    let Some(code) = req.get("code").and_then(Value::as_str).map(str::to_string) else {
        let _ = send(&mut sock, json!({"type": "error", "name": "InvalidRequest", "value": "code is required", "traceback": ""})).await;
        close(&mut sock, 1008, "code is required").await;
        return;
    };
    let ctx = {
        let mut m = d.interp.ctxs.lock().await;
        match req
            .get("contextId")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
        {
            Some(cid) => match m.get(&(r.id.clone(), cid.to_string())) {
                Some(c) => c.clone(),
                None => {
                    drop(m);
                    let _ = send(&mut sock, json!({"type": "error", "name": "ContextNotFound", "value": format!("context {cid} not found"), "traceback": ""})).await;
                    close(&mut sock, 1008, "context not found").await;
                    return;
                }
            },
            None => m
                .entry((r.id.clone(), "default".to_string()))
                .or_insert_with(|| {
                    Arc::new(Ctx {
                        id: "default".into(),
                        cwd: home.clone(),
                        created: rfc3339_now(),
                        default: true,
                        worker: Mutex::new(None),
                    })
                })
                .clone(),
        }
    };
    let envs: HashMap<String, String> = req
        .get("envs")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string()),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let timeout = req
        .get("timeout")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT);
    // One execution at a time per context.
    let mut slot = ctx.worker.lock().await;
    let driver = d.drivers.of(&r.id, &c).await;
    let worker = match slot.clone() {
        Some(w) => w,
        None => {
            let mut env: HashMap<String, String> = r.env_vars.clone().into_iter().collect();
            env.insert("PYTHONUNBUFFERED".into(), "1".into());
            match Stdio::start(
                &c,
                &driver,
                &user,
                vec!["python3".into(), "-u".into(), "-c".into(), WORKER.into()],
                env,
                ctx.cwd.clone(),
            )
            .await
            {
                Ok(w) => {
                    let w = Arc::new(w);
                    *slot = Some(w.clone());
                    w
                }
                Err(e) => {
                    let _ = send(&mut sock, json!({"type": "error", "name": "InterpreterUnavailable", "value": e, "traceback": ""})).await;
                    close(&mut sock, 1011, "interpreter unavailable").await;
                    return;
                }
            }
        }
    };
    let line = json!({"code": code, "envs": envs}).to_string() + "\n";
    if worker.write(line.into_bytes()).await.is_err() {
        *slot = None;
        let _ = send(&mut sock, json!({"type": "error", "name": "InterpreterUnavailable", "value": "the interpreter process is gone", "traceback": ""})).await;
        close(&mut sock, 1011, "interpreter gone").await;
        return;
    }
    let deadline =
        (timeout > 0).then(|| tokio::time::Instant::now() + Duration::from_secs(timeout));
    let mut buf: Vec<u8> = Vec::new();
    let mut interrupted_at: Option<tokio::time::Instant> = None;
    let mut timed_out = false;
    let outcome: &str = 'exec: loop {
        let wait = match (deadline, interrupted_at) {
            (_, Some(t)) => t + GRACE,
            (Some(dl), None) => dl,
            (None, None) => tokio::time::Instant::now() + Duration::from_secs(3600),
        };
        tokio::select! {
            out = worker.next(Duration::from_secs(3600)) => {
                match out {
                    Some(Out::Stdout(b)) => {
                        buf.extend_from_slice(&b);
                        while let Some(p) = buf.iter().position(|c| *c == b'\n') {
                            let l: Vec<u8> = buf.drain(..=p).collect();
                            let Ok(v) = serde_json::from_slice::<Value>(&l[..l.len() - 1]) else { continue };
                            if v["type"] == "done" {
                                break 'exec "done";
                            }
                            if !timed_out && send(&mut sock, v).await.is_err() {
                                worker.signal(pb::Signal::Int).await;
                                break 'exec "client";
                            }
                        }
                    }
                    Some(Out::Stderr(b)) => {
                        tracing::debug!("compat: interpreter worker: {}", String::from_utf8_lossy(&b));
                    }
                    Some(Out::Exit(_)) | None => break 'exec "died",
                }
            }
            _ = tokio::time::sleep_until(wait) => {
                if interrupted_at.is_none() {
                    // Ask the code to stop; it reports, then `done`.
                    timed_out = true;
                    worker.signal(pb::Signal::Int).await;
                    interrupted_at = Some(tokio::time::Instant::now());
                } else {
                    break 'exec "stuck";
                }
            }
            m = sock.recv() => {
                if matches!(m, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                    worker.signal(pb::Signal::Int).await;
                    break 'exec "client";
                }
            }
        }
    };
    match outcome {
        "done" if timed_out => {
            close(&mut sock, TIMEOUT_CLOSE, "execution timed out").await;
        }
        "done" => {
            let _ = send(&mut sock, json!({"type": "control", "text": "completed"})).await;
            close(&mut sock, 1000, "").await;
        }
        "stuck" | "died" => {
            // The worker cannot go on: its state is gone with it.
            worker.kill().await;
            *slot = None;
            if timed_out {
                close(&mut sock, TIMEOUT_CLOSE, "execution timed out").await;
            } else {
                let _ = send(&mut sock, json!({"type": "error", "name": "InterpreterExited", "value": "the interpreter process ended; its variables are lost", "traceback": ""})).await;
                close(&mut sock, 1011, "interpreter exited").await;
            }
        }
        _ => {}
    }
}

async fn send(sock: &mut WebSocket, v: Value) -> Result<(), axum::Error> {
    sock.send(Message::Text(v.to_string().into())).await
}

async fn close(sock: &mut WebSocket, code: u16, reason: &str) {
    let _ = sock
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

/// The guest side: reads one JSON request per line on stdin, runs it in this
/// context's namespace, relays output as JSON lines on the original stdout
/// and ends each run with `{"type":"done"}`.
const WORKER: &str = r#"
import sys, os, json, threading, traceback, signal
_proto = os.fdopen(os.dup(1), "w", buffering=1, encoding="utf-8", errors="replace")
_lock = threading.Lock()
def emit(o):
    with _lock:
        _proto.write(json.dumps(o) + "\n"); _proto.flush()
_marks = {"stdout": threading.Event(), "stderr": threading.Event()}
def _pump(fd, kind, mark):
    buf = b""
    while True:
        chunk = os.read(fd, 65536)
        if not chunk:
            return
        buf += chunk
        while buf:
            i = buf.find(b"\x00")
            if i < 0:
                emit({"type": kind, "text": buf.decode("utf-8", "replace")}); buf = b""
                break
            if i:
                emit({"type": kind, "text": buf[:i].decode("utf-8", "replace")}); buf = buf[i:]
            if buf.startswith(mark):
                buf = buf[len(mark):]; _marks[kind].set(); continue
            if mark.startswith(buf):
                break
            emit({"type": kind, "text": "\x00"}); buf = buf[1:]
def _redirect(fd, kind, mark):
    r, w = os.pipe()
    os.dup2(w, fd)
    os.close(w)
    threading.Thread(target=_pump, args=(r, kind, mark), daemon=True).start()
_MO, _ME = b"\x00cua-out-end\x00", b"\x00cua-err-end\x00"
_redirect(1, "stdout", _MO)
_redirect(2, "stderr", _ME)
sys.stdout = os.fdopen(1, "w", buffering=1, encoding="utf-8", errors="replace", closefd=False)
sys.stderr = os.fdopen(2, "w", buffering=1, encoding="utf-8", errors="replace", closefd=False)
g = {"__name__": "__main__", "__builtins__": __builtins__}
def _error(e):
    tb = "".join(traceback.format_exception(type(e), e, e.__traceback__))
    emit({"type": "error", "name": type(e).__name__, "value": str(e), "traceback": tb})
for line in sys.stdin:
    req = json.loads(line)
    old = {}
    for k, v in (req.get("envs") or {}).items():
        old[k] = os.environ.get(k); os.environ[k] = str(v)
    try:
        exec(compile(req["code"], "<code>", "exec"), g)
    except SystemExit as e:
        if e.code not in (None, 0):
            _error(e)
    except KeyboardInterrupt as e:
        _error(e)
    except BaseException as e:
        _error(e)
    finally:
        try:
            sys.stdout.flush(); sys.stderr.flush()
        except Exception:
            pass
        for k, v in old.items():
            if v is None: os.environ.pop(k, None)
            else: os.environ[k] = v
        for ev in _marks.values(): ev.clear()
        os.write(1, _MO); os.write(2, _ME)
        _marks["stdout"].wait(10); _marks["stderr"].wait(10)
        emit({"type": "done"})
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_script_is_valid_python_shaped_text() {
        // The worker is sent through `python3 -c`: it must not contain a NUL.
        assert!(!WORKER.contains('\0'));
        assert!(WORKER.contains("\"type\": \"done\""));
    }
}
