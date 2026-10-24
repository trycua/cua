//! Daytona snapshots (`/snapshots`) and `Sandbox.create_snapshot`.
//!
//! A snapshot is a name for something sandboxes start from. Here it is one
//! of: an image reference (Cua pulls it: `imageName`), a Dockerfile (built
//! into Cua image layers: `buildInfo`), or a checkpoint of a running
//! sandbox (`POST /sandbox/{id}/snapshot`, a copy-on-write fork source).
//!
//! Creating one from an image or a Dockerfile is asynchronous like
//! Daytona's: the DTO comes back `pending`, goes `pulling` or `building`
//! and ends `active`, or `error` / `build_failed` with `errorReason`; the
//! SDK polls it (`SnapshotService.create`) and follows
//! `/snapshots/{id}/build-logs`. Resources given at creation (`cpu`,
//! `memory`, `disk`) are the defaults of sandboxes started from it.
//!
//! Shapes: `daytona_api_client` 0.225.0 `snapshots_api.py`, `SnapshotDto`,
//! `CreateSnapshot`, `PaginatedSnapshots`.

use super::api::{self, FollowQuery, authed, logs_url, owned, rfc3339, stream_log};
use super::{Daytona, SnapshotBuild, SnapshotEntry, backend_error, error, ok_json};
use crate::backend::CreateExtras;
use crate::e2b::TemplateTarget;
use crate::state::now_ms;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/api/snapshots", get(list).post(create))
        .route("/api/snapshots/{id}", get(get_one).delete(remove))
        .route("/api/snapshots/{id}/activate", post(activate))
        .route("/api/snapshots/{id}/deactivate", post(deactivate))
        .route("/api/snapshots/{id}/build-logs", get(build_logs))
        .route("/api/snapshots/{id}/build-logs-url", get(build_logs_url))
        .with_state(d)
}

pub(super) fn dto(s: &SnapshotEntry) -> Value {
    let image = match &s.target {
        TemplateTarget::Image(i) => Some(i.clone()),
        TemplateTarget::Checkpoint(_) => None,
    };
    let mut v = json!({
        "id": s.id,
        "organizationId": "cua",
        "general": false,
        "name": s.name,
        "imageName": s.image_name.clone().or(image),
        "state": s.state,
        "size": null,
        "entrypoint": s.entrypoint,
        "cpu": s.cpu,
        "gpu": s.gpu,
        "mem": s.memory,
        "disk": s.disk,
        "errorReason": s.error_reason,
        "createdAt": rfc3339(s.created_at_ms),
        "updatedAt": rfc3339(s.updated_ms.unwrap_or(s.created_at_ms)),
        "lastUsedAt": s.last_used_ms.map(rfc3339),
        "sourceSandboxId": s.source_sandbox_id,
        "ref": s.name,
        "sandboxClass": "cua",
    });
    if let Some(b) = &s.build {
        let t = rfc3339(s.created_at_ms);
        v["buildInfo"] = json!({
            "dockerfileContent": b.dockerfile,
            "contextHashes": b.context_hashes,
            "createdAt": t,
            "updatedAt": t,
            "snapshotRef": s.name,
        });
    }
    v
}

pub(super) async fn find(d: &Daytona, key_id: &str, id: &str) -> Option<SnapshotEntry> {
    d.snapshots
        .lock()
        .await
        .iter()
        .find(|s| (s.id == id || s.name == id) && s.visible_to(key_id))
        .cloned()
}

async fn update<T>(d: &Daytona, id: &str, f: impl FnOnce(&mut SnapshotEntry) -> T) -> Option<T> {
    let out = {
        let mut list = d.snapshots.lock().await;
        let s = list.iter_mut().find(|s| s.id == id)?;
        let out = f(s);
        s.updated_ms = Some(now_ms());
        out
    };
    d.save_snapshots().await;
    Some(out)
}

#[derive(serde::Deserialize, Default)]
struct ListQuery {
    page: Option<i64>,
    limit: Option<i64>,
    name: Option<String>,
    #[serde(rename = "sourceSandboxId")]
    source_sandbox_id: Option<String>,
    sort: Option<String>,
    order: Option<String>,
}

async fn list(
    State(d): State<Arc<Daytona>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let limit = match q.limit {
        None => 100,
        Some(l) if (1..=200).contains(&l) => l as usize,
        Some(_) => return error(StatusCode::BAD_REQUEST, "limit must be between 1 and 200"),
    };
    let page = match q.page {
        None => 1,
        Some(p) if p >= 1 => p as usize,
        Some(_) => return error(StatusCode::BAD_REQUEST, "page must not be less than 1"),
    };
    let kid = key.id();
    let mut all: Vec<SnapshotEntry> = d
        .snapshots
        .lock()
        .await
        .iter()
        .filter(|s| s.visible_to(&kid))
        .filter(|s| {
            q.name
                .as_ref()
                .is_none_or(|n| s.name.to_lowercase().contains(&n.to_lowercase()))
        })
        .filter(|s| {
            q.source_sandbox_id
                .as_ref()
                .is_none_or(|i| s.source_sandbox_id.as_ref() == Some(i))
        })
        .cloned()
        .collect();
    match q.sort.as_deref() {
        Some("name") => all.sort_by(|a, b| a.name.cmp(&b.name)),
        Some("state") => all.sort_by(|a, b| a.state.cmp(&b.state)),
        Some("lastUsedAt") => all.sort_by_key(|s| s.last_used_ms),
        Some("createdAt") | None => all.sort_by_key(|s| s.created_at_ms),
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
    let items: Vec<Value> = all
        .iter()
        .skip((page - 1) * limit)
        .take(limit)
        .map(dto)
        .collect();
    ok_json(json!({
        "items": items,
        "total": all.len(),
        "page": page,
        "totalPages": all.len().div_ceil(limit).max(1),
    }))
}

async fn get_one(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match find(&d, &key.id(), &id).await {
        Some(s) => ok_json(dto(&s)),
        None => error(
            StatusCode::NOT_FOUND,
            format!("Snapshot with ID or name {id} not found"),
        ),
    }
}

/// A snapshot name: no whitespace or slashes, at most 255 characters.
pub(crate) fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 255
        && !n
            .chars()
            .any(|c| c.is_whitespace() || c == '/' || c.is_control())
}

/// `POST /snapshots` `{name, imageName | buildInfo, entrypoint?, cpu?, memory?, disk?, gpu?}`.
async fn create(State(d): State<Arc<Daytona>>, headers: HeaderMap, body: Body) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b = match api::body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(name) = b.get("name").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "name should not be empty");
    };
    if !valid_name(name) {
        return error(
            StatusCode::BAD_REQUEST,
            "Snapshot name may not contain whitespace or '/'",
        );
    }
    let image_name = b
        .get("imageName")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let build_info = b.get("buildInfo").filter(|v| !v.is_null());
    let dockerfile = build_info
        .and_then(|bi| bi.get("dockerfileContent"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    let hashes: Vec<String> = build_info
        .and_then(|bi| bi.get("contextHashes"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|h| h.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    match (image_name, dockerfile) {
        (None, None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "Either imageName or buildInfo must be provided",
            );
        }
        (Some(_), Some(_)) => {
            return error(
                StatusCode::BAD_REQUEST,
                "imageName and buildInfo are mutually exclusive",
            );
        }
        _ => {}
    }
    #[allow(clippy::result_large_err)]
    let num = |k: &str| -> Result<u32, Response<Body>> {
        match b.get(k) {
            None | Some(Value::Null) => Ok(0),
            Some(v) => v
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    error(
                        StatusCode::BAD_REQUEST,
                        format!("{k} must be a non-negative integer"),
                    )
                }),
        }
    };
    let (cpu, memory, disk, gpu) = match (num("cpu"), num("memory"), num("disk"), num("gpu")) {
        (Ok(a), Ok(b), Ok(c), Ok(d)) => (a, b, c, d),
        (Err(r), ..) | (_, Err(r), ..) | (_, _, Err(r), _) | (.., Err(r)) => return r,
    };
    let entrypoint: Option<Vec<String>> = b.get("entrypoint").and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    });
    let kid = key.id();
    let mut list = d.snapshots.lock().await;
    if list.iter().any(|s| s.name == name && s.visible_to(&kid)) {
        return error(
            StatusCode::CONFLICT,
            format!("Snapshot with name {name} already exists"),
        );
    }
    let now = now_ms();
    let entry = SnapshotEntry {
        id: api::uuid(),
        name: name.into(),
        target: TemplateTarget::Image(image_name.unwrap_or("").into()),
        state: "pending".into(),
        created_at_ms: now,
        source_sandbox_id: None,
        build: dockerfile.map(|df| SnapshotBuild {
            dockerfile: df.to_string(),
            context_hashes: hashes.clone(),
        }),
        key_id: Some(kid),
        cpu,
        memory,
        disk,
        gpu,
        entrypoint,
        image_name: image_name.map(str::to_string),
        error_reason: None,
        updated_ms: Some(now),
        last_used_ms: None,
    };
    list.push(entry.clone());
    drop(list);
    d.save_snapshots().await;
    let out = dto(&entry);
    tokio::spawn(prepare(d.clone(), key, entry));
    ok_json(out)
}

/// Pulls or builds what a new snapshot names, so a bad image or Dockerfile
/// fails now (`error`/`build_failed`) and not at the first sandbox.
async fn prepare(d: Arc<Daytona>, key: crate::auth::KeyEntry, e: SnapshotEntry) {
    let log = super::build::log_for(&d, &e.id).await;
    let who = Daytona::identity(&key);
    let outcome: Result<(), (String, &'static str)> = async {
        if let Some(b) = &e.build {
            update(&d, &e.id, |s| s.state = "building".into()).await;
            log.line(&format!("Building snapshot {}", e.name)).await;
            let built = super::build::resolve(&d, &b.dockerfile, &b.context_hashes)
                .await
                .map_err(|m| (m, "build_failed"))?;
            log.line(&format!("Base image {}", built.from)).await;
            d.backend
                .check_image(&who, &built.from)
                .await
                .map_err(|e| (e.to_string(), "build_failed"))?;
            if let Some(layers) = &built.build {
                for (i, l) in layers.layers.iter().enumerate() {
                    log.line(&format!("Step {}/{}: {l:?}", i + 1, layers.layers.len()))
                        .await;
                }
                // Build once now (the core caches the result by content): a
                // throwaway sandbox from the layers, then deleted.
                let spec = crate::backend::CreateSpec {
                    name: format!("daytona-snap-{}", &e.id[..8]),
                    source: crate::backend::Source::Image(Some(built.from.clone())),
                    env: Default::default(),
                    internet: true,
                    place: Default::default(),
                };
                let extras = CreateExtras {
                    build: built.build.clone(),
                    command: built.command.clone(),
                    ..Default::default()
                };
                let created = d
                    .backend
                    .create_with(&who, spec, extras)
                    .await
                    .map_err(|e| (e.to_string(), "build_failed"))?;
                d.backend.delete(&who, &created.name).await.ok();
            }
            update(&d, &e.id, |s| {
                s.image_name = Some(built.from.clone());
                s.entrypoint = s.entrypoint.clone().or(built.command.clone());
            })
            .await;
        } else if let TemplateTarget::Image(img) = &e.target {
            update(&d, &e.id, |s| s.state = "pulling".into()).await;
            log.line(&format!("Pulling {img}")).await;
            d.backend
                .check_image(&who, img)
                .await
                .map_err(|e| (e.to_string(), "error"))?;
        }
        Ok(())
    }
    .await;
    match outcome {
        Ok(()) => {
            log.line("Snapshot is active").await;
            update(&d, &e.id, |s| s.state = "active".into()).await;
        }
        Err((reason, state)) => {
            log.line(&format!("Failed: {reason}")).await;
            update(&d, &e.id, |s| {
                s.state = state.into();
                s.error_reason = Some(reason.clone());
            })
            .await;
        }
    }
    log.finish().await;
}

/// `DELETE /snapshots/{id}`.
async fn remove(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let Some(s) = find(&d, &key.id(), &id).await else {
        return error(
            StatusCode::NOT_FOUND,
            format!("Snapshot with ID or name {id} not found"),
        );
    };
    if let TemplateTarget::Checkpoint(cp) = &s.target
        && let Err(e) = d
            .backend
            .delete_checkpoint(&Daytona::identity(&key), cp)
            .await
    {
        return backend_error(e);
    }
    d.snapshots.lock().await.retain(|x| x.id != s.id);
    d.save_snapshots().await;
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .expect("response")
}

async fn set_state(d: &Daytona, headers: &HeaderMap, id: &str, state: &str) -> Response<Body> {
    let key = match authed(d, headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let Some(s) = find(d, &key.id(), id).await else {
        return error(
            StatusCode::NOT_FOUND,
            format!("Snapshot with ID or name {id} not found"),
        );
    };
    match (s.state.as_str(), state) {
        ("active", "active") => {
            return error(
                StatusCode::BAD_REQUEST,
                format!("Snapshot {} is already active", s.name),
            );
        }
        ("inactive", "inactive") => {
            return error(
                StatusCode::BAD_REQUEST,
                format!("Snapshot {} is already inactive", s.name),
            );
        }
        ("active" | "inactive", _) => {}
        (other, _) => {
            return error(
                StatusCode::BAD_REQUEST,
                format!(
                    "Snapshot {} is {other}: only active or inactive snapshots can change",
                    s.name
                ),
            );
        }
    }
    update(d, &s.id, |x| x.state = state.into()).await;
    let cur = find(d, &key.id(), &s.id).await.unwrap_or(s);
    if state == "inactive" {
        return Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .expect("response");
    }
    ok_json(dto(&cur))
}

async fn activate(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    set_state(&d, &h, &id, "active").await
}

async fn deactivate(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    set_state(&d, &h, &id, "inactive").await
}

async fn build_logs_url(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    let key = match authed(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match find(&d, &key.id(), &id).await {
        Some(s) => logs_url(&d, format!("/snapshots/{}/build-logs", s.id)).await,
        None => error(
            StatusCode::NOT_FOUND,
            format!("Snapshot with ID or name {id} not found"),
        ),
    }
}

async fn build_logs(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    Query(q): Query<FollowQuery>,
) -> Response<Body> {
    let key = match authed(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match find(&d, &key.id(), &id).await {
        Some(s) => {
            stream_log(
                super::build::log_for(&d, &s.id).await,
                q.follow.unwrap_or(false),
            )
            .await
        }
        None => error(
            StatusCode::NOT_FOUND,
            format!("Snapshot with ID or name {id} not found"),
        ),
    }
}

/// `POST /sandbox/{id}/snapshot` `{name}`: a checkpoint of the live sandbox,
/// registered under `name` so `create(snapshot=name)` forks from it.
pub(super) async fn sandbox_snapshot(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let key = match authed(&d, &headers).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let r = match owned(&d, &key, &id).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    let b = match api::body_json(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    let Some(name) = b
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| valid_name(n))
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "name is required and may not contain whitespace or '/'",
        );
    };
    let kid = key.id();
    if find(&d, &kid, name).await.is_some() {
        return error(
            StatusCode::CONFLICT,
            format!("Snapshot with name {name} already exists"),
        );
    }
    if r.paused {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "Sandbox {} is not started (state {})",
                r.id,
                api::state_of(&r)
            ),
        );
    }
    let old = api::state_of(&r);
    d.store
        .update(&r.id, |r| {
            r.extra.insert("dtState".into(), json!("snapshotting"));
        })
        .await;
    if let Some(cur) = d.store.get(&r.id).await {
        d.announce(&cur, &old).await;
    }
    let res = d.backend.checkpoint(&r.identity(), &r.cua_name, name).await;
    d.store
        .update(&r.id, |r| {
            r.extra.remove("dtState");
        })
        .await;
    let cur = d.store.get(&r.id).await.unwrap_or(r.clone());
    d.announce(&cur, "snapshotting").await;
    if let Err(e) = res {
        return backend_error(e);
    }
    let now = now_ms();
    d.snapshots.lock().await.push(SnapshotEntry {
        id: api::uuid(),
        name: name.to_string(),
        target: TemplateTarget::Checkpoint(name.to_string()),
        state: "active".into(),
        created_at_ms: now,
        source_sandbox_id: Some(r.id.clone()),
        build: None,
        key_id: Some(kid),
        cpu: r.cpus,
        memory: (r.memory_mb / 1024).max(1) as u32,
        disk: (r.disk_mb / 1024) as u32,
        gpu: 0,
        entrypoint: None,
        image_name: None,
        error_reason: None,
        updated_ms: Some(now),
        last_used_ms: None,
    });
    d.save_snapshots().await;
    ok_json(api::sandbox_dto(&d, &cur))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("my-snap_1.0"));
        assert!(!valid_name("a b") && !valid_name("a/b") && !valid_name(""));
    }
}
