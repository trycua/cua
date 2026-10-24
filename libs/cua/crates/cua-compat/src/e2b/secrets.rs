//! E2B secrets (`/secrets`): named, versioned, write-only values. The
//! gateway keeps them (0600, next to the sandbox records) for the key that
//! made them. Values are never returned by any call. E2B resolves
//! `${e2b.secrets.<name>}` placeholders in its egress proxy; this gateway
//! has no egress proxy, so a placeholder reaches a sandbox as typed (see
//! the coverage matrix: `network.rules` is refused).

use super::E2b;
use super::api::{api_error, authed, rfc3339};
use crate::state::now_ms;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use http::{HeaderMap, Response, StatusCode, header};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRec {
    pub id: String,
    pub name: String,
    /// The API key (id) that owns it.
    pub key_id: String,
    /// Every version's value, oldest first (write-only).
    pub values: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    secrets: Vec<SecretRec>,
}

/// The secrets of one gateway.
pub struct Secrets {
    path: Option<PathBuf>,
    inner: Mutex<BTreeMap<String, SecretRec>>,
}

impl Secrets {
    pub fn open(path: Option<PathBuf>) -> std::io::Result<Arc<Self>> {
        let file: File = match &path {
            Some(p) => match std::fs::read(p) {
                Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
                Err(e) => return Err(e),
            },
            None => File::default(),
        };
        Ok(Arc::new(Self {
            path,
            inner: Mutex::new(
                file.secrets
                    .into_iter()
                    .map(|s| (s.id.clone(), s))
                    .collect(),
            ),
        }))
    }

    fn persist(&self, map: &BTreeMap<String, SecretRec>) {
        let Some(p) = &self.path else { return };
        let body = serde_json::to_vec_pretty(&File {
            secrets: map.values().cloned().collect(),
        })
        .expect("secrets");
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = crate::tls::write_file(p, &body, 0o600) {
            tracing::warn!("compat: saving secrets: {e}");
        }
    }

    /// The value of a secret by name (for a later egress feature and tests).
    pub async fn value(&self, key_id: &str, name: &str) -> Option<String> {
        self.inner
            .lock()
            .await
            .values()
            .find(|s| s.key_id == key_id && s.name == name)
            .and_then(|s| s.values.last().cloned())
    }
}

fn new_secret_id() -> String {
    use rand::Rng;
    const A: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut rng = rand::rng();
    format!(
        "sec_{}",
        (0..26)
            .map(|_| A[rng.random_range(0..A.len())] as char)
            .collect::<String>()
    )
}

fn secret_json(s: &SecretRec) -> Value {
    json!({
        "secretID": s.id,
        "name": s.name,
        "currentVersion": s.values.len(),
        "metadata": s.metadata,
        "createdAt": rfc3339(s.created_ms),
        "updatedAt": rfc3339(s.updated_ms),
    })
}

/// Names E2B accepts (probed against the real service): letters, digits,
/// `_` and `-`, at most 128 characters.
fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 128
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && n.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
}

fn find<'a>(
    map: &'a mut BTreeMap<String, SecretRec>,
    key_id: &str,
    id_or_name: &str,
) -> Option<&'a mut SecretRec> {
    map.values_mut()
        .find(|s| s.key_id == key_id && (s.id == id_or_name || s.name == id_or_name))
}

fn meta(v: &Value) -> Option<BTreeMap<String, String>> {
    v.as_object().map(|m| {
        m.iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
            .collect()
    })
}

pub async fn create(State(e): State<Arc<E2b>>, headers: HeaderMap, body: Body) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let (Some(name), Some(value)) = (
        b.get("name").and_then(Value::as_str),
        b.get("value").and_then(Value::as_str),
    ) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid secrets request");
    };
    if !valid_name(name) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid secrets request");
    }
    let mut g = e.secrets.inner.lock().await;
    if g.values().any(|s| s.key_id == key.id() && s.name == name) {
        return api_error(
            StatusCode::CONFLICT,
            "Secret cannot be modified in its current state",
        );
    }
    let now = now_ms();
    let rec = SecretRec {
        id: new_secret_id(),
        name: name.to_string(),
        key_id: key.id(),
        values: vec![value.to_string()],
        metadata: b.get("metadata").and_then(meta).unwrap_or_default(),
        created_ms: now,
        updated_ms: now,
    };
    let out = secret_json(&rec);
    g.insert(rec.id.clone(), rec);
    e.secrets.persist(&g);
    json_created(out)
}

fn json_created(v: Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v.to_string()))
        .expect("response")
}

fn json_ok(v: Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(v.to_string()))
        .expect("response")
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    limit: Option<usize>,
    #[serde(rename = "nextToken")]
    next_token: Option<String>,
}

/// `GET /secrets`: newest first, paged by `limit` and `nextToken`.
pub async fn list(
    State(e): State<Arc<E2b>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let g = e.secrets.inner.lock().await;
    let mut mine: Vec<&SecretRec> = g.values().filter(|s| s.key_id == key.id()).collect();
    mine.sort_by(|a, b| (b.created_ms, &b.id).cmp(&(a.created_ms, &a.id)));
    let start: usize = q
        .next_token
        .as_deref()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    let limit = q.limit.filter(|l| *l > 0).unwrap_or(100);
    let page: Vec<Value> = mine
        .iter()
        .skip(start)
        .take(limit)
        .map(|s| secret_json(s))
        .collect();
    let mut resp = json_ok(Value::Array(page));
    if start + limit < mine.len() {
        resp.headers_mut().insert(
            "x-next-token",
            (start + limit).to_string().parse().expect("header"),
        );
    }
    resp
}

pub async fn get(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let mut g = e.secrets.inner.lock().await;
    match find(&mut g, &key.id(), &id) {
        Some(s) => json_ok(secret_json(s)),
        None => not_found(&id),
    }
}

fn not_found(id: &str) -> Response<Body> {
    api_error(StatusCode::NOT_FOUND, format!("Secret {id} not found"))
}

/// `POST /secrets/{id}`: a new version (and, when given, new metadata).
pub async fn update(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match super::api::body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(value) = b.get("value").and_then(Value::as_str) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid secrets request");
    };
    let mut g = e.secrets.inner.lock().await;
    let Some(s) = find(&mut g, &key.id(), &id) else {
        return not_found(&id);
    };
    s.values.push(value.to_string());
    if let Some(m) = b.get("metadata").and_then(meta) {
        s.metadata = m;
    }
    s.updated_ms = now_ms();
    let out = secret_json(s);
    e.secrets.persist(&g);
    json_ok(out)
}

pub async fn destroy(
    State(e): State<Arc<E2b>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&e, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let mut g = e.secrets.inner.lock().await;
    let found = find(&mut g, &key.id(), &id).map(|s| s.id.clone());
    match found {
        Some(sid) => {
            g.remove(&sid);
            e.secrets.persist(&g);
            Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .expect("response")
        }
        None => not_found(&id),
    }
}
