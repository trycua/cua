//! How `cua spaces ls` and `cua host status` show processor, memory and
//! disk: this Mac's numbers, each extra Space's, and your other computers'.
//! The words come from [`cua_spaces::resources::words`] (the Spaces apps use
//! the same ones); `--json` carries exact bytes.

use cua_proto::daemon::v1 as pb;
use cua_spaces::resources::{
    ComputerResources, DiskLevel, Measure, Resources, SpaceResources, words,
};
use cua_spaces::{SpaceInfo, Spaces};
use serde_json::{Value, json};
use std::time::Duration;

/// How long a command waits for the daemon's numbers before it measures
/// them itself.
const DAEMON_WAIT: Duration = Duration::from_secs(3);

/// The numbers a command shows. A running daemon answers from its shared
/// sampler (live guest numbers, nothing measured twice); without one this
/// reads allocations and disks itself and asks no guest anything, so the
/// command stays fast. `disk` false skips the disk measurement
/// (`--no-size`). Your other computers always come from `spaces`' machine
/// list (read it first with [`Spaces::relay_machines`]).
pub async fn fetch(spaces: &Spaces, disk: bool) -> Resources {
    let mut r = match from_daemon().await {
        Some(r) => r,
        None => {
            spaces
                .resources_now(Measure {
                    disk,
                    guests: false,
                })
                .await
        }
    };
    if !disk {
        for s in &mut r.spaces {
            s.disk_on_host = None;
        }
    }
    r.computers = spaces.other_computers();
    r
}

async fn from_daemon() -> Option<Resources> {
    cua_daemon::client::live_address()?;
    tokio::time::timeout(DAEMON_WAIT, async {
        let client = cua_daemon::client::existing_daemon().await.ok()?;
        let reply = client
            .spaces()
            .get_resources(pb::GetResourcesRequest { visible: false })
            .await
            .ok()?;
        reply.into_inner().snapshot.map(Resources::from)
    })
    .await
    .ok()
    .flatten()
}

/// The three extra cells of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cells {
    /// CPU.
    pub cpu: String,
    /// MEMORY.
    pub memory: String,
    /// DISK ON THIS MAC.
    pub disk: String,
}

impl Cells {
    fn none() -> Self {
        Cells {
            cpu: "-".into(),
            memory: "-".into(),
            disk: "-".into(),
        }
    }

    fn of_space(s: &SpaceResources) -> Self {
        Cells {
            cpu: words::space_cpu(s),
            memory: words::space_memory(s),
            disk: words::space_disk(s),
        }
    }

    fn of_computer(c: &ComputerResources) -> Self {
        Cells {
            cpu: words::computer_cpu(c),
            memory: words::computer_memory(c),
            disk: words::computer_disk(c),
        }
    }
}

/// Column widths: the row's id column is the list's own.
pub const CPU_WIDTH: usize = 22;
/// Width of the MEMORY column.
pub const MEMORY_WIDTH: usize = 15;

/// The header of the table the extra columns make.
pub fn header(id_width: usize) -> String {
    format!(
        "{:<id_width$} {:<24} {:<8} {:<9} {:<CPU_WIDTH$} {:<MEMORY_WIDTH$} DISK ON THIS MAC",
        "ID", "NAME", "PROVIDER", "VERSION", "CPU", "MEMORY"
    )
}

/// What `cua spaces ls` shows next to each Space.
pub struct Columns<'a> {
    /// The numbers.
    pub resources: &'a Resources,
    /// This install's own relay machine id, so its row shows this Mac.
    pub own_machine: Option<String>,
}

impl Columns<'_> {
    /// The cells of `info`'s row: a local Space's, a relay machine's from
    /// its published summary (this Mac's own row from this Mac), else
    /// dashes.
    pub fn cells(&self, info: &SpaceInfo) -> Cells {
        // The column is this Mac's disk: a Space on another computer (its
        // size there is in `--json`) shows dashes.
        if let Some(s) = self.resources.spaces.iter().find(|s| s.id == info.id)
            && info.host.is_empty()
        {
            return Cells::of_space(s);
        }
        let Some(machine) = info.id.strip_prefix("relay:") else {
            return Cells::none();
        };
        if self.own_machine.as_deref() == Some(machine) {
            return Cells::of_computer(&self.resources.this_mac);
        }
        self.resources
            .computers
            .iter()
            .find(|c| c.id == machine)
            .map(Cells::of_computer)
            .unwrap_or_else(Cells::none)
    }

    /// `row` with this Space's numbers: the JSON the Space already has plus
    /// `resources` (a local Space's [`SpaceResources`], a relay machine's
    /// [`ComputerResources`], else null), in the camelCase the apps read.
    pub fn annotate(&self, info: &SpaceInfo, row: &mut Value) {
        let Some(o) = row.as_object_mut() else { return };
        let machine = info.id.strip_prefix("relay:");
        let value = if let Some(s) = self.resources.spaces.iter().find(|s| s.id == info.id) {
            serde_json::to_value(s).ok()
        } else if machine.is_some() && machine == self.own_machine.as_deref() {
            serde_json::to_value(&self.resources.this_mac).ok()
        } else {
            machine
                .and_then(|m| self.resources.computers.iter().find(|c| c.id == m))
                .and_then(|c| serde_json::to_value(c).ok())
        };
        o.insert("resources".into(), value.unwrap_or(Value::Null));
    }

    /// The warning sentence when this Mac is low on disk.
    pub fn warning(&self) -> Option<&'static str> {
        words::warning(&self.resources.warning)
    }
}

/// The "This Mac" block of `cua host status`.
pub fn this_mac_lines(r: &Resources) -> Vec<String> {
    let m = &r.this_mac;
    let mut out = vec!["This Mac".to_string()];
    if m.cpu_count > 0 {
        out.push(format!(
            "  Processor: {}",
            words::processor(m.cpu_count, Some(m.cpu_percent))
        ));
    }
    if m.memory_total > 0 {
        out.push(format!(
            "  Memory: {}",
            words::memory_in_use(m.memory_used, m.memory_total)
        ));
    }
    if m.disk_total > 0 {
        out.push(format!(
            "  Space for files: {}",
            words::disk_free(m.disk_free, m.disk_total)
        ));
    }
    if let Some(extra) = words::extra_spaces_use(m) {
        out.push(format!("  {extra}"));
    }
    if let Some(w) = words::warning(&m.warning) {
        out.push(format!("  {w}"));
    }
    out
}

/// `cua host status --json`: the status plus `resources`.
pub fn status_json(status: Value, r: &Resources) -> Value {
    let mut status = status;
    if let Some(o) = status.as_object_mut() {
        o.insert(
            "resources".into(),
            serde_json::to_value(r).unwrap_or(Value::Null),
        );
    }
    status
}

/// `cua spaces ls --json` fields next to the Spaces: this Mac and its
/// warning.
pub fn ls_extras(r: &Resources) -> Value {
    json!({
        "thisMac": r.this_mac,
        "warning": r.warning,
        "warningText": words::warning(&r.warning),
        "lowDisk": r.warning.level != DiskLevel::Ok,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_spaces::resources::{DiskOnHost, DiskWarning, GuestUsage, THIS_MAC};

    const GIB: u64 = 1 << 30;
    const TB: u64 = 1024 * GIB;

    fn info(id: &str) -> SpaceInfo {
        SpaceInfo {
            id: id.into(),
            name: id.into(),
            display_name: String::new(),
            provider: if id.starts_with("relay:") {
                cua_spaces::Provider::Relay
            } else {
                cua_spaces::Provider::Local
            },
            spacesd_version: "0.10.0".into(),
            features: vec![],
            os: String::new(),
            os_name: String::new(),
            os_pretty_name: String::new(),
            image: String::new(),
            image_digest: String::new(),
            kind: String::new(),
            arch: String::new(),
            services: vec![],
            added_at: None,
            host: String::new(),
            host_name: String::new(),
            power: String::new(),
            power_state: String::new(),
            cloud: String::new(),
            cloud_place: String::new(),
            cloud_delete: String::new(),
            status: String::new(),
            stop_reason: String::new(),
            durable: String::new(),
            labels: Default::default(),
            expires_at_unix: 0,
            on_expire: String::new(),
            created_at_unix: 0,
            egress: Default::default(),
            egress_enforced: None,
            sharing: None,
            volumes: Vec::new(),
        }
    }

    fn resources() -> Resources {
        let warning = DiskWarning::assess(300 * GIB, TB, 5 * GIB, DiskLevel::Ok);
        Resources {
            sequence: 1,
            sampled_at_ms: 1,
            this_mac: ComputerResources {
                id: THIS_MAC.into(),
                online: true,
                cpu_count: 12,
                cpu_percent: 38.0,
                memory_total: 64 * GIB,
                memory_used: 41 * GIB,
                disk_total: TB,
                disk_free: 300 * GIB,
                spaces_count: 1,
                spaces_running: 1,
                spaces_cpu_allocated: 4,
                spaces_memory_allocated: 8 * GIB,
                spaces_disk_bytes: 31 * GIB,
                spaces_disk_exact: true,
                warning,
                ..Default::default()
            },
            spaces: vec![SpaceResources {
                id: "local:aurora".into(),
                power_state: "running".into(),
                cpu_allocated: 4,
                memory_allocated: 8 * GIB,
                disk_allocated: 80 * GIB,
                usage: Some(GuestUsage {
                    cpu_count: 4,
                    cpu_percent: 22.0,
                    memory_used: 3 * GIB + GIB / 10,
                    memory_total: 8 * GIB,
                    memory_limited: true,
                    ..Default::default()
                }),
                disk_on_host: Some(DiskOnHost {
                    bytes: Some(31 * GIB),
                    allocated: 87 * GIB,
                    shares_space: true,
                }),
            }],
            computers: vec![ComputerResources {
                id: "mini1234".into(),
                name: "Mac mini".into(),
                online: true,
                cpu_count: 10,
                cpu_percent: 5.0,
                memory_total: 32 * GIB,
                memory_used: 12 * GIB,
                disk_total: 512 * GIB,
                disk_free: 300 * GIB,
                ..Default::default()
            }],
            warning,
        }
    }

    #[test]
    fn each_row_gets_its_own_numbers() {
        let r = resources();
        let c = Columns {
            resources: &r,
            own_machine: Some("studio99".into()),
        };
        let local = c.cells(&info("local:aurora"));
        assert_eq!(local.cpu, "4 cores \u{b7} 22% busy");
        assert_eq!(local.memory, "3.1 of 8 GB");
        assert_eq!(local.disk, "about 31 GB, shares space with other copies");
        // Another computer: what it last reported.
        let mini = c.cells(&info("relay:mini1234"));
        assert_eq!(mini.cpu, "10 cores \u{b7} 5% busy");
        assert_eq!(mini.memory, "12 of 32 GB");
        assert_eq!(mini.disk, "300 GB free of 512 GB");
        // This install's own relay row is this Mac.
        let own = c.cells(&info("relay:studio99"));
        assert_eq!(own.cpu, "12 cores \u{b7} 38% busy");
        assert_eq!(own.disk, "300 GB free of 1 TB");
        // A machine that published nothing, and a Space added by address.
        assert_eq!(c.cells(&info("relay:nobody")), Cells::none());
        assert_eq!(c.cells(&info("direct:10.0.0.5:3211")), Cells::none());
        // One of yours on another computer: its size there is not this
        // Mac's disk (the JSON carries it).
        let mut r = r.clone();
        r.spaces.push(SpaceResources {
            id: "relay:space-qa-1234".into(),
            disk_on_host: Some(DiskOnHost {
                bytes: Some(19 * GIB),
                allocated: 19 * GIB,
                shares_space: false,
            }),
            ..Default::default()
        });
        let c = Columns {
            resources: &r,
            own_machine: None,
        };
        let mut remote = info("relay:space-qa-1234");
        remote.host = "mini1234".into();
        assert_eq!(c.cells(&remote), Cells::none());
        let mut row = json!({});
        c.annotate(&remote, &mut row);
        assert_eq!(row["resources"]["diskOnHost"]["bytes"], 19 * GIB);
    }

    #[test]
    fn the_header_lines_up_with_the_rows() {
        let h = header(48);
        let r = resources();
        let c = Columns {
            resources: &r,
            own_machine: None,
        };
        let lines = crate::host::grouped_lines_for_tests(&[info("local:aurora")], Some(&c));
        // Every column starts where its header does.
        let row = &lines[1];
        for column in [
            "NAME",
            "PROVIDER",
            "VERSION",
            "CPU",
            "MEMORY",
            "DISK ON THIS MAC",
        ] {
            let at = h.find(column).unwrap();
            assert!(at > 0);
            assert_eq!(row.as_bytes()[at - 1], b' ', "{column}: {h}\n{row}");
        }
        assert!(row.contains("4 cores"), "{row}");
        assert_eq!(lines[0], h);
    }

    #[test]
    fn json_rows_carry_the_numbers_in_camel_case() {
        let r = resources();
        let c = Columns {
            resources: &r,
            own_machine: None,
        };
        let mut row = json!({"id": "local:aurora"});
        c.annotate(&info("local:aurora"), &mut row);
        assert_eq!(row["resources"]["cpuAllocated"], 4);
        assert_eq!(row["resources"]["usage"]["cpuPercent"], 22.0);
        assert_eq!(row["resources"]["diskOnHost"]["bytes"], 31 * GIB);
        let mut row = json!({"id": "relay:mini1234"});
        c.annotate(&info("relay:mini1234"), &mut row);
        assert_eq!(row["resources"]["name"], "Mac mini");
        let mut row = json!({"id": "direct:1"});
        c.annotate(&info("direct:1"), &mut row);
        assert_eq!(row["resources"], Value::Null);
        let extras = ls_extras(&r);
        assert_eq!(extras["thisMac"]["cpuCount"], 12);
        assert_eq!(extras["warning"]["level"], "ok");
        assert_eq!(extras["lowDisk"], false);
    }

    #[test]
    fn this_mac_in_words() {
        let r = resources();
        assert_eq!(
            this_mac_lines(&r),
            [
                "This Mac",
                "  Processor: 12 cores \u{b7} 38% busy",
                "  Memory: 41 of 64 GB in use",
                "  Space for files: 300 GB free of 1 TB",
                "  Your extra Spaces use: 4 cores, 8 GB of memory, about 31 GB for files.",
            ]
        );
        let mut low = resources();
        low.this_mac.warning = DiskWarning::assess(10 * GIB, TB, 5 * GIB, DiskLevel::Ok);
        let lines = this_mac_lines(&low);
        assert_eq!(
            lines.last().unwrap(),
            "  This Mac is almost out of space. Delete old Spaces or snapshots."
        );
        let c = Columns {
            resources: &low,
            own_machine: None,
        };
        assert_eq!(c.warning(), None, "uses the snapshot's own warning");
    }

    #[test]
    fn status_json_gains_resources() {
        let v = status_json(json!({"configured": true}), &resources());
        assert_eq!(v["configured"], true);
        assert_eq!(v["resources"]["thisMac"]["memoryTotal"], 64 * GIB);
        assert_eq!(v["resources"]["spaces"][0]["id"], "local:aurora");
    }
}
