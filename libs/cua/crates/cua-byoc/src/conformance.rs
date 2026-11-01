// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The provider conformance suite: what every cloud must do for the elastic
//! pool (`provider-contract.md`), checked the same way for each one. Each
//! provider's test file runs it against its fake API (and against the real
//! one in an ignored live test), so a provider that drifts from the
//! contract fails CI, not a user's account.
//!
//! - [`static_checks`]: capabilities and host types are sane.
//! - [`lifecycle_checks`]: a host is created exactly as asked (type, region,
//!   user data byte for byte, every tag in the create call), listed with its
//!   role, refused to anyone else, deleted, and deleting it again is fine.
//!   A Spaces supplier refuses hosts.

use std::collections::BTreeMap;

use crate::api::CloudApi;
use crate::model::{self, Connection, ProvisionSpec, ROLE_HOST, Supplies};

/// What a fake API saw in one create call (filled by the provider's test
/// harness from the request it received).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateCall {
    /// The machine type in the request.
    pub machine_type: String,
    /// The region or location in the request.
    pub region: String,
    /// The user data in the request (decoded).
    pub user_data: String,
    /// The tags or labels in the request.
    pub tags: BTreeMap<String, String>,
    /// An SSH key was attached.
    pub ssh_key: bool,
}

/// Capabilities and host types are sane. Returns every failure.
pub async fn static_checks(api: &dyn CloudApi, conn: &Connection) -> Vec<String> {
    let mut bad = Vec::new();
    let name = api.name();
    let caps = api.capabilities(conn);
    if caps.supplies == Supplies::Nothing {
        bad.push(format!("{name}: supplies nothing to the elastic pool"));
        return bad;
    }
    if caps.regions.is_empty() {
        bad.push(format!("{name}: no regions"));
    }
    if caps.billing.granularity_secs == 0 {
        bad.push(format!("{name}: billing granularity is 0"));
    }
    if caps.max_machines == 0 {
        bad.push(format!("{name}: max_machines is 0"));
    }
    let types = match api.host_types(conn).await {
        Ok(t) => t,
        Err(e) => {
            bad.push(format!("{name}: host_types failed: {e}"));
            return bad;
        }
    };
    if types.is_empty() {
        bad.push(format!("{name}: no host types"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for t in &types {
        if !seen.insert(t.name.clone()) {
            bad.push(format!("{name}: type {} listed twice", t.name));
        }
        if t.usd_per_hour <= 0.0 || !t.usd_per_hour.is_finite() {
            bad.push(format!("{name}: {} has no price", t.name));
        }
        if t.vcpus == 0 || t.memory_mb == 0 {
            bad.push(format!("{name}: {} has no size", t.name));
        }
        if t.arch != "amd64" && t.arch != "arm64" {
            bad.push(format!("{name}: {} arch {:?}", t.name, t.arch));
        }
        if let Some(r) = t.regions.iter().find(|r| !caps.regions.contains(r)) {
            bad.push(format!(
                "{name}: {} offered in {r}, not a region it lists",
                t.name
            ));
        }
        if t.boot_secs == 0 {
            bad.push(format!("{name}: {} has no boot time", t.name));
        }
    }
    if types
        .windows(2)
        .any(|w| w[0].usd_per_hour > w[1].usd_per_hour + 1e-12)
    {
        bad.push(format!("{name}: host types are not cheapest first"));
    }
    bad
}

/// The host spec the suite creates: the cheapest type in its first region.
pub async fn host_spec(
    api: &dyn CloudApi,
    conn: &Connection,
    owner: &str,
) -> Option<ProvisionSpec> {
    let t = api.host_types(conn).await.ok()?.into_iter().next()?;
    let region = t
        .regions
        .first()
        .cloned()
        .unwrap_or_else(|| conn.region.clone());
    let expires = model::now() + 600;
    let mut tags = model::tags(owner, "host:conformance-1", "", expires);
    tags.insert(model::tag::ROLE.into(), ROLE_HOST.into());
    Some(ProvisionSpec {
        name: "cua-host-conformance-1".into(),
        family: "host".into(),
        kind: "vm".into(),
        machine_type: t.name.clone(),
        disk_gb: t.disk_gb.max(60),
        user_data: "#!/bin/bash\n# conformance: exact bytes \u{2713}\necho 'cua host'\n".into(),
        arch: t.arch.clone(),
        tags,
        ttl_secs: 600,
        role: ROLE_HOST.into(),
        region,
        ..Default::default()
    })
}

/// The host lifecycle (see the module docs). `seen` returns the create
/// calls the fake API received so far. Returns every failure.
pub async fn lifecycle_checks(
    api: &dyn CloudApi,
    conn: &Connection,
    owner: &str,
    seen: &dyn Fn() -> Vec<CreateCall>,
) -> Vec<String> {
    let mut bad = Vec::new();
    let name = api.name();
    let caps = api.capabilities(conn);
    let Some(spec) = host_spec(api, conn, owner).await else {
        return vec![format!("{name}: no host type to create")];
    };
    if caps.supplies == Supplies::Spaces {
        if api.provision(conn, &spec).await.is_ok() {
            bad.push(format!("{name}: supplies Spaces but created a host"));
        }
        return bad;
    }
    let before = seen().len();
    let made = match api.provision(conn, &spec).await {
        Ok(m) => m,
        Err(e) => return vec![format!("{name}: host create failed: {e}")],
    };
    let Some(host) = made.first().cloned() else {
        return vec![format!("{name}: the create returned nothing")];
    };
    if host.resource_type != "instance" {
        bad.push(format!(
            "{name}: the first resource is a {}, not the machine",
            host.resource_type
        ));
    }
    let calls = seen();
    match calls.get(before) {
        None => bad.push(format!("{name}: no create call reached the API")),
        Some(c) => {
            if c.machine_type != spec.machine_type {
                bad.push(format!(
                    "{name}: created {} instead of the asked {}",
                    c.machine_type, spec.machine_type
                ));
            }
            if !spec.region.is_empty() && c.region != spec.region {
                bad.push(format!(
                    "{name}: created in {} instead of {}",
                    c.region, spec.region
                ));
            }
            if c.user_data != spec.user_data {
                bad.push(format!("{name}: the user data was changed"));
            }
            // Every tag key in the create call itself (label values may be
            // mapped to what the cloud accepts; each provider's own tests
            // check its mapping).
            for k in spec.tags.keys() {
                if !c.tags.contains_key(k) {
                    bad.push(format!("{name}: tag {k} missing from the create call"));
                }
            }
            if c.ssh_key {
                bad.push(format!("{name}: an SSH key was attached"));
            }
        }
    }
    match api.list_owned(conn, owner).await {
        Ok(rows) => match rows.iter().find(|r| r.id == host.id) {
            Some(r) => {
                if r.tags.get(model::tag::ROLE).map(String::as_str) != Some(ROLE_HOST) {
                    bad.push(format!("{name}: listed without cua-role=host"));
                }
            }
            None => bad.push(format!("{name}: the host is not listed as this owner's")),
        },
        Err(e) => bad.push(format!("{name}: list_owned failed: {e}")),
    }
    if api
        .list_owned(conn, "someone-else")
        .await
        .is_ok_and(|rows| rows.iter().any(|r| r.id == host.id))
    {
        bad.push(format!("{name}: listed for another owner"));
    }
    if api.delete(conn, &host, "someone-else").await.is_ok() {
        bad.push(format!("{name}: delete for another owner was not refused"));
    }
    if let Err(e) = api.delete(conn, &host, owner).await {
        bad.push(format!("{name}: delete failed: {e}"));
    }
    if let Err(e) = api.delete(conn, &host, owner).await {
        bad.push(format!("{name}: deleting it again failed: {e}"));
    }
    bad
}
