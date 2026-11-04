// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `resources`: enough disk (and inodes) where tasks write, a usable
//! `/dev/shm` (browsers), memory and CPUs, via `SystemService.Metrics` and
//! `statvfs`.

use std::path::Path;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};
use cua_spacesd_client::pb;

use crate::sys;
use crate::{Ctx, Recorder};

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("resources") {
        return;
    }
    let floors = ctx.manifest.manifest.resources.clone();
    let metrics = tokio::time::timeout(
        Duration::from_secs(10),
        ctx.client.system().metrics(pb::MetricsRequest {
            disk_path: "/".into(),
        }),
    )
    .await;
    match metrics {
        Ok(Ok(m)) => {
            let m = m.into_inner();
            let mem_mib = m.memory_total_bytes / (1024 * 1024);
            {
                let mut fidelity = ctx.fidelity.lock().await;
                fidelity.cpu_count = m.cpu_count;
                fidelity.memory_mib = mem_mib;
            }
            rec.push(
                Check::new(
                    "resources.memory",
                    super::verdict(mem_mib >= floors.min_memory_mib),
                    format!(
                        "{mem_mib} MiB total, {} MiB used (floor {} MiB)",
                        m.memory_used_bytes / (1024 * 1024),
                        floors.min_memory_mib
                    ),
                )
                .fact("memory_mib", mem_mib),
                &["manifest:resources"],
            )
            .await;
            rec.push(
                Check::new(
                    "resources.cpus",
                    super::verdict(m.cpu_count >= 1),
                    format!(
                        "{} logical CPUs, {:.0}% busy, load {:.2}",
                        m.cpu_count, m.cpu_percent, m.load_average_1m
                    ),
                )
                .fact("cpu_count", m.cpu_count),
                &["core"],
            )
            .await;
        }
        Ok(Err(status)) => {
            rec.push(
                Check::new(
                    "resources.memory",
                    Status::Fail,
                    format!("Metrics: {}", status.message()),
                ),
                &["core"],
            )
            .await;
        }
        Err(_) => {
            rec.push(
                Check::new("resources.memory", Status::Fail, "Metrics timed out"),
                &["core"],
            )
            .await;
        }
    }

    if !cfg!(unix) {
        return;
    }
    let home = if ctx.manifest.manifest.display.user.is_empty() {
        None
    } else {
        Some(format!("/home/{}", ctx.manifest.manifest.display.user))
    };
    let mut paths = vec![
        "/".to_owned(),
        std::env::temp_dir().to_string_lossy().into_owned(),
    ];
    paths.extend(home.filter(|h| Path::new(h).is_dir()));
    paths.dedup();
    for path in paths {
        let id = format!(
            "resources.disk.{}",
            if path == "/" {
                "root".to_owned()
            } else {
                path.trim_matches('/').replace('/', "_")
            }
        );
        let Some(stat) = sys::statvfs(Path::new(&path)) else {
            rec.push(
                Check::new(&id, Status::Fail, format!("statvfs {path} failed")),
                &["manifest:resources"],
            )
            .await;
            continue;
        };
        let free = stat.available as f64 / GIB;
        let inodes_ok = stat.files == 0 || stat.files_free * 100 / stat.files.max(1) >= 5;
        rec.push(
            Check::new(
                &id,
                super::verdict(free >= floors.min_disk_gib && inodes_ok),
                format!(
                    "{path}: {free:.1} GiB free of {:.1} GiB, {} inodes free (floor {:.1} GiB)",
                    stat.total as f64 / GIB,
                    stat.files_free,
                    floors.min_disk_gib
                ),
            )
            .fact("free_gib", format!("{free:.2}")),
            &["manifest:resources"],
        )
        .await;
    }
    if Path::new("/dev/shm").is_dir() {
        let shm = sys::statvfs(Path::new("/dev/shm"))
            .map(|s| s.total / (1024 * 1024))
            .unwrap_or(0);
        rec.push(
            Check::new(
                "resources.shm",
                // A small /dev/shm breaks browsers, but it is set by whoever
                // runs the container (--shm-size): a warning, not the image's fault.
                if shm >= floors.min_shm_mib {
                    Status::Pass
                } else {
                    Status::Warn
                },
                format!("/dev/shm {shm} MiB (floor {} MiB)", floors.min_shm_mib),
            )
            .fix("run the container with --shm-size=512m (or more)")
            .fact("shm_mib", shm),
            &[],
        )
        .await;
    }
}
