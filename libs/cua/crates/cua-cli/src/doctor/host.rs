//! Host, image and cross checks of `cua doctor`.

use std::sync::Arc;
use std::time::Duration;

use cua_sdk::{Cua, RuntimeCheckStatus};
use cua_spacesd_client::diagnose::{Check, Report, Severity, Status};

/// This host's architecture, OCI spelling.
pub fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "amd64",
    }
}

/// The guest runtime (`GetCapabilities` spelling) a sandbox runtime type
/// maps to.
pub fn expected_runtime(runtime_type: &str) -> Option<&'static str> {
    Some(match runtime_type.to_ascii_lowercase().as_str() {
        "runc" | "container" | "docker" | "podman" => "container",
        "gvisor" | "runsc" => "gvisor",
        "qemu" | "vm" => "qemu",
        "kubevirt" => "kubevirt",
        "lume" => "lume",
        _ => return None,
    })
}

/// Which runtime-doctor backend a sandbox runtime depends on.
fn backend_for(runtime_type: &str) -> Option<&'static str> {
    Some(match expected_runtime(runtime_type)? {
        "container" | "gvisor" => "container",
        "qemu" => "qemu",
        "lume" => "lume",
        _ => return None,
    })
}

fn with_severity(mut check: Check, required: bool) -> Check {
    check.severity = if required {
        Severity::Required
    } else {
        Severity::Info
    };
    check
}

/// Host checks: the runtime doctor's backends, gVisor, acceleration,
/// emulation and image cache space. Required only for what the target
/// sandbox's runtime needs.
/// The user defaults (`cua config`): each effective value and where it
/// comes from, informational. A value that does not parse fails (every
/// create would); a cloud default without cloud credentials warns.
pub fn defaults_checks() -> Vec<Check> {
    use cua_sandbox_core::settings::Settings;
    let settings = match Settings::load() {
        Ok(s) => s,
        Err(e) => {
            return vec![with_severity(
                Check::new("host.defaults", Status::Fail, e.to_string())
                    .fix("fix or remove it: `cua config unset <key>`"),
                true,
            )];
        }
    };
    let mut checks = Vec::new();
    for e in settings.list() {
        let id = format!("host.defaults.{}", e.key.name.replace('.', "_"));
        let valid = cua_sandbox_core::settings::normalize(e.key, &e.value);
        let check = match valid {
            Ok(_) => Check::new(
                &id,
                Status::Pass,
                format!("{} = {} ({})", e.key.name, e.value, e.source),
            ),
            Err(err) => {
                Check::new(&id, Status::Fail, format!("{err} ({})", e.source)).fix(format!(
                    "`cua config set {} <value>` or unset {}",
                    e.key.name, e.key.env
                ))
            }
        };
        let failed = check.status == Status::Fail;
        checks.push(with_severity(check, failed));
    }
    if let Ok((cua_sandbox_core::placement::On::Cloud, source)) = settings.default_on()
        && !crate::auth::fleet_maybe_configured()
        && let Some(hint) = cua_sandbox_core::settings::cloud_default_hint(&source)
    {
        checks.push(with_severity(
            Check::new(
                "host.defaults.cloud_credentials",
                Status::Warn,
                "the default location is cloud, but no cloud credentials are configured",
            )
            .fix(hint),
            false,
        ));
    }
    checks
}

pub async fn host_checks(cua: &Arc<Cua>, runtime_type: Option<&str>, arch: &str) -> Vec<Check> {
    let mut checks = defaults_checks();
    let needed = runtime_type.and_then(backend_for);
    let report = match tokio::time::timeout(Duration::from_secs(60), cua.local().doctor()).await {
        Ok(Ok(report)) => report,
        Ok(Err(error)) => {
            checks.push(with_severity(
                Check::new(
                    "host.runtime",
                    Status::Fail,
                    format!("runtime doctor failed: {error}"),
                ),
                needed.is_some(),
            ));
            return checks;
        }
        Err(_) => {
            checks.push(with_severity(
                Check::new("host.runtime", Status::Fail, "runtime doctor timed out"),
                needed.is_some(),
            ));
            return checks;
        }
    };
    for c in &report.checks {
        let id = format!("host.runtime.{}", c.name);
        let required = needed == Some(c.name.as_str());
        // A backend the target does not use is informational: missing is
        // a skip, not a warning (warnings fail --strict).
        let (status, reason) = match (c.status, required) {
            (RuntimeCheckStatus::Ok, _) => (Status::Pass, None),
            (RuntimeCheckStatus::Installable, true) => (Status::Warn, None),
            (RuntimeCheckStatus::Error, true) => (Status::Fail, None),
            _ => (Status::Skip, Some("not_applicable")),
        };
        let mut check = Check::new(
            &id,
            status,
            format!(
                "{}{}",
                c.detail,
                c.version
                    .as_ref()
                    .map(|v| format!(" ({v})"))
                    .unwrap_or_default()
            ),
        )
        .fix("`cua runtime setup` provisions local runtimes");
        if let Some(reason) = reason {
            check = check.skip_reason(reason);
        }
        checks.push(with_severity(check, required));
    }
    let vmm: serde_json::Value = serde_json::from_str(&report.report_json).unwrap_or_default();
    let gvisor = vmm["container"]["gvisor"].as_bool().unwrap_or(false);
    let wants_gvisor = runtime_type.and_then(expected_runtime) == Some("gvisor");
    checks.push(with_severity(
        Check::new(
            "host.gvisor",
            if gvisor {
                Status::Pass
            } else if wants_gvisor {
                Status::Fail
            } else {
                Status::Skip
            },
            if gvisor {
                "runsc is registered with the container engine".to_owned()
            } else {
                format!(
                    "runsc is not registered ({})",
                    vmm["container"]["gvisor_provisioning"]
                        .as_str()
                        .unwrap_or("no provisioning path")
                )
            },
        ),
        wants_gvisor,
    ));
    let qemu_arch = if arch == "arm64" { "aarch64" } else { "x86_64" };
    let accel = vmm["host"]["accel"][qemu_arch]
        .as_str()
        .unwrap_or("")
        .to_owned();
    let wants_vm = runtime_type.and_then(expected_runtime) == Some("qemu");
    if !accel.is_empty() {
        checks.push(with_severity(
            Check::new(
                "host.accel",
                if accel == "tcg" { Status::Warn } else { Status::Pass },
                format!("QEMU would use {accel} for {arch} guests"),
            )
            .fix("TCG is emulation: VMs are 10x slower; use a host with KVM (Linux) or HVF (macOS, native arch)"),
            wants_vm,
        ));
    }
    if arch != host_arch() {
        let fatal = wants_gvisor;
        checks.push(with_severity(
            Check::new(
                "host.emulation",
                if fatal { Status::Fail } else { Status::Warn },
                format!(
                    "{arch} guests on this {} host run emulated{}",
                    host_arch(),
                    if fatal {
                        "; gVisor (runsc) cannot run emulated binaries"
                    } else {
                        " (slow)"
                    }
                ),
            ),
            fatal,
        ));
    }
    {
        // The cua home (`$CUA_HOME`, else `~/.cua`), or the nearest existing
        // parent when it does not exist yet.
        let path = cua_daemon::cua_home();
        let dir = std::iter::successors(Some(path.as_path()), |p| p.parent())
            .find(|p| p.is_dir())
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        if let Some(free) = free_bytes(&dir) {
            let gib = free as f64 / (1024.0 * 1024.0 * 1024.0);
            checks.push(with_severity(
                Check::new(
                    "host.disk",
                    if gib >= 10.0 {
                        Status::Pass
                    } else {
                        Status::Warn
                    },
                    format!(
                        "{gib:.1} GiB free under {} (images and disks live there)",
                        dir.display()
                    ),
                )
                .fix("free space or move the image cache; a VM disk alone is ~20 GiB"),
                false,
            ));
        }
    }
    checks.extend(host_sharing_checks());
    checks
}

/// This machine's `cua host` readiness: a GUI (Aqua) session for the
/// LaunchAgent `cua host setup` installs, and (when it shares its own
/// desktop) the macOS permissions cua-spacesd needs. Nothing here grants
/// or opens anything. Informational: `cua doctor` mainly diagnoses a
/// sandbox's runtime, not whether this machine can host Spaces, so a
/// failure here does not fail `--strict`. Only runs when this machine is
/// actually set up as a host (`cua host setup`); otherwise there is
/// nothing to check.
fn host_sharing_checks() -> Vec<Check> {
    use cua_host::preflight::{self, SessionProbe as _};
    if std::env::consts::OS != "macos" {
        return Vec::new();
    }
    let Ok(Some(config)) = cua_host::Host::new(cua_daemon::cua_home()).config() else {
        return Vec::new();
    };
    let probe = preflight::SystemProbe;
    let mut checks = Vec::new();
    let gui = probe.gui_session(preflight::current_uid());
    checks.push(with_severity(
        Check::new(
            "host.gui_session",
            if gui { Status::Pass } else { Status::Fail },
            if gui {
                "a GUI (Aqua) session is active; cua-spacesd's LaunchAgent can run".to_owned()
            } else {
                "no GUI (Aqua) session; cua-spacesd's LaunchAgent cannot start".to_owned()
            },
        )
        .fix(
            "log in at the console once, or turn on automatic login (System Settings > Users \
             & Groups > Login Options > Automatic login), then `cua host setup` again",
        ),
        false,
    ));
    if config.share_desktop
        && let Some((sr, ax)) = probe.permission_status(&config.driver_bin)
    {
        for hint in cua_host::permission_hints("macos", &config.driver_bin) {
            let granted = match hint.id.as_str() {
                "screen-recording" => sr,
                "accessibility" => ax,
                _ => continue,
            };
            checks.push(with_severity(
                Check::new(
                    format!("host.permissions.{}", hint.id),
                    if granted { Status::Pass } else { Status::Warn },
                    if granted {
                        format!("{} is granted", hint.title)
                    } else {
                        hint.instructions.clone()
                    },
                )
                .fix(format!("open {}", hint.settings_url)),
                false,
            ));
        }
    }
    checks
}

#[cfg(unix)]
fn free_bytes(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: zeroed out-struct, valid C string.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(not(unix))]
fn free_bytes(_path: &std::path::Path) -> Option<u64> {
    None
}

/// Image checks: resolve the reference, then compare the registry's facts
/// with the guest's report.
pub async fn image_checks(
    image: &str,
    arch: &str,
    runtime_type: Option<&str>,
    guest: &Report,
) -> Vec<Check> {
    let backend = runtime_type
        .and_then(|r| cua_image::Backend::parse(r).ok())
        .unwrap_or(cua_image::Backend::Auto);
    let resolved = match tokio::time::timeout(
        Duration::from_secs(60),
        cua_image::resolve(image, backend, arch),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(error)) => {
            return vec![with_severity(
                Check::new("image.resolve", Status::Fail, format!("{image}: {error}")),
                true,
            )];
        }
        Err(_) => {
            return vec![with_severity(
                Check::new(
                    "image.resolve",
                    Status::Fail,
                    format!("{image}: resolve timed out"),
                ),
                true,
            )];
        }
    };
    compare_image(&resolved, guest)
}

/// The comparisons behind [`image_checks`] (pure, for tests).
pub fn compare_image(resolved: &cua_image::ResolvedImage, guest: &Report) -> Vec<Check> {
    let mut checks = vec![with_severity(
        Check::new(
            "image.resolve",
            Status::Pass,
            format!(
                "{} -> {} ({} {}, {})",
                resolved.reference,
                resolved.pinned_ref,
                resolved.os,
                resolved.variant.as_str(),
                resolved.arch.clone().unwrap_or_default()
            ),
        )
        .fact("digest", &resolved.digest)
        .fact("variant", resolved.variant.as_str())
        .fact("spacesd_annotation", format!("{:?}", resolved.spacesd)),
        true,
    )];
    let answered = guest.spacesd.is_some();
    if let Some(claims) = resolved.spacesd {
        let ok = claims == answered;
        checks.push(with_severity(
            Check::new(
                "cross.spacesd",
                super::verdict(ok),
                format!(
                    "registry says ai.cua.spacesd={claims}; a cua-spacesd {}",
                    if answered { "answers" } else { "does not answer" }
                ),
            )
            .fix(if claims {
                "the image claims cua-spacesd but none answers: check the service (cua-spacesd doctor in the guest) or the label"
            } else {
                "the label is wrong: images must be labelled from what was built (cua-image-manifest check-labels)"
            }),
            true,
        ));
    }
    if !guest.image.variant.is_empty() {
        checks.push(with_severity(
            Check::new(
                "cross.variant",
                super::verdict(guest.image.variant == resolved.variant.as_str()),
                format!(
                    "registry variant {}, the guest's manifest says {}",
                    resolved.variant.as_str(),
                    guest.image.variant
                ),
            ),
            true,
        ));
    }
    if !guest.image.os.is_empty() && !resolved.os.is_empty() {
        checks.push(with_severity(
            Check::new(
                "cross.os",
                super::verdict(guest.image.os == resolved.os),
                format!("registry os {}, guest {}", resolved.os, guest.image.os),
            ),
            true,
        ));
    }
    checks
}

/// The sandbox's runtime against what the guest detects.
pub fn cross_runtime(runtime_type: &str, guest: &Report) -> Vec<Check> {
    let (Some(want), got) = (
        expected_runtime(runtime_type),
        guest.environment.runtime.as_str(),
    ) else {
        return Vec::new();
    };
    if got.is_empty() {
        return Vec::new();
    }
    vec![with_severity(
        Check::new(
            "cross.runtime",
            super::verdict(want == got),
            format!("sandbox runtime {runtime_type}, the guest detects {got}"),
        ),
        true,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_spacesd_client::diagnose::{Image, Spacesd};

    fn resolved(spacesd: Option<bool>) -> cua_image::ResolvedImage {
        cua_image::ResolvedImage {
            reference: "ghcr.io/trycua/linux:24.04".into(),
            variant_ref: "ghcr.io/trycua/linux:24.04".into(),
            pinned_ref: "ghcr.io/trycua/linux@sha256:ab".into(),
            digest: "sha256:ab".into(),
            variant: cua_image::Variant::Rootfs,
            arch: Some("arm64".into()),
            architectures: vec!["arm64".into()],
            emulated: false,
            os: "linux".into(),
            spacesd,
            layers: vec![],
        }
    }

    fn guest(answering: bool, variant: &str) -> Report {
        Report {
            spacesd: answering.then(|| Spacesd {
                version: "0.1.0".into(),
                ..Default::default()
            }),
            image: Image {
                variant: variant.into(),
                os: "linux".into(),
                ..Default::default()
            },
            ..Report::default()
        }
    }

    #[test]
    fn a_mislabelled_image_fails_the_cross_check() {
        let checks = compare_image(&resolved(Some(false)), &guest(true, "rootfs"));
        let c = checks.iter().find(|c| c.id == "cross.spacesd").unwrap();
        assert_eq!(c.status, Status::Fail);
        assert_eq!(c.severity, Severity::Required);
        let ok = compare_image(&resolved(Some(true)), &guest(true, "rootfs"));
        assert!(ok.iter().all(|c| c.status == Status::Pass), "{ok:?}");
        let wrong_variant = compare_image(&resolved(Some(true)), &guest(true, "containerdisk"));
        assert_eq!(
            wrong_variant
                .iter()
                .find(|c| c.id == "cross.variant")
                .unwrap()
                .status,
            Status::Fail
        );
    }

    #[test]
    fn runtimes_map_and_compare() {
        assert_eq!(expected_runtime("runsc"), Some("gvisor"));
        assert_eq!(expected_runtime("docker"), Some("container"));
        assert_eq!(expected_runtime("weird"), None);
        let mut g = guest(true, "rootfs");
        g.environment.runtime = "container".into();
        assert_eq!(cross_runtime("gvisor", &g)[0].status, Status::Fail);
        assert_eq!(cross_runtime("runc", &g)[0].status, Status::Pass);
        assert!(cross_runtime("weird", &g).is_empty());
    }
}
