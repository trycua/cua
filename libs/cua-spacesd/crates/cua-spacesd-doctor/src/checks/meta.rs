// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `meta`: the doctor reached cua-spacesd, which is the build the image
//! claims, healthy, and self-consistent.

use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::pb;

use crate::{Ctx, Recorder};

/// Source revision compiled into this build (`CUA_SPACESD_GIT_SHA` at build
/// time), empty when unknown.
pub fn build_git_sha(_ctx: &Ctx) -> String {
    option_env!("CUA_SPACESD_GIT_SHA").unwrap_or("").to_owned()
}

/// The git revision of the image's installed cua-spacesd (its `build-info`)
/// when this doctor is a different binary (run with `image-doctor-lane.sh
/// --doctor-bin`); else this doctor's own. The pin compares the image's
/// daemon, never the doctor's build.
pub async fn service_git_sha(ctx: &Ctx) -> String {
    let own = build_git_sha(ctx);
    // The image's binary (its manifest path), not `spacesd_exe`: the CLI
    // sets that to this doctor's own executable.
    let path = &ctx.manifest.manifest.spacesd.path;
    if path.is_empty() || !std::path::Path::new(path).is_file() {
        return own;
    }
    let exe = std::path::PathBuf::from(path);
    let same = std::env::current_exe()
        .ok()
        .and_then(|c| std::fs::canonicalize(c).ok())
        .is_some_and(|c| std::fs::canonicalize(&exe).is_ok_and(|e| e == c));
    if same {
        return own;
    }
    let exe_s = exe.to_string_lossy().into_owned();
    match crate::sys::run_local(
        &exe_s,
        &["build-info"],
        &[],
        std::time::Duration::from_secs(15),
    )
    .await
    {
        Ok(out) if out.ok() => serde_json::from_str::<serde_json::Value>(&out.stdout)
            .ok()
            .and_then(|v| v["git_sha"].as_str().map(str::to_owned))
            .filter(|s| !s.is_empty())
            .unwrap_or(own),
        _ => own,
    }
}

/// The cua-spacesd binary to probe.
pub fn spacesd_exe(ctx: &Ctx) -> Option<std::path::PathBuf> {
    ctx.options.spacesd_exe.clone().or_else(|| {
        let path = &ctx.manifest.manifest.spacesd.path;
        if !path.is_empty() && std::path::Path::new(path).is_file() {
            return Some(path.into());
        }
        std::env::current_exe().ok().filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("cua-spacesd"))
        })
    })
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    let caps = &ctx.caps;
    rec.push(
        Check::new(
            "meta.spacesd.reachable",
            Status::Pass,
            format!(
                "cua-spacesd {} (protocol {}.{}) answers GetCapabilities over {:?}",
                caps.version,
                caps.protocol_version,
                caps.protocol_revision,
                ctx.client.transport()
            ),
        )
        .fact("version", &caps.version)
        .fact("protocol_revision", caps.protocol_revision),
        &["core"],
    )
    .await;

    let want = cua_proto::ENV_PROTOCOL_REVISION;
    rec.push(
        Check::new(
            "meta.protocol_revision",
            super::verdict(caps.protocol_revision == want && caps.protocol_version == 1),
            format!(
                "server protocol {}.{}, doctor built for 1.{want}",
                caps.protocol_version, caps.protocol_revision
            ),
        )
        .fix("the doctor and the service come from different builds; reinstall cua-spacesd"),
        &["core"],
    )
    .await;

    manifest_check(ctx, rec).await;
    pin_check(ctx, rec).await;

    rec.run("meta.health", &["core"], Duration::from_secs(10), async {
        match ctx.client.health().await {
            Ok(health) => {
                let status = pb::HealthStatus::try_from(health.status).unwrap_or_default();
                let bad: Vec<String> = health
                    .components
                    .iter()
                    .filter(|c| c.status != pb::HealthStatus::Serving as i32)
                    .map(|c| format!("{}: {}", c.name, c.detail))
                    .collect();
                let mut check = Check::new(
                    "meta.health",
                    match status {
                        pb::HealthStatus::Serving => Status::Pass,
                        pb::HealthStatus::Degraded => Status::Warn,
                        _ => Status::Fail,
                    },
                    if bad.is_empty() {
                        format!("{} components serving", health.components.len())
                    } else {
                        format!("{status:?}: {}", bad.join("; "))
                    },
                )
                .fix("see the named components; the spacesd log has details");
                for c in &health.components {
                    check = check.fact(
                        format!("component.{}", c.name),
                        format!(
                            "{:?}",
                            pb::HealthStatus::try_from(c.status).unwrap_or_default()
                        ),
                    );
                }
                check
            }
            Err(error) => Check::new(
                "meta.health",
                Status::Fail,
                format!("Health failed: {error}"),
            ),
        }
    })
    .await;

    rec.run("meta.print_config", &["core"], Duration::from_secs(20), async {
        let Some(exe) = spacesd_exe(ctx) else {
            return Check::new(
                "meta.print_config",
                Status::Skip,
                "no cua-spacesd binary to probe (not running inside an image)",
            )
            .skip_reason("not_applicable");
        };
        let exe_s = exe.to_string_lossy().into_owned();
        match crate::sys::run_local(&exe_s, &["--print-config"], &[], Duration::from_secs(15)).await
        {
            Ok(out) if out.ok() => match serde_json::from_str::<serde_json::Value>(&out.stdout) {
                Ok(json) => {
                    let version = json["version"].as_str().unwrap_or_default();
                    let revision = json["protocol_revision"].as_u64().unwrap_or_default();
                    let leaked = ctx
                        .token
                        .as_ref()
                        .is_some_and(|t| !t.is_empty() && out.stdout.contains(t.as_str()));
                    *ctx.print_config.lock().unwrap() = Some(json.clone());
                    let ok = version == caps.version
                        && revision == caps.protocol_revision as u64
                        && !leaked;
                    Check::new(
                        "meta.print_config",
                        super::verdict(ok),
                        if leaked {
                            "--print-config printed the access token".to_owned()
                        } else {
                            format!(
                                "{exe_s} --print-config: version {version}, protocol revision {revision}, listen {}",
                                json["server"]["listen"].as_str().unwrap_or("?")
                            )
                        },
                    )
                    .fact("listen", json["server"]["listen"].as_str().unwrap_or_default())
                    .fact(
                        "token_source",
                        json["server"]["token_source"].as_str().unwrap_or_default(),
                    )
                    .fix("the installed binary differs from the running service; restart it")
                }
                Err(error) => Check::new(
                    "meta.print_config",
                    Status::Fail,
                    format!("--print-config output is not JSON: {error}"),
                ),
            },
            Ok(out) => Check::new(
                "meta.print_config",
                Status::Fail,
                format!(
                    "--print-config exited {:?}: {}",
                    out.code,
                    out.stderr.lines().last().unwrap_or_default()
                ),
            ),
            Err(error) => Check::new(
                "meta.print_config",
                Status::Fail,
                format!("running {exe_s}: {error}"),
            ),
        }
    })
    .await;
}

async fn manifest_check(ctx: &Ctx, rec: &mut Recorder<'_>) {
    let loaded = &ctx.manifest;
    let image_dir = std::path::Path::new(cua_spacesd_client::manifest::local_path())
        .parent()
        .unwrap_or(std::path::Path::new("/etc/cua-image"));
    let in_image = image_dir.is_dir();
    let check = if let Some(error) = &loaded.error {
        Check::new("meta.manifest", Status::Fail, error.clone())
            .fix("rebuild the image; libs/images/common/tools/cua-image-manifest writes it")
    } else if loaded.present() {
        Check::new(
            "meta.manifest",
            Status::Pass,
            format!(
                "{} ({} {}, {}) from {}",
                loaded.manifest.name,
                loaded.manifest.os,
                loaded.manifest.variant,
                loaded.manifest.arch,
                loaded.source
            ),
        )
        .fact("sha256", &loaded.sha256)
    } else if in_image {
        Check::new(
            "meta.manifest",
            Status::Fail,
            format!(
                "this is a cua image ({} exists) but it has no manifest.json",
                image_dir.display()
            ),
        )
        .fix("rebuild the image with libs/images/build.sh (it generates /etc/cua-image/manifest.json)")
    } else {
        Check::new(
            "meta.manifest",
            Status::Skip,
            "no image manifest; checks are informational",
        )
        .skip_reason("not_applicable")
    };
    let claims: &[&str] = if in_image || loaded.error.is_some() {
        &["core"]
    } else {
        &[]
    };
    rec.push(check, claims).await;
}

async fn pin_check(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !ctx.manifest.present() {
        return;
    }
    let pin = &ctx.manifest.manifest.spacesd;
    let caps = &ctx.caps;
    let overlay = super::build::daemon_overlay();
    let check = if let Some(o) = &overlay {
        super::build::overlaid_pin("meta.spacesd.pin", o)
    } else if !pin.present {
        Check::new(
            "meta.spacesd.pin",
            Status::Fail,
            "the manifest says this image has no cua-spacesd, but one answers",
        )
        .fix("the image was mislabelled at build time; rebuild it")
    } else {
        let mut problems = Vec::new();
        if pin.version != caps.version {
            problems.push(format!(
                "version {} (manifest {})",
                caps.version, pin.version
            ));
        }
        if pin.protocol_revision != 0 && pin.protocol_revision != caps.protocol_revision {
            problems.push(format!(
                "protocol revision {} (manifest {})",
                caps.protocol_revision, pin.protocol_revision
            ));
        }
        let sha = service_git_sha(ctx).await;
        if !pin.git_sha.is_empty() && !sha.is_empty() && pin.git_sha != sha {
            problems.push(format!("build {sha} (manifest {})", pin.git_sha));
        }
        if problems.is_empty() {
            Check::new(
                "meta.spacesd.pin",
                Status::Pass,
                format!("cua-spacesd {} matches the image manifest", caps.version),
            )
        } else {
            Check::new(
                "meta.spacesd.pin",
                Status::Fail,
                format!(
                    "running cua-spacesd differs from the one baked in: {}",
                    problems.join(", ")
                ),
            )
            .fix("a different binary is running than the image shipped; rebuild or restart")
        }
    };
    rec.push(
        check.fact("manifest_version", &pin.version),
        &["manifest:spacesd"],
    )
    .await;
}
