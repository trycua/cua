//! Daytona's toolbox (`/api/toolbox/<id>/...`): process exec and code run,
//! sessions, files and ports, over the sandbox's cua-spacesd.
//!
//! A Daytona *session* is a shell whose working directory and exported
//! variables carry over between commands. Each session command runs as its
//! own cua-spacesd process that restores and saves that state in the guest
//! (`/tmp/.cua-sessions/<id>`), so commands of one session see each other's
//! `cd` and `export`.

use super::{Daytona, backend_error, daemon_error, ok_json};
use crate::e2b::resolve_path;
use crate::guestfs::{FsError, Multipart, Writer, multipart_boundary};
use crate::state::Record;
use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use bytes::Bytes;
use cua_spacesd_client::pb;
use futures_util::StreamExt;
use http::{HeaderMap, Response, StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, watch};

const STDOUT_PREFIX: &[u8] = b"\x01\x01\x01";
const STDERR_PREFIX: &[u8] = b"\x02\x02\x02";

/// An error of the sandbox daemon: `source: DAYTONA_DAEMON`, and the code
/// the SDKs map to a precise exception for the cases the daemon names
/// (`daytona/common/errors.py` `CODE_TO_ERROR`): file errors by status,
/// session/command errors for the 404 and 410 of sessions.
pub(super) fn error(status: StatusCode, message: impl Into<String>) -> Response<Body> {
    let message = message.into();
    let code = match status {
        StatusCode::GONE => Some("SESSION_ENDED"),
        StatusCode::NOT_FOUND
            if message.starts_with("command ")
                || message.starts_with("session ")
                || message.starts_with("PTY session ") =>
        {
            Some("PROCESS_NOT_FOUND")
        }
        _ => None,
    };
    match code {
        Some(c) => daemon_error(status, c, message),
        None => daemon_error_plain(status, message),
    }
}

fn daemon_error_plain(status: StatusCode, message: String) -> Response<Body> {
    super::daemon_plain(status, message)
}

/// The exit code of a finished process the way a shell reports it: the
/// code, or 128 plus the signal that killed it (-1 when unknown).
pub fn end_code(e: &pb::ProcessEnd) -> i32 {
    match (e.exit_code, e.signal) {
        (Some(c), _) => c,
        (None, sig) if sig > 0 => 128 + sig,
        _ => -1,
    }
}

/// `204 No Content`.
pub(super) fn no_content() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("response")
}

/// A JSON body with a status other than 200.
pub(super) fn json_status(status: StatusCode, v: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v.to_string()))
        .expect("response")
}

/// A filesystem error with the daemon code of its status.
pub(super) fn fs_error(status: StatusCode, message: impl Into<String>) -> Response<Body> {
    let code = match status {
        StatusCode::NOT_FOUND => Some("FILE_NOT_FOUND"),
        StatusCode::FORBIDDEN => Some("FILE_ACCESS_DENIED"),
        _ => None,
    };
    match code {
        Some(c) => daemon_error(status, c, message),
        None => super::daemon_plain(status, message.into()),
    }
}

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/{id}/process/execute", post(execute))
        .route("/{id}/process/code-run", post(code_run))
        .route(
            "/{id}/process/session",
            post(create_session).get(list_sessions),
        )
        .route("/{id}/process/session/entrypoint", get(entrypoint))
        .route(
            "/{id}/process/session/entrypoint/logs",
            get(entrypoint_logs),
        )
        .route(
            "/{id}/process/session/{sid}",
            get(get_session).delete(delete_session),
        )
        .route("/{id}/process/session/{sid}/exec", post(session_exec))
        .route(
            "/{id}/process/session/{sid}/command/{cid}",
            get(get_command),
        )
        .route(
            "/{id}/process/session/{sid}/command/{cid}/logs",
            get(command_logs),
        )
        .route(
            "/{id}/process/session/{sid}/command/{cid}/input",
            post(command_input),
        )
        .route("/{id}/files", get(list_files).delete(delete_file))
        .route("/{id}/files/info", get(file_info))
        .route("/{id}/files/folder", post(create_folder))
        .route("/{id}/files/move", post(move_file))
        .route("/{id}/files/permissions", post(set_permissions))
        .route("/{id}/files/find", get(find_in_files))
        .route("/{id}/files/search", get(search_files))
        .route("/{id}/files/replace", post(replace_in_files))
        .route("/{id}/files/download", get(download))
        .route("/{id}/files/upload-v2", post(upload_v2))
        .route("/{id}/system/metrics", get(system_metrics))
        .route("/{id}/env", post(update_env))
        .route("/{id}/files/bulk-download", post(bulk_download))
        .route("/{id}/files/bulk-upload", post(bulk_upload))
        .route("/{id}/user-home-dir", get(user_home_dir))
        .route("/{id}/work-dir", get(home_dir))
        .route("/{id}/version", get(version))
        .route("/{id}/port", get(ports))
        .route("/{id}/port/{port}/in-use", get(port_in_use))
        .route("/{id}/process/pty", post(pty_create).get(pty_list))
        .route("/{id}/process/pty/create-connect", get(pty_create_connect))
        .route("/{id}/process/pty/{pid}", get(pty_info).delete(pty_kill))
        .route("/{id}/process/pty/{pid}/connect", get(pty_connect))
        .route("/{id}/process/pty/{pid}/resize", post(pty_resize))
        .with_state(d.clone())
        .merge(super::git::routes(d.clone()))
        .merge(super::lsp::routes(d.clone()))
        .merge(super::interpreter::routes(d.clone()))
        .merge(super::computer::routes(d))
}

/// The caller's live sandbox, its spacesd and its user; activity resets
/// its idle timer.
pub(super) async fn sandbox(
    d: &Daytona,
    headers: &HeaderMap,
    id: &str,
) -> Result<(Record, cua_spacesd_client::SpacesdClient, String), Response<Body>> {
    let key = d
        .caller(headers)
        .await
        .ok_or_else(|| super::error(StatusCode::UNAUTHORIZED, "Invalid credentials"))?;
    let r = d
        .find(&key, id)
        .await
        .ok_or_else(|| super::error(StatusCode::NOT_FOUND, format!("Sandbox {id} not found")))?;
    if r.paused || r.extra.contains_key("dtState") {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!(
                "Sandbox {id} is not started (state {})",
                super::api::state_of(&r)
            ),
        ));
    }
    super::api::touch(d, &r).await;
    let c = d.spacesd(&r).await.map_err(backend_error)?;
    let user = d.user_of(&r);
    Ok((r, c, user))
}

/// envd-style signed file URL check (Daytona's `download_url`/`upload_url`):
/// `signature = v1_` + base64url(HMAC-SHA256(key, `v1:files:<method>:<path>:<expires>`)).
pub fn file_signature(key: &str, method: &str, path: &str, expires: i64) -> String {
    use base64::Engine;
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).expect("hmac key");
    mac.update(format!("v1:files:{method}:{path}:{expires}").as_bytes());
    format!(
        "v1_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

#[derive(Deserialize, Default)]
struct SignedQuery {
    path: Option<String>,
    expires: Option<i64>,
    signature: Option<String>,
}

/// A request authorized by Bearer key, or by a signed URL for `method`.
async fn sandbox_or_signed(
    d: &Daytona,
    headers: &HeaderMap,
    id: &str,
    method: &str,
    q: &SignedQuery,
) -> Result<(Record, cua_spacesd_client::SpacesdClient, String), Response<Body>> {
    let Some(sig) = q.signature.as_deref() else {
        return sandbox(d, headers, id).await;
    };
    let expires = q.expires.unwrap_or(0);
    if expires > 0 && expires < (crate::state::now_ms() / 1000) as i64 {
        return Err(error(StatusCode::FORBIDDEN, "signed URL expired"));
    }
    let r =
        d.store.get(id).await.ok_or_else(|| {
            super::error(StatusCode::NOT_FOUND, format!("Sandbox {id} not found"))
        })?;
    let want = file_signature(
        &r.access_token,
        method,
        q.path.as_deref().unwrap_or(""),
        expires,
    );
    if !crate::auth::same(&want, sig) {
        return Err(error(StatusCode::FORBIDDEN, "invalid signature"));
    }
    if r.paused {
        return Err(error(
            StatusCode::BAD_REQUEST,
            format!("Sandbox {id} is not started"),
        ));
    }
    super::api::touch(d, &r).await;
    let c = d.spacesd(&r).await.map_err(backend_error)?;
    let user = d.user_of(&r);
    Ok((r, c, user))
}

pub(super) async fn body_json(body: Body) -> Result<Value, Response<Body>> {
    let bytes = http_body_util::BodyExt::collect(http_body_util::Limited::new(body, 8 << 20))
        .await
        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("reading body: {e}")))?
        .to_bytes();
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")))
}

pub(super) fn fs_err(e: FsError) -> Response<Body> {
    fs_error(e.0, e.1)
}

pub(super) fn status_err(s: tonic::Status) -> Response<Body> {
    fs_err(FsError::from(&s))
}

/// The first of zsh, bash, sh in the guest (Daytona's rule), found once
/// per sandbox.
pub(super) async fn shell_of(
    d: &Daytona,
    r: &Record,
    c: &cua_spacesd_client::SpacesdClient,
) -> String {
    if let Some(s) = d.shells.lock().await.get(&r.id) {
        return s.clone();
    }
    let probe = cua_spacesd_client::Command::new("/bin/sh").args([
        "-c",
        "for s in zsh bash sh; do command -v $s && exit 0; done; exit 1",
    ]);
    let sh = match c.run(probe).await {
        Ok(o) if o.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .filter(|l| l.starts_with('/'))
            .unwrap_or("/bin/sh")
            .to_string(),
        _ => "/bin/sh".to_string(),
    };
    d.shells.lock().await.insert(r.id.clone(), sh.clone());
    sh
}

pub(super) fn shell(sh: &str, script: &str) -> Vec<String> {
    vec![sh.to_string(), "-c".into(), script.to_string()]
}

/// Runs a command to completion: exit code, combined output (in arrival
/// order), stdout, stderr. `timeout`: the process is killed at the limit
/// (exit code -1).
pub(super) async fn run_collect(
    c: &cua_spacesd_client::SpacesdClient,
    argv: Vec<String>,
    env: HashMap<String, String>,
    cwd: String,
    user: &str,
    driver: &str,
    timeout: Option<Duration>,
) -> Result<(i32, String, String, String), tonic::Status> {
    run_collect_t(c, argv, env, cwd, user, driver, timeout)
        .await
        .map(|(code, a, o, e, _)| (code, a, o, e))
}

/// [`run_collect`] that also says whether the timeout ended the command.
pub(super) async fn run_collect_t(
    c: &cua_spacesd_client::SpacesdClient,
    argv: Vec<String>,
    env: HashMap<String, String>,
    cwd: String,
    user: &str,
    driver: &str,
    timeout: Option<Duration>,
) -> Result<(i32, String, String, String, bool), tonic::Status> {
    use pb::process_data::Output;
    use pb::process_event::Event;
    let mut it = argv.into_iter();
    let (command, args, user, cwd) = crate::guest::as_user_in(
        driver,
        user,
        it.next().unwrap_or_default(),
        it.collect(),
        cwd,
    );
    let req = pb::StartProcessRequest {
        config: Some(pb::ProcessConfig {
            command,
            args,
            env,
            cwd,
            user,
            timeout: timeout.map(|t| pbjson_types::Duration {
                seconds: t.as_secs() as i64,
                nanos: t.subsec_nanos() as i32,
            }),
        }),
        kill_on_disconnect: true,
        ..Default::default()
    };
    let mut s = c.process().start_process(req).await?.into_inner();
    let (mut all, mut out, mut err) = (Vec::new(), Vec::new(), Vec::new());
    let mut code = -1;
    let mut timed_out = false;
    while let Some(m) = s.next().await {
        match m?.event.and_then(|e| e.event) {
            Some(Event::Data(dd)) => match dd.output {
                Some(Output::Stdout(b)) => {
                    all.extend_from_slice(&b);
                    out.extend_from_slice(&b);
                }
                Some(Output::Stderr(b)) => {
                    all.extend_from_slice(&b);
                    err.extend_from_slice(&b);
                }
                Some(Output::Pty(b)) => all.extend_from_slice(&b),
                None => {}
            },
            Some(Event::End(e)) => {
                timed_out = e.timed_out;
                code = if e.timed_out { -1 } else { end_code(&e) };
                break;
            }
            _ => {}
        }
    }
    let s = |b: Vec<u8>| String::from_utf8_lossy(&b).into_owned();
    Ok((code, s(all), s(out), s(err), timed_out))
}

pub(super) async fn workdir(d: &Daytona, r: &Record, user: &str) -> String {
    // The image's WORKDIR (a Dockerfile build), else the user's home: where
    // Daytona runs commands by default.
    match r.extra.get("workDir").and_then(Value::as_str) {
        Some(w) if w.starts_with('/') => w.to_string(),
        _ => d.home(r, user).await,
    }
}

pub(super) fn timeout_of(b: &Value) -> Option<Duration> {
    b.get("timeout")
        .and_then(Value::as_u64)
        .filter(|t| *t > 0)
        .map(Duration::from_secs)
}

/// The error a command that outlived its `timeout` gets: 408 with the
/// daemon code the SDKs map to `DaytonaProcessExecutionTimeoutError`
/// (daytona/common/errors.py `CODE_TO_ERROR`).
fn timeout_error(t: Option<Duration>) -> Response<Body> {
    super::daemon_error(
        StatusCode::REQUEST_TIMEOUT,
        "PROCESS_EXECUTION_TIMEOUT",
        format!(
            "command execution timeout after {} seconds",
            t.map_or(0, |t| t.as_secs())
        ),
    )
}

/// `POST /process/execute` `{command, cwd?, envs?, timeout?}` ->
/// `{exitCode, result}` (combined output).
async fn execute(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(command) = b.get("command").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "command is required");
    };
    let home = workdir(&d, &r, &user).await;
    let cwd = b
        .get("cwd")
        .and_then(Value::as_str)
        .map(|p| resolve_path(p, &home))
        .unwrap_or(home);
    let mut env: HashMap<String, String> = r.env_vars.clone().into_iter().collect();
    if let Some(m) = b.get("envs").and_then(Value::as_object) {
        for (k, v) in m {
            env.insert(
                k.clone(),
                v.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string()),
            );
        }
    }
    let sh = shell_of(&d, &r, &c).await;
    let driver = d.drivers.of(&r.id, &c).await;
    let timeout = timeout_of(&b);
    match run_collect_t(&c, shell(&sh, command), env, cwd, &user, &driver, timeout).await {
        Ok((_, _, _, _, true)) => timeout_error(timeout),
        Ok((code, all, _, _, false)) => ok_json(json!({"exitCode": code, "result": all})),
        Err(s) => status_err(s),
    }
}

/// `POST /process/code-run` `{code, language?, argv?, envs?, timeout?}`.
async fn code_run(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(code) = b.get("code").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "code is required");
    };
    let language = b
        .get("language")
        .and_then(Value::as_str)
        .or_else(|| {
            r.extra
                .get("labels")
                .and_then(|l| l.get("code-toolbox-language"))
                .and_then(Value::as_str)
        })
        .unwrap_or("python");
    let mut argv: Vec<String> = match language {
        "python" => vec!["python3".into(), "-c".into(), super::charts::wrap(code)],
        "javascript" => vec!["node".into(), "-e".into(), code.into()],
        "typescript" => vec![
            "npx".into(),
            "--yes".into(),
            "tsx".into(),
            "-e".into(),
            code.into(),
        ],
        other => {
            return error(
                StatusCode::BAD_REQUEST,
                format!("unsupported language {other}"),
            );
        }
    };
    if let Some(a) = b.get("argv").and_then(Value::as_array) {
        argv.extend(a.iter().filter_map(|v| v.as_str().map(str::to_string)));
    }
    let mut env: HashMap<String, String> = r.env_vars.clone().into_iter().collect();
    if let Some(m) = b.get("envs").and_then(Value::as_object) {
        for (k, v) in m {
            env.insert(
                k.clone(),
                v.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string()),
            );
        }
    }
    let home = workdir(&d, &r, &user).await;
    let driver = d.drivers.of(&r.id, &c).await;
    let timeout = timeout_of(&b);
    match run_collect_t(&c, argv, env, home, &user, &driver, timeout).await {
        Ok((_, _, _, _, true)) => timeout_error(timeout),
        Ok((code, all, out, _, false)) => {
            // Figures the Python code left open (see charts.rs).
            let (all, charts) = super::charts::split(&all);
            let (out, _) = super::charts::split(&out);
            ok_json(json!({
                "exitCode": code,
                "result": all,
                "artifacts": {"stdout": out, "charts": charts},
            }))
        }
        Err(s) => status_err(s),
    }
}

// ── sessions ────────────────────────────────────────────────────────────

/// One session command's state and output.
pub struct Cmd {
    pub id: String,
    pub command: String,
    pub exit: watch::Receiver<Option<i32>>,
    pub logs: Mutex<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    pub live: broadcast::Sender<(bool, Bytes)>,
    pub pid: Mutex<Option<u32>>,
}

#[derive(Default)]
pub struct Session {
    pub commands: Vec<Arc<Cmd>>,
}

/// Sessions by (sandbox id, session id).
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<(String, String), Session>>);

impl Sessions {
    pub async fn drop_sandbox(&self, id: &str) {
        self.0.lock().await.retain(|(sid, _), _| sid != id);
    }
}

fn command_json(c: &Cmd) -> Value {
    let mut v = json!({"id": c.id, "command": c.command});
    if let Some(code) = *c.exit.borrow() {
        v["exitCode"] = json!(code);
    }
    v
}

fn session_json(sid: &str, s: &Session) -> Value {
    json!({
        "sessionId": sid,
        "commands": s.commands.iter().map(|c| command_json(c)).collect::<Vec<_>>(),
    })
}

async fn create_session(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, _, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(sid) = b
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return error(StatusCode::BAD_REQUEST, "sessionId is required");
    };
    if sid.contains('/') || sid == "entrypoint" {
        return error(StatusCode::BAD_REQUEST, "invalid sessionId");
    }
    let mut all = d.sessions.0.lock().await;
    let key = (r.id.clone(), sid.to_string());
    if all.contains_key(&key) {
        return error(
            StatusCode::CONFLICT,
            format!("session {sid} already exists"),
        );
    }
    all.insert(key, Session::default());
    Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .expect("response")
}

async fn list_sessions(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, _, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let all = d.sessions.0.lock().await;
    ok_json(Value::Array(
        all.iter()
            .filter(|((sid, _), _)| *sid == r.id)
            .map(|((_, s), sess)| session_json(s, sess))
            .collect(),
    ))
}

async fn get_session(
    State(d): State<Arc<Daytona>>,
    Path((id, sid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, _, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    match d.sessions.0.lock().await.get(&(r.id.clone(), sid.clone())) {
        Some(s) => ok_json(session_json(&sid, s)),
        None => error(StatusCode::NOT_FOUND, format!("session {sid} not found")),
    }
}

async fn delete_session(
    State(d): State<Arc<Daytona>>,
    Path((id, sid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Some(sess) = d
        .sessions
        .0
        .lock()
        .await
        .remove(&(r.id.clone(), sid.clone()))
    else {
        return error(StatusCode::NOT_FOUND, format!("session {sid} not found"));
    };
    for cmd in sess.commands {
        if cmd.exit.borrow().is_none()
            && let Some(pid) = *cmd.pid.lock().await
        {
            let _ = c
                .process()
                .signal_process(pb::SignalProcessRequest {
                    process: Some(pb::ProcessSelector {
                        selector: Some(pb::process_selector::Selector::Pid(pid)),
                    }),
                    signal: pb::Signal::Kill as i32,
                    process_group: true,
                })
                .await;
        }
    }
    let _ = c
        .run(
            cua_spacesd_client::Command::new("rm")
                .args(["-rf", &format!("/tmp/.cua-sessions/{sid}")])
                .user("root"),
        )
        .await;
    no_content()
}

/// The session wrapper: restore cwd and exports, run, save them.
fn session_script(sid: &str) -> String {
    format!(
        "d=/tmp/.cua-sessions/{sid}; mkdir -p \"$d\"; \
         [ -f \"$d/env\" ] && . \"$d/env\"; \
         [ -f \"$d/cwd\" ] && cd \"$(cat \"$d/cwd\")\"; \
         __c=\"$CUA_SESSION_CMD\"; unset CUA_SESSION_CMD; \
         eval \"$__c\"; __e=$?; \
         pwd > \"$d/cwd\"; export -p > \"$d/env\"; exit $__e"
    )
}

/// Waits until a command has ended (or its sender is gone): the next
/// command of a session starts after it, never beside it.
async fn wait_exit(mut p: watch::Receiver<Option<i32>>) {
    while p.borrow().is_none() {
        if p.changed().await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod session_order_tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Commands submitted together run one after another, whatever their
    /// own durations, and each reports its exit code only after it ran.
    #[tokio::test]
    async fn chained_commands_run_in_submission_order() {
        let ran: Arc<StdMutex<Vec<u32>>> = Arc::default();
        let mut prev: Option<watch::Receiver<Option<i32>>> = None;
        let mut handles = Vec::new();
        // Later commands are the quick ones: without the chain they would
        // finish first.
        for (i, ms) in [(0u32, 60u64), (1, 40), (2, 20), (3, 0), (4, 0)] {
            let (tx, rx) = watch::channel(None);
            let (p, ran) = (prev.replace(rx.clone()), ran.clone());
            handles.push(tokio::spawn(async move {
                if let Some(p) = p {
                    wait_exit(p).await;
                }
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                ran.lock().unwrap().push(i);
                let _ = tx.send(Some(0));
            }));
            // the exit code of a command is not visible before it ran
            assert!(rx.borrow().is_none());
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(*ran.lock().unwrap(), vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn a_vanished_predecessor_does_not_block_the_queue() {
        let (tx, rx) = watch::channel::<Option<i32>>(None);
        drop(tx);
        tokio::time::timeout(std::time::Duration::from_secs(1), wait_exit(rx))
            .await
            .expect("a dropped sender releases the next command");
    }
}

/// `POST /process/session/{sid}/exec` `{command, runAsync|async}`.
async fn session_exec(
    State(d): State<Arc<Daytona>>,
    Path((id, sid)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    use pb::process_data::Output;
    use pb::process_event::Event;
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(command) = b.get("command").and_then(Value::as_str).map(str::to_string) else {
        return error(StatusCode::BAD_REQUEST, "command is required");
    };
    let run_async = b.get("runAsync").and_then(Value::as_bool).unwrap_or(false)
        || b.get("async").and_then(Value::as_bool).unwrap_or(false);
    if !d
        .sessions
        .0
        .lock()
        .await
        .contains_key(&(r.id.clone(), sid.clone()))
    {
        return error(StatusCode::NOT_FOUND, format!("session {sid} not found"));
    }
    let home = workdir(&d, &r, &user).await;
    let mut env: HashMap<String, String> = r.env_vars.clone().into_iter().collect();
    env.insert("CUA_SESSION_CMD".into(), command.clone());
    let sh = shell_of(&d, &r, &c).await;
    let driver = d.drivers.of(&r.id, &c).await;
    let mut argv = shell(&sh, &session_script(&sid)).into_iter();
    let (run_cmd, run_args, run_user, home) = crate::guest::as_user_in(
        &driver,
        &user,
        argv.next().unwrap_or_default(),
        argv.collect(),
        home,
    );
    let req = pb::StartProcessRequest {
        config: Some(pb::ProcessConfig {
            command: run_cmd,
            args: run_args,
            env,
            cwd: home,
            user: run_user,
            timeout: None,
        }),
        stdin: true,
        kill_on_disconnect: false,
        ..Default::default()
    };
    let (exit_tx, exit_rx) = watch::channel(None);
    let cmd = Arc::new(Cmd {
        id: super::api_uuid(),
        command,
        exit: exit_rx,
        logs: Mutex::new((Vec::new(), Vec::new(), Vec::new())),
        live: broadcast::channel(256).0,
        pid: Mutex::new(None),
    });
    // Registered in submission order, with the command it must wait for: a
    // session is one shell, so its commands run strictly one after another
    // and a command reports its exit code only once it has run.
    let prev = match d
        .sessions
        .0
        .lock()
        .await
        .get_mut(&(r.id.clone(), sid.clone()))
    {
        Some(s) => {
            let prev = s.commands.last().map(|c| c.exit.clone());
            s.commands.push(cmd.clone());
            prev
        }
        None => return error(StatusCode::NOT_FOUND, format!("session {sid} not found")),
    };
    let work = {
        let cmd = cmd.clone();
        let c = c.clone();
        async move {
            if let Some(p) = prev {
                wait_exit(p).await;
            }
            let mut stream = match c.process().start_process(req).await {
                Ok(s) => s.into_inner(),
                Err(s) => {
                    let _ = exit_tx.send(Some(-1));
                    return Err(s);
                }
            };
            let mut code = -1;
            while let Some(m) = stream.next().await {
                let Ok(m) = m else { break };
                match m.event.and_then(|e| e.event) {
                    Some(Event::Start(s)) => *cmd.pid.lock().await = Some(s.pid),
                    Some(Event::Data(dd)) => {
                        let (is_err, b) = match dd.output {
                            Some(Output::Stdout(b)) | Some(Output::Pty(b)) => (false, b),
                            Some(Output::Stderr(b)) => (true, b),
                            None => continue,
                        };
                        // Appended and broadcast under one lock: a
                        // follower that snapshots and subscribes under it
                        // misses nothing and sees nothing twice.
                        let mut l = cmd.logs.lock().await;
                        l.0.extend_from_slice(&b);
                        if is_err {
                            l.2.extend_from_slice(&b);
                        } else {
                            l.1.extend_from_slice(&b);
                        }
                        let _ = cmd.live.send((is_err, Bytes::from(b)));
                        drop(l);
                    }
                    Some(Event::End(e)) => {
                        code = end_code(&e);
                        break;
                    }
                    _ => {}
                }
            }
            let _ = exit_tx.send(Some(code));
            Ok(())
        }
    };
    if run_async {
        tokio::spawn(work);
        return ok_json(json!({"cmdId": cmd.id}));
    }
    if let Err(s) = work.await {
        return status_err(s);
    }
    let l = cmd.logs.lock().await;
    let s = |b: &Vec<u8>| String::from_utf8_lossy(b).into_owned();
    ok_json(json!({
        "cmdId": cmd.id,
        "exitCode": *cmd.exit.borrow(),
        "output": s(&l.0),
        "stdout": s(&l.1),
        "stderr": s(&l.2),
    }))
}

async fn find_cmd(d: &Daytona, rid: &str, sid: &str, cid: &str) -> Option<Arc<Cmd>> {
    d.sessions
        .0
        .lock()
        .await
        .get(&(rid.to_string(), sid.to_string()))?
        .commands
        .iter()
        .find(|c| c.id == cid)
        .cloned()
}

async fn get_command(
    State(d): State<Arc<Daytona>>,
    Path((id, sid, cid)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, _, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    match find_cmd(&d, &r.id, &sid, &cid).await {
        Some(c) => ok_json(command_json(&c)),
        None => error(StatusCode::NOT_FOUND, format!("command {cid} not found")),
    }
}

#[derive(Deserialize, Default)]
struct LogsQuery {
    follow: Option<bool>,
}

/// `GET .../command/{cid}/logs[?follow=true]`: JSON, or a WebSocket that
/// streams `\x01\x01\x01`/`\x02\x02\x02`-prefixed stdout/stderr frames
/// until the command ends.
async fn command_logs(
    State(d): State<Arc<Daytona>>,
    Path((id, sid, cid)): Path<(String, String, String)>,
    headers: HeaderMap,
    Query(q): Query<LogsQuery>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response<Body> {
    let (r, _, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Some(cmd) = find_cmd(&d, &r.id, &sid, &cid).await else {
        return error(StatusCode::NOT_FOUND, format!("command {cid} not found"));
    };
    if let (Ok(ws), true) = (ws, q.follow.unwrap_or(false)) {
        return ws.on_upgrade(move |socket| follow(socket, cmd));
    }
    let l = cmd.logs.lock().await;
    let s = |b: &Vec<u8>| String::from_utf8_lossy(b).into_owned();
    ok_json(json!({"output": s(&l.0), "stdout": s(&l.1), "stderr": s(&l.2)}))
}

async fn follow(mut socket: WebSocket, cmd: Arc<Cmd>) {
    let frame = |is_err: bool, b: &[u8]| {
        let mut v = if is_err {
            STDERR_PREFIX.to_vec()
        } else {
            STDOUT_PREFIX.to_vec()
        };
        v.extend_from_slice(b);
        Message::Binary(v.into())
    };
    let (mut live, out, err) = {
        let l = cmd.logs.lock().await;
        (cmd.live.subscribe(), l.1.clone(), l.2.clone())
    };
    if !out.is_empty() && socket.send(frame(false, &out)).await.is_err() {
        return;
    }
    if !err.is_empty() && socket.send(frame(true, &err)).await.is_err() {
        return;
    }
    let mut exit = cmd.exit.clone();
    loop {
        if exit.borrow().is_some() {
            while let Ok((is_err, b)) = live.try_recv() {
                if socket.send(frame(is_err, &b)).await.is_err() {
                    return;
                }
            }
            break;
        }
        tokio::select! {
            m = live.recv() => match m {
                Ok((is_err, b)) => {
                    if socket.send(frame(is_err, &b)).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = exit.changed() => {}
        }
    }
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code: 1000,
            reason: "".into(),
        })))
        .await;
}

async fn command_input(
    State(d): State<Arc<Daytona>>,
    Path((id, sid, cid)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(cmd) = find_cmd(&d, &r.id, &sid, &cid).await else {
        return error(StatusCode::NOT_FOUND, format!("command {cid} not found"));
    };
    if cmd.exit.borrow().is_some() {
        return daemon_error(
            StatusCode::GONE,
            "COMMAND_ALREADY_COMPLETED",
            format!("command {cid} has already completed"),
        );
    }
    let Some(pid) = *cmd.pid.lock().await else {
        return error(StatusCode::BAD_REQUEST, "command has not started");
    };
    let data = b
        .get("data")
        .and_then(Value::as_str)
        .unwrap_or("")
        .as_bytes()
        .to_vec();
    match c
        .process()
        .send_input(pb::SendInputRequest {
            process: Some(pb::ProcessSelector {
                selector: Some(pb::process_selector::Selector::Pid(pid)),
            }),
            input: Some(pb::ProcessInput {
                input: Some(pb::process_input::Input::Stdin(data)),
            }),
            sequence: 0,
            writer_id: String::new(),
        })
        .await
    {
        Ok(_) => no_content(),
        Err(s) => status_err(s),
    }
}

/// Cua images have no Daytona entrypoint session; it is reported empty.
async fn entrypoint(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(e) = sandbox(&d, &headers, &id).await {
        return e;
    }
    ok_json(json!({"sessionId": "entrypoint", "commands": []}))
}

async fn entrypoint_logs(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(e) = sandbox(&d, &headers, &id).await {
        return e;
    }
    ok_json(json!({"output": "", "stdout": "", "stderr": ""}))
}

// ── PTY sessions ────────────────────────────────────────────────────────
//
// A Daytona PTY session is a cua-spacesd PTY process tagged
// `daytona-pty-<id>`: create starts it, connect attaches a WebSocket
// (binary frames both ways; `{"type":"control","status":...}` text frames
// for connected / exited; close 1000 with `{"exitCode":...}` as the
// reason), resize and kill act on the tag.

/// PTY sessions' metadata by (sandbox id, session id).
#[derive(Default)]
pub struct Ptys(Mutex<HashMap<(String, String), PtyInfo>>);

#[derive(Clone, Debug)]
pub struct PtyInfo {
    pub cwd: String,
    pub envs: BTreeMap<String, String>,
    pub cols: u32,
    pub rows: u32,
    pub created_ms: u64,
}

fn pty_tag(id: &str) -> String {
    format!("daytona-pty-{id}")
}

fn pty_json(id: &str, i: &PtyInfo, active: bool) -> Value {
    json!({
        "id": id,
        "cwd": i.cwd,
        "envs": i.envs,
        "cols": i.cols,
        "rows": i.rows,
        "createdAt": time::OffsetDateTime::from_unix_timestamp((i.created_ms / 1000) as i64)
            .ok()
            .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
        "active": active,
        "lazyStart": false,
    })
}

#[derive(Deserialize, Default)]
struct PtyQuery {
    id: Option<String>,
    cols: Option<u32>,
    rows: Option<u32>,
    cwd: Option<String>,
}

async fn pty_running(c: &cua_spacesd_client::SpacesdClient, id: &str) -> bool {
    c.process()
        .list_processes(pb::ListProcessesRequest {
            include_exited: false,
            tag_prefix: pty_tag(id),
        })
        .await
        .map(|r| {
            r.into_inner()
                .processes
                .iter()
                .any(|p| p.tag == pty_tag(id))
        })
        .unwrap_or(false)
}

/// Starts the PTY process for a session.
#[allow(clippy::too_many_arguments)]
async fn pty_start(
    d: &Daytona,
    r: &Record,
    c: &cua_spacesd_client::SpacesdClient,
    user: &str,
    id: &str,
    cwd: Option<String>,
    envs: BTreeMap<String, String>,
    cols: u32,
    rows: u32,
) -> Result<PtyInfo, Response<Body>> {
    if id.is_empty() || id.contains('/') {
        return Err(error(StatusCode::BAD_REQUEST, "invalid PTY session id"));
    }
    if pty_running(c, id).await {
        return Err(error(
            StatusCode::CONFLICT,
            format!("PTY session {id} already exists"),
        ));
    }
    let home = workdir(d, r, user).await;
    let cwd = cwd.map(|p| resolve_path(&p, &home)).unwrap_or(home);
    let sh = shell_of(d, r, c).await;
    let driver = d.drivers.of(&r.id, c).await;
    let mut env: HashMap<String, String> = r.env_vars.clone().into_iter().collect();
    env.extend(envs.clone());
    env.entry("TERM".into())
        .or_insert_with(|| "xterm-256color".into());
    let (command, args, u, run_cwd) =
        crate::guest::as_user_in(&driver, user, sh, vec!["-l".into()], cwd.clone());
    c.process()
        .start_process(pb::StartProcessRequest {
            config: Some(pb::ProcessConfig {
                command,
                args,
                env,
                cwd: run_cwd,
                user: u,
                timeout: None,
            }),
            pty: Some(pb::PtyConfig {
                size: Some(pb::PtySize {
                    cols,
                    rows,
                    ..Default::default()
                }),
                term: "xterm-256color".into(),
            }),
            tag: pty_tag(id),
            stdin: true,
            kill_on_disconnect: false,
            ..Default::default()
        })
        .await
        .map_err(status_err)?;
    let info = PtyInfo {
        cwd,
        envs,
        cols,
        rows,
        created_ms: crate::state::now_ms(),
    };
    d.ptys
        .0
        .lock()
        .await
        .insert((r.id.clone(), id.to_string()), info.clone());
    Ok(info)
}

/// `POST /process/pty` `{id, cwd?, envs?, cols?, rows?}` -> `{sessionId}`.
async fn pty_create(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let sid = b
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let envs = b
        .get("envs")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let cols = b.get("cols").and_then(Value::as_u64).unwrap_or(80) as u32;
    let rows = b.get("rows").and_then(Value::as_u64).unwrap_or(24) as u32;
    let cwd = b.get("cwd").and_then(Value::as_str).map(str::to_string);
    match pty_start(&d, &r, &c, &user, &sid, cwd, envs, cols, rows).await {
        Ok(_) => json_status(StatusCode::CREATED, json!({"sessionId": sid})),
        Err(e) => e,
    }
}

/// The envs a client sends as a `X-Daytona-Pty-Envs~<base64url JSON>`
/// WebSocket subprotocol.
fn envs_from_protocols(headers: &HeaderMap) -> BTreeMap<String, String> {
    use base64::Engine;
    headers
        .get_all("sec-websocket-protocol")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|p| p.trim().strip_prefix("X-Daytona-Pty-Envs~"))
        .filter_map(|b| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(b.trim_end_matches('='))
                .ok()
        })
        .filter_map(|j| serde_json::from_slice::<BTreeMap<String, String>>(&j).ok())
        .next()
        .unwrap_or_default()
}

/// `GET /process/pty/create-connect?id&cols&rows&cwd` (WebSocket).
async fn pty_create_connect(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<PtyQuery>,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Ok(ws) = ws else {
        return error(StatusCode::BAD_REQUEST, "expected a WebSocket upgrade");
    };
    let sid = q.id.clone().unwrap_or_default();
    if let Err(e) = pty_start(
        &d,
        &r,
        &c,
        &user,
        &sid,
        q.cwd.clone(),
        envs_from_protocols(&headers),
        q.cols.unwrap_or(80),
        q.rows.unwrap_or(24),
    )
    .await
    {
        return e;
    }
    ws.protocols(["X-Daytona-Pty-Exit-Control"])
        .on_upgrade(move |socket| pty_session(socket, c, sid))
}

/// `GET /process/pty/{pid}/connect` (WebSocket): attach to a running PTY.
async fn pty_connect(
    State(d): State<Arc<Daytona>>,
    Path((id, pid)): Path<(String, String)>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response<Body> {
    let (_, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let Ok(ws) = ws else {
        return error(StatusCode::BAD_REQUEST, "expected a WebSocket upgrade");
    };
    if !pty_running(&c, &pid).await {
        return error(
            StatusCode::NOT_FOUND,
            format!("PTY session {pid} not found"),
        );
    }
    ws.protocols(["X-Daytona-Pty-Exit-Control"])
        .on_upgrade(move |socket| pty_session(socket, c, pid))
}

async fn pty_session(mut socket: WebSocket, c: cua_spacesd_client::SpacesdClient, sid: String) {
    use pb::process_data::Output;
    use pb::process_event::Event;
    let sel = pb::ProcessSelector {
        selector: Some(pb::process_selector::Selector::Tag(pty_tag(&sid))),
    };
    let mut events = match c
        .process()
        .connect_process(pb::ConnectProcessRequest {
            process: Some(sel.clone()),
            replay_bytes: 64 * 1024,
            replay_from_offset: None,
            keepalive_interval: None,
        })
        .await
    {
        Ok(s) => s.into_inner(),
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    json!({"type": "control", "status": "error", "error": e.message()})
                        .to_string()
                        .into(),
                ))
                .await;
            return;
        }
    };
    let _ = socket
        .send(Message::Text(
            json!({"type": "control", "status": "connected"})
                .to_string()
                .into(),
        ))
        .await;
    let mut exit: Option<i32> = None;
    loop {
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(m)) => match m.event.and_then(|e| e.event) {
                    Some(Event::Data(dd)) => {
                        if let Some(Output::Pty(b) | Output::Stdout(b) | Output::Stderr(b)) = dd.output
                            && socket.send(Message::Binary(b.into())).await.is_err()
                        {
                            return;
                        }
                    }
                    Some(Event::End(e)) => {
                        exit = Some(end_code(&e));
                        break;
                    }
                    _ => {}
                },
                _ => break,
            },
            m = socket.recv() => match m {
                Some(Ok(Message::Binary(b))) => {
                    let _ = c.process().send_input(pb::SendInputRequest {
                        process: Some(sel.clone()),
                        input: Some(pb::ProcessInput { input: Some(pb::process_input::Input::Pty(b.to_vec())) }),
                        sequence: 0,
                        writer_id: String::new(),
                    }).await;
                }
                Some(Ok(Message::Text(t))) => {
                    let _ = c.process().send_input(pb::SendInputRequest {
                        process: Some(sel.clone()),
                        input: Some(pb::ProcessInput { input: Some(pb::process_input::Input::Pty(t.as_bytes().to_vec())) }),
                        sequence: 0,
                        writer_id: String::new(),
                    }).await;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                _ => {}
            },
        }
    }
    let code = exit.unwrap_or(-1);
    let _ = socket
        .send(Message::Text(
            json!({"type": "control", "status": "exited", "exitCode": code})
                .to_string()
                .into(),
        ))
        .await;
    let _ = socket
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code: 1000,
            reason: json!({"exitCode": code}).to_string().into(),
        })))
        .await;
}

async fn pty_list(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let infos: Vec<(String, PtyInfo)> = d
        .ptys
        .0
        .lock()
        .await
        .iter()
        .filter(|((sid, _), _)| *sid == r.id)
        .map(|((_, p), i)| (p.clone(), i.clone()))
        .collect();
    let mut out = Vec::new();
    for (p, i) in infos {
        let active = pty_running(&c, &p).await;
        out.push(pty_json(&p, &i, active));
    }
    ok_json(json!({"sessions": out}))
}

async fn pty_info(
    State(d): State<Arc<Daytona>>,
    Path((id, pid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let info = d
        .ptys
        .0
        .lock()
        .await
        .get(&(r.id.clone(), pid.clone()))
        .cloned();
    match info {
        Some(i) => ok_json(pty_json(&pid, &i, pty_running(&c, &pid).await)),
        None => error(
            StatusCode::NOT_FOUND,
            format!("PTY session {pid} not found"),
        ),
    }
}

async fn pty_kill(
    State(d): State<Arc<Daytona>>,
    Path((id, pid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let known = d
        .ptys
        .0
        .lock()
        .await
        .remove(&(r.id.clone(), pid.clone()))
        .is_some();
    let _ = c
        .process()
        .signal_process(pb::SignalProcessRequest {
            process: Some(pb::ProcessSelector {
                selector: Some(pb::process_selector::Selector::Tag(pty_tag(&pid))),
            }),
            signal: pb::Signal::Kill as i32,
            process_group: true,
        })
        .await;
    if known {
        ok_json(json!({}))
    } else {
        error(
            StatusCode::NOT_FOUND,
            format!("PTY session {pid} not found"),
        )
    }
}

async fn pty_resize(
    State(d): State<Arc<Daytona>>,
    Path((id, pid)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let cols = b.get("cols").and_then(Value::as_u64).unwrap_or(80) as u32;
    let rows = b.get("rows").and_then(Value::as_u64).unwrap_or(24) as u32;
    if let Err(s) = c
        .process()
        .resize_pty(pb::ResizePtyRequest {
            process: Some(pb::ProcessSelector {
                selector: Some(pb::process_selector::Selector::Tag(pty_tag(&pid))),
            }),
            size: Some(pb::PtySize {
                cols,
                rows,
                ..Default::default()
            }),
        })
        .await
    {
        return status_err(s);
    }
    let mut ptys = d.ptys.0.lock().await;
    let Some(i) = ptys.get_mut(&(r.id.clone(), pid.clone())) else {
        return error(
            StatusCode::NOT_FOUND,
            format!("PTY session {pid} not found"),
        );
    };
    i.cols = cols;
    i.rows = rows;
    let info = i.clone();
    drop(ptys);
    ok_json(pty_json(&pid, &info, true))
}

// ── files ───────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct FsQuery {
    path: Option<String>,
    depth: Option<u32>,
    recursive: Option<bool>,
    mode: Option<String>,
    owner: Option<String>,
    group: Option<String>,
    source: Option<String>,
    destination: Option<String>,
    pattern: Option<String>,
}

fn file_info_json(e: &pb::EntryInfo) -> Value {
    let modified = e
        .modified_at
        .as_ref()
        .map(crate::e2b::pb_timestamp)
        .unwrap_or_default();
    json!({
        "name": e.name,
        "path": e.path,
        "isDir": e.r#type == pb::FileType::Directory as i32,
        "size": e.size,
        "mode": crate::e2b::permissions_string(e.r#type, e.mode),
        "permissions": format!("{:04o}", e.mode & 0o7777),
        "owner": e.owner,
        "group": e.group,
        "modTime": modified,
        "modifiedAt": modified,
    })
}

async fn path_of(
    d: &Daytona,
    r: &Record,
    user: &str,
    q: &FsQuery,
) -> Result<String, Response<Body>> {
    let p = q
        .path
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "path is required"))?;
    Ok(resolve_path(p, &workdir(d, r, user).await))
}

async fn list_files(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match q.path.as_deref() {
        Some(_) => match path_of(&d, &r, &user, &q).await {
            Ok(p) => p,
            Err(e) => return e,
        },
        None => workdir(&d, &r, &user).await,
    };
    let mut entries = Vec::new();
    let mut token = String::new();
    loop {
        let resp = match c
            .filesystem()
            .list_dir(pb::ListDirRequest {
                path: path.clone(),
                depth: q.depth.unwrap_or(1).max(1),
                include_hidden: true,
                page_size: 0,
                page_token: token,
                ..Default::default()
            })
            .await
        {
            Ok(r) => r.into_inner(),
            Err(s) if s.code() == tonic::Code::PermissionDenied => {
                // A directory the driver user may not read (the sandbox
                // user's home): list it as the sandbox user.
                let driver = d.drivers.of(&r.id, &c).await;
                return match find_entries(
                    &c,
                    &driver,
                    &user,
                    &path,
                    q.depth.unwrap_or(1).max(1),
                    false,
                )
                .await
                {
                    Ok(v) => ok_json(Value::Array(v)),
                    Err(e) => e,
                };
            }
            Err(s) => return status_err(s),
        };
        entries.extend(resp.entries.iter().map(file_info_json));
        if resp.next_page_token.is_empty() {
            break;
        }
        token = resp.next_page_token;
    }
    ok_json(Value::Array(entries))
}

async fn file_info(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match path_of(&d, &r, &user, &q).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    match c
        .filesystem()
        .stat(pb::StatRequest {
            path: path.clone(),
            no_follow_symlinks: false,
            ..Default::default()
        })
        .await
    {
        Ok(resp) => ok_json(
            resp.into_inner()
                .entry
                .as_ref()
                .map(file_info_json)
                .unwrap_or(Value::Null),
        ),
        Err(s) if s.code() == tonic::Code::PermissionDenied => {
            let driver = d.drivers.of(&r.id, &c).await;
            match find_entries(&c, &driver, &user, &path, 0, true).await {
                Ok(mut v) if !v.is_empty() => ok_json(v.remove(0)),
                Ok(_) => fs_error(StatusCode::NOT_FOUND, format!("{path}: no such file")),
                Err(e) => e,
            }
        }
        Err(s) => status_err(s),
    }
}

/// Entries of `path` read as `user` with `find` (for what the driver user
/// cannot read). `depth` 0 with `itself`: the path alone.
async fn find_entries(
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    user: &str,
    path: &str,
    depth: u32,
    itself: bool,
) -> Result<Vec<Value>, Response<Body>> {
    let script = if itself {
        "find \"$1\" -maxdepth 0 -printf '%y\\t%s\\t%m\\t%u\\t%g\\t%T@\\t%p\\0'"
    } else {
        "find \"$1\" -mindepth 1 -maxdepth \"$2\" -printf '%y\\t%s\\t%m\\t%u\\t%g\\t%T@\\t%p\\0'"
    };
    let depth_s = depth.to_string();
    let out = guest_bytes(
        c,
        driver,
        user,
        &["/bin/sh", "-c", script, "sh", path, &depth_s],
    )
    .await?;
    Ok(parse_find(&String::from_utf8_lossy(&out)))
}

/// Parses `find -printf '%y\t%s\t%m\t%u\t%g\t%T@\t%p\0'` records.
pub fn parse_find(out: &str) -> Vec<Value> {
    out.split('\0')
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let f: Vec<&str> = r.splitn(7, '\t').collect();
            if f.len() != 7 {
                return None;
            }
            let is_dir = f[0] == "d";
            let mode = u32::from_str_radix(f[2], 8).unwrap_or(0);
            let secs = f[5]
                .split('.')
                .next()
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let t = time::OffsetDateTime::from_unix_timestamp(secs)
                .ok()
                .and_then(|t| {
                    t.format(&time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_default();
            let name = f[6].rsplit('/').next().unwrap_or(f[6]);
            Some(json!({
                "name": name, "path": f[6], "isDir": is_dir,
                "size": f[1].parse::<u64>().unwrap_or(0),
                "mode": format!("{}{}", if is_dir { "d" } else { "-" }, mode_string(mode)),
                "permissions": format!("{mode:04o}"),
                "owner": f[3], "group": f[4], "modTime": t, "modifiedAt": t,
            }))
        })
        .collect()
}

fn mode_string(m: u32) -> String {
    "rwxrwxrwx"
        .chars()
        .enumerate()
        .map(|(i, c)| if m & (1 << (8 - i)) != 0 { c } else { '-' })
        .collect()
}

/// Runs a short guest command as `user`; non-zero exit is a 400 with its
/// stderr.
pub(super) async fn guest(
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    user: &str,
    argv: &[&str],
) -> Result<String, Response<Body>> {
    guest_bytes(c, driver, user, argv)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Runs `argv` as `user` (through `sudo` when the driver cannot switch);
/// non-zero exit is a 400 with its stderr.
pub(super) async fn guest_bytes(
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    user: &str,
    argv: &[&str],
) -> Result<Vec<u8>, Response<Body>> {
    // Entered after the user switch: the driver's own directory may be
    // closed to `user`.
    let (command, args, u, cwd) = crate::guest::as_user_in(
        driver,
        user,
        argv[0].to_string(),
        argv[1..].iter().map(|a| a.to_string()).collect(),
        "/".into(),
    );
    let mut cmd = cua_spacesd_client::Command::new(command).args(args);
    if !u.is_empty() {
        cmd = cmd.user(u);
    }
    if !cwd.is_empty() {
        cmd = cmd.cwd(cwd);
    }
    match c.run(cmd).await {
        Ok(out) if out.success() => Ok(out.stdout),
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            let status = if err.contains("No such file") || err.contains("not found") {
                StatusCode::NOT_FOUND
            } else if err.contains("ermission denied") {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            Err(fs_error(status, err))
        }
        Err(e) => Err(error(StatusCode::BAD_GATEWAY, e.to_string())),
    }
}

/// Whether file operations for `user` must go through `sudo`.
fn switched(driver: &str, user: &str) -> bool {
    driver != "root" && user != driver
}

async fn create_folder(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match path_of(&d, &r, &user, &q).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mode = q.mode.clone().unwrap_or_else(|| "755".into());
    let Ok(mode) = u32::from_str_radix(&mode, 8) else {
        return error(StatusCode::BAD_REQUEST, "mode must be octal");
    };
    let driver = d.drivers.of(&r.id, &c).await;
    if switched(&driver, &user) {
        let m = format!("{mode:o}");
        return match guest(&c, &driver, &user, &["mkdir", "-p", "-m", &m, "--", &path]).await {
            Ok(_) => Response::builder()
                .status(StatusCode::CREATED)
                .body(Body::empty())
                .expect("response"),
            Err(e) => e,
        };
    }
    let from = if driver == "root" && user != "root" {
        crate::guestfs::first_missing(&c, &path).await
    } else {
        None
    };
    if let Err(s) = c
        .filesystem()
        .make_dir(pb::MakeDirRequest {
            path: path.clone(),
            parents: true,
            mode,
            ..Default::default()
        })
        .await
    {
        return status_err(s);
    }
    if let Some(from) = from {
        crate::guestfs::chown_created(&c, &user, &from).await;
    }
    Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .expect("response")
}

async fn delete_file(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match path_of(&d, &r, &user, &q).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let driver = d.drivers.of(&r.id, &c).await;
    if switched(&driver, &user) {
        let flags = if q.recursive.unwrap_or(false) {
            "-rf"
        } else {
            "-f"
        };
        let script = format!(
            "[ -e \"$1\" ] || [ -L \"$1\" ] || {{ echo \"No such file: $1\" >&2; exit 1; }}; rm {flags} -- \"$1\""
        );
        return match guest(&c, &driver, &user, &["/bin/sh", "-c", &script, "sh", &path]).await {
            Ok(_) => no_content(),
            Err(e) => e,
        };
    }
    match c
        .filesystem()
        .remove(pb::RemoveRequest {
            path,
            recursive: q.recursive.unwrap_or(false),
            missing_ok: false,
            ..Default::default()
        })
        .await
    {
        Ok(_) => no_content(),
        Err(s) => status_err(s),
    }
}

async fn move_file(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let (Some(src), Some(dst)) = (q.source.as_deref(), q.destination.as_deref()) else {
        return error(
            StatusCode::BAD_REQUEST,
            "source and destination are required",
        );
    };
    let home = workdir(&d, &r, &user).await;
    let driver = d.drivers.of(&r.id, &c).await;
    // The API lists 409 for a move: the destination is not replaced.
    let target = resolve_path(dst, &home);
    if c.filesystem()
        .stat(pb::StatRequest {
            path: target.clone(),
            no_follow_symlinks: true,
            ..Default::default()
        })
        .await
        .is_ok()
    {
        return super::daemon_plain(
            StatusCode::CONFLICT,
            format!("destination already exists: {target}"),
        );
    }
    if switched(&driver, &user) {
        let (a, b) = (resolve_path(src, &home), resolve_path(dst, &home));
        return match guest(&c, &driver, &user, &["mv", "-f", "--", &a, &b]).await {
            Ok(_) => ok_json(json!({})),
            Err(e) => e,
        };
    }
    match c
        .filesystem()
        .r#move(pb::MoveRequest {
            source: resolve_path(src, &home),
            destination: resolve_path(dst, &home),
            overwrite: true,
            create_parents: false,
            ..Default::default()
        })
        .await
    {
        Ok(_) => ok_json(json!({})),
        Err(s) => status_err(s),
    }
}

async fn set_permissions(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match path_of(&d, &r, &user, &q).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(mode) = q.mode.as_deref().filter(|m| !m.is_empty()) {
        if !mode.chars().all(|ch| ch.is_digit(8)) {
            return error(StatusCode::BAD_REQUEST, "mode must be octal");
        }
        if let Err(e) = guest(
            &c,
            &d.drivers.of(&r.id, &c).await,
            "root",
            &["chmod", mode, "--", &path],
        )
        .await
        {
            return e;
        }
    }
    let owner = match (q.owner.as_deref(), q.group.as_deref()) {
        (Some(o), Some(g)) if !o.is_empty() && !g.is_empty() => Some(format!("{o}:{g}")),
        (Some(o), _) if !o.is_empty() => Some(o.to_string()),
        (_, Some(g)) if !g.is_empty() => Some(format!(":{g}")),
        _ => None,
    };
    if let Some(o) = owner
        && let Err(e) = guest(
            &c,
            &d.drivers.of(&r.id, &c).await,
            "root",
            &["chown", &o, "--", &path],
        )
        .await
    {
        return e;
    }
    ok_json(json!({}))
}

/// `GET /files/find?path=&pattern=`: lines containing `pattern` -> `[Match]`.
async fn find_in_files(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match path_of(&d, &r, &user, &q).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let pattern = q.pattern.clone().unwrap_or_default();
    let driver = d.drivers.of(&r.id, &c).await;
    let (command, args, u, cwd) = crate::guest::as_user_in(
        &driver,
        &user,
        "grep".into(),
        ["-rnIF", "--", pattern.as_str(), path.as_str()]
            .map(String::from)
            .to_vec(),
        "/".into(),
    );
    let mut cmd = cua_spacesd_client::Command::new(command).args(args);
    if !u.is_empty() {
        cmd = cmd.user(u);
    }
    if !cwd.is_empty() {
        cmd = cmd.cwd(cwd);
    }
    let out = match c.run(cmd).await {
        // grep exits 1 when nothing matched.
        Ok(o) if o.success() || o.status.code == Some(1) => {
            String::from_utf8_lossy(&o.stdout).into_owned()
        }
        Ok(o) => {
            return error(
                StatusCode::BAD_REQUEST,
                String::from_utf8_lossy(&o.stderr).trim().to_string(),
            );
        }
        Err(e) => return error(StatusCode::BAD_GATEWAY, e.to_string()),
    };
    let matches: Vec<Value> = out
        .lines()
        .filter_map(|l| {
            let (file, rest) = l.split_once(':')?;
            let (line, content) = rest.split_once(':')?;
            Some(json!({"file": file, "line": line.parse::<u64>().ok()?, "content": content}))
        })
        .collect();
    ok_json(Value::Array(matches))
}

/// `GET /files/search?path=&pattern=`: names matching a glob -> `{files}`.
async fn search_files(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match path_of(&d, &r, &user, &q).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let pattern = q.pattern.clone().unwrap_or_else(|| "*".into());
    match guest(
        &c,
        &d.drivers.of(&r.id, &c).await,
        &user,
        &["find", &path, "-name", &pattern],
    )
    .await
    {
        Ok(out) => {
            ok_json(json!({"files": out.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>()}))
        }
        Err(e) => e,
    }
}

/// A whole file: read directly, or as `user` when the driver may not.
async fn read_as(
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    user: &str,
    path: &str,
) -> Result<Vec<u8>, FsError> {
    match read_all(c, path).await {
        Ok(b) => Ok(b),
        Err(s) if s.code() == tonic::Code::PermissionDenied && switched(driver, user) => {
            guest_bytes(c, driver, user, &["cat", "--", path])
                .await
                .map_err(|_| FsError(StatusCode::FORBIDDEN, format!("permission denied: {path}")))
        }
        Err(s) => Err(FsError::from(&s)),
    }
}

async fn read_all(
    c: &cua_spacesd_client::SpacesdClient,
    path: &str,
) -> Result<Vec<u8>, tonic::Status> {
    use pb::read_file_response::Message as M;
    let mut s = c
        .filesystem()
        .read_file(pb::ReadFileRequest {
            path: path.to_string(),
            ..Default::default()
        })
        .await?
        .into_inner();
    let mut out = Vec::new();
    while let Some(m) = s.next().await {
        if let Some(M::Chunk(ch)) = m?.message {
            out.extend_from_slice(&ch.data);
        }
    }
    Ok(out)
}

/// `POST /files/replace` `{files, pattern, newValue}`: literal replacement
/// in each file -> `[{file, success, error?}]`.
async fn replace_in_files(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let pattern = b
        .get("pattern")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let new = b
        .get("newValue")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if pattern.is_empty() {
        return error(StatusCode::BAD_REQUEST, "pattern is required");
    }
    let home = workdir(&d, &r, &user).await;
    let driver = d.drivers.of(&r.id, &c).await;
    let mut results = Vec::new();
    for f in b
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let Some(f) = f.as_str() else { continue };
        let path = resolve_path(f, &home);
        let res = async {
            let data = read_as(&c, &driver, &user, &path).await.map_err(|e| e.1)?;
            let text = String::from_utf8(data).map_err(|_| "not a text file".to_string())?;
            let replaced = text.replace(&pattern, &new);
            let mut w = Writer::begin(&c, &path, "root", &driver)
                .await
                .map_err(|e| e.1)?;
            w.write(Bytes::from(replaced)).await.map_err(|e| e.1)?;
            w.finish().await.map_err(|e| e.1)?;
            Ok::<_, String>(())
        }
        .await;
        results.push(match res {
            Ok(()) => json!({"file": f, "success": true}),
            Err(e) => json!({"file": f, "success": false, "error": e}),
        });
    }
    ok_json(Value::Array(results))
}

/// `GET /files/download?path=` (Bearer, or a signed URL): the file's bytes.
async fn download(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
    Query(sq): Query<SignedQuery>,
) -> Response<Body> {
    let (r, c, user) = match sandbox_or_signed(&d, &headers, &id, "GET", &sq).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match path_of(&d, &r, &user, &q).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    use pb::read_file_response::Message as M;
    let s = match c
        .filesystem()
        .read_file(pb::ReadFileRequest {
            path: path.clone(),
            ..Default::default()
        })
        .await
    {
        Ok(s) => s.into_inner(),
        Err(s) if s.code() == tonic::Code::PermissionDenied => {
            // A file the driver may not read: the sandbox user reads it.
            let driver = d.drivers.of(&r.id, &c).await;
            return match read_as(&c, &driver, &user, &path).await {
                Ok(data) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(data))
                    .expect("response"),
                Err(FsError(status, message)) => error(status, message),
            };
        }
        Err(s) => return status_err(s),
    };
    let body = s.filter_map(|m| async move {
        match m {
            Ok(pb::ReadFileResponse {
                message: Some(M::Chunk(ch)),
            }) => Some(Ok::<Bytes, std::io::Error>(ch.data.into())),
            Ok(_) => None,
            Err(e) => Some(Err(std::io::Error::other(e.message().to_string()))),
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .body(Body::from_stream(body))
        .expect("response")
}

/// A `Content-Disposition` for a bulk-download part: the requested path
/// echoed back as `filename` (ASCII fallback) and `filename*` (RFC 5987,
/// UTF-8), which the SDK prefers and matches byte for byte
/// (`common/file_transfer.py` `parse_content_disposition`).
fn content_disposition(name: &str, path: &str) -> String {
    let ascii: String = path
        .chars()
        .map(|c| {
            if c.is_ascii() && c != '"' && c != '\\' && !c.is_control() {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut enc = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            enc.push(b as char);
        } else {
            enc.push_str(&format!("%{b:02X}"));
        }
    }
    format!("form-data; name=\"{name}\"; filename=\"{ascii}\"; filename*=UTF-8''{enc}")
}

/// `POST /files/bulk-download` `{paths}`: `multipart/form-data`, one part
/// per path (`name="file"`, or `name="error"` with a JSON reason).
async fn bulk_download(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let home = workdir(&d, &r, &user).await;
    let paths: Vec<String> = b
        .get("paths")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let boundary = format!("cua{}", &crate::auth::new_token()[..24]);
    let ctype = format!("multipart/form-data; boundary={boundary}");
    let closing = Bytes::from(format!("--{boundary}--\r\n"));
    let driver = d.drivers.of(&r.id, &c).await;
    let stream = futures_util::stream::iter(paths).then(move |p| {
        let c = c.clone();
        let boundary = boundary.clone();
        let full = resolve_path(&p, &home);
        let (driver, user) = (driver.clone(), user.clone());
        async move {
            let disposition = |name: &str| content_disposition(name, &p);
            let part = match read_as(&c, &driver, &user, &full).await {
                Ok(data) => {
                    let mut v = format!(
                        "--{boundary}\r\nContent-Disposition: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n",
                        disposition("file"),
                        data.len()
                    )
                    .into_bytes();
                    v.extend_from_slice(&data);
                    v.extend_from_slice(b"\r\n");
                    v
                }
                Err(FsError(status, message)) => {
                    // `parse_file_download_error_payload` (SDK): message,
                    // statusCode, code, source.
                    let mut e = json!({"message": message, "statusCode": status.as_u16(), "source": super::SOURCE_DAEMON});
                    match status {
                        StatusCode::NOT_FOUND => e["code"] = json!("FILE_NOT_FOUND"),
                        StatusCode::FORBIDDEN => e["code"] = json!("FILE_ACCESS_DENIED"),
                        _ => {}
                    }
                    format!(
                        "--{boundary}\r\nContent-Disposition: {}\r\nContent-Type: application/json\r\n\r\n{e}\r\n",
                        disposition("error")
                    )
                    .into_bytes()
                }
            };
            Ok::<Bytes, std::io::Error>(Bytes::from(part))
        }
    });
    let stream = stream.chain(futures_util::stream::once(async move { Ok(closing) }));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ctype)
        .body(Body::from_stream(stream))
        .expect("response")
}

/// `POST /files/bulk-upload` multipart: `files[i].path` then `files[i].file`.
async fn bulk_upload(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, c, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let ctype = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let Some(boundary) = multipart_boundary(&ctype) else {
        return error(StatusCode::BAD_REQUEST, "expected multipart/form-data");
    };
    let home = workdir(&d, &r, &user).await;
    let mut parts = Multipart::new(body.into_data_stream(), boundary);
    let mut paths: HashMap<String, String> = HashMap::new();
    loop {
        let part = match parts.next_part().await {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => return error(StatusCode::BAD_REQUEST, e),
        };
        let name = part.name.clone().unwrap_or_default();
        let Some(idx) = name.strip_prefix("files[").and_then(|n| n.split_once(']')) else {
            while let Ok(Some(_)) = parts.read_chunk().await {}
            continue;
        };
        let (i, field) = (idx.0.to_string(), idx.1.to_string());
        if field == ".path" {
            let mut v = Vec::new();
            loop {
                match parts.read_chunk().await {
                    Ok(Some(ch)) => v.extend_from_slice(&ch),
                    Ok(None) => break,
                    Err(e) => return error(StatusCode::BAD_REQUEST, e),
                }
            }
            paths.insert(i, String::from_utf8_lossy(&v).into_owned());
        } else if field == ".file" {
            let Some(dest) = paths.get(&i) else {
                return error(
                    StatusCode::BAD_REQUEST,
                    format!("files[{i}].path must precede files[{i}].file"),
                );
            };
            let path = resolve_path(dest, &home);
            let mut w = match Writer::begin(&c, &path, &user, &d.drivers.of(&r.id, &c).await).await
            {
                Ok(w) => w,
                Err(e) => return fs_err(e),
            };
            loop {
                match parts.read_chunk().await {
                    Ok(Some(ch)) => {
                        if let Err(e) = w.write(ch).await {
                            return fs_err(e);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => return error(StatusCode::BAD_REQUEST, e),
                }
            }
            if let Err(e) = w.finish().await {
                return fs_err(e);
            }
        } else {
            while let Ok(Some(_)) = parts.read_chunk().await {}
        }
    }
    ok_json(json!({}))
}

/// `POST /files/upload-v2?path=` (Bearer, or a signed URL): the body
/// (raw, or the first file part of a multipart form) becomes the file.
async fn upload_v2(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<FsQuery>,
    Query(sq): Query<SignedQuery>,
    body: Body,
) -> Response<Body> {
    let (r, c, user) = match sandbox_or_signed(&d, &headers, &id, "POST", &sq).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = match path_of(&d, &r, &user, &q).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut w = match Writer::begin(&c, &path, &user, &d.drivers.of(&r.id, &c).await).await {
        Ok(w) => w,
        Err(e) => return fs_err(e),
    };
    let ctype = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if let Some(boundary) = multipart_boundary(&ctype) {
        let mut parts = Multipart::new(body.into_data_stream(), boundary);
        loop {
            match parts.next_part().await {
                Ok(Some(p)) if p.filename.is_some() => break,
                Ok(Some(_)) => while let Ok(Some(_)) = parts.read_chunk().await {},
                Ok(None) => return error(StatusCode::BAD_REQUEST, "no file part"),
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            }
        }
        loop {
            match parts.read_chunk().await {
                Ok(Some(ch)) => {
                    if let Err(e) = w.write(ch).await {
                        return fs_err(e);
                    }
                }
                Ok(None) => break,
                Err(e) => return error(StatusCode::BAD_REQUEST, e),
            }
        }
    } else {
        let mut data = body.into_data_stream();
        while let Some(ch) = data.next().await {
            let ch = match ch {
                Ok(c) => c,
                Err(e) => return error(StatusCode::BAD_REQUEST, e.to_string()),
            };
            if let Err(e) = w.write(ch).await {
                return fs_err(e);
            }
        }
    }
    match w.finish().await {
        Ok(_) => ok_json(json!({})),
        Err(e) => fs_err(e),
    }
}

/// `GET /system/metrics`: the guest's current reading.
async fn system_metrics(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let (_, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    match c.metrics().await {
        Ok(m) => {
            let now = crate::state::now_ms();
            ok_json(json!({
                "cpuCount": m.cpu_count,
                "cpuUsedPct": m.cpu_percent,
                "memUsed": m.memory_used_bytes,
                "memTotal": m.memory_total_bytes,
                "memCache": 0,
                "diskUsed": m.disk_used_bytes,
                "diskTotal": m.disk_total_bytes,
                "diskFree": m.disk_total_bytes.saturating_sub(m.disk_used_bytes),
                "timestamp": time::OffsetDateTime::from_unix_timestamp((now / 1000) as i64)
                    .ok()
                    .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()),
                "timestampUnix": now / 1000,
            }))
        }
        Err(e) => error(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

/// `POST /env` `{set, unset}`: the environment of later processes.
async fn update_env(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (r, _, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let set: Vec<(String, String)> = b
        .get("set")
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
    let unset: Vec<String> = b
        .get("unset")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    d.store
        .update(&r.id, |r| {
            for k in &unset {
                r.env_vars.remove(k);
            }
            r.env_vars.extend(set);
        })
        .await;
    ok_json(json!({}))
}

async fn home_dir(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, _, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    ok_json(json!({"dir": workdir(&d, &r, &user).await}))
}

/// `GET /user-home-dir`: the sandbox user's home, whatever the image's
/// WORKDIR is.
async fn user_home_dir(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, _, user) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    ok_json(json!({"dir": d.home(&r, &user).await}))
}

async fn version(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    if let Err(e) = sandbox(&d, &headers, &id).await {
        return e;
    }
    ok_json(json!({"version": "cua-compat"}))
}

/// Listening TCP ports in the guest (`/proc/net/tcp{,6}`, state 0A).
async fn listening(
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
) -> Result<Vec<u16>, Response<Body>> {
    let out = guest(
        c,
        driver,
        "root",
        &[
            "/bin/sh",
            "-c",
            "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null",
        ],
    )
    .await?;
    let mut ports: Vec<u16> = out
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() > 3 && f[3] == "0A")
                .then(|| f[1].rsplit_once(':'))
                .flatten()
                .and_then(|(_, p)| u16::from_str_radix(p, 16).ok())
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    Ok(ports)
}

async fn ports(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    match listening(&c, &d.drivers.of(&r.id, &c).await).await {
        Ok(p) => ok_json(json!({"ports": p})),
        Err(e) => e,
    }
}

async fn port_in_use(
    State(d): State<Arc<Daytona>>,
    Path((id, port)): Path<(String, u16)>,
    headers: HeaderMap,
) -> Response<Body> {
    let (r, c, _) = match sandbox(&d, &headers, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    match listening(&c, &d.drivers.of(&r.id, &c).await).await {
        Ok(p) => ok_json(json!({"isInUse": p.contains(&port)})),
        Err(e) => e,
    }
}

#[cfg(test)]
mod find_tests {
    use super::*;

    #[test]
    fn find_records_become_file_infos() {
        let v = parse_find(
            "d\t4096\t755\tdaytona\tdaytona\t1700000000.5\t/home/daytona/src\0f\t7\t640\tdaytona\tdaytona\t1700000001.0\t/home/daytona/a b.txt\0",
        );
        assert_eq!(v.len(), 2);
        assert_eq!(
            (v[0]["name"].as_str(), v[0]["isDir"].as_bool()),
            (Some("src"), Some(true))
        );
        assert_eq!(v[1]["name"], "a b.txt");
        assert_eq!(v[1]["permissions"], "0640");
        assert_eq!(v[1]["mode"], "-rw-r-----");
        assert!(parse_find("garbage\0").is_empty());
    }
}
