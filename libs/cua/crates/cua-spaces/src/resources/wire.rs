//! `cua.daemon.v1` messages for [`Resources`]: the daemon serves them
//! (`SpaceService.GetResources`, `WatchResources`) and its clients read
//! them back, so both topologies hand the apps the same numbers.

use super::{
    ComputerResources, DiskLevel, DiskOnHost, DiskWarning, GuestUsage, Resources, ResourcesEvent,
    ResourcesUpdate, SharedBase, SpaceResources,
};
use cua_proto::daemon::v1 as pb;
use pb::watch_resources_response::Event;

impl From<&DiskWarning> for pb::DiskWarning {
    fn from(w: &DiskWarning) -> Self {
        pb::DiskWarning {
            level: w.level.as_str().into(),
            free: w.free,
            total: w.total,
            threshold: w.threshold,
        }
    }
}

impl From<pb::DiskWarning> for DiskWarning {
    fn from(w: pb::DiskWarning) -> Self {
        DiskWarning {
            level: DiskLevel::parse(&w.level),
            free: w.free,
            total: w.total,
            threshold: w.threshold,
        }
    }
}

impl From<&ComputerResources> for pb::ComputerResources {
    fn from(c: &ComputerResources) -> Self {
        pb::ComputerResources {
            id: c.id.clone(),
            name: c.name.clone(),
            online: c.online,
            sampled_at_ms: c.sampled_at_ms,
            cpu_count: c.cpu_count,
            cpu_percent: c.cpu_percent,
            load_average_1m: c.load_average_1m,
            memory_total: c.memory_total,
            memory_used: c.memory_used,
            disk_total: c.disk_total,
            disk_free: c.disk_free,
            spaces_count: c.spaces_count,
            spaces_running: c.spaces_running,
            spaces_cpu_allocated: c.spaces_cpu_allocated,
            spaces_memory_allocated: c.spaces_memory_allocated,
            spaces_memory_used: c.spaces_memory_used,
            spaces_disk_bytes: c.spaces_disk_bytes,
            spaces_disk_exact: c.spaces_disk_exact,
            warning: Some((&c.warning).into()),
        }
    }
}

impl From<pb::ComputerResources> for ComputerResources {
    fn from(c: pb::ComputerResources) -> Self {
        ComputerResources {
            id: c.id,
            name: c.name,
            online: c.online,
            sampled_at_ms: c.sampled_at_ms,
            cpu_count: c.cpu_count,
            cpu_percent: c.cpu_percent,
            load_average_1m: c.load_average_1m,
            memory_total: c.memory_total,
            memory_used: c.memory_used,
            disk_total: c.disk_total,
            disk_free: c.disk_free,
            spaces_count: c.spaces_count,
            spaces_running: c.spaces_running,
            spaces_cpu_allocated: c.spaces_cpu_allocated,
            spaces_memory_allocated: c.spaces_memory_allocated,
            spaces_memory_used: c.spaces_memory_used,
            spaces_disk_bytes: c.spaces_disk_bytes,
            spaces_disk_exact: c.spaces_disk_exact,
            warning: c.warning.map(Into::into).unwrap_or_default(),
        }
    }
}

impl From<&SpaceResources> for pb::SpaceResources {
    fn from(s: &SpaceResources) -> Self {
        pb::SpaceResources {
            id: s.id.clone(),
            power_state: s.power_state.clone(),
            cpu_allocated: s.cpu_allocated,
            memory_allocated: s.memory_allocated,
            disk_allocated: s.disk_allocated,
            usage: s.usage.map(|u| pb::GuestUsage {
                cpu_count: u.cpu_count,
                cpu_percent: u.cpu_percent,
                load_average_1m: u.load_average_1m,
                memory_used: u.memory_used,
                memory_total: u.memory_total,
                memory_limited: u.memory_limited,
                disk_used: u.disk_used,
                // 0 on the wire: no size of its own.
                disk_total: u.disk_total.unwrap_or(0),
                disk_limited: u.disk_limited,
            }),
            disk_on_host: s.disk_on_host.map(|d| pb::DiskOnHost {
                bytes: d.bytes,
                allocated: d.allocated,
                shares_space: d.shares_space,
            }),
            base: s.base.as_ref().map(|b| pb::SharedBase {
                key: b.key.clone(),
                image: b.image.clone(),
                bytes: b.bytes,
                shared_with: b.shared_with,
            }),
        }
    }
}

impl From<pb::SpaceResources> for SpaceResources {
    fn from(s: pb::SpaceResources) -> Self {
        SpaceResources {
            id: s.id,
            power_state: s.power_state,
            cpu_allocated: s.cpu_allocated,
            memory_allocated: s.memory_allocated,
            disk_allocated: s.disk_allocated,
            // An older daemon sends a guest's "unlimited" as a number:
            // `known_size` turns it into no size.
            usage: s.usage.map(|u| {
                let disk_total = super::known_size(u.disk_total);
                GuestUsage {
                    cpu_count: u.cpu_count,
                    cpu_percent: u.cpu_percent,
                    load_average_1m: u.load_average_1m,
                    memory_used: u.memory_used,
                    memory_total: u.memory_total,
                    memory_limited: u.memory_limited,
                    disk_used: if u.disk_used >= super::SIZE_SENTINEL_FLOOR {
                        0
                    } else {
                        u.disk_used
                    },
                    disk_total,
                    disk_limited: u.disk_limited && disk_total.is_some(),
                }
            }),
            disk_on_host: s.disk_on_host.map(|d| DiskOnHost {
                bytes: d.bytes,
                allocated: d.allocated,
                shares_space: d.shares_space,
            }),
            base: s.base.map(|b| SharedBase {
                key: b.key,
                image: b.image,
                bytes: b.bytes,
                shared_with: b.shared_with,
            }),
        }
    }
}

impl From<&Resources> for pb::ResourcesSnapshot {
    fn from(r: &Resources) -> Self {
        pb::ResourcesSnapshot {
            sequence: r.sequence,
            sampled_at_ms: r.sampled_at_ms,
            this_mac: Some((&r.this_mac).into()),
            spaces: r.spaces.iter().map(Into::into).collect(),
            computers: r.computers.iter().map(Into::into).collect(),
            warning: Some((&r.warning).into()),
        }
    }
}

impl From<pb::ResourcesSnapshot> for Resources {
    fn from(r: pb::ResourcesSnapshot) -> Self {
        Resources {
            sequence: r.sequence,
            sampled_at_ms: r.sampled_at_ms,
            this_mac: r.this_mac.map(Into::into).unwrap_or_default(),
            spaces: r.spaces.into_iter().map(Into::into).collect(),
            computers: r.computers.into_iter().map(Into::into).collect(),
            warning: r.warning.map(Into::into).unwrap_or_default(),
        }
    }
}

impl From<&ResourcesUpdate> for pb::ResourcesUpdate {
    fn from(u: &ResourcesUpdate) -> Self {
        pb::ResourcesUpdate {
            sequence: u.sequence,
            sampled_at_ms: u.sampled_at_ms,
            this_mac: u.this_mac.as_ref().map(Into::into),
            spaces: u.spaces.iter().map(Into::into).collect(),
            computers: u.computers.iter().map(Into::into).collect(),
            removed: u.removed.clone(),
        }
    }
}

impl From<pb::ResourcesUpdate> for ResourcesUpdate {
    fn from(u: pb::ResourcesUpdate) -> Self {
        ResourcesUpdate {
            sequence: u.sequence,
            sampled_at_ms: u.sampled_at_ms,
            this_mac: u.this_mac.map(Into::into),
            spaces: u.spaces.into_iter().map(Into::into).collect(),
            computers: u.computers.into_iter().map(Into::into).collect(),
            removed: u.removed,
        }
    }
}

impl From<&ResourcesEvent> for pb::WatchResourcesResponse {
    fn from(e: &ResourcesEvent) -> Self {
        pb::WatchResourcesResponse {
            event: Some(match e {
                ResourcesEvent::Snapshot(s) => Event::Snapshot(s.into()),
                ResourcesEvent::Update(u) => Event::Update(u.into()),
                ResourcesEvent::LowDisk(w) => Event::LowDisk(w.into()),
            }),
        }
    }
}

impl TryFrom<pb::WatchResourcesResponse> for ResourcesEvent {
    type Error = &'static str;

    fn try_from(r: pb::WatchResourcesResponse) -> Result<Self, Self::Error> {
        match r.event {
            Some(Event::Snapshot(s)) => Ok(ResourcesEvent::Snapshot(s.into())),
            Some(Event::Update(u)) => Ok(ResourcesEvent::Update(u.into())),
            Some(Event::LowDisk(w)) => Ok(ResourcesEvent::LowDisk(w.into())),
            None => Err("an empty resources event"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{GIB, THIS_MAC};

    fn sample() -> Resources {
        let warning = DiskWarning::assess(40 * GIB, 1024 * GIB, 5 * GIB, DiskLevel::Ok);
        Resources {
            sequence: 9,
            sampled_at_ms: 1_760_000_000_000,
            this_mac: ComputerResources {
                id: THIS_MAC.into(),
                online: true,
                cpu_count: 12,
                cpu_percent: 38.5,
                load_average_1m: 2.5,
                memory_total: 64 * GIB,
                memory_used: 41 * GIB,
                disk_total: 1024 * GIB,
                disk_free: 40 * GIB,
                spaces_count: 2,
                spaces_running: 1,
                spaces_cpu_allocated: 4,
                spaces_memory_allocated: 8 * GIB,
                spaces_memory_used: 3 * GIB,
                spaces_disk_bytes: 25 * GIB,
                spaces_disk_exact: false,
                warning,
                ..Default::default()
            },
            spaces: vec![
                SpaceResources {
                    id: "local:a".into(),
                    power_state: "running".into(),
                    cpu_allocated: 4,
                    memory_allocated: 8 * GIB,
                    disk_allocated: 80 * GIB,
                    usage: Some(GuestUsage {
                        cpu_count: 4,
                        cpu_percent: 22.0,
                        load_average_1m: 0.5,
                        memory_used: 3 * GIB,
                        memory_total: 8 * GIB,
                        memory_limited: true,
                        disk_used: 20 * GIB,
                        disk_total: Some(80 * GIB),
                        disk_limited: true,
                    }),
                    disk_on_host: Some(DiskOnHost {
                        bytes: Some(6 * GIB),
                        allocated: 31 * GIB,
                        shares_space: true,
                    }),
                    base: Some(SharedBase {
                        key: "sha256:abc".into(),
                        image: "ghcr.io/trycua/macos:26".into(),
                        bytes: Some(25 * GIB),
                        shared_with: 2,
                    }),
                },
                SpaceResources {
                    id: "local:b".into(),
                    power_state: "stopped".into(),
                    disk_on_host: Some(DiskOnHost {
                        bytes: None,
                        allocated: 12 * GIB,
                        shares_space: true,
                    }),
                    ..Default::default()
                },
            ],
            computers: vec![ComputerResources {
                id: "m-1".into(),
                name: "Mac mini".into(),
                online: true,
                ..Default::default()
            }],
            warning,
        }
    }

    #[test]
    fn a_snapshot_survives_the_wire() {
        let r = sample();
        let back: Resources = pb::ResourcesSnapshot::from(&r).into();
        assert_eq!(back, r);
    }

    #[test]
    fn an_unmeasured_disk_stays_unmeasured_on_the_wire() {
        let r = sample();
        let wire = pb::ResourcesSnapshot::from(&r);
        assert_eq!(wire.spaces[1].disk_on_host.as_ref().unwrap().bytes, None);
        assert_eq!(
            wire.spaces[0].disk_on_host.as_ref().unwrap().bytes,
            Some(6 * GIB)
        );
        assert!(wire.spaces[1].usage.is_none());
    }

    #[test]
    fn every_event_survives_the_wire() {
        let r = sample();
        let update = ResourcesUpdate {
            sequence: 10,
            sampled_at_ms: 5,
            this_mac: Some(r.this_mac.clone()),
            spaces: vec![r.spaces[0].clone()],
            computers: vec![],
            removed: vec!["local:gone".into()],
        };
        for e in [
            ResourcesEvent::Snapshot(r.clone()),
            ResourcesEvent::Update(update),
            ResourcesEvent::LowDisk(r.warning),
        ] {
            let back = ResourcesEvent::try_from(pb::WatchResourcesResponse::from(&e)).unwrap();
            assert_eq!(back, e);
        }
        assert!(ResourcesEvent::try_from(pb::WatchResourcesResponse { event: None }).is_err());
    }
}
