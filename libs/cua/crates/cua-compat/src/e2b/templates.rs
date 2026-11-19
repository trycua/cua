//! E2B templates (`/v3/templates`, `/templates/...`): the build API the
//! `Template` builder talks to, run on Cua Spaces.
//!
//! A build boots a Space from the base Linux image (or from another
//! template's checkpoint), applies the builder's steps as root or the named
//! user (`RUN`, `COPY`, `ENV`, `WORKDIR`, `USER` are everything the SDK's
//! helpers compile to), starts the start command and waits for the ready
//! command, then takes a memory checkpoint. `create(template=name[:tag])`
//! forks that checkpoint, so a started service is running in every
//! sandbox, as with E2B's snapshots.
//!
//! The base image must be one the gateway can boot: `from_base_image()`,
//! E2B's own base names, or a Cua image reference. Other OCI images have no
//! cua-spacesd; a build from one fails with a build error that says so.

use super::E2b;
use super::api::{api_error, authed, body_json, rfc3339};
use crate::auth::KeyEntry;
use crate::backend::{CreateSpec, Identity, Placement, Source};
use crate::state::now_ms;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use bytes::Bytes;
use futures_util::StreamExt;
use http::{HeaderMap, Response, StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub ts_ms: u64,
    pub level: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    pub id: String,
    /// `waiting`, `building`, `ready`, `error`.
    pub status: String,
    pub cpus: u32,
    pub memory_mb: u64,
    pub created_ms: u64,
    #[serde(default)]
    pub finished_ms: Option<u64>,
    #[serde(default)]
    pub reason: Option<(String, Option<String>)>,
    #[serde(default)]
    pub logs: Vec<LogLine>,
    /// The Cua checkpoint a ready build left.
    #[serde(default)]
    pub checkpoint: Option<String>,
    /// Sandboxes of this build start in this directory and as this user.
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    /// Tags that name this build (`default` for an untagged name).
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tpl {
    pub id: String,
    pub key_id: String,
    /// Bare names (no namespace, no tag).
    pub names: Vec<String>,
    /// tag -> (build id, created ms)
    pub tags: BTreeMap<String, (String, u64)>,
    pub builds: Vec<Build>,
    pub created_ms: u64,
    #[serde(default)]
    pub spawn_count: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    templates: Vec<Tpl>,
}

/// The templates the gateway built.
pub struct Templates {
    path: Option<PathBuf>,
    /// Uploaded `COPY` contexts: `<dir>/<template>/<hash>`.
    pub uploads: PathBuf,
    inner: Mutex<BTreeMap<String, Tpl>>,
}

fn new_template_id() -> String {
    crate::e2b::api::new_id()
}

fn uuid() -> String {
    use rand::Rng;
    let mut r = rand::rng();
    let b: [u8; 16] = r.random();
    let h = hex::encode(b);
    format!(
        "{}-{}-4{}-{:x}{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        8 + (b[8] & 3),
        &h[17..20],
        &h[20..32]
    )
}

impl Templates {
    pub fn open(path: Option<PathBuf>, uploads: PathBuf) -> std::io::Result<Arc<Self>> {
        let file: File = match &path {
            Some(p) => match std::fs::read(p) {
                Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
                Err(e) => return Err(e),
            },
            None => File::default(),
        };
        let mut map: BTreeMap<String, Tpl> = file
            .templates
            .into_iter()
            .map(|t| (t.id.clone(), t))
            .collect();
        // A build the gateway was stopped in the middle of will not finish.
        for t in map.values_mut() {
            for b in &mut t.builds {
                if b.status == "building" {
                    b.status = "error".into();
                    b.reason = Some(("the gateway stopped during the build".into(), None));
                }
            }
        }
        Ok(Arc::new(Self {
            path,
            uploads,
            inner: Mutex::new(map),
        }))
    }

    fn persist(&self, map: &BTreeMap<String, Tpl>) {
        let Some(p) = &self.path else { return };
        let body = serde_json::to_vec_pretty(&File {
            templates: map.values().cloned().collect(),
        })
        .expect("templates");
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = crate::tls::write_file(p, &body, 0o600) {
            tracing::warn!("compat: saving templates: {e}");
        }
    }

    /// The ready build named by `name[:tag]` (or a template id) for `key`.
    pub async fn resolve(&self, key_id: &str, name: &str) -> Option<(Tpl, Build)> {
        let (base, tag) = split_name(name);
        let g = self.inner.lock().await;
        let tpl = g
            .values()
            .find(|t| t.key_id == key_id && (t.id == base || t.names.iter().any(|n| n == base)))?;
        let (bid, _) = tpl.tags.get(tag)?;
        let b = tpl
            .builds
            .iter()
            .find(|b| b.id == *bid && b.status == "ready")?;
        Some((tpl.clone(), b.clone()))
    }

    pub async fn bump_spawn(&self, id: &str) {
        let mut g = self.inner.lock().await;
        if let Some(t) = g.get_mut(id) {
            t.spawn_count += 1;
        }
        self.persist(&g);
    }

    async fn log(
        &self,
        tid: &str,
        bid: &str,
        level: &str,
        message: impl Into<String>,
        step: Option<&str>,
    ) {
        let mut g = self.inner.lock().await;
        if let Some(b) = g
            .get_mut(tid)
            .and_then(|t| t.builds.iter_mut().find(|b| b.id == bid))
        {
            b.logs.push(LogLine {
                ts_ms: now_ms(),
                level: level.into(),
                message: message.into(),
                step: step.map(str::to_string),
            });
        }
    }

    async fn update_build(&self, tid: &str, bid: &str, f: impl FnOnce(&mut Build)) {
        let mut g = self.inner.lock().await;
        if let Some(b) = g
            .get_mut(tid)
            .and_then(|t| t.builds.iter_mut().find(|b| b.id == bid))
        {
            f(b);
        }
        self.persist(&g);
    }
}

/// `name` and `tag` of `name[:tag]` (tag `default` when absent).
pub fn split_name(n: &str) -> (&str, &str) {
    match n.rsplit_once(':') {
        Some((a, t)) if !a.is_empty() && !t.contains('/') => (a, t),
        _ => (n, "default"),
    }
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

fn valid_tag(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 128
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn ns(key: &KeyEntry) -> String {
    let n = u16::from_str_radix(&key.id()[..4], 16).unwrap_or(0);
    format!("cua-{n}")
}

fn json_resp(status: StatusCode, v: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v.to_string()))
        .expect("response")
}

fn empty(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .expect("response")
}

fn not_found(what: &str) -> Response<Body> {
    api_error(
        StatusCode::NOT_FOUND,
        format!("template '{what}' not found"),
    )
}

/// `POST /v3/templates` (and `/v2/templates`): reserves the template name
/// and a build id.
pub async fn request_build(
    State(e): State<Arc<E2b>>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let given = b
        .get("name")
        .or_else(|| b.get("alias"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (name, tag) = split_name(given);
    if !valid_name(name) || !valid_tag(tag) {
        return api_error(
            StatusCode::BAD_REQUEST,
            format!("Invalid name: invalid template ID: {given}"),
        );
    }
    let mut tags: Vec<String> = b
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if given.contains(':') {
        tags.insert(0, tag.to_string());
    }
    if tags.is_empty() {
        tags.push("default".into());
    }
    if let Some(t) = tags.iter().find(|t| !valid_tag(t)) {
        return api_error(StatusCode::BAD_REQUEST, format!("Invalid tag: {t}"));
    }
    let cpus = b.get("cpuCount").and_then(Value::as_u64).unwrap_or(2) as u32;
    let mem = b.get("memoryMB").and_then(Value::as_u64).unwrap_or(1024);
    let mut g = e.templates.inner.lock().await;
    let tid = match g
        .values()
        .find(|t| t.key_id == key.id() && t.names.iter().any(|n| n == name))
    {
        Some(t) => t.id.clone(),
        None => {
            let id = new_template_id();
            g.insert(
                id.clone(),
                Tpl {
                    id: id.clone(),
                    key_id: key.id(),
                    names: vec![name.to_string()],
                    tags: BTreeMap::new(),
                    builds: vec![],
                    created_ms: now_ms(),
                    spawn_count: 0,
                },
            );
            id
        }
    };
    let bid = uuid();
    let t = g.get_mut(&tid).expect("template");
    t.builds.push(Build {
        id: bid.clone(),
        status: "waiting".into(),
        cpus,
        memory_mb: mem,
        created_ms: now_ms(),
        finished_ms: None,
        reason: None,
        logs: vec![],
        checkpoint: None,
        workdir: None,
        user: None,
        tags: tags.clone(),
    });
    e.templates.persist(&g);
    json_resp(
        StatusCode::ACCEPTED,
        json!({
            "aliases": [name],
            "buildID": bid,
            "names": [format!("{}/{}", ns(&key), name)],
            "public": false,
            "tags": tags,
            "templateID": tid,
        }),
    )
}

fn valid_hash(h: &str) -> bool {
    h.len() == 64 && h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn upload_path(e: &E2b, tid: &str, hash: &str) -> PathBuf {
    e.templates.uploads.join(tid).join(hash)
}

/// `GET /templates/{id}/files/{hash}`: where to upload a `COPY` context.
pub async fn file_upload_link(
    State(e): State<Arc<E2b>>,
    Path((tid, hash)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if !valid_hash(&hash) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "validation error: error in openapi3filter.RequestError: parameter \"hash\" in path has an error: string doesn't match the regular expression \"^[0-9a-f]{64}$\"",
        );
    }
    {
        let g = e.templates.inner.lock().await;
        if !g.get(&tid).is_some_and(|t| t.key_id == key.id()) {
            return not_found(&tid);
        }
    }
    let present = upload_path(&e, &tid, &hash).is_file();
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("127.0.0.1");
    let mut v = json!({"present": present});
    if !present {
        v["url"] = json!(format!("http://{host}/templates/{tid}/upload/{hash}"));
    }
    json_resp(StatusCode::CREATED, v)
}

/// `PUT /templates/{id}/upload/{hash}`: stores a build context archive.
/// The URL is the capability (as E2B's presigned storage URLs are): it is
/// handed out only to the owner, for a 64 hex digit hash.
pub async fn upload_context(
    State(e): State<Arc<E2b>>,
    Path((tid, hash)): Path<(String, String)>,
    body: Body,
) -> Response<Body> {
    if !valid_hash(&hash) || !tid.chars().all(|c| c.is_ascii_alphanumeric()) {
        return api_error(StatusCode::BAD_REQUEST, "invalid upload url");
    }
    if !e.templates.inner.lock().await.contains_key(&tid) {
        return not_found(&tid);
    }
    let final_path = upload_path(&e, &tid, &hash);
    if let Some(d) = final_path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = final_path.with_extension(format!("part{}", std::process::id()));
    let mut f = match tokio::fs::File::create(&tmp).await {
        Ok(f) => f,
        Err(err) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    let mut data = body.into_data_stream();
    let mut total: u64 = 0;
    while let Some(chunk) = data.next().await {
        let chunk: Bytes = match chunk {
            Ok(c) => c,
            Err(err) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return api_error(StatusCode::BAD_REQUEST, err.to_string());
            }
        };
        total += chunk.len() as u64;
        if total > (4u64 << 30) {
            let _ = tokio::fs::remove_file(&tmp).await;
            return api_error(StatusCode::PAYLOAD_TOO_LARGE, "build context over 4 GiB");
        }
        if let Err(err) = f.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
        }
    }
    drop(f);
    if let Err(err) = tokio::fs::rename(&tmp, &final_path).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
    }
    empty(StatusCode::OK)
}

/// What `POST /v2/templates/{id}/builds/{build}` carries.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct StartBody {
    from_image: Option<String>,
    from_template: Option<String>,
    steps: Vec<Step>,
    start_cmd: Option<String>,
    ready_cmd: Option<String>,
    from_image_registry: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Step {
    r#type: String,
    args: Vec<String>,
    files_hash: Option<String>,
}

/// `POST /v2/templates/{id}/builds/{build}`: runs the build.
pub async fn start_build(
    State(e): State<Arc<E2b>>,
    Path((tid, bid)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let raw = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let req: StartBody = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(err) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                format!("invalid build request: {err}"),
            );
        }
    };
    let status = {
        let g = e.templates.inner.lock().await;
        match g.get(&tid) {
            Some(t) if t.key_id == key.id() => t
                .builds
                .iter()
                .find(|b| b.id == bid)
                .map(|b| b.status.clone()),
            _ => None,
        }
    };
    match status.as_deref() {
        None => return not_found(&tid),
        Some("waiting") => {}
        Some(_) => return api_error(StatusCode::CONFLICT, "the build was already started"),
    }
    e.templates
        .update_build(&tid, &bid, |b| b.status = "building".into())
        .await;
    let ee = e.clone();
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let res = run_build(&ee, &key, &tid, &bid, req).await;
        let took = started.elapsed().as_secs();
        match res {
            Ok(done) => {
                ee.templates
                    .log(
                        &tid,
                        &bid,
                        "info",
                        format!("Build finished, took {took}s"),
                        None,
                    )
                    .await;
                ee.templates
                    .update_build(&tid, &bid, |b| {
                        b.status = "ready".into();
                        b.finished_ms = Some(now_ms());
                        b.checkpoint = Some(done.checkpoint);
                        b.workdir = done.workdir;
                        b.user = done.user;
                    })
                    .await;
                // Its tags now point at this build.
                let tags = {
                    let g = ee.templates.inner.lock().await;
                    g.get(&tid)
                        .and_then(|t| t.builds.iter().find(|b| b.id == bid))
                        .map(|b| b.tags.clone())
                        .unwrap_or_default()
                };
                let mut g = ee.templates.inner.lock().await;
                if let Some(t) = g.get_mut(&tid) {
                    for tag in tags {
                        t.tags.insert(tag, (bid.clone(), now_ms()));
                    }
                }
                ee.templates.persist(&g);
            }
            Err(f) => {
                ee.templates
                    .log(&tid, &bid, "error", f.message.clone(), f.step.as_deref())
                    .await;
                ee.templates
                    .update_build(&tid, &bid, |b| {
                        b.status = "error".into();
                        b.finished_ms = Some(now_ms());
                        b.reason = Some((f.message, f.step));
                    })
                    .await;
            }
        }
    });
    empty(StatusCode::ACCEPTED)
}

struct Done {
    checkpoint: String,
    workdir: Option<String>,
    user: Option<String>,
}

struct Failed {
    message: String,
    step: Option<String>,
}

fn fail(message: impl Into<String>, step: Option<&str>) -> Failed {
    Failed {
        message: message.into(),
        step: step.map(str::to_string),
    }
}

/// The image a build starts from, or why it cannot.
async fn base_source(e: &E2b, key: &KeyEntry, req: &StartBody) -> Result<Source, Failed> {
    if let Some(t) = req.from_template.as_deref().filter(|t| !t.is_empty()) {
        return match e.templates.resolve(&key.id(), t).await {
            Some((_, b)) => Ok(Source::Checkpoint(b.checkpoint.unwrap_or_default())),
            None => Err(fail(
                format!("base template '{t}' not found or not ready"),
                Some("base"),
            )),
        };
    }
    let image = req.from_image.clone().unwrap_or_default();
    let base = matches!(
        image.as_str(),
        "" | "e2bdev/base" | "e2bdev/base:latest" | "e2bdev/code-interpreter-v1"
    );
    if base {
        return Ok(Source::Image(None));
    }
    if req.from_image_registry.is_some() {
        return Err(fail(
            "private registries are not supported by the Cua gateway: base images must be Cua images (from_base_image() or a Cua image reference)",
            Some("base"),
        ));
    }
    // A Cua image reference: `vm:<path>`, a registry path with a digest or
    // a namespace the Cua image store knows.
    if image.starts_with("vm:")
        || image.starts_with("ghcr.io/trycua/")
        || image.contains("@sha256:")
    {
        return Ok(Source::Image(Some(image)));
    }
    Err(fail(
        format!(
            "base image '{image}' cannot be booted: Cua Spaces boot images that carry cua-spacesd (the default Linux image via from_base_image(), or a Cua image reference), not arbitrary OCI images"
        ),
        Some("base"),
    ))
}

async fn run_build(
    e: &Arc<E2b>,
    key: &KeyEntry,
    tid: &str,
    bid: &str,
    req: StartBody,
) -> Result<Done, Failed> {
    let t = &e.templates;
    t.log(
        tid,
        bid,
        "info",
        format!("Building template {tid}/{bid}"),
        None,
    )
    .await;
    let source = base_source(e, key, &req).await?;
    let (cpus, mem) = {
        let g = t.inner.lock().await;
        g.get(tid)
            .and_then(|x| x.builds.iter().find(|b| b.id == bid))
            .map(|b| (b.cpus, b.memory_mb))
            .unwrap_or((2, 1024))
    };
    let who = Identity {
        daemon: key.daemon.clone(),
        token: key.token.clone(),
    };
    let name = format!("e2b-tb-{}", &bid[..8]);
    t.log(
        tid,
        bid,
        "info",
        format!(
            "[base] FROM {}",
            req.from_image
                .clone()
                .or(req.from_template.clone())
                .unwrap_or_else(|| "e2bdev/base".into())
        ),
        Some("base"),
    )
    .await;
    let created = e
        .backend
        .create(
            &who,
            CreateSpec {
                name: name.clone(),
                source: source.clone(),
                env: Default::default(),
                internet: true,
                place: Placement {
                    cpus: Some(cpus),
                    memory_mb: Some(mem),
                    ..Default::default()
                },
            },
        )
        .await
        .map_err(|err| fail(format!("starting the build sandbox: {err}"), Some("base")))?;
    let cleanup = |e: Arc<E2b>, who: Identity, name: String| async move {
        let _ = e.backend.delete(&who, &name).await;
    };
    let out = build_steps(e, key, &who, &name, tid, bid, &req, &source).await;
    cleanup(e.clone(), who.clone(), created.name.clone()).await;
    out
}

#[allow(clippy::too_many_arguments)]
async fn build_steps(
    e: &Arc<E2b>,
    _key: &KeyEntry,
    who: &Identity,
    name: &str,
    tid: &str,
    bid: &str,
    req: &StartBody,
    source: &Source,
) -> Result<Done, Failed> {
    let t = &e.templates;
    let c = e.backend.spacesd(who, name).await.map_err(|err| {
        fail(
            format!("connecting to the build sandbox: {err}"),
            Some("base"),
        )
    })?;
    let driver = e.drivers.of(&format!("build-{bid}"), &c).await;
    if matches!(source, Source::Image(_)) {
        super::api::ensure_default_user(e, &c, &driver, &e.cfg.default_user).await;
        if req
            .from_image
            .as_deref()
            .is_some_and(super::code_interpreter::wants)
        {
            super::code_interpreter::provision(e, &c, &driver).await;
        }
    }
    let mut user = e.cfg.default_user.clone();
    let mut workdir = e.home_of(&c, &driver, &user).await;
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    let n = req.steps.len();
    for (i, s) in req.steps.iter().enumerate() {
        let step = i.to_string();
        let label = format!("[builder {}/{}]", i + 1, n);
        match s.r#type.as_str() {
            "ENV" => {
                for kv in s.args.chunks(2) {
                    if let [k, v] = kv {
                        env.insert(k.clone(), v.clone());
                    }
                }
                t.log(
                    tid,
                    bid,
                    "info",
                    format!("{label} ENV {}", s.args.join(" ")),
                    Some(&step),
                )
                .await;
            }
            "WORKDIR" => {
                let d = s.args.first().cloned().unwrap_or_default();
                t.log(
                    tid,
                    bid,
                    "info",
                    format!("{label} WORKDIR {d}"),
                    Some(&step),
                )
                .await;
                let abs = if d.starts_with('/') {
                    d.clone()
                } else {
                    format!("{}/{d}", workdir.trim_end_matches('/'))
                };
                let o = crate::guest::run_as(
                    &c,
                    &driver,
                    "root",
                    "mkdir -p -- \"$1\" && chown \"$2\": \"$1\"",
                    &[&abs, &user],
                )
                .await
                .map_err(|err| fail(err, Some(&step)))?;
                if !o.success() {
                    return Err(fail(
                        String::from_utf8_lossy(&o.stderr).trim().to_string(),
                        Some(&step),
                    ));
                }
                workdir = abs;
            }
            "USER" => {
                let u = s.args.first().cloned().unwrap_or_default();
                t.log(tid, bid, "info", format!("{label} USER {u}"), Some(&step))
                    .await;
                user = u;
            }
            "COPY" => {
                t.log(
                    tid,
                    bid,
                    "info",
                    format!(
                        "{label} COPY {}",
                        s.args.iter().take(2).cloned().collect::<Vec<_>>().join(" ")
                    ),
                    Some(&step),
                )
                .await;
                copy_step(e, &c, &driver, tid, s, &user, &workdir)
                    .await
                    .map_err(|m| fail(m, Some(&step)))?;
            }
            "RUN" => {
                let cmd = s.args.first().cloned().unwrap_or_default();
                let run_user = s
                    .args
                    .get(1)
                    .filter(|u| !u.is_empty())
                    .cloned()
                    .unwrap_or_else(|| user.clone());
                t.log(tid, bid, "info", format!("{label} RUN {cmd}"), Some(&step))
                    .await;
                let code = run_logged(
                    e, &c, &driver, tid, bid, &step, &label, &run_user, &workdir, &env, &cmd,
                )
                .await?;
                if code != 0 {
                    return Err(fail(
                        format!("failed to run command: exit status {code}"),
                        Some(&step),
                    ));
                }
            }
            other => {
                return Err(fail(
                    format!("unknown build instruction '{other}'"),
                    Some(&step),
                ));
            }
        }
    }
    // The start command runs in the background; the ready command says when
    // the template is ready to snapshot.
    if let Some(start) = req.start_cmd.as_deref().filter(|s| !s.is_empty()) {
        t.log(
            tid,
            bid,
            "info",
            format!("Running start command: {start}"),
            None,
        )
        .await;
        let (command, args, u, cwd) = crate::guest::as_user_in(
            &driver,
            &user,
            "/bin/bash".into(),
            vec!["-l".into(), "-c".into(), start.to_string()],
            workdir.clone(),
        );
        let p = cua_spacesd_client::pb::StartProcessRequest {
            config: Some(cua_spacesd_client::pb::ProcessConfig {
                command,
                args,
                env: env.clone().into_iter().collect(),
                cwd,
                user: u,
                timeout: None,
            }),
            kill_on_disconnect: false,
            ..Default::default()
        };
        // Held open until the checkpoint is taken: dropping the stream must
        // not end the process (kill_on_disconnect is false).
        let _keep = c
            .process()
            .start_process(p)
            .await
            .map_err(|err| fail(format!("starting the start command: {err}"), None))?;
        if let Some(ready) = req.ready_cmd.as_deref().filter(|s| !s.is_empty()) {
            t.log(
                tid,
                bid,
                "info",
                format!("Waiting for template to be ready: {ready}"),
                None,
            )
            .await;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
            loop {
                let o = crate::guest::run_as(
                    &c,
                    &driver,
                    &user,
                    "cd -- \"$1\" 2>/dev/null; exec /bin/bash -l -c \"$2\"",
                    &[&workdir, ready],
                )
                .await;
                if matches!(o, Ok(ref o) if o.success()) {
                    break;
                }
                if std::time::Instant::now() > deadline {
                    return Err(fail(
                        format!("the ready command '{ready}' did not succeed within 10 minutes"),
                        None,
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            t.log(tid, bid, "info", "Template is ready", None).await;
        }
    }
    let cp = format!("tpl-{tid}-{}", &bid[..8]);
    e.backend
        .checkpoint(who, name, &cp)
        .await
        .map_err(|err| fail(format!("saving the template: {err}"), None))?;
    Ok(Done {
        checkpoint: cp,
        workdir: Some(workdir),
        user: Some(user),
    })
}

/// Runs `cmd` and logs its output as it comes; the exit code.
#[allow(clippy::too_many_arguments)]
async fn run_logged(
    e: &Arc<E2b>,
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    tid: &str,
    bid: &str,
    step: &str,
    label: &str,
    user: &str,
    workdir: &str,
    env: &BTreeMap<String, String>,
    cmd: &str,
) -> Result<i32, Failed> {
    use cua_spacesd_client::pb;
    let (command, args, u, cwd) = crate::guest::as_user_in(
        driver,
        user,
        "/bin/bash".into(),
        vec!["-l".into(), "-c".into(), cmd.to_string()],
        workdir.to_string(),
    );
    let req = pb::StartProcessRequest {
        config: Some(pb::ProcessConfig {
            command,
            args,
            env: env.clone().into_iter().collect(),
            cwd,
            user: u,
            timeout: None,
        }),
        kill_on_disconnect: true,
        ..Default::default()
    };
    let mut stream = c
        .process()
        .start_process(req)
        .await
        .map_err(|err| fail(format!("starting '{cmd}': {err}"), Some(step)))?
        .into_inner();
    let mut pending: [String; 2] = [String::new(), String::new()];
    let mut code = -1;
    while let Some(m) = stream.next().await {
        let m = m.map_err(|err| fail(format!("running '{cmd}': {err}"), Some(step)))?;
        match m.event.and_then(|e| e.event) {
            Some(pb::process_event::Event::Data(d)) => {
                let (idx, bytes, tag) = match d.output {
                    Some(pb::process_data::Output::Stdout(b)) => (0, b, "stdout"),
                    Some(pb::process_data::Output::Stderr(b)) => (1, b, "stderr"),
                    _ => continue,
                };
                pending[idx].push_str(&String::from_utf8_lossy(&bytes));
                while let Some(i) = pending[idx].find('\n') {
                    let line: String = pending[idx].drain(..=i).collect();
                    e.templates
                        .log(
                            tid,
                            bid,
                            "info",
                            format!("{label} [{tag}]: {}", line.trim_end()),
                            Some(step),
                        )
                        .await;
                }
            }
            Some(pb::process_event::Event::End(end)) => {
                code = end.exit_code.unwrap_or(-1);
                break;
            }
            _ => {}
        }
    }
    for (idx, tag) in [(0, "stdout"), (1, "stderr")] {
        if !pending[idx].is_empty() {
            e.templates
                .log(
                    tid,
                    bid,
                    "info",
                    format!("{label} [{tag}]: {}", pending[idx]),
                    Some(step),
                )
                .await;
        }
    }
    Ok(code)
}

/// `COPY src dest [user [mode]]`: unpacks the uploaded context and copies
/// the matches the way a Dockerfile `COPY` does.
async fn copy_step(
    e: &Arc<E2b>,
    c: &cua_spacesd_client::SpacesdClient,
    driver: &str,
    tid: &str,
    s: &Step,
    user: &str,
    workdir: &str,
) -> Result<(), String> {
    let hash = s.files_hash.clone().ok_or("COPY has no files hash")?;
    let archive = upload_path(e, tid, &hash);
    if !archive.is_file() {
        return Err(format!(
            "the files for COPY {} were not uploaded",
            s.args.first().cloned().unwrap_or_default()
        ));
    }
    let src = s.args.first().cloned().unwrap_or_default();
    let dest = s.args.get(1).cloned().unwrap_or_default();
    let owner = s
        .args
        .get(2)
        .filter(|u| !u.is_empty())
        .cloned()
        .unwrap_or_else(|| user.to_string());
    let mode = s
        .args
        .get(3)
        .filter(|m| !m.is_empty())
        .cloned()
        .unwrap_or_default();
    // Stream the archive into the guest.
    let staged = format!("/tmp/.cua-ctx-{}.tar", crate::auth::new_token());
    let mut w = crate::guestfs::Writer::begin(c, &staged, "root", driver)
        .await
        .map_err(|f| f.1)?;
    let mut f = tokio::fs::File::open(&archive)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        use tokio::io::AsyncReadExt;
        let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        w.write(Bytes::copy_from_slice(&buf[..n]))
            .await
            .map_err(|f| f.1)?;
    }
    w.finish().await.map_err(|f| f.1)?;
    let script = r#"
set -e
ctx=$(mktemp -d /tmp/.cua-ctx.XXXXXX)
trap 'rm -rf "$ctx" "$1"' EXIT
if tar -tzf "$1" >/dev/null 2>&1; then tar -xzf "$1" -C "$ctx"; else tar -xf "$1" -C "$ctx"; fi
src="$2"; dest="$3"; owner="$4"; mode="$5"; wd="$6"
case "$dest" in /*) ;; *) dest="$wd/$dest";; esac
cd "$ctx"
found=0
for m in $src; do
  [ -e "$m" ] || [ -L "$m" ] || continue
  found=1
  if [ -d "$m" ] && [ ! -L "$m" ]; then
    mkdir -p -- "$dest"; cp -a -- "$m"/. "$dest"/
  elif [ "${dest%/}" != "$dest" ] || [ -d "$dest" ]; then
    mkdir -p -- "$dest"; cp -a -- "$m" "$dest"/
  else
    mkdir -p -- "$(dirname "$dest")"; cp -a -- "$m" "$dest"
  fi
done
[ "$found" = 1 ] || { echo "COPY $src: no files matched" >&2; exit 2; }
if [ -n "$owner" ] && [ -e "$dest" ]; then chown -R "$owner" "$dest" 2>/dev/null || chown -R "${owner%%:*}:" "$dest"; fi
if [ -n "$mode" ] && [ -e "$dest" ]; then chmod -R "$mode" "$dest"; fi
"#;
    let o = crate::guest::run_as(
        c,
        driver,
        "root",
        script,
        &[&staged, &src, &dest, &owner, &mode, workdir],
    )
    .await?;
    if o.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

#[derive(Deserialize, Default)]
pub struct StatusQuery {
    #[serde(rename = "logsOffset")]
    logs_offset: Option<usize>,
    limit: Option<usize>,
}

fn log_json(l: &LogLine) -> Value {
    let mut v = json!({"level": l.level, "message": l.message, "timestamp": rfc3339(l.ts_ms)});
    if let Some(s) = &l.step {
        v["step"] = json!(s);
    }
    v
}

/// `GET /templates/{id}/builds/{build}/status`.
pub async fn build_status(
    State(e): State<Arc<E2b>>,
    Path((tid, bid)): Path<(String, String)>,
    Query(q): Query<StatusQuery>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let g = e.templates.inner.lock().await;
    let Some((t, b)) = g
        .get(&tid)
        .filter(|t| t.key_id == key.id())
        .and_then(|t| t.builds.iter().find(|b| b.id == bid).map(|b| (t, b)))
    else {
        return api_error(StatusCode::NOT_FOUND, format!("build '{bid}' not found"));
    };
    // The status endpoint returns at most 100 entries per call.
    let from = q.logs_offset.unwrap_or(0).min(b.logs.len());
    let take = q.limit.unwrap_or(100).min(100);
    let entries: Vec<Value> = b.logs.iter().skip(from).take(take).map(log_json).collect();
    let mut v = json!({
        "buildID": b.id,
        "templateID": t.id,
        "status": b.status,
        "logEntries": entries,
        "logs": [],
    });
    if let Some((m, s)) = &b.reason {
        let mut r = json!({"message": m});
        if let Some(s) = s {
            r["step"] = json!(s);
        }
        v["reason"] = r;
    }
    json_resp(StatusCode::OK, v)
}

fn template_json(key: &KeyEntry, t: &Tpl) -> Value {
    let latest = t
        .builds
        .iter()
        .rev()
        .find(|b| b.status == "ready")
        .or(t.builds.last());
    json!({
        "aliases": t.names,
        "buildCount": t.builds.len(),
        "buildID": latest.map(|b| b.id.clone()).unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".into()),
        "buildStatus": latest.map(|b| b.status.clone()).unwrap_or_else(|| "waiting".into()),
        "cpuCount": latest.map(|b| b.cpus).unwrap_or(0),
        "createdAt": rfc3339(t.created_ms),
        "createdBy": null,
        "diskSizeMB": 0,
        "envdVersion": super::ENVD_VERSION,
        "lastSpawnedAt": null,
        "memoryMB": latest.map(|b| b.memory_mb).unwrap_or(0),
        "names": t.names.iter().map(|n| format!("{}/{n}", ns(key))).collect::<Vec<_>>(),
        "public": false,
        "spawnCount": t.spawn_count,
        "templateID": t.id,
        "updatedAt": rfc3339(t.builds.last().map(|b| b.created_ms).unwrap_or(t.created_ms)),
    })
}

/// `GET /templates`.
pub async fn list(State(e): State<Arc<E2b>>, headers: HeaderMap) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let g = e.templates.inner.lock().await;
    let mut mine: Vec<&Tpl> = g.values().filter(|t| t.key_id == key.id()).collect();
    mine.sort_by_key(|t| t.created_ms);
    json_resp(
        StatusCode::OK,
        Value::Array(mine.iter().map(|t| template_json(&key, t)).collect()),
    )
}

/// `GET /templates/{id}`: the template and its builds.
pub async fn get(
    State(e): State<Arc<E2b>>,
    Path(tid): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let g = e.templates.inner.lock().await;
    let Some(t) = g.get(&tid).filter(|t| t.key_id == key.id()) else {
        return not_found(&tid);
    };
    let mut builds: Vec<Value> = t
        .builds
        .iter()
        .rev()
        .map(|b| {
            let mut v = json!({
                "buildID": b.id,
                "cpuCount": b.cpus,
                "createdAt": rfc3339(b.created_ms),
                "memoryMB": b.memory_mb,
                "status": b.status,
                "updatedAt": rfc3339(b.finished_ms.unwrap_or(b.created_ms)),
            });
            if let Some(f) = b.finished_ms {
                v["finishedAt"] = json!(rfc3339(f));
            }
            if b.status == "ready" {
                v["envdVersion"] = json!(super::ENVD_VERSION);
            }
            v
        })
        .collect();
    builds.truncate(200);
    json_resp(
        StatusCode::OK,
        json!({
            "aliases": t.names,
            "builds": builds,
            "createdAt": rfc3339(t.created_ms),
            "lastSpawnedAt": null,
            "names": t.names.iter().map(|n| format!("{}/{n}", ns(&key))).collect::<Vec<_>>(),
            "public": false,
            "spawnCount": t.spawn_count,
            "templateID": t.id,
            "updatedAt": rfc3339(t.builds.last().map(|b| b.created_ms).unwrap_or(t.created_ms)),
        }),
    )
}

/// `PATCH /templates/{id}` (`public`): templates are private to the key.
pub async fn update(
    State(e): State<Arc<E2b>>,
    Path(tid): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    if b.get("public").and_then(Value::as_bool) == Some(true) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "public templates are not supported by the Cua gateway: a template belongs to the API key that built it",
        );
    }
    let g = e.templates.inner.lock().await;
    if !g.get(&tid).is_some_and(|t| t.key_id == key.id()) {
        return not_found(&tid);
    }
    json_resp(StatusCode::OK, Value::Null)
}

/// `DELETE /templates/{id}` for a template (snapshots are handled by the
/// caller).
pub async fn delete_template(e: &E2b, key: &KeyEntry, tid: &str) -> Option<Response<Body>> {
    let removed = {
        let mut g = e.templates.inner.lock().await;
        if !g.get(tid).is_some_and(|t| t.key_id == key.id()) {
            return None;
        }
        let t = g.remove(tid);
        e.templates.persist(&g);
        t
    };
    let who = Identity {
        daemon: key.daemon.clone(),
        token: key.token.clone(),
    };
    if let Some(t) = removed {
        for b in t.builds {
            if let Some(cp) = b.checkpoint
                && !e.store.is_held(&cp).await
            {
                let _ = e.backend.delete_checkpoint(&who, &cp).await;
            }
        }
        let _ = std::fs::remove_dir_all(e.templates.uploads.join(tid));
    }
    Some(empty(StatusCode::NO_CONTENT))
}

/// `GET /templates/aliases/{alias}`.
pub async fn alias(
    State(e): State<Arc<E2b>>,
    Path(alias): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let (name, tag) = split_name(&alias);
    let g = e.templates.inner.lock().await;
    match g.values().find(|t| {
        t.key_id == key.id()
            && t.names.iter().any(|n| n == name)
            && (tag == "default" || t.tags.contains_key(tag))
    }) {
        Some(t) => json_resp(StatusCode::OK, json!({"public": false, "templateID": t.id})),
        None => not_found(&alias),
    }
}

/// `GET /templates/{id}/tags`.
pub async fn tags(
    State(e): State<Arc<E2b>>,
    Path(tid): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let g = e.templates.inner.lock().await;
    let Some(t) = g.get(&tid).filter(|t| t.key_id == key.id()) else {
        return not_found(&tid);
    };
    let mut rows: Vec<(&String, &(String, u64))> = t.tags.iter().collect();
    rows.sort_by_key(|(tag, (_, at))| (*at, (*tag).clone()));
    json_resp(
        StatusCode::OK,
        Value::Array(
            rows.iter()
                .map(|(tag, (bid, at))| json!({"buildID": bid, "createdAt": rfc3339(*at), "tag": tag}))
                .collect(),
        ),
    )
}

/// `POST /templates/tags` `{target: "name:tag", tags}`: more tags for a
/// build.
pub async fn assign_tags(
    State(e): State<Arc<E2b>>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let target = b
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let new: Vec<String> = b
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if let Some(t) = new.iter().find(|t| !valid_tag(t)) {
        return api_error(StatusCode::BAD_REQUEST, format!("Invalid tag: {t}"));
    }
    let (name, tag) = split_name(&target);
    let mut g = e.templates.inner.lock().await;
    let Some(t) = g
        .values_mut()
        .find(|t| t.key_id == key.id() && (t.id == name || t.names.iter().any(|n| n == name)))
    else {
        return api_error(
            StatusCode::NOT_FOUND,
            format!("template '{name}' not found"),
        );
    };
    let Some((bid, _)) = t.tags.get(tag).cloned() else {
        return api_error(
            StatusCode::NOT_FOUND,
            format!("Template '{target}' with tag '{tag}' not found"),
        );
    };
    for n in &new {
        t.tags.insert(n.clone(), (bid.clone(), now_ms()));
    }
    e.templates.persist(&g);
    json_resp(StatusCode::CREATED, json!({"buildID": bid, "tags": new}))
}

/// `DELETE /templates/tags` `{name, tags}`.
pub async fn remove_tags(
    State(e): State<Arc<E2b>>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let name = b.get("name").and_then(Value::as_str).unwrap_or_default();
    let drop: Vec<String> = b
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut g = e.templates.inner.lock().await;
    let Some(t) = g
        .values_mut()
        .find(|t| t.key_id == key.id() && (t.id == name || t.names.iter().any(|n| n == name)))
    else {
        return api_error(
            StatusCode::NOT_FOUND,
            format!("template '{name}' not found"),
        );
    };
    for tag in &drop {
        t.tags.remove(tag);
    }
    e.templates.persist(&g);
    empty(StatusCode::NO_CONTENT)
}
