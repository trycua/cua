// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `compat`: the older names still work for one release: the
//! `cua-env-driver` and `cua-guestd` binary links and helper links, their
//! `.service` systemd aliases (resolved by systemd at runtime in VMs), and
//! the `CUA_ENV_*`, `CUA_GUESTD_*` and `CUA_SPACESD_*` environment spellings.
//! Driven by the manifest's `compat_links`, so the checks disappear with
//! the links.

use std::path::Path;
use std::time::Duration;

use cua_spacesd_client::diagnose::{Check, Status};

use crate::sys;
use crate::{Ctx, Recorder};

/// Whether `link` is a symlink resolving to `target` (relative to the
/// link's directory, or absolute).
pub fn link_ok(link: &Path, target: &str) -> Result<String, String> {
    let meta = std::fs::symlink_metadata(link).map_err(|e| format!("{}: {e}", link.display()))?;
    if !meta.file_type().is_symlink() {
        return Err(format!("{} is not a symlink", link.display()));
    }
    let want = link.parent().unwrap_or(Path::new("/")).join(target);
    let got = sys::resolve(link).ok_or_else(|| format!("{} is dangling", link.display()))?;
    let want = sys::resolve(&want).ok_or_else(|| format!("{} does not exist", want.display()))?;
    if got == want {
        Ok(got.display().to_string())
    } else {
        Err(format!(
            "{} resolves to {}, not {}",
            link.display(),
            got.display(),
            want.display()
        ))
    }
}

fn port_of(stdout: &str) -> Option<u16> {
    let json: serde_json::Value = serde_json::from_str(stdout).ok()?;
    json["server"]["listen"]
        .as_str()?
        .parse::<std::net::SocketAddr>()
        .ok()
        .map(|a| a.port())
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    if !rec.wants_group("compat") {
        return;
    }
    let links = ctx.manifest.manifest.compat_links.clone();
    let claims: &[&str] = &["manifest:compat_links"];
    for link in links.iter().filter(|l| !l.path.is_empty()) {
        let id = format!(
            "compat.link.{}",
            Path::new(&link.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        let check = match link_ok(Path::new(&link.path), &link.target) {
            Ok(resolved) => Check::new(&id, Status::Pass, format!("{} -> {resolved}", link.path)),
            Err(error) => Check::new(&id, Status::Fail, error)
                .fix("the image build must keep the older name's link for one release"),
        };
        rec.push(check, claims).await;
    }
    for link in links.iter().filter(|l| !l.unit_alias.is_empty()) {
        let id = format!("compat.unit.{}", link.unit_alias);
        if ctx.init == "systemd" {
            let (alias, unit) = (link.unit_alias.clone(), link.unit.clone());
            rec.run(&id.clone(), claims, Duration::from_secs(20), async move {
                let shown = sys::run_local(
                    "systemctl",
                    &["show", "-p", "Id", "--value", &alias],
                    &[],
                    Duration::from_secs(10),
                )
                .await;
                let active = sys::run_local(
                    "systemctl",
                    &["is-active", &alias],
                    &[],
                    Duration::from_secs(10),
                )
                .await;
                let shown = shown
                    .map(|o| o.stdout.trim().to_owned())
                    .unwrap_or_default();
                let active = active
                    .map(|o| o.stdout.trim().to_owned())
                    .unwrap_or_default();
                Check::new(
                    &id,
                    super::verdict(shown == unit && active == "active"),
                    format!("systemd resolves {alias} to {shown:?} ({active})"),
                )
                .fix("cua-spacesd.service must carry the Alias= of every older unit name and be enabled")
            })
            .await;
        } else {
            // No systemd at runtime (container variant): the unit alias file
            // must still point at the real unit for the VM layer.
            let file = Path::new("/usr/lib/systemd/system").join(&link.unit_alias);
            let check = match link_ok(&file, &link.unit) {
                Ok(resolved) => Check::new(
                    &id,
                    Status::Pass,
                    format!(
                        "{} -> {resolved} (not booted with systemd here)",
                        file.display()
                    ),
                ),
                Err(error) => Check::new(&id, Status::Fail, error),
            };
            rec.push(check, claims).await;
        }
    }

    if links.is_empty() || ctx.os() != "linux" {
        return;
    }
    let Some(exe) = super::meta::spacesd_exe(ctx) else {
        return;
    };
    let exe = exe.to_string_lossy().into_owned();
    rec.run("compat.env", claims, Duration::from_secs(30), async {
        // Each spelling alone must reach the config: the new one, the
        // interim CUA_GUESTD_* and the original CUA_ENV_*.
        let mut ports = Vec::new();
        for (set, port) in [
            ("CUA_SPACESD_PORT", 4999),
            ("CUA_GUESTD_PORT", 4997),
            ("CUA_ENV_PORT", 4998),
        ] {
            let mut args = Vec::new();
            for other in ["CUA_SPACESD_PORT", "CUA_GUESTD_PORT", "CUA_ENV_PORT"] {
                if other != set {
                    args.extend(["-u".to_owned(), other.to_owned()]);
                }
            }
            args.push(format!("{set}={port}"));
            args.extend([exe.clone(), "--print-config".to_owned()]);
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let got = sys::run_local("env", &args, &[], Duration::from_secs(10))
                .await
                .ok()
                .and_then(|o| port_of(&o.stdout));
            ports.push((set, port, got));
        }
        let ok = ports.iter().all(|(_, want, got)| *got == Some(*want));
        let detail = ports
            .iter()
            .map(|(set, want, got)| format!("{set}={want} -> {got:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        Check::new("compat.env", super::verdict(ok), detail)
            .fix("every spelling must be honoured for one release (promote_spacesd_env_vars)")
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn links_resolve_relative_to_their_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cua-spacesd"), "x").unwrap();
        std::os::unix::fs::symlink("cua-spacesd", dir.path().join("cua-env-driver")).unwrap();
        assert!(link_ok(&dir.path().join("cua-env-driver"), "cua-spacesd").is_ok());
        std::fs::write(dir.path().join("other"), "y").unwrap();
        assert!(link_ok(&dir.path().join("cua-env-driver"), "other")
            .unwrap_err()
            .contains("not"));
        assert!(link_ok(&dir.path().join("cua-spacesd"), "cua-spacesd")
            .unwrap_err()
            .contains("not a symlink"));
        std::os::unix::fs::symlink("gone", dir.path().join("dangling")).unwrap();
        assert!(link_ok(&dir.path().join("dangling"), "gone")
            .unwrap_err()
            .contains("dangling"));
    }

    #[test]
    fn reads_the_printed_port() {
        assert_eq!(
            port_of(r#"{"server":{"listen":"0.0.0.0:4999"}}"#),
            Some(4999)
        );
        assert_eq!(port_of("{}"), None);
    }
}
