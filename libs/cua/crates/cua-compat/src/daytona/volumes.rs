//! Daytona volumes (`/volumes`) on Cua Volume.
//!
//! A Daytona volume is a named, durable folder several sandboxes mount.
//! Cua has exactly that: Cua Volume, the user's one volume every Space
//! mounts (`/volume` on Linux, `~/Cua Volume` on macOS), served from the
//! user's own S3 or this Mac. A Daytona volume is the folder
//! `daytona-volumes/<volume id>` of it, and mounting it at `mountPath` is a
//! bind mount (a symbolic link where the guest cannot bind) of that folder
//! or of its `subpath`. Nothing else is stored by the gateway but the
//! volume's name and id.
//!
//! Shapes: `daytona_api_client` 0.225.0 `volumes_api.py`, `VolumeDto`,
//! `CreateVolume`, `SandboxVolume`.

use super::stores::VolumeEntry;
use super::{Daytona, backend_error, error, ok_json};
use crate::auth::KeyEntry;
use crate::state::{Record, now_ms};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::routing::get;
use http::{HeaderMap, Response, StatusCode};
use serde_json::{Value, json};
use std::sync::Arc;

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/api/volumes", get(list).post(create))
        .route("/api/volumes/by-name/{name}", get(by_name))
        .route("/api/volumes/{id}", get(by_id).delete(delete))
        .with_state(d)
}

fn rfc3339(ms: u64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

pub(super) fn dto(v: &VolumeEntry) -> Value {
    json!({
        "id": v.id,
        "name": v.name,
        "organizationId": "cua",
        "state": v.state,
        "createdAt": rfc3339(v.created_ms),
        "updatedAt": rfc3339(v.updated_ms),
        "lastUsedAt": v.last_used_ms.map(rfc3339),
        "errorReason": v.error_reason,
    })
}

async fn caller(d: &Daytona, h: &HeaderMap) -> Result<KeyEntry, Response<Body>> {
    d.caller(h)
        .await
        .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "Invalid credentials"))
}

/// A volume name: letters, digits, `-`, `_`, `.`; at most 63 characters.
pub(crate) fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= 63
        && n.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

async fn list(State(d): State<Arc<Daytona>>, h: HeaderMap) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let kid = key.id();
    let v: Vec<Value> = d
        .volumes
        .items
        .lock()
        .await
        .iter()
        .filter(|v| v.key_id == kid && v.state != "deleted")
        .map(dto)
        .collect();
    ok_json(Value::Array(v))
}

async fn create(State(d): State<Arc<Daytona>>, h: HeaderMap, body: Body) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let b: Value = match http_body_util::BodyExt::collect(body).await {
        Ok(c) => serde_json::from_slice(&c.to_bytes()).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    let Some(name) = b.get("name").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "name should not be empty");
    };
    if !valid_name(name) {
        return error(
            StatusCode::BAD_REQUEST,
            format!(
                "Volume name {name:?} is invalid: use letters, digits, '-', '_' and '.' (at most 63 characters)"
            ),
        );
    }
    let kid = key.id();
    let mut items = d.volumes.items.lock().await;
    if items
        .iter()
        .any(|v| v.key_id == kid && v.name == name && v.state != "deleted")
    {
        return error(
            StatusCode::CONFLICT,
            format!("Volume with name {name} already exists"),
        );
    }
    let now = now_ms();
    let v = VolumeEntry {
        id: super::api_uuid(),
        name: name.to_string(),
        key_id: kid,
        created_ms: now,
        updated_ms: now,
        last_used_ms: None,
        state: "ready".into(),
        error_reason: None,
    };
    items.push(v.clone());
    drop(items);
    d.volumes.save().await;
    ok_json(dto(&v))
}

async fn find(d: &Daytona, key: &KeyEntry, id_or_name: &str) -> Option<VolumeEntry> {
    let kid = key.id();
    d.volumes
        .items
        .lock()
        .await
        .iter()
        .find(|v| {
            v.key_id == kid && v.state != "deleted" && (v.id == id_or_name || v.name == id_or_name)
        })
        .cloned()
}

async fn by_name(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Path(name): Path<String>,
) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match find(&d, &key, &name).await {
        Some(v) => ok_json(dto(&v)),
        None => error(
            StatusCode::NOT_FOUND,
            format!("Volume with name {name} not found"),
        ),
    }
}

async fn by_id(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    match find(&d, &key, &id).await {
        Some(v) => ok_json(dto(&v)),
        None => error(
            StatusCode::NOT_FOUND,
            format!("Volume with ID {id} not found"),
        ),
    }
}

/// `DELETE /volumes/{id}`: refused while a sandbox has it mounted (as
/// Daytona refuses), else the volume's folder is removed with it.
async fn delete(
    State(d): State<Arc<Daytona>>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> Response<Body> {
    let key = match caller(&d, &h).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let Some(v) = find(&d, &key, &id).await else {
        return error(
            StatusCode::NOT_FOUND,
            format!("Volume with ID {id} not found"),
        );
    };
    let kid = key.id();
    for r in d.store.all().await {
        if r.key_id == kid
            && r.extra
                .get("volumes")
                .and_then(Value::as_array)
                .is_some_and(|m| {
                    m.iter()
                        .any(|x| x.get("volumeId").and_then(Value::as_str) == Some(&v.id))
                })
        {
            return error(
                StatusCode::CONFLICT,
                format!("Volume {} is attached to sandbox {}", v.name, r.id),
            );
        }
    }
    if let Err(e) = remove_folder(&d, &key, &v).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    d.volumes.items.lock().await.retain(|x| x.id != v.id);
    d.volumes.save().await;
    let mut gone = v;
    gone.state = "deleted".into();
    ok_json(dto(&gone))
}

/// The folder of a volume in Cua Volume, relative to its root.
pub(crate) fn folder(v: &VolumeEntry) -> String {
    format!("daytona-volumes/{}", v.id)
}

/// Where Cua Volume is mounted in the guest of `r`, if it is.
async fn volume_root(c: &cua_spacesd_client::SpacesdClient) -> Result<String, String> {
    let st = c
        .volume()
        .get_volume_status(cua_spacesd_client::pb::GetVolumeStatusRequest {})
        .await
        .map_err(|e| format!("Cua Volume is not available in this Space: {e}"))?
        .into_inner();
    let mounted = st.state == cua_spacesd_client::pb::VolumeState::Mounted as i32;
    if mounted && !st.mount_path.is_empty() {
        Ok(st.mount_path)
    } else {
        Err(format!(
            "Cua Volume is not mounted in this Space (state {}): sign in to Cua Spaces and enable Cua Volume to use Daytona volumes",
            st.state
        ))
    }
}

async fn remove_folder(d: &Daytona, key: &KeyEntry, v: &VolumeEntry) -> Result<(), String> {
    // The folder lives in Cua Volume; any live sandbox of the caller can
    // reach it. With none running there is nothing to remove from here (the
    // folder stays empty-by-name and is reused by a later volume of that id,
    // which never happens: ids are fresh).
    let kid = key.id();
    for r in d.store.all().await {
        if r.key_id != kid || r.paused {
            continue;
        }
        let Ok(c) = d.spacesd(&r).await else { continue };
        let Ok(root) = volume_root(&c).await else {
            continue;
        };
        let driver = d.drivers.of(&r.id, &c).await;
        let path = format!("{}/{}", root.trim_end_matches('/'), folder(v));
        return match crate::guest::run_as(&c, &driver, "cua", "rm -rf -- \"$1\"", &[&path]).await {
            Ok(o) if o.success() => Ok(()),
            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
            Err(e) => Err(e),
        };
    }
    Ok(())
}

/// One requested mount of a create: `{volumeId, mountPath, subpath?}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mount {
    pub volume: String,
    pub path: String,
    pub subpath: Option<String>,
}

/// Parses and validates the `volumes` of a create: absolute, distinct,
/// non-system mount paths and a relative `subpath` without `..`.
pub(crate) fn parse_mounts(v: Option<&Value>) -> Result<Vec<Mount>, String> {
    let Some(a) = v.and_then(Value::as_array) else {
        return Ok(vec![]);
    };
    let mut out: Vec<Mount> = Vec::new();
    for m in a {
        let volume = m
            .get("volumeId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("each volume needs a volumeId")?;
        let path = m
            .get("mountPath")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("each volume needs a mountPath")?;
        if !path.starts_with('/') || path.contains("..") || path == "/" {
            return Err(format!(
                "mountPath {path:?} must be an absolute path other than /"
            ));
        }
        let path = path.trim_end_matches('/').to_string();
        for sys in [
            "/proc", "/sys", "/dev", "/bin", "/sbin", "/usr", "/lib", "/etc", "/boot",
        ] {
            if path == sys || path.starts_with(&format!("{sys}/")) {
                return Err(format!("mountPath {path:?} is a system directory"));
            }
        }
        if out.iter().any(|o| o.path == path) {
            return Err(format!("two volumes mount at {path}"));
        }
        let subpath = m
            .get("subpath")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_matches('/').to_string());
        if let Some(s) = &subpath
            && (s.split('/').any(|p| p == ".." || p == ".") || s.starts_with('/'))
        {
            return Err(format!("subpath {s:?} must stay inside the volume"));
        }
        out.push(Mount {
            volume: volume.to_string(),
            path,
            subpath,
        });
    }
    Ok(out)
}

/// Resolves requested mounts against the caller's volumes (id or name).
pub(crate) async fn resolve_mounts(
    d: &Daytona,
    key: &KeyEntry,
    mounts: &[Mount],
) -> Result<Vec<(VolumeEntry, Mount)>, Response<Body>> {
    let mut out = Vec::new();
    for m in mounts {
        let Some(v) = find(d, key, &m.volume).await else {
            return Err(error(
                StatusCode::NOT_FOUND,
                format!("Volume {} not found", m.volume),
            ));
        };
        if v.state != "ready" {
            return Err(error(
                StatusCode::BAD_REQUEST,
                format!(
                    "Volume {} is not in a ready state (state {})",
                    v.name, v.state
                ),
            ));
        }
        out.push((v, m.clone()));
    }
    Ok(out)
}

/// Mounts each volume in the guest: the volume's (sub)folder of Cua Volume
/// bound at its mount path (symbolic link where binding is impossible).
pub(crate) async fn mount_all(
    d: &Daytona,
    r: &Record,
    mounts: &[(VolumeEntry, Mount)],
) -> Result<(), Response<Body>> {
    if mounts.is_empty() {
        return Ok(());
    }
    let c = d.spacesd(r).await.map_err(backend_error)?;
    let root = volume_root(&c)
        .await
        .map_err(|e| error(StatusCode::BAD_REQUEST, e))?;
    let driver = d.drivers.of(&r.id, &c).await;
    let user = d.user_of(r);
    for (v, m) in mounts {
        let mut src = format!("{}/{}", root.trim_end_matches('/'), folder(v));
        if let Some(s) = &m.subpath {
            src = format!("{src}/{s}");
        }
        // The Cua Volume mount only lets its own user (uid 1000) in, root
        // included: the folder is made as that user, the bind mount (root)
        // is tried, and a symbolic link stands in where it cannot be made.
        let make = r#"mkdir -p -- "$1""#;
        match crate::guest::run_as(&c, &driver, "cua", make, &[&src]).await {
            Ok(o) if o.success() => {}
            Ok(o) => {
                let why = String::from_utf8_lossy(&o.stderr).trim().to_string();
                let hint = if why.contains("Permission denied") || why.contains("Read-only") {
                    format!(
                        ". Cua Volume lets a Space write only its own folder; a folder several sandboxes share needs a grant that you confirm: cua volume grant space:<this sandbox's Space> --prefix {} --mode rw",
                        folder(v)
                    )
                } else {
                    String::new()
                };
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Mounting volume {} at {} failed in Cua Volume: {why}{hint}",
                        v.name, m.path
                    ),
                ));
            }
            Err(e) => return Err(error(StatusCode::BAD_GATEWAY, e)),
        }
        let script = r#"
set -e
src="$1"; dst="$2"; user="$3"
if [ -L "$dst" ]; then rm -f -- "$dst"; fi
mkdir -p -- "$dst"
if mountpoint -q -- "$dst" 2>/dev/null; then exit 0; fi
if mount --bind -- "$src" "$dst" 2>/dev/null; then exit 0; fi
rmdir -- "$dst" 2>/dev/null || true
ln -s -- "$src" "$dst"
"#;
        match crate::guest::run_as(&c, &driver, "root", script, &[&src, &m.path, &user]).await {
            Ok(o) if o.success() => {}
            Ok(o) => {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Mounting volume {} at {} failed: {}",
                        v.name,
                        m.path,
                        String::from_utf8_lossy(&o.stderr).trim()
                    ),
                ));
            }
            Err(e) => return Err(error(StatusCode::BAD_GATEWAY, e)),
        }
    }
    let now = now_ms();
    let mut items = d.volumes.items.lock().await;
    for (v, _) in mounts {
        if let Some(x) = items.iter_mut().find(|x| x.id == v.id) {
            x.last_used_ms = Some(now);
        }
    }
    drop(items);
    d.volumes.save().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn mounts_are_validated() {
        let ok = json!([{"volumeId":"v","mountPath":"/data/","subpath":"a/b"}]);
        let m = parse_mounts(Some(&ok)).unwrap();
        assert_eq!(m[0].path, "/data");
        assert_eq!(m[0].subpath.as_deref(), Some("a/b"));
        for bad in [
            json!([{"volumeId":"v","mountPath":"data"}]),
            json!([{"volumeId":"v","mountPath":"/"}]),
            json!([{"volumeId":"v","mountPath":"/etc/x"}]),
            json!([{"volumeId":"v","mountPath":"/a/../b"}]),
            json!([{"volumeId":"v","mountPath":"/a","subpath":"../x"}]),
            json!([{"volumeId":"v","mountPath":"/a"},{"volumeId":"w","mountPath":"/a"}]),
            json!([{"mountPath":"/a"}]),
        ] {
            assert!(parse_mounts(Some(&bad)).is_err(), "{bad}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn valid_names_have_no_path_separators(n in "\\PC{0,80}") {
            if valid_name(&n) {
                prop_assert!(!n.contains('/') && !n.contains(' ') && n.len() <= 63);
            }
        }
    }
}
