//! `HostSpacesService.DiagnoseHost`: this machine's health as a place to
//! run Spaces, checked here in the daemon, the process that runs them.
//!
//! Every check is read-only and bounded (a few seconds in all). The ids are
//! `machine.*`; the Machines page shows the warnings and failures as the
//! machine's issues (the app core's `machines_view`), and `cua doctor
//! --machine` prints the report. Facts carry what a UI needs to word an
//! issue and offer its fix (`app`, `settings_url`, `free_gb`, ...).
//!
//! - `machine.local_network`: whether this process reaches this Mac's
//!   macOS VMs, and the app macOS holds responsible for it
//!   ([`cua_vmm::local_network`]), whose Local Network access decides it.
//! - `machine.disk`: free space where Spaces are written.
//! - `machine.virtualization`: whether this machine can run its Spaces at
//!   all (Lume on a Mac, a VM or container runtime elsewhere).
//! - `machine.memory`: memory the running macOS VMs leave for another.
//! - `machine.permissions.*`: what sharing this desktop needs (Screen
//!   Recording, Accessibility for cua-driver).

use std::time::{Duration, Instant, SystemTime};

use cua_proto::env::v1 as pb;
use cua_spacesd_client::diagnose::{Check, Report, Severity, Status};
use cua_vmm::local_network;

/// Below this much free space a Space can't be created.
pub const DISK_FAIL_GB: f64 = 10.0;
/// Below this a macOS Space (its image and disk) may not fit.
pub const DISK_WARN_GB: f64 = 30.0;
/// The memory a new macOS VM and macOS itself want left over.
pub const MEMORY_HEADROOM_GB: f64 = 8.0;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
const LOCAL_NETWORK_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork";
const STORAGE_URL: &str = "x-apple.systempreferences:com.apple.settings.Storage";

/// What the checks read, gathered by [`gather`] (so [`checks`] is pure).
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// `macos`, `linux`, `windows`.
    pub os: String,
    /// Host settings, when this machine is set up as a host.
    pub provide_spaces: bool,
    pub share_desktop: bool,
    /// Lume: this Mac can run it, and it is serving.
    pub lume_supported: bool,
    pub lume_serving: bool,
    /// Runtimes ready now, and why the others are not (`backend: detail`).
    pub runtimes_ready: Vec<String>,
    pub runtimes_missing: Vec<String>,
    /// The daemon's own Local Network probe (macOS).
    pub local_network: Option<local_network::Report>,
    /// A connect from the daemon to this Mac's router, for when no macOS VM
    /// runs to test with (`local_network`'s probe says nothing then).
    pub local_network_lan: Option<cua_create_failure::local_network::Access>,
    /// Where Spaces are written: (`name`, available bytes).
    pub disk: Option<(String, u64)>,
    /// Physical memory, and what running macOS VMs were given (bytes).
    pub memory_total: Option<u64>,
    pub memory_vms: u64,
    pub running_vms: usize,
    /// cua-driver's own (Screen Recording, Accessibility), when sharing the
    /// desktop and known.
    pub permissions: Option<(bool, bool)>,
}

fn gb(bytes: u64) -> f64 {
    bytes as f64 / GIB
}

/// A failure stays a failure (a check of severity `info` would report it
/// as a warning): this machine can't do what the check is about.
fn info(mut c: Check) -> Check {
    c.severity = if c.status == Status::Fail {
        Severity::Required
    } else {
        Severity::Info
    };
    c
}

/// The checks for `f` (pure).
pub fn checks(f: &Facts) -> Vec<Check> {
    let mut out = Vec::new();
    let mac = f.os == "macos";
    let runs_macos_vms = mac && (f.provide_spaces || f.lume_serving);

    // Local Network access (macOS VMs only).
    if runs_macos_vms && let Some(r) = &f.local_network {
        let app = r.app.as_ref();
        let name = app
            .filter(|a| a.bundle)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "the app that started the cua daemon".into());
        let mut c = match &r.probe {
            local_network::Probe::Blocked { vm, ip } => Check::new(
                "machine.local_network",
                Status::Fail,
                format!(
                    "Cua can't reach this Mac's macOS VMs ({vm} at {ip}, No route to host): \
                     Local Network access is off for {name}"
                ),
            )
            .fix(format!(
                "open System Settings > Privacy & Security > Local Network and turn on {name}"
            ))
            .fact("settings_url", LOCAL_NETWORK_URL),
            local_network::Probe::Reached { vm, .. } => Check::new(
                "machine.local_network",
                Status::Pass,
                format!("reached the macOS VM {vm}: Local Network access is on for {name}"),
            ),
            local_network::Probe::NoRunningVm => Check::new(
                "machine.local_network",
                Status::Skip,
                "no macOS VM is running to test Local Network access with",
            )
            .skip_reason("no_running_vm"),
            local_network::Probe::NoList { why } => Check::new(
                "machine.local_network",
                Status::Skip,
                format!("couldn't list this Mac's VMs ({why})"),
            )
            .skip_reason("no_vm_list"),
            local_network::Probe::Unclear { vm, why, .. } => Check::new(
                "machine.local_network",
                Status::Skip,
                format!("couldn't tell whether the macOS VM {vm} is reachable ({why})"),
            )
            .skip_reason("inconclusive"),
        };
        // No VM to test with: the daemon's connect to the router says the
        // same (a first connect also makes macOS ask, in this app's name).
        if c.status == Status::Skip {
            use cua_create_failure::local_network::Access;
            match f.local_network_lan {
                Some(Access::Blocked) => {
                    c = Check::new(
                        "machine.local_network",
                        Status::Fail,
                        format!(
                            "Cua can't reach this Mac's local network, so it can't reach its \
                             macOS VMs: Local Network access is off for {name}"
                        ),
                    )
                    .fix(format!(
                        "open System Settings > Privacy & Security > Local Network and turn on {name}"
                    ))
                    .fact("settings_url", LOCAL_NETWORK_URL);
                }
                Some(Access::Allowed) => {
                    c = Check::new(
                        "machine.local_network",
                        Status::Pass,
                        format!(
                            "reached this Mac's local network: Local Network access is on for {name}"
                        ),
                    );
                }
                _ => {}
            }
        }
        c = c.fact("app", &name);
        if let Some(a) = app {
            c = c.fact("app_path", a.path.display());
        }
        out.push(info(c));
    }

    // Free disk.
    if let Some((volume, available)) = &f.disk {
        let free = gb(*available);
        let (status, message) = if free < DISK_FAIL_GB {
            (
                Status::Fail,
                format!("only {free:.0} GB free on {volume}: new Spaces can't be created"),
            )
        } else if free < DISK_WARN_GB {
            (
                Status::Warn,
                format!("only {free:.0} GB free on {volume}; a macOS Space needs about 30 GB"),
            )
        } else {
            (Status::Pass, format!("{free:.0} GB free on {volume}"))
        };
        let mut c = Check::new("machine.disk", status, message)
            .fix("free up space, or run `cua cache prune`")
            .fact("free_gb", format!("{free:.0}"))
            .fact("volume", volume);
        if mac {
            c = c.fact("settings_url", STORAGE_URL);
        }
        out.push(info(c));
    }

    // Can this machine run Spaces at all.
    if mac && !f.lume_supported {
        out.push(info(Check::new(
            "machine.virtualization",
            if f.provide_spaces {
                Status::Fail
            } else {
                Status::Skip
            },
            "this Mac can't run macOS VMs (they need Apple silicon)",
        )));
    } else if f.provide_spaces && f.runtimes_ready.is_empty() {
        out.push(info(
            Check::new(
                "machine.virtualization",
                Status::Warn,
                format!(
                    "no Space runtime is ready on this machine yet ({})",
                    f.runtimes_missing.join("; ")
                ),
            )
            .fix("run `cua runtime setup`, or create a Space here to set one up"),
        ));
    } else if !f.runtimes_ready.is_empty() {
        out.push(info(Check::new(
            "machine.virtualization",
            Status::Pass,
            format!("ready: {}", f.runtimes_ready.join(", ")),
        )));
    }

    // Memory the running macOS VMs leave.
    if let Some(total) = f.memory_total
        && f.running_vms > 0
    {
        let left = gb(total.saturating_sub(f.memory_vms));
        let status = if left < MEMORY_HEADROOM_GB {
            Status::Warn
        } else {
            Status::Pass
        };
        out.push(info(
            Check::new(
                "machine.memory",
                status,
                format!(
                    "running Spaces use {:.0} of {:.0} GB of memory{}",
                    gb(f.memory_vms),
                    gb(total),
                    if status == Status::Warn {
                        "; another may not start"
                    } else {
                        ""
                    }
                ),
            )
            .fix("turn off a Space you're not using")
            .fact("used_gb", format!("{:.0}", gb(f.memory_vms)))
            .fact("total_gb", format!("{:.0}", gb(total))),
        ));
    }

    // What sharing the desktop needs.
    if mac
        && f.share_desktop
        && let Some((screen, access)) = f.permissions
    {
        for (id, title, granted, pane) in [
            (
                "screen-recording",
                "Screen Recording",
                screen,
                "Privacy_ScreenCapture",
            ),
            (
                "accessibility",
                "Accessibility",
                access,
                "Privacy_Accessibility",
            ),
        ] {
            out.push(info(
                Check::new(
                    format!("machine.permissions.{id}"),
                    if granted { Status::Pass } else { Status::Warn },
                    if granted {
                        format!("cua-driver has {title}")
                    } else {
                        format!("sharing this desktop needs {title} for cua-driver")
                    },
                )
                .fix(format!(
                    "open System Settings > Privacy & Security > {title} and turn on cua-driver"
                ))
                .fact("permission", title)
                .fact(
                    "settings_url",
                    format!("x-apple.systempreferences:com.apple.preference.security?{pane}"),
                ),
            ));
        }
    }
    out
}

/// Gathers [`Facts`] on this machine. Bounded: each probe has its own
/// limit, the slowest about 3.5 s.
pub async fn gather() -> Facts {
    let os = std::env::consts::OS.to_string();
    let doctor = crate::local::doctor_report().await;
    let mut f = Facts {
        os: os.clone(),
        lume_supported: doctor.lume.supported_host,
        lume_serving: doctor.lume.serving,
        ..Facts::default()
    };
    for b in &doctor.backends {
        if b.ready {
            f.runtimes_ready.push(b.backend.as_str().to_string());
        } else if !b.detail.is_empty() {
            f.runtimes_missing
                .push(format!("{}: {}", b.backend.as_str(), b.detail));
        }
    }
    if let Ok(Some(config)) = cua_host::Host::new(crate::cua_home()).config() {
        f.provide_spaces = config.provide_spaces;
        f.share_desktop = config.share_desktop;
        if os == "macos" && config.share_desktop {
            let driver = config.driver_bin.clone();
            f.permissions = tokio::task::spawn_blocking(move || {
                use cua_host::preflight::SessionProbe as _;
                cua_host::preflight::SystemProbe.permission_status(&driver)
            })
            .await
            .ok()
            .flatten();
        }
    }
    let storage = crate::storage::storage_report().await;
    f.disk = storage
        .lume
        .filter(|_| os == "macos")
        .or(storage.qemu)
        .or(storage.container)
        .map(|v| (v.name, v.available));
    if os == "macos" && (f.provide_spaces || f.lume_serving) {
        let base = local_network::lume_base();
        let report = local_network::report(&base).await;
        if !matches!(report.probe, local_network::Probe::Reached { .. }) {
            f.local_network_lan = Some(local_network::lan_access_now().await);
        }
        f.local_network = Some(report);
        let client = cua_vmm::lume::LumeClient::new(&base);
        if let Ok(Ok(vms)) = tokio::time::timeout(Duration::from_secs(2), client.list()).await {
            for vm in vms.iter().filter(|v| v.status == "running") {
                f.running_vms += 1;
                f.memory_vms += vm.memory_size;
            }
        }
        f.memory_total = physical_memory();
    }
    f
}

/// The RPC's answer.
pub async fn response() -> pb::DiagnoseHostResponse {
    let started = Instant::now();
    let facts = gather().await;
    let mut report = Report {
        schema_version: cua_spacesd_client::diagnose::SCHEMA_VERSION,
        producer: format!("cua daemon {}", crate::VERSION),
        checks: checks(&facts),
        ..Report::default()
    };
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    report.started_at = Some(pbjson_types::Timestamp {
        seconds: now.as_secs() as i64,
        nanos: 0,
    });
    report.finalize(false, started.elapsed());
    pb::DiagnoseHostResponse {
        report: Some(report.to_pb()),
        cua_version: crate::VERSION.to_string(),
        os: facts.os,
    }
}

#[cfg(target_os = "macos")]
fn physical_memory() -> Option<u64> {
    let mut size: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let name = c"hw.memsize";
    // SAFETY: `size` is a u64 and `len` its size; sysctlbyname writes at
    // most `len` bytes.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut size as *mut u64).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && size > 0).then_some(size)
}

#[cfg(not(target_os = "macos"))]
fn physical_memory() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mac() -> Facts {
        Facts {
            os: "macos".into(),
            provide_spaces: true,
            lume_supported: true,
            lume_serving: true,
            runtimes_ready: vec!["lume".into()],
            disk: Some(("Macintosh HD".into(), 200 * GIB as u64)),
            ..Facts::default()
        }
    }

    fn by_id<'a>(cs: &'a [Check], id: &str) -> &'a Check {
        cs.iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("{id}"))
    }

    fn not_ok(cs: &[Check]) -> Vec<&str> {
        cs.iter()
            .filter(|c| matches!(c.status, Status::Warn | Status::Fail))
            .map(|c| c.id.as_str())
            .collect()
    }

    #[test]
    fn a_healthy_mac_has_no_warnings() {
        let mut f = mac();
        f.local_network = Some(local_network::Report {
            probe: local_network::Probe::Reached {
                vm: "m".into(),
                ip: "192.168.64.2".into(),
            },
            app: None,
            pid: 1,
        });
        assert!(not_ok(&checks(&f)).is_empty(), "{:?}", checks(&f));
    }

    /// CUA-1807: the daemon is blocked; the check fails and names its app.
    #[test]
    fn a_blocked_daemon_fails_local_network_and_names_the_app() {
        let mut f = mac();
        f.local_network = Some(local_network::Report {
            probe: local_network::Probe::Blocked {
                vm: "m".into(),
                ip: "192.168.64.2".into(),
            },
            app: Some(local_network::ResponsibleApp {
                name: "Cua Spaces".into(),
                path: PathBuf::from("/Applications/Cua Spaces.app"),
                bundle: true,
            }),
            pid: 1,
        });
        let cs = checks(&f);
        let c = by_id(&cs, "machine.local_network");
        assert_eq!(c.status, Status::Fail);
        // Still a failure once the report is finalized.
        let mut report = Report {
            checks: cs.clone(),
            ..Report::default()
        };
        report.finalize(false, Duration::ZERO);
        let c = by_id(&report.checks, "machine.local_network");
        assert_eq!(c.status, Status::Fail);
        assert!(c.message.contains("off for Cua Spaces"), "{}", c.message);
        assert_eq!(c.facts["app"], "Cua Spaces");
        assert!(c.facts["settings_url"].contains("Privacy_LocalNetwork"));
    }

    /// With no macOS VM running, the daemon's own connect to the router
    /// decides the check: blocked fails it, allowed passes it.
    #[test]
    fn without_a_vm_the_daemons_lan_connect_decides_local_network() {
        use cua_create_failure::local_network::Access;
        let mut f = mac();
        f.local_network = Some(local_network::Report {
            probe: local_network::Probe::NoRunningVm,
            app: Some(local_network::ResponsibleApp {
                name: "Cua Spaces Local".into(),
                path: PathBuf::from("/Applications/Cua Spaces Local.app"),
                bundle: true,
            }),
            pid: 1,
        });
        f.local_network_lan = Some(Access::Blocked);
        let cs = checks(&f);
        let c = by_id(&cs, "machine.local_network");
        assert_eq!(c.status, Status::Fail);
        assert!(
            c.message.contains("off for Cua Spaces Local"),
            "{}",
            c.message
        );
        assert!(c.facts["settings_url"].contains("Privacy_LocalNetwork"));
        f.local_network_lan = Some(Access::Allowed);
        assert_eq!(
            by_id(&checks(&f), "machine.local_network").status,
            Status::Pass
        );
        f.local_network_lan = Some(Access::Unknown);
        assert_eq!(
            by_id(&checks(&f), "machine.local_network").status,
            Status::Skip
        );
    }

    #[test]
    fn low_disk_warns_then_fails() {
        let mut f = mac();
        f.disk = Some(("Macintosh HD".into(), 20 * GIB as u64));
        let cs = checks(&f);
        assert_eq!(by_id(&cs, "machine.disk").status, Status::Warn);
        assert_eq!(by_id(&cs, "machine.disk").facts["free_gb"], "20");
        f.disk = Some(("Macintosh HD".into(), 4 * GIB as u64));
        assert_eq!(by_id(&checks(&f), "machine.disk").status, Status::Fail);
    }

    #[test]
    fn running_vms_that_leave_little_memory_warn() {
        let mut f = mac();
        f.memory_total = Some(16 * GIB as u64);
        f.running_vms = 2;
        f.memory_vms = 12 * GIB as u64;
        let cs = checks(&f);
        let c = by_id(&cs, "machine.memory");
        assert_eq!(c.status, Status::Warn);
        assert!(c.message.contains("12 of 16 GB"), "{}", c.message);
        f.memory_vms = 4 * GIB as u64;
        assert_eq!(by_id(&checks(&f), "machine.memory").status, Status::Pass);
    }

    #[test]
    fn an_intel_mac_providing_spaces_fails_and_missing_permissions_warn() {
        let mut f = mac();
        f.lume_supported = false;
        assert_eq!(
            by_id(&checks(&f), "machine.virtualization").status,
            Status::Fail
        );
        let mut f = mac();
        f.share_desktop = true;
        f.permissions = Some((false, true));
        let cs = checks(&f);
        assert_eq!(not_ok(&cs), ["machine.permissions.screen-recording"]);
        assert!(
            by_id(&cs, "machine.permissions.screen-recording").facts["settings_url"]
                .contains("Privacy_ScreenCapture")
        );
    }

    #[test]
    fn a_machine_that_runs_no_spaces_is_not_asked_about_them() {
        let f = Facts {
            os: "macos".into(),
            lume_supported: true,
            disk: Some(("Macintosh HD".into(), 200 * GIB as u64)),
            ..Facts::default()
        };
        let cs = checks(&f);
        assert!(cs.iter().all(|c| c.id != "machine.local_network"));
        assert!(not_ok(&cs).is_empty());
    }
}
