//! Daytona's LSP routes (`/lsp/*`) over real language servers in the guest.
//!
//! `sandbox.create_lsp_server(language, project)` returns an object whose
//! calls the SDKs make as `POST /lsp/start|stop|did-open|did-close|
//! completions` and `GET /lsp/document-symbols|workspacesymbols`
//! (daytona_toolbox_api_client 0.225.0 `lsp_api.py`). Here each
//! (sandbox, language, project) is one language server process
//! (`pylsp` for Python, `typescript-language-server --stdio` for TypeScript
//! and JavaScript) that the gateway speaks LSP to over its stdio: JSON-RPC
//! with `Content-Length` framing, our own minimal client (initialize,
//! didOpen/didClose, completion, documentSymbol, workspace/symbol,
//! shutdown). A server that is not installed is installed on first start
//! (pip or npm) when the guest has network; otherwise `start` fails with the
//! daemon's `LSP_SERVER_NOT_INITIALIZED` style error naming what is missing.

use super::stdio::{Out, Stdio};
use super::toolbox::{error, sandbox, workdir};
use super::{Daytona, daemon_error, ok_json};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/{id}/lsp/start", post(start))
        .route("/{id}/lsp/stop", post(stop))
        .route("/{id}/lsp/did-open", post(did_open))
        .route("/{id}/lsp/did-close", post(did_close))
        .route("/{id}/lsp/completions", post(completions))
        .route("/{id}/lsp/document-symbols", get(document_symbols))
        .route("/{id}/lsp/workspacesymbols", get(workspace_symbols))
        .with_state(d)
}

// ── framing ──────────────────────────────────────────────────────────────

/// Encodes a JSON-RPC message with its `Content-Length` header.
pub fn frame(v: &Value) -> Vec<u8> {
    let body = v.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Pulls the complete messages out of `buf`, leaving a partial one.
pub fn unframe(buf: &mut Vec<u8>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(h_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
        let head = String::from_utf8_lossy(&buf[..h_end]).into_owned();
        let len = head.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        });
        let Some(len) = len else {
            // A header block without a length: skip it.
            buf.drain(..h_end + 4);
            continue;
        };
        if buf.len() < h_end + 4 + len {
            break;
        }
        let body: Vec<u8> = buf.drain(..h_end + 4 + len).skip(h_end + 4).collect();
        if let Ok(v) = serde_json::from_slice::<Value>(&body) {
            out.push(v);
        }
    }
    out
}

// ── the client ───────────────────────────────────────────────────────────

pub struct Server {
    proc: Stdio,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    opened: Mutex<std::collections::HashSet<String>>,
    #[allow(dead_code)]
    pub root: String,
    pub language: String,
}

#[derive(Default)]
pub struct Registry {
    servers: Mutex<HashMap<(String, String, String), Arc<Server>>>,
}

impl Registry {
    pub async fn drop_sandbox(&self, sandbox_id: &str) {
        let gone: Vec<Arc<Server>> = {
            let mut m = self.servers.lock().await;
            let keys: Vec<_> = m
                .keys()
                .filter(|(s, _, _)| s == sandbox_id)
                .cloned()
                .collect();
            keys.iter().filter_map(|k| m.remove(k)).collect()
        };
        for s in gone {
            s.proc.kill().await;
        }
    }
}

impl Server {
    async fn send(&self, v: Value) -> Result<(), String> {
        self.proc.write(frame(&v)).await
    }

    async fn request(&self, method: &str, params: Value, t: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        match tokio::time::timeout(t, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("the language server ended".into()),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!(
                    "the language server did not answer {method} in {}s",
                    t.as_secs()
                ))
            }
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }

    /// Routes the server's output: replies to their requests; requests of
    /// the server (configuration, capability registration, progress) get an
    /// empty answer so it never waits on us.
    async fn pump(self: Arc<Self>) {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            match self.proc.next(Duration::from_secs(24 * 3600)).await {
                Some(Out::Stdout(b)) => {
                    buf.extend_from_slice(&b);
                    for m in unframe(&mut buf) {
                        if let (Some(id), Some(method)) =
                            (m.get("id"), m.get("method").and_then(Value::as_str))
                        {
                            let result = if method == "workspace/configuration" {
                                let n = m["params"]["items"].as_array().map_or(0, Vec::len);
                                Value::Array(vec![Value::Null; n])
                            } else {
                                Value::Null
                            };
                            let _ = self
                                .send(json!({"jsonrpc": "2.0", "id": id, "result": result}))
                                .await;
                        } else if let Some(id) = m.get("id").and_then(Value::as_u64) {
                            let r = match m.get("error") {
                                Some(e) => Err(e["message"]
                                    .as_str()
                                    .unwrap_or("language server error")
                                    .to_string()),
                                None => Ok(m.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            if let Some(tx) = self.pending.lock().await.remove(&id) {
                                let _ = tx.send(r);
                            }
                        }
                    }
                }
                Some(Out::Stderr(b)) => {
                    tracing::debug!("compat: lsp: {}", String::from_utf8_lossy(&b).trim_end());
                }
                Some(Out::Exit(_)) | None => {
                    let mut p = self.pending.lock().await;
                    for (_, tx) in p.drain() {
                        let _ = tx.send(Err("the language server ended".into()));
                    }
                    return;
                }
            }
        }
    }
}

/// The server command and how to install it, per `languageId`.
pub fn server_for(
    language: &str,
) -> Option<(
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
)> {
    match language {
        "python" => Some((
            "pylsp",
            &[],
            "python3 -m pip install --user --break-system-packages python-lsp-server 2>&1 || python3 -m pip install --user python-lsp-server 2>&1",
            "python-lsp-server (pylsp)",
        )),
        "typescript" | "javascript" => Some((
            "typescript-language-server",
            &["--stdio"],
            "npm install -g typescript-language-server typescript 2>&1 || sudo -n npm install -g typescript-language-server typescript 2>&1",
            "typescript-language-server",
        )),
        _ => None,
    }
}

fn lsp_error(msg: impl Into<String>) -> Response<Body> {
    daemon_error(StatusCode::BAD_REQUEST, "LSP_SERVER_NOT_INITIALIZED", msg)
}

/// `file://<path>` (or a bare path) to the path.
pub fn path_of_uri(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

fn uri_of(path: &str) -> String {
    format!("file://{path}")
}

async fn key_of(
    d: &Daytona,
    headers: &HeaderMap,
    id: &str,
    language: &str,
    project: &str,
) -> Result<
    (
        crate::state::Record,
        cua_spacesd_client::SpacesdClient,
        String,
        (String, String, String),
        String,
    ),
    Response<Body>,
> {
    let (r, c, user) = sandbox(d, headers, id).await?;
    let home = workdir(d, &r, &user).await;
    let root = crate::e2b::resolve_path(project, &home);
    Ok((
        r.clone(),
        c,
        user,
        (r.id, language.to_string(), root.clone()),
        root,
    ))
}

fn field<'a>(b: &'a Value, k: &str) -> Option<&'a str> {
    b.get(k).and_then(Value::as_str).filter(|s| !s.is_empty())
}

async fn start(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let (Some(lang), Some(project)) = (field(&b, "languageId"), field(&b, "pathToProject")) else {
        return error(
            StatusCode::BAD_REQUEST,
            "languageId and pathToProject are required",
        );
    };
    let Some((cmd, args, install, label)) = server_for(lang) else {
        return error(
            StatusCode::BAD_REQUEST,
            format!("languageId {lang:?} is not supported: use python, typescript or javascript"),
        );
    };
    let (r, c, user, key, root) = match key_of(&d, &headers, &id, lang, project).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    if d.lsp.servers.lock().await.contains_key(&key) {
        return ok_json(json!({}));
    }
    let driver = d.drivers.of(&r.id, &c).await;
    // Installed? (as the sandbox user's PATH sees it)
    let probe = format!("command -v {cmd} >/dev/null 2>&1 || [ -x \"$HOME/.local/bin/{cmd}\" ]");
    let present =
        matches!(crate::guest::run_as(&c, &driver, &user, &probe, &[]).await, Ok(o) if o.success());
    if !present {
        let out = crate::guest::run_as(&c, &driver, &user, install, &[]).await;
        let ok = matches!(&out, Ok(o) if o.success());
        if !ok {
            let why = match out {
                Ok(o) => String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .last()
                    .unwrap_or("")
                    .to_string(),
                Err(e) => e,
            };
            return lsp_error(format!(
                "The {label} language server is not installed in this sandbox and could not be installed ({why})"
            ));
        }
    }
    let mut env: HashMap<String, String> = r.env_vars.clone().into_iter().collect();
    env.insert(
        "PATH".into(),
        format!(
            "$HOME/.local/bin:{}",
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    let mut argv = vec![
        "/bin/sh".to_string(),
        "-c".into(),
        format!(
            "export PATH=\"$HOME/.local/bin:$PATH\"; exec {cmd} {}",
            args.join(" ")
        ),
    ];
    let proc = match Stdio::start(
        &c,
        &driver,
        &user,
        std::mem::take(&mut argv),
        HashMap::new(),
        root.clone(),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return lsp_error(format!("starting the language server failed: {e}")),
    };
    let server = Arc::new(Server {
        proc,
        next_id: AtomicU64::new(1),
        pending: Mutex::default(),
        opened: Mutex::default(),
        root: root.clone(),
        language: lang.to_string(),
    });
    tokio::spawn(server.clone().pump());
    let init = server
        .request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": uri_of(&root),
                "capabilities": {
                    "textDocument": {
                        "completion": {"completionItem": {"documentationFormat": ["markdown", "plaintext"]}},
                        "documentSymbol": {"hierarchicalDocumentSymbolSupport": false},
                    },
                    "workspace": {"symbol": {}},
                },
                "workspaceFolders": [{"uri": uri_of(&root), "name": "project"}],
            }),
            Duration::from_secs(60),
        )
        .await;
    if let Err(e) = init {
        server.proc.kill().await;
        return lsp_error(format!("the language server did not initialize: {e}"));
    }
    let _ = server.notify("initialized", json!({})).await;
    d.lsp.servers.lock().await.insert(key, server);
    ok_json(json!({}))
}

async fn lookup(
    d: &Daytona,
    headers: &HeaderMap,
    id: &str,
    b: &Value,
) -> Result<
    (
        Arc<Server>,
        String,
        cua_spacesd_client::SpacesdClient,
        String,
        String,
    ),
    Response<Body>,
> {
    let (Some(lang), Some(project)) = (field(b, "languageId"), field(b, "pathToProject")) else {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "languageId and pathToProject are required",
        ));
    };
    let (r, c, user, key, root) = key_of(d, headers, id, lang, project).await?;
    let s = d.lsp.servers.lock().await.get(&key).cloned();
    match s {
        Some(s) => {
            let driver = d.drivers.of(&r.id, &c).await;
            Ok((s, root, c, user, driver))
        }
        None => Err(lsp_error(
            "The LSP server for this language and project is not started: call start() first",
        )),
    }
}

/// A document's path from a request `uri`: relative paths resolve against
/// the project root.
fn doc_path(root: &str, uri: &str) -> String {
    let p = path_of_uri(uri);
    if p.starts_with('/') {
        p
    } else {
        format!("{}/{}", root.trim_end_matches('/'), p)
    }
}

async fn stop(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let (Some(lang), Some(project)) = (field(&b, "languageId"), field(&b, "pathToProject")) else {
        return error(
            StatusCode::BAD_REQUEST,
            "languageId and pathToProject are required",
        );
    };
    let (_, _, _, key, _) = match key_of(&d, &headers, &id, lang, project).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    if let Some(s) = d.lsp.servers.lock().await.remove(&key) {
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            s.request("shutdown", Value::Null, Duration::from_secs(5)),
        )
        .await;
        let _ = s.notify("exit", Value::Null).await;
        s.proc.kill().await;
    }
    ok_json(json!({}))
}

async fn did_open(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(uri) = field(&b, "uri") else {
        return error(StatusCode::BAD_REQUEST, "uri is required");
    };
    let (s, root, c, user, driver) = match lookup(&d, &headers, &id, &b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = doc_path(&root, uri);
    let text = match crate::guest::run_as(&c, &driver, &user, "cat -- \"$1\"", &[&path]).await {
        Ok(o) if o.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => {
            return super::toolbox::fs_error(
                StatusCode::NOT_FOUND,
                String::from_utf8_lossy(&o.stderr).trim().to_string(),
            );
        }
        Err(e) => return error(StatusCode::BAD_GATEWAY, e),
    };
    let u = uri_of(&path);
    if !s.opened.lock().await.insert(u.clone()) {
        return ok_json(json!({}));
    }
    match s
        .notify("textDocument/didOpen", json!({"textDocument": {"uri": u, "languageId": s.language, "version": 1, "text": text}}))
        .await
    {
        Ok(()) => ok_json(json!({})),
        Err(e) => lsp_error(e),
    }
}

async fn did_close(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(uri) = field(&b, "uri") else {
        return error(StatusCode::BAD_REQUEST, "uri is required");
    };
    let (s, root, _, _, _) = match lookup(&d, &headers, &id, &b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let u = uri_of(&doc_path(&root, uri));
    s.opened.lock().await.remove(&u);
    match s
        .notify("textDocument/didClose", json!({"textDocument": {"uri": u}}))
        .await
    {
        Ok(()) => ok_json(json!({})),
        Err(e) => lsp_error(e),
    }
}

/// An LSP `CompletionItem.documentation` (a string or MarkupContent) as the
/// toolbox's object.
fn documentation(v: &Value) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::String(s) => json!({"kind": "plaintext", "value": s}),
        o => o.clone(),
    }
}

/// An LSP completion result (a list or a `CompletionList`) as the toolbox's
/// `CompletionList`.
pub fn completion_list(r: &Value) -> Value {
    let (items, incomplete) = match r {
        Value::Array(a) => (a.clone(), false),
        Value::Object(o) => (
            o.get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            o.get("isIncomplete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        _ => (vec![], false),
    };
    let items: Vec<Value> = items
        .iter()
        .map(|i| {
            let mut o = json!({"label": i["label"].as_str().unwrap_or("")});
            for (to, from) in [
                ("kind", "kind"),
                ("detail", "detail"),
                ("sortText", "sortText"),
                ("filterText", "filterText"),
                ("insertText", "insertText"),
            ] {
                if !i[from].is_null() {
                    o[to] = i[from].clone();
                }
            }
            if !i["documentation"].is_null() {
                o["documentation"] = documentation(&i["documentation"]);
            }
            o
        })
        .collect();
    json!({"isIncomplete": incomplete, "items": items})
}

async fn completions(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(uri) = field(&b, "uri") else {
        return error(StatusCode::BAD_REQUEST, "uri is required");
    };
    let (s, root, _, _, _) = match lookup(&d, &headers, &id, &b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let u = uri_of(&doc_path(&root, uri));
    let mut params = json!({"textDocument": {"uri": u}, "position": b.get("position").cloned().unwrap_or(json!({"line": 0, "character": 0}))});
    if let Some(ctx) = b.get("context").filter(|c| !c.is_null()) {
        params["context"] = ctx.clone();
    }
    match s
        .request("textDocument/completion", params, Duration::from_secs(30))
        .await
    {
        Ok(r) => ok_json(completion_list(&r)),
        Err(e) => lsp_error(e),
    }
}

/// LSP symbols (`SymbolInformation`, `DocumentSymbol` trees, workspace
/// symbols) as flat toolbox `LspSymbol`s `{name, kind, location}`.
pub fn symbols(r: &Value, doc_uri: &str) -> Value {
    fn walk(out: &mut Vec<Value>, v: &Value, doc_uri: &str) {
        let Some(a) = v.as_array() else { return };
        for s in a {
            let name = s["name"].as_str().unwrap_or("");
            let kind = s["kind"].as_i64().unwrap_or(0);
            let location = if let Some(l) = s.get("location").filter(|l| l.get("range").is_some()) {
                l.clone()
            } else if let Some(l) = s.get("location") {
                json!({"uri": l["uri"], "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}})
            } else {
                json!({"uri": doc_uri, "range": s.get("range").cloned().unwrap_or(json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}))})
            };
            out.push(json!({"name": name, "kind": kind, "location": location}));
            if let Some(ch) = s.get("children") {
                walk(out, ch, doc_uri);
            }
        }
    }
    let mut out = Vec::new();
    walk(&mut out, r, doc_uri);
    Value::Array(out)
}

#[derive(serde::Deserialize, Default)]
struct SymQuery {
    #[serde(rename = "languageId")]
    language_id: Option<String>,
    #[serde(rename = "pathToProject")]
    path_to_project: Option<String>,
    uri: Option<String>,
    query: Option<String>,
}

async fn document_symbols(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<SymQuery>,
) -> Response<Body> {
    let b = json!({"languageId": q.language_id, "pathToProject": q.path_to_project});
    let Some(uri) = q.uri.as_deref().filter(|u| !u.is_empty()) else {
        return error(StatusCode::BAD_REQUEST, "uri is required");
    };
    let (s, root, _, _, _) = match lookup(&d, &headers, &id, &b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let u = uri_of(&doc_path(&root, uri));
    match s
        .request(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": u}}),
            Duration::from_secs(30),
        )
        .await
    {
        Ok(r) => ok_json(symbols(&r, &u)),
        Err(e) => lsp_error(e),
    }
}

async fn workspace_symbols(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<SymQuery>,
) -> Response<Body> {
    let b = json!({"languageId": q.language_id, "pathToProject": q.path_to_project});
    let (s, root, c, user, driver) = match lookup(&d, &headers, &id, &b).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let query = q.query.unwrap_or_default();
    match s
        .request(
            "workspace/symbol",
            json!({"query": query}),
            Duration::from_secs(30),
        )
        .await
    {
        Ok(r) => ok_json(symbols(&r, "")),
        // Servers such as pylsp do not implement workspace/symbol: answer
        // from each project file's document symbols instead.
        Err(e) if e.contains("Method Not Found") || e.contains("-32601") => {
            ok_json(scan_symbols(&s, &root, &c, &driver, &user, &query).await)
        }
        Err(e) => lsp_error(e),
    }
}

/// `workspace/symbol` for a server without it: the document symbols of the
/// project's source files (bounded), filtered by `query` the way editors
/// match (case-insensitive subsequence-free substring).
async fn scan_symbols(
    s: &Server,
    root: &str,
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    user: &str,
    query: &str,
) -> Value {
    let _ = driver;
    let exts: &[&str] = match s.language.as_str() {
        "python" => &["py"],
        "typescript" | "javascript" => &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        _ => &[],
    };
    let mut names = String::new();
    for e in exts {
        names.push_str(&format!(" -o -name '*.{e}'"));
    }
    let script = format!(
        "find \"$1\" \\( -name node_modules -o -name .git -o -name __pycache__ -o -name .venv -o -name venv -o -name site-packages \\) -prune -o -type f \\( -false{names} \\) -print 2>/dev/null | head -n 200"
    );
    let Ok(list) = crate::guest::run_as(c, driver, user, &script, &[root]).await else {
        return json!([]);
    };
    let needle = query.to_lowercase();
    let mut out = Vec::new();
    for path in String::from_utf8_lossy(&list.stdout).lines() {
        let Ok(o) = crate::guest::run_as(c, driver, user, "cat -- \"$1\"", &[path]).await else {
            continue;
        };
        if !o.success() {
            continue;
        }
        let u = uri_of(path);
        let was_open = !s.opened.lock().await.insert(u.clone());
        if !was_open {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            if s.notify("textDocument/didOpen", json!({"textDocument": {"uri": u, "languageId": s.language, "version": 1, "text": text}})).await.is_err() {
                continue;
            }
        }
        if let Ok(r) = s
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": u}}),
                Duration::from_secs(15),
            )
            .await
            && let Some(a) = symbols(&r, &u).as_array()
        {
            out.extend(
                a.iter()
                    .filter(|x| {
                        x["name"]
                            .as_str()
                            .is_some_and(|n| n.to_lowercase().contains(&needle))
                    })
                    .cloned(),
            );
        }
        if !was_open {
            s.opened.lock().await.remove(&u);
            let _ = s
                .notify("textDocument/didClose", json!({"textDocument": {"uri": u}}))
                .await;
        }
    }
    Value::Array(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn frames_split_anywhere() {
        let a = frame(&json!({"jsonrpc": "2.0", "id": 1, "result": "héllo"}));
        let b = frame(&json!({"jsonrpc": "2.0", "method": "x"}));
        let all: Vec<u8> = a.iter().chain(b.iter()).copied().collect();
        for cut in 0..all.len() {
            let mut buf = all[..cut].to_vec();
            let mut got = unframe(&mut buf);
            buf.extend_from_slice(&all[cut..]);
            got.extend(unframe(&mut buf));
            assert_eq!(got.len(), 2, "cut at {cut}");
            assert_eq!(got[0]["result"], "héllo");
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn symbols_flatten_both_shapes() {
        let info = json!([{"name": "f", "kind": 12, "location": {"uri": "file:///a.py", "range": {"start": {"line": 1, "character": 0}, "end": {"line": 2, "character": 0}}}}]);
        let s = symbols(&info, "file:///a.py");
        assert_eq!(s[0]["location"]["uri"], "file:///a.py");
        assert_eq!(s[0]["location"]["range"]["start"]["line"], 1);
        let tree = json!([{"name": "C", "kind": 5, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}}, "children": [{"name": "m", "kind": 6, "range": {"start": {"line": 1, "character": 4}, "end": {"line": 2, "character": 0}}}]}]);
        let s = symbols(&tree, "file:///b.py");
        assert_eq!(s.as_array().unwrap().len(), 2);
        assert_eq!(s[1]["name"], "m");
        assert_eq!(s[1]["location"]["uri"], "file:///b.py");
    }

    #[test]
    fn completion_results_become_a_list() {
        let r = completion_list(&json!([{"label": "print", "kind": 3, "documentation": "d"}]));
        assert_eq!(r["isIncomplete"], false);
        assert_eq!(r["items"][0]["documentation"]["value"], "d");
        let r = completion_list(
            &json!({"isIncomplete": true, "items": [{"label": "x", "insertText": "x()"}]}),
        );
        assert_eq!(r["isIncomplete"], true);
        assert_eq!(r["items"][0]["insertText"], "x()");
        assert_eq!(completion_list(&Value::Null)["items"], json!([]));
    }

    #[test]
    fn documents_resolve_against_the_project() {
        assert_eq!(
            doc_path("/home/u/p", "file://src/a.ts"),
            "/home/u/p/src/a.ts"
        );
        assert_eq!(doc_path("/home/u/p", "file:///abs/a.ts"), "/abs/a.ts");
        assert_eq!(server_for("python").unwrap().0, "pylsp");
        assert!(server_for("rust").is_none());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn unframe_never_panics(b in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..300)) {
            let mut buf = b;
            let _ = unframe(&mut buf);
        }

        #[test]
        fn frame_roundtrips(s in "\\PC{0,200}", n in 0i64..1000) {
            let v = json!({"jsonrpc": "2.0", "id": n, "result": s});
            let mut buf = frame(&v);
            let got = unframe(&mut buf);
            prop_assert_eq!(got, vec![v]);
        }
    }
}
