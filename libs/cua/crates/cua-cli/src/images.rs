//! Fleet image resources (`images.cua.ai/v1alpha1 Image`) for
//! `cua image ls|info|rm|create`. Local OCI pull/build/push live in main.

use crate::{
    auth,
    util::{self, line},
};
use cua_sdk::CuaError;
use std::io::Write;

async fn namespaces(
    c: &cua_fleet::FleetClient,
    ns: Option<String>,
) -> Result<Vec<String>, CuaError> {
    if let Some(n) = ns.or_else(|| {
        std::env::var("CUA_FLEET_NAMESPACE")
            .ok()
            .filter(|s| !s.is_empty())
    }) {
        return Ok(vec![n]);
    }
    Ok(c.sdk()
        .list_namespaces()
        .await
        .map_err(|e| auth::fleet_err(cua_fleet::Error::Sdk(e)))?
        .into_iter()
        .map(|n| n.name)
        .collect())
}

fn row(ns: &str, v: &serde_json::Value) -> serde_json::Value {
    let s = |p: &str| {
        v.pointer(p)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string()
    };
    serde_json::json!({
        "name": s("/metadata/name"),
        "namespace": ns,
        "phase": s("/status/phase"),
        "reference": v.pointer("/status/image").or(v.pointer("/status/reference")).and_then(|x| x.as_str()),
        "created": s("/metadata/creationTimestamp"),
    })
}

/// `cua image ls`.
pub async fn list(ns: Option<String>, json: bool, out: &mut dyn Write) -> Result<i32, CuaError> {
    let (c, _) = auth::fleet_client().await?;
    let explicit = ns.is_some();
    let mut rows = vec![];
    let mut skipped = 0;
    for n in namespaces(&c, ns).await? {
        match c.list_images(&n).await.map_err(auth::fleet_err) {
            Ok(images) => rows.extend(images.iter().map(|i| row(&n, i))),
            // Scanning every namespace: skip the ones without image access.
            Err(CuaError::PermissionDenied(_)) if !explicit => skipped += 1,
            Err(e) => return Err(e),
        }
    }
    if skipped > 0 {
        eprintln!("note: skipped {skipped} namespace(s) without permission to list images");
    }
    if json {
        util::json_line(out, &serde_json::Value::Array(rows));
        return Ok(0);
    }
    if rows.is_empty() {
        line(out, "No images found.");
        return Ok(0);
    }
    let s = |v: &serde_json::Value| v.as_str().unwrap_or("-").to_string();
    let t: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                s(&r["name"]),
                s(&r["namespace"]),
                s(&r["phase"]),
                s(&r["created"]).chars().take(10).collect(),
            ]
        })
        .collect();
    util::table(out, &["NAME", "NAMESPACE", "PHASE", "CREATED"], &t);
    Ok(0)
}

async fn find(
    c: &cua_fleet::FleetClient,
    name: &str,
    ns: Option<String>,
) -> Result<(String, serde_json::Value), CuaError> {
    for n in namespaces(c, ns).await? {
        match c.get_image(&n, name).await {
            Ok(v) => return Ok((n, v)),
            Err(e) => match auth::fleet_err(e) {
                CuaError::NotFound(_) => continue,
                e => return Err(e),
            },
        }
    }
    Err(CuaError::NotFound(format!("image not found: {name}")))
}

/// `cua image info`.
pub async fn info(name: String, ns: Option<String>, out: &mut dyn Write) -> Result<i32, CuaError> {
    let (c, _) = auth::fleet_client().await?;
    let (_, v) = find(&c, &name, ns).await?;
    util::json_line(out, &v);
    Ok(0)
}

/// `cua image rm`.
pub async fn delete(
    name: String,
    ns: Option<String>,
    force: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let (c, _) = auth::fleet_client().await?;
    let (n, _) = find(&c, &name, ns).await?;
    if !force && !util::confirm(&format!("Delete image {n}/{name}?"), false) {
        line(
            out,
            format!("This will delete {n}/{name}. Use --force to confirm."),
        );
        return Ok(1);
    }
    c.delete_image(&n, &name).await.map_err(auth::fleet_err)?;
    line(out, format!("Deleted: {n}/{name}"));
    Ok(0)
}

/// `cua image create`: submits an Image manifest (remote build).
pub async fn create(
    file: std::path::PathBuf,
    ns: Option<String>,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let raw = std::fs::read_to_string(&file).map_err(util::internal)?;
    let mut v: serde_json::Value = serde_json::from_str(&raw)?;
    let ns = ns
        .or_else(|| {
            v.pointer("/metadata/namespace")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        })
        .ok_or_else(|| {
            CuaError::InvalidArgument("pass --namespace or set metadata.namespace".into())
        })?;
    v["metadata"]["namespace"] = serde_json::json!(ns);
    let (c, _) = auth::fleet_client().await?;
    let r = c.create_image(&ns, v).await.map_err(auth::fleet_err)?;
    line(out, r.to_string());
    Ok(0)
}
