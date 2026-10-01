//! Loopback mocks of the provider APIs (feature `testing`), for hermetic
//! tests and the language-binding fixtures.
//!
//! These are **schema fixtures, not recordings**: every path, status code
//! and JSON field follows the providers' published API specs, pinned here:
//!
//! - E2B: `spec/openapi.yml` of <https://github.com/e2b-dev/E2B> at
//!   `ccaf9fc0ffe6` (2026-09-18).
//! - Daytona: `openapi-specs/api.json` of <https://github.com/daytona/clients>
//!   at `158e3bfdcfee` (2026-09-24).
//!
//! No live traffic was recorded (no provider keys were available); the
//! opt-in live tests (`tests/live.rs`) are what checks the real services.
//! Only the endpoints the providers call are served; anything else is 404.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    net::SocketAddr,
    sync::{Arc, Mutex},
};

/// One request a mock saw (headers other than auth are not kept).
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    /// `METHOD /path`.
    pub route: String,
    /// JSON body (`Null` when none).
    pub body: Value,
}

const TS: &str = "2026-09-24T00:00:00Z";

fn err(code: StatusCode, message: &str) -> Response {
    (
        code,
        Json(json!({"code": code.as_u16(), "message": message})),
    )
        .into_response()
}

async fn serve(router: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (addr, task)
}

// ------------------------------------------------------------------ E2B

#[derive(Default)]
struct E2bState {
    key: String,
    seen: Vec<Seen>,
    /// name -> template id
    aliases: BTreeMap<String, String>,
    /// template id -> [(build id, status, polls left)]
    builds: BTreeMap<String, Vec<(String, String, u32)>>,
    sandboxes: BTreeMap<String, Value>,
    fail_builds: bool,
    next: u32,
}

/// A mock E2B control plane.
pub struct MockE2b {
    addr: SocketAddr,
    state: Arc<Mutex<E2bState>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockE2b {
    fn drop(&mut self) {
        self.task.abort();
    }
}

type E2b = Arc<Mutex<E2bState>>;

fn e2b_auth(s: &E2b, h: &HeaderMap, route: String, body: Value) -> Option<Response> {
    let mut st = s.lock().unwrap();
    st.seen.push(Seen { route, body });
    let ok = h
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|k| k == st.key);
    (!ok).then(|| err(StatusCode::UNAUTHORIZED, "Invalid API key"))
}

fn e2b_sandbox(id: &str, template: &str, metadata: &Value, state: &str) -> Value {
    json!({
        "templateID": template, "sandboxID": id, "clientID": "6532622b",
        "startedAt": TS, "endAt": TS, "cpuCount": 2, "memoryMB": 4096,
        "diskSizeMB": 10240, "state": state, "envdVersion": "0.2.4",
        "metadata": metadata,
    })
}

impl MockE2b {
    /// Starts it, accepting `api_key`.
    pub async fn start(api_key: &str) -> Self {
        let state: E2b = Arc::new(Mutex::new(E2bState {
            key: api_key.into(),
            ..Default::default()
        }));
        let router = Router::new()
            .route("/templates/aliases/{alias}", get(e2b_alias))
            .route("/templates/{id}", get(e2b_template))
            .route("/v3/templates", post(e2b_new_template))
            .route("/v2/templates/{id}/builds/{build}", post(e2b_start_build))
            .route(
                "/templates/{id}/builds/{build}/status",
                get(e2b_build_status),
            )
            .route("/v2/sandboxes", post(e2b_create).get(e2b_list))
            .route("/sandboxes/{id}", get(e2b_get).delete(e2b_delete))
            .route("/sandboxes/{id}/timeout", post(e2b_timeout))
            .route("/sandboxes/{id}/pause", post(e2b_pause))
            .route("/v2/sandboxes/{id}/connect", post(e2b_connect))
            .with_state(state.clone());
        let (addr, task) = serve(router).await;
        Self { addr, state, task }
    }

    /// The API base URL (`E2B_API_URL`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Requests so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    /// Live sandbox ids.
    pub fn sandboxes(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .sandboxes
            .keys()
            .cloned()
            .collect()
    }

    /// Template names built so far.
    pub fn templates(&self) -> Vec<String> {
        self.state.lock().unwrap().aliases.keys().cloned().collect()
    }

    /// Makes every new build end in `error`.
    pub fn fail_builds(&self, fail: bool) {
        self.state.lock().unwrap().fail_builds = fail;
    }
}

async fn e2b_alias(State(s): State<E2b>, h: HeaderMap, Path(alias): Path<String>) -> Response {
    if let Some(r) = e2b_auth(
        &s,
        &h,
        format!("GET /templates/aliases/{alias}"),
        Value::Null,
    ) {
        return r;
    }
    let st = s.lock().unwrap();
    match st.aliases.get(&alias) {
        Some(id) => Json(json!({"templateID": id, "public": false})).into_response(),
        None => err(
            StatusCode::NOT_FOUND,
            &format!("template '{alias}' not found"),
        ),
    }
}

async fn e2b_template(State(s): State<E2b>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = e2b_auth(&s, &h, format!("GET /templates/{id}"), Value::Null) {
        return r;
    }
    let st = s.lock().unwrap();
    let Some(builds) = st.builds.get(&id) else {
        return err(StatusCode::NOT_FOUND, "template not found");
    };
    let name = st
        .aliases
        .iter()
        .find(|(_, v)| **v == id)
        .map(|(k, _)| k.clone())
        .unwrap_or_default();
    let builds: Vec<Value> = builds
        .iter()
        .map(|(b, status, _)| {
            json!({"buildID": b, "status": status, "createdAt": TS, "updatedAt": TS,
                "cpuCount": 2, "memoryMB": 4096})
        })
        .collect();
    Json(json!({
        "templateID": id, "public": false, "aliases": [name], "names": [name],
        "createdAt": TS, "updatedAt": TS, "lastSpawnedAt": null, "spawnCount": 0,
        "builds": builds,
    }))
    .into_response()
}

async fn e2b_new_template(State(s): State<E2b>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if let Some(r) = e2b_auth(&s, &h, "POST /v3/templates".into(), b.clone()) {
        return r;
    }
    let Some(name) = b["name"].as_str().map(str::to_string) else {
        return err(StatusCode::BAD_REQUEST, "name is required");
    };
    let mut st = s.lock().unwrap();
    st.next += 1;
    let build = format!("b0000000-0000-4000-8000-{:012}", st.next);
    let id = st
        .aliases
        .get(&name)
        .cloned()
        .unwrap_or_else(|| format!("tpl{}", st.next));
    st.aliases.insert(name.clone(), id.clone());
    st.builds
        .entry(id.clone())
        .or_default()
        .push((build.clone(), "waiting".into(), 2));
    (
        StatusCode::ACCEPTED,
        Json(json!({"templateID": id, "buildID": build, "public": false,
            "names": [name], "tags": [], "aliases": [name]})),
    )
        .into_response()
}

async fn e2b_start_build(
    State(s): State<E2b>,
    h: HeaderMap,
    Path((id, build)): Path<(String, String)>,
    Json(b): Json<Value>,
) -> Response {
    let route = format!("POST /v2/templates/{id}/builds/{build}");
    if let Some(r) = e2b_auth(&s, &h, route, b.clone()) {
        return r;
    }
    if b["fromImage"].as_str().is_none_or(str::is_empty)
        == b["fromTemplate"].as_str().is_none_or(str::is_empty)
    {
        return err(
            StatusCode::BAD_REQUEST,
            "exactly one of fromImage or fromTemplate must be given",
        );
    }
    let mut st = s.lock().unwrap();
    let fail = st.fail_builds;
    match st
        .builds
        .get_mut(&id)
        .and_then(|v| v.iter_mut().find(|(bid, _, _)| *bid == build))
    {
        Some(entry) => {
            entry.1 = if fail {
                "error".into()
            } else {
                "building".into()
            };
            StatusCode::ACCEPTED.into_response()
        }
        None => err(StatusCode::NOT_FOUND, "build not found"),
    }
}

async fn e2b_build_status(
    State(s): State<E2b>,
    h: HeaderMap,
    Path((id, build)): Path<(String, String)>,
) -> Response {
    let route = format!("GET /templates/{id}/builds/{build}/status");
    if let Some(r) = e2b_auth(&s, &h, route, Value::Null) {
        return r;
    }
    let mut st = s.lock().unwrap();
    let Some(entry) = st
        .builds
        .get_mut(&id)
        .and_then(|v| v.iter_mut().find(|(bid, _, _)| *bid == build))
    else {
        return err(StatusCode::NOT_FOUND, "build not found");
    };
    if entry.1 == "building" {
        if entry.2 == 0 {
            entry.1 = "ready".into();
        } else {
            entry.2 -= 1;
        }
    }
    let mut v = json!({"templateID": id, "buildID": build, "status": entry.1,
        "logs": [], "logEntries": []});
    if entry.1 == "error" {
        v["reason"] = json!({"message": "failed to pull image: manifest unknown", "step": "base"});
    }
    Json(v).into_response()
}

async fn e2b_create(State(s): State<E2b>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if let Some(r) = e2b_auth(&s, &h, "POST /v2/sandboxes".into(), b.clone()) {
        return r;
    }
    let mut st = s.lock().unwrap();
    let Some(tpl) = b["templateID"].as_str() else {
        return err(StatusCode::BAD_REQUEST, "templateID is required");
    };
    let tpl = st
        .aliases
        .get(tpl)
        .cloned()
        .unwrap_or_else(|| tpl.to_string());
    let ready = st
        .builds
        .get(&tpl)
        .is_some_and(|b| b.iter().any(|(_, s, _)| s == "ready"));
    if !ready {
        return err(StatusCode::BAD_REQUEST, "template has no ready build");
    }
    st.next += 1;
    let id = format!("i{:019}", st.next);
    let doc = e2b_sandbox(&id, &tpl, &b["metadata"], "running");
    st.sandboxes.insert(id.clone(), doc);
    (
        StatusCode::CREATED,
        Json(
            json!({"templateID": tpl, "sandboxID": id, "clientID": "6532622b",
            "envdVersion": "0.2.4", "envdAccessToken": "envd-token",
            "trafficAccessToken": null, "domain": null}),
        ),
    )
        .into_response()
}

async fn e2b_list(
    State(s): State<E2b>,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = e2b_auth(&s, &h, "GET /v2/sandboxes".into(), Value::Null) {
        return r;
    }
    let st = s.lock().unwrap();
    let filter: Vec<(String, String)> = q
        .get("metadata")
        .map(|m| {
            url::form_urlencoded::parse(m.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .unwrap_or_default();
    let items: Vec<Value> = st
        .sandboxes
        .values()
        .filter(|d| {
            filter
                .iter()
                .all(|(k, v)| d["metadata"][k].as_str() == Some(v))
        })
        .cloned()
        .collect();
    Json(Value::Array(items)).into_response()
}

async fn e2b_get(State(s): State<E2b>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = e2b_auth(&s, &h, format!("GET /sandboxes/{id}"), Value::Null) {
        return r;
    }
    match s.lock().unwrap().sandboxes.get(&id) {
        Some(d) => {
            let mut d = d.clone();
            d["envdAccessToken"] = json!("envd-token");
            d["domain"] = Value::Null;
            Json(d).into_response()
        }
        None => err(
            StatusCode::NOT_FOUND,
            &format!("sandbox \"{id}\" doesn't exist"),
        ),
    }
}

async fn e2b_delete(State(s): State<E2b>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = e2b_auth(&s, &h, format!("DELETE /sandboxes/{id}"), Value::Null) {
        return r;
    }
    match s.lock().unwrap().sandboxes.remove(&id) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => err(
            StatusCode::NOT_FOUND,
            &format!("sandbox \"{id}\" doesn't exist"),
        ),
    }
}

async fn e2b_timeout(
    State(s): State<E2b>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Value>,
) -> Response {
    if let Some(r) = e2b_auth(&s, &h, format!("POST /sandboxes/{id}/timeout"), b) {
        return r;
    }
    if s.lock().unwrap().sandboxes.contains_key(&id) {
        StatusCode::NO_CONTENT.into_response()
    } else {
        err(StatusCode::NOT_FOUND, "sandbox not found")
    }
}

async fn e2b_pause(State(s): State<E2b>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = e2b_auth(&s, &h, format!("POST /sandboxes/{id}/pause"), Value::Null) {
        return r;
    }
    match s.lock().unwrap().sandboxes.get_mut(&id) {
        Some(d) if d["state"] == "paused" => err(StatusCode::CONFLICT, "sandbox is already paused"),
        Some(d) => {
            d["state"] = json!("paused");
            StatusCode::NO_CONTENT.into_response()
        }
        None => err(StatusCode::NOT_FOUND, "sandbox not found"),
    }
}

async fn e2b_connect(
    State(s): State<E2b>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(b): Json<Value>,
) -> Response {
    if let Some(r) = e2b_auth(&s, &h, format!("POST /v2/sandboxes/{id}/connect"), b) {
        return r;
    }
    match s.lock().unwrap().sandboxes.get_mut(&id) {
        Some(d) => {
            let resumed = d["state"] == "paused";
            d["state"] = json!("running");
            let code = if resumed {
                StatusCode::CREATED
            } else {
                StatusCode::OK
            };
            (
                code,
                Json(json!({"templateID": d["templateID"], "sandboxID": id,
                    "clientID": "6532622b", "envdVersion": "0.2.4",
                    "envdAccessToken": "envd-token", "domain": null})),
            )
                .into_response()
        }
        None => err(StatusCode::NOT_FOUND, "sandbox not found"),
    }
}

// -------------------------------------------------------------- Daytona

#[derive(Default)]
struct DaytonaState {
    key: String,
    seen: Vec<Seen>,
    snapshots: BTreeMap<String, (Value, u32)>,
    sandboxes: BTreeMap<String, (Value, u32)>,
    /// port -> URL template (`{id}`), else the standard preview domain.
    port_urls: BTreeMap<u16, String>,
    fail_snapshots: bool,
    next: u32,
}

type Dt = Arc<Mutex<DaytonaState>>;

/// A mock Daytona API.
pub struct MockDaytona {
    addr: SocketAddr,
    state: Dt,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockDaytona {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn dt_auth(s: &Dt, h: &HeaderMap, route: String, body: Value) -> Option<Response> {
    let mut st = s.lock().unwrap();
    st.seen.push(Seen { route, body });
    let want = format!("Bearer {}", st.key);
    let ok = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|k| k == want);
    (!ok).then(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"statusCode": 401, "message": "Unauthorized", "error": "Unauthorized"})),
        )
            .into_response()
    })
}

fn dt_err(code: StatusCode, message: &str) -> Response {
    (
        code,
        Json(json!({"statusCode": code.as_u16(), "message": message,
            "error": code.canonical_reason().unwrap_or("Error")})),
    )
        .into_response()
}

impl MockDaytona {
    /// Starts it, accepting `api_key`.
    pub async fn start(api_key: &str) -> Self {
        let state: Dt = Arc::new(Mutex::new(DaytonaState {
            key: api_key.into(),
            ..Default::default()
        }));
        let router = Router::new()
            .route("/snapshots", get(dt_snapshots).post(dt_new_snapshot))
            .route(
                "/snapshots/{id}",
                get(dt_snapshot).delete(dt_delete_snapshot),
            )
            .route("/sandbox", post(dt_create).get(dt_list))
            .route("/sandbox/{id}", get(dt_get).delete(dt_delete))
            .route("/sandbox/{id}/ports/{port}/preview-url", get(dt_preview))
            .route("/sandbox/{id}/stop", post(dt_stop))
            .route("/sandbox/{id}/start", post(dt_start))
            .route("/sandbox/{id}/ttl/{minutes}", post(dt_ttl))
            .with_state(state.clone());
        let (addr, task) = serve(router).await;
        Self { addr, state, task }
    }

    /// The API base URL (`DAYTONA_API_URL`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Requests so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    /// Live sandbox ids.
    pub fn sandboxes(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .sandboxes
            .keys()
            .cloned()
            .collect()
    }

    /// Snapshot names.
    pub fn snapshots(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .snapshots
            .values()
            .map(|(s, _)| s["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// Serves `port`'s preview URL as `url` (for example a mock spacesd).
    pub fn route_port(&self, port: u16, url: &str) {
        self.state
            .lock()
            .unwrap()
            .port_urls
            .insert(port, url.trim_end_matches('/').to_string());
    }

    /// Makes every new snapshot end in `error`.
    pub fn fail_snapshots(&self, fail: bool) {
        self.state.lock().unwrap().fail_snapshots = fail;
    }
}

fn dt_snapshot_doc(id: &str, name: &str, image: &str, state: &str) -> Value {
    json!({
        "id": id, "organizationId": "org-1", "general": false, "name": name,
        "imageName": image, "state": state, "size": null, "entrypoint": null,
        "cpu": 2, "gpu": 0, "mem": 4, "disk": 10, "errorReason": null,
        "createdAt": TS, "updatedAt": TS, "lastUsedAt": null, "regionIds": ["us"],
        "sandboxClass": "container",
    })
}

async fn dt_snapshots(
    State(s): State<Dt>,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = dt_auth(&s, &h, "GET /snapshots".into(), Value::Null) {
        return r;
    }
    let st = s.lock().unwrap();
    let items: Vec<Value> = st
        .snapshots
        .values()
        .map(|(v, _)| v.clone())
        .filter(|v| {
            q.get("name")
                .is_none_or(|n| v["name"].as_str().is_some_and(|x| x.contains(n.as_str())))
        })
        .collect();
    Json(json!({"items": items, "total": items.len(), "page": 1, "totalPages": 1})).into_response()
}

async fn dt_new_snapshot(State(s): State<Dt>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if let Some(r) = dt_auth(&s, &h, "POST /snapshots".into(), b.clone()) {
        return r;
    }
    let (Some(name), Some(image)) = (b["name"].as_str(), b["imageName"].as_str()) else {
        return dt_err(StatusCode::BAD_REQUEST, "name and imageName are required");
    };
    if image.ends_with(":latest") {
        return dt_err(
            StatusCode::BAD_REQUEST,
            "Images with tag latest are not allowed",
        );
    }
    let mut st = s.lock().unwrap();
    if st.snapshots.values().any(|(v, _)| v["name"] == name) {
        return dt_err(
            StatusCode::CONFLICT,
            &format!("Snapshot with name {name} already exists"),
        );
    }
    st.next += 1;
    let id = format!("snap-{}", st.next);
    let state = if st.fail_snapshots {
        "error"
    } else {
        "pending"
    };
    let mut doc = dt_snapshot_doc(&id, name, image, state);
    if st.fail_snapshots {
        doc["errorReason"] = json!("Failed to pull image: not found");
    }
    st.snapshots.insert(id, (doc.clone(), 2));
    Json(doc).into_response()
}

async fn dt_snapshot(State(s): State<Dt>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = dt_auth(&s, &h, format!("GET /snapshots/{id}"), Value::Null) {
        return r;
    }
    let mut st = s.lock().unwrap();
    match st.snapshots.get_mut(&id) {
        Some((doc, polls)) => {
            if doc["state"] == "pending" || doc["state"] == "pulling" {
                if *polls == 0 {
                    doc["state"] = json!("active");
                } else {
                    *polls -= 1;
                    doc["state"] = json!("pulling");
                }
            }
            Json(doc.clone()).into_response()
        }
        None => dt_err(
            StatusCode::NOT_FOUND,
            &format!("Snapshot with ID {id} not found"),
        ),
    }
}

async fn dt_delete_snapshot(State(s): State<Dt>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = dt_auth(&s, &h, format!("DELETE /snapshots/{id}"), Value::Null) {
        return r;
    }
    match s.lock().unwrap().snapshots.remove(&id) {
        Some(_) => StatusCode::OK.into_response(),
        None => dt_err(StatusCode::NOT_FOUND, "Snapshot not found"),
    }
}

fn dt_sandbox_doc(id: &str, b: &Value, state: &str) -> Value {
    json!({
        "id": id, "organizationId": "org-1", "name": b["name"].as_str().unwrap_or(id),
        "snapshot": b["snapshot"], "user": "daytona", "env": b["env"],
        "labels": b["labels"], "public": b["public"].as_bool().unwrap_or(false),
        "networkBlockAll": false, "target": b["target"].as_str().unwrap_or("us"),
        "cpu": 2, "gpu": 0, "memory": 4, "disk": 10, "state": state,
        "desiredState": "started", "errorReason": null, "recoverable": false,
        "autoStopInterval": b["autoStopInterval"], "autoDeleteInterval": -1,
        "createdAt": TS, "updatedAt": TS, "sandboxClass": "container",
        "toolboxProxyUrl": "https://proxy.app.daytona.io/toolbox",
    })
}

async fn dt_create(State(s): State<Dt>, h: HeaderMap, Json(b): Json<Value>) -> Response {
    if let Some(r) = dt_auth(&s, &h, "POST /sandbox".into(), b.clone()) {
        return r;
    }
    let mut st = s.lock().unwrap();
    let snap = b["snapshot"].as_str().unwrap_or_default();
    if !st
        .snapshots
        .values()
        .any(|(v, _)| v["name"] == snap && v["state"] == "active")
    {
        return dt_err(
            StatusCode::BAD_REQUEST,
            &format!("Snapshot {snap} is not active"),
        );
    }
    st.next += 1;
    let id = format!(
        "0b3a2f1c-{:04}-4e0e-9f7c-5d2b1a0c9e{:02}",
        st.next,
        st.next % 100
    );
    let doc = dt_sandbox_doc(&id, &b, "creating");
    st.sandboxes.insert(id, (doc.clone(), 1));
    Json(doc).into_response()
}

async fn dt_list(
    State(s): State<Dt>,
    h: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Some(r) = dt_auth(&s, &h, "GET /sandbox".into(), Value::Null) {
        return r;
    }
    let labels: BTreeMap<String, String> = q
        .get("labels")
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or_default();
    let st = s.lock().unwrap();
    let items: Vec<Value> = st
        .sandboxes
        .values()
        .map(|(v, _)| v.clone())
        .filter(|v| {
            labels
                .iter()
                .all(|(k, want)| v["labels"][k].as_str() == Some(want))
        })
        .collect();
    Json(json!({"items": items, "nextCursor": null})).into_response()
}

async fn dt_get(State(s): State<Dt>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = dt_auth(&s, &h, format!("GET /sandbox/{id}"), Value::Null) {
        return r;
    }
    let mut st = s.lock().unwrap();
    match st.sandboxes.get_mut(&id) {
        Some((doc, polls)) => {
            if doc["state"] == "creating" || doc["state"] == "starting" {
                if *polls == 0 {
                    doc["state"] = json!("started");
                } else {
                    *polls -= 1;
                }
            }
            Json(doc.clone()).into_response()
        }
        None => dt_err(
            StatusCode::NOT_FOUND,
            &format!("Sandbox with ID or name {id} not found"),
        ),
    }
}

async fn dt_delete(State(s): State<Dt>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = dt_auth(&s, &h, format!("DELETE /sandbox/{id}"), Value::Null) {
        return r;
    }
    match s.lock().unwrap().sandboxes.remove(&id) {
        Some((mut doc, _)) => {
            doc["state"] = json!("destroying");
            Json(doc).into_response()
        }
        None => dt_err(
            StatusCode::NOT_FOUND,
            &format!("Sandbox with ID or name {id} not found"),
        ),
    }
}

async fn dt_preview(
    State(s): State<Dt>,
    h: HeaderMap,
    Path((id, port)): Path<(String, u16)>,
) -> Response {
    let route = format!("GET /sandbox/{id}/ports/{port}/preview-url");
    if let Some(r) = dt_auth(&s, &h, route, Value::Null) {
        return r;
    }
    let st = s.lock().unwrap();
    if !st.sandboxes.contains_key(&id) {
        return dt_err(StatusCode::NOT_FOUND, "Sandbox not found");
    }
    let url = st
        .port_urls
        .get(&port)
        .map(|u| u.replace("{id}", &id))
        .unwrap_or_else(|| format!("https://{port}-{id}.proxy.daytona.works"));
    Json(json!({"sandboxId": id, "url": url, "token": "preview-token"})).into_response()
}

async fn dt_stop(State(s): State<Dt>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = dt_auth(&s, &h, format!("POST /sandbox/{id}/stop"), Value::Null) {
        return r;
    }
    match s.lock().unwrap().sandboxes.get_mut(&id) {
        Some((doc, _)) => {
            doc["state"] = json!("stopped");
            StatusCode::OK.into_response()
        }
        None => dt_err(StatusCode::NOT_FOUND, "Sandbox not found"),
    }
}

async fn dt_start(State(s): State<Dt>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if let Some(r) = dt_auth(&s, &h, format!("POST /sandbox/{id}/start"), Value::Null) {
        return r;
    }
    match s.lock().unwrap().sandboxes.get_mut(&id) {
        Some((doc, polls)) => {
            doc["state"] = json!("starting");
            *polls = 1;
            StatusCode::OK.into_response()
        }
        None => dt_err(StatusCode::NOT_FOUND, "Sandbox not found"),
    }
}

async fn dt_ttl(
    State(s): State<Dt>,
    h: HeaderMap,
    Path((id, minutes)): Path<(String, u64)>,
) -> Response {
    if let Some(r) = dt_auth(
        &s,
        &h,
        format!("POST /sandbox/{id}/ttl/{minutes}"),
        Value::Null,
    ) {
        return r;
    }
    if s.lock().unwrap().sandboxes.contains_key(&id) {
        StatusCode::OK.into_response()
    } else {
        dt_err(StatusCode::NOT_FOUND, "Sandbox not found")
    }
}
