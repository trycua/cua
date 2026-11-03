//! Daytona secrets (`/secret`) and mounting them into sandboxes.
//!
//! Daytona keeps a secret's value on the server, puts an opaque placeholder
//! in the sandbox's environment and substitutes the real value in outbound
//! HTTPS requests to the secret's allowed hosts (so sandbox code never sees
//! it). That substitution needs a TLS-intercepting egress proxy, which Cua
//! Spaces do not have. The gateway therefore stores secrets (the value
//! write-only, in a 0600 file) and, to mount one, injects the real value
//! into the environment variable instead of the placeholder when the
//! sandbox says so with the label `cua.secrets=plaintext`; without that
//! label a create or `update_secrets` that names secrets is refused with the
//! reason. With the label, `hosts` is advisory: the value is visible to
//! sandbox code and nothing stops it going elsewhere. This is the
//! difference to read before choosing the label.
//!
//! Shapes: `daytona_api_client` 0.225.0 `secret_api.py`, `Secret`,
//! `CreateSecret`, `UpdateSecret`, `ListSecretsResponse`,
//! `UpdateSandboxSecrets`.

use super::stores::SecretEntry;
use super::{Daytona, error, ok_json};
use crate::auth::KeyEntry;
use crate::state::now_ms;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/api/secret", get(list_all).post(create))
        .route("/api/secret/paginated", get(list_paged))
        .route(
            "/api/secret/{id}",
            get(get_one).patch(update).delete(delete),
        )
        .with_state(d)
}

fn rfc3339(ms: u64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn dto(s: &SecretEntry) -> Value {
    json!({
        "id": s.id,
        "name": s.name,
        "description": s.description,
        "createdAt": rfc3339(s.created_ms),
        "updatedAt": rfc3339(s.updated_ms),
        "placeholder": s.placeholder,
        "hosts": s.hosts,
    })
}

fn created(v: Value) -> Response<Body> {
    Response::builder()
        .status(StatusCode::CREATED)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(v.to_string()))
        .expect("response")
}

/// `^[a-zA-Z_][a-zA-Z0-9_-]*$` (the SDK's documented rule for names).
pub(crate) fn valid_name(n: &str) -> bool {
    let mut c = n.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// A host entry: `api.example.com` or `*.example.com`, no port.
pub(crate) fn valid_host(h: &str) -> bool {
    let body = h.strip_prefix("*.").unwrap_or(h);
    !body.is_empty()
        && body.len() <= 253
        && !body.contains(':')
        && !body.contains('/')
        && body.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
}

async fn caller(d: &Daytona, h: &HeaderMap) -> Result<KeyEntry, Response<Body>> {
    d.caller(h)
        .await
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid credentials"))
}

async fn json_body(body: Body) -> Result<Value, Response<Body>> {
    let bytes = http_body_util::BodyExt::collect(http_body_util::Limited::new(body, 1 << 20))
        .await
        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("reading body: {e}")))?
        .to_bytes();
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| error(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")))
}

fn hosts_of(v: Option<&Value>) -> Result<Option<Vec<String>>, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let a = v.as_array().ok_or("hosts must be an array")?;
    let mut out = Vec::new();
    for h in a {
        let h = h.as_str().ok_or("each host must be a string")?;
        if !valid_host(h) {
            return Err(format!(
                "invalid host {h:?}: use a hostname or *.hostname, without a port"
            ));
        }
        out.push(h.to_string());
    }
    Ok(Some(out))
}

async fn create(State(d): State<Arc<Daytona>>, h: HeaderMap, body: Body) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match json_body(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(name) = b.get("name").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "name should not be empty");
    };
    if !valid_name(name) {
        return error(
            StatusCode::BAD_REQUEST,
            "name must match ^[a-zA-Z_][a-zA-Z0-9_-]*$",
        );
    }
    let Some(value) = b
        .get("value")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
    else {
        return error(StatusCode::BAD_REQUEST, "value should not be empty");
    };
    let hosts = match hosts_of(b.get("hosts")) {
        Ok(h) => h.unwrap_or_default(),
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let kid = key.id();
    let mut items = d.secrets.items.lock().await;
    if items.iter().any(|s| s.key_id == kid && s.name == name) {
        return error(
            StatusCode::CONFLICT,
            format!("Secret with name {name} already exists"),
        );
    }
    let now = now_ms();
    let s = SecretEntry {
        id: super::api_uuid(),
        name: name.to_string(),
        key_id: kid,
        value: value.to_string(),
        placeholder: format!("dtn_secret_{}", &crate::auth::new_token()[..24]),
        description: b
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        hosts,
        created_ms: now,
        updated_ms: now,
    };
    items.push(s.clone());
    drop(items);
    d.secrets.save().await;
    created(dto(&s))
}

async fn list_all(State(d): State<Arc<Daytona>>, h: HeaderMap) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let kid = key.id();
    ok_json(Value::Array(
        d.secrets
            .items
            .lock()
            .await
            .iter()
            .filter(|s| s.key_id == kid)
            .map(dto)
            .collect(),
    ))
}

#[derive(serde::Deserialize, Default)]
struct PageQuery {
    cursor: Option<String>,
    limit: Option<i64>,
    name: Option<String>,
    sort: Option<String>,
    order: Option<String>,
}

async fn list_paged(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let limit = match q.limit {
        None => 100,
        Some(l) if (1..=200).contains(&l) => l as usize,
        Some(_) => return error(StatusCode::BAD_REQUEST, "limit must be between 1 and 200"),
    };
    let kid = key.id();
    let mut all: Vec<SecretEntry> = d
        .secrets
        .items
        .lock()
        .await
        .iter()
        .filter(|s| s.key_id == kid)
        .filter(|s| {
            q.name
                .as_ref()
                .is_none_or(|n| s.name.to_lowercase().contains(&n.to_lowercase()))
        })
        .cloned()
        .collect();
    match q.sort.as_deref() {
        Some("name") => all.sort_by(|a, b| a.name.cmp(&b.name)),
        Some("updatedAt") => all.sort_by_key(|s| s.updated_ms),
        Some("createdAt") | None => all.sort_by_key(|s| s.created_ms),
        Some(o) => {
            return error(
                StatusCode::BAD_REQUEST,
                format!("sort {o:?} is not supported"),
            );
        }
    }
    if q.order.as_deref() == Some("desc") {
        all.reverse();
    }
    let start: usize = match &q.cursor {
        Some(c) => match c.strip_prefix("o:").and_then(|n| n.parse().ok()) {
            Some(n) => n,
            None => return error(StatusCode::BAD_REQUEST, "Invalid cursor"),
        },
        None => 0,
    };
    let total = all.len();
    let items: Vec<Value> = all.iter().skip(start).take(limit).map(dto).collect();
    let next = (start + limit < total).then(|| format!("o:{}", start + limit));
    ok_json(json!({"items": items, "total": total, "nextCursor": next}))
}

async fn find(d: &Daytona, key: &KeyEntry, id: &str) -> Option<SecretEntry> {
    let kid = key.id();
    d.secrets
        .items
        .lock()
        .await
        .iter()
        .find(|s| s.key_id == kid && (s.id == id || s.name == id))
        .cloned()
}

async fn get_one(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match find(&d, &key, &id).await {
        Some(s) => ok_json(dto(&s)),
        None => error(StatusCode::NOT_FOUND, format!("Secret {id} not found")),
    }
}

async fn update(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match json_body(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let hosts = match hosts_of(b.get("hosts")) {
        Ok(h) => h,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let Some(cur) = find(&d, &key, &id).await else {
        return error(StatusCode::NOT_FOUND, format!("Secret {id} not found"));
    };
    let mut items = d.secrets.items.lock().await;
    let Some(s) = items.iter_mut().find(|s| s.id == cur.id) else {
        return error(StatusCode::NOT_FOUND, format!("Secret {id} not found"));
    };
    if let Some(v) = b.get("value").and_then(Value::as_str) {
        if v.is_empty() {
            return error(StatusCode::BAD_REQUEST, "value should not be empty");
        }
        s.value = v.to_string();
    }
    if let Some(v) = b.get("description").and_then(Value::as_str) {
        s.description = Some(v.to_string());
    }
    if let Some(h) = hosts {
        s.hosts = h;
    }
    s.updated_ms = now_ms();
    let out = dto(s);
    drop(items);
    d.secrets.save().await;
    ok_json(out)
}

async fn delete(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let Some(s) = find(&d, &key, &id).await else {
        return error(StatusCode::NOT_FOUND, format!("Secret {id} not found"));
    };
    d.secrets.items.lock().await.retain(|x| x.id != s.id);
    d.secrets.save().await;
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("response")
}

/// Whether a sandbox's labels opt into plaintext secrets.
pub(crate) fn plaintext_ok(labels: &BTreeMap<String, String>) -> bool {
    labels.get("cua.secrets").map(String::as_str) == Some("plaintext")
}

/// Resolves `secrets: [{ENV: secret name}]` into `(env var, value)` pairs,
/// refusing when the sandbox did not opt into plaintext injection.
pub(crate) async fn resolve(
    d: &Daytona,
    key: &KeyEntry,
    wanted: &[(String, String)],
    labels: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, Response<Body>> {
    if wanted.is_empty() {
        return Ok(vec![]);
    }
    if !plaintext_ok(labels) {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Daytona keeps secret values out of the sandbox and substitutes them in outbound HTTPS requests; Cua Spaces have no such egress proxy. Add the label cua.secrets=plaintext to inject the secret's value into the environment variable instead (its hosts list then is advisory)",
        ));
    }
    let mut out = Vec::new();
    for (env, name) in wanted {
        if !env
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            || !env.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(error(
                StatusCode::BAD_REQUEST,
                format!("{env:?} is not a valid environment variable name"),
            ));
        }
        let Some(s) = find(d, key, name).await else {
            return Err(error(
                StatusCode::NOT_FOUND,
                format!("Secret {name} not found"),
            ));
        };
        out.push((env.clone(), s.value));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_hosts() {
        assert!(valid_name("anthropic-prod") && valid_name("_x1"));
        assert!(!valid_name("1a") && !valid_name("a b") && !valid_name(""));
        assert!(valid_host("api.example.com") && valid_host("*.example.com"));
        assert!(
            !valid_host("example.com:443")
                && !valid_host("*.")
                && !valid_host("a/b")
                && !valid_host("-a.com")
        );
    }
}
