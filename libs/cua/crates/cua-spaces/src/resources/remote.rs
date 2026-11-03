//! Your Spaces on your other computers: what each takes on its computer's
//! disk, for the Spaces page's sizes and the totals of a bulk delete.
//!
//! Nothing is measured here. The computer that runs a Space measures it (its
//! own sampler, at most every [`super::DISK_MEASURE_EVERY`]) and says so when
//! asked (`GetHostSpacesRequest.disk`, through the relay or its direct
//! address, the way a delete reaches it). This device asks each of those
//! computers at most every [`REMOTE_DISK_EVERY`], and only while somebody
//! reads the numbers; the sampler never waits for the answers, which join
//! the next sample. A computer that is offline, or did not answer the last
//! time, gives its Spaces no entry (the page shows nothing for them).

use super::{DiskOnHost, REMOTE_DISK_EVERY, SpaceResources};
use crate::{SpaceId, SpaceInfo, Spaces};
use cua_proto::env::v1 as pb;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A computer that did not answer is asked again after this long.
const RETRY_AFTER: Duration = Duration::from_secs(15);
/// How long one computer gets to answer.
const ASK_TIMEOUT: Duration = Duration::from_secs(20);

/// What a computer said about one Space it runs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HostDisk {
    /// The relay machine the Space is reached as (empty for a direct one).
    pub relay_machine: String,
    /// Its id on that computer (`local:<name>`).
    pub local_space: String,
    /// The port that computer forwards to it (0: none).
    pub direct_port: u32,
    /// What it takes there.
    pub disk: DiskOnHost,
}

/// The measured Spaces of a `GetHostSpaces` answer.
pub(crate) fn host_disks(spaces: &[pb::HostSpace]) -> Vec<HostDisk> {
    spaces
        .iter()
        .filter_map(|s| {
            let d = s.disk.as_ref()?;
            Some(HostDisk {
                relay_machine: s.relay_machine.clone(),
                local_space: s.local_space.clone(),
                direct_port: s.direct_port,
                disk: DiskOnHost {
                    bytes: d.bytes,
                    allocated: d.allocated,
                    shares_space: d.shares_space,
                },
            })
        })
        .collect()
}

/// Whether Space `id`, as this device reaches it, is `d` on its computer.
/// `on_host` is its id there when this device recorded it (a Space a host
/// added by its address created).
pub(crate) fn is_space(id: &str, on_host: Option<&str>, d: &HostDisk) -> bool {
    if let Some(local) = on_host {
        return !d.local_space.is_empty() && local == d.local_space;
    }
    match SpaceId::parse(id) {
        Ok(SpaceId::Relay { machine_id }) => {
            !d.relay_machine.is_empty() && machine_id == d.relay_machine
        }
        // A Space reached at the port its host forwards.
        Ok(SpaceId::Direct { authority }) => {
            d.direct_port != 0
                && authority
                    .rsplit_once(':')
                    .and_then(|(_, p)| p.parse::<u32>().ok())
                    == Some(d.direct_port)
        }
        _ => false,
    }
}

/// One of your Spaces on another computer, as this device lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RemoteSpace {
    /// Its id here.
    pub id: String,
    /// The computer that runs it ([`SpaceInfo::host`]).
    pub host: String,
    /// Its id on that computer, when this device recorded it.
    pub on_host: Option<String>,
    /// `running`, `stopped`, `suspended`, or empty.
    pub power_state: String,
}

/// An entry for each Space in `spaces` that its computer's answer measured,
/// keyed by this device's id, with only its disk (and power state) known.
/// A Space whose computer has no answer in `answers` gets none.
pub(crate) fn merge(
    spaces: &[RemoteSpace],
    answers: &HashMap<String, Vec<HostDisk>>,
) -> Vec<SpaceResources> {
    spaces
        .iter()
        .filter_map(|s| {
            let disk = answers
                .get(&s.host)?
                .iter()
                .find(|d| is_space(&s.id, s.on_host.as_deref(), d))?
                .disk;
            Some(SpaceResources {
                id: s.id.clone(),
                power_state: s.power_state.clone(),
                disk_on_host: Some(disk),
                ..Default::default()
            })
        })
        .collect()
}

#[derive(Default)]
struct Asked {
    /// When the last ask started.
    at: Option<Instant>,
    /// An ask is running.
    busy: bool,
    /// The last answer; `None` when the computer did not answer.
    disks: Option<Vec<HostDisk>>,
}

/// What each computer last answered, by [`SpaceInfo::host`].
#[derive(Default)]
pub(crate) struct RemoteDisks {
    hosts: Mutex<HashMap<String, Asked>>,
}

impl RemoteDisks {
    /// The computers among `hosts` to ask now, marked as being asked:
    /// never asked, last asked [`REMOTE_DISK_EVERY`] ago, or
    /// [`RETRY_AFTER`] ago when they did not answer. Forgets the others.
    pub(crate) fn due(&self, hosts: &BTreeSet<String>, now: Instant) -> Vec<String> {
        let mut all = self.hosts.lock().expect("remote disks");
        all.retain(|h, _| hosts.contains(h));
        let mut due = Vec::new();
        for h in hosts {
            let a = all.entry(h.clone()).or_default();
            let every = if a.disks.is_some() {
                REMOTE_DISK_EVERY
            } else {
                RETRY_AFTER
            };
            if !a.busy && a.at.is_none_or(|at| now.duration_since(at) >= every) {
                a.busy = true;
                a.at = Some(now);
                due.push(h.clone());
            }
        }
        due
    }

    /// `host` answered (`None`: it did not).
    pub(crate) fn answered(&self, host: &str, disks: Option<Vec<HostDisk>>) {
        let mut all = self.hosts.lock().expect("remote disks");
        let a = all.entry(host.to_string()).or_default();
        a.busy = false;
        a.disks = disks;
    }

    /// The last answer of each computer that answered.
    pub(crate) fn known(&self) -> HashMap<String, Vec<HostDisk>> {
        self.hosts
            .lock()
            .expect("remote disks")
            .iter()
            .filter_map(|(h, a)| Some((h.clone(), a.disks.clone()?)))
            .collect()
    }
}

impl Spaces {
    /// Your Spaces on your other computers, with what each takes there:
    /// the last answers. Computers that are due are asked (in the
    /// background, or at once with `wait`, for a command that samples
    /// once). An offline computer's Spaces are left out.
    pub(crate) async fn remote_space_disks(&self, wait: bool) -> Vec<SpaceResources> {
        let spaces: Vec<RemoteSpace> = self
            .list()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| !i.host.is_empty())
            .filter(|i| self.host_online(&i.host) != Some(false))
            .map(|i| self.remote_space(i))
            .collect();
        let hosts: BTreeSet<String> = spaces.iter().map(|s| s.host.clone()).collect();
        let hub = self.inner.resources.clone();
        let due = hub.remote.due(&hosts, Instant::now());
        if wait {
            futures_util::future::join_all(due.iter().map(|h| self.ask_host_disks(h))).await;
        } else if !due.is_empty() {
            let weak = std::sync::Arc::downgrade(&self.inner);
            tokio::spawn(async move {
                let Some(inner) = weak.upgrade() else { return };
                let spaces = Spaces { inner };
                futures_util::future::join_all(due.iter().map(|h| spaces.ask_host_disks(h))).await;
                // The answers join the next sample: take it now.
                spaces.inner.resources.wake();
            });
        }
        merge(&spaces, &hub.remote.known())
    }

    fn remote_space(&self, i: SpaceInfo) -> RemoteSpace {
        let on_host = self
            .registry()
            .direct_host_of(&i.id)
            .ok()
            .flatten()
            .map(|(_, local)| local);
        RemoteSpace {
            id: i.id,
            host: i.host,
            on_host,
            power_state: i.power_state,
        }
    }

    /// Whether the relay says computer `host` is online: false too when it
    /// is not among your machines (the host of a Space shared with you);
    /// `None` for a computer added by its address (asking says).
    fn host_online(&self, host: &str) -> Option<bool> {
        let machine = host.strip_prefix("relay:").unwrap_or(host);
        if machine.contains(':') {
            return None;
        }
        Some(
            self.inner
                .relay_cache
                .lock()
                .expect("relay cache")
                .iter()
                .any(|m| m.id == machine && m.online),
        )
    }

    async fn ask_host_disks(&self, host: &str) {
        let answer = tokio::time::timeout(ASK_TIMEOUT, self.host_space_disks(host)).await;
        let disks = match answer {
            Ok(Ok(spaces)) => Some(host_disks(&spaces)),
            Ok(Err(e)) => {
                tracing::debug!(host, "Space sizes not read: {e}");
                None
            }
            Err(_) => {
                tracing::debug!(
                    host,
                    "Space sizes not read: no answer within {ASK_TIMEOUT:?}"
                );
                None
            }
        };
        self.inner.resources.remote.answered(host, disks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    fn disk(bytes: Option<u64>) -> DiskOnHost {
        DiskOnHost {
            bytes,
            allocated: bytes.unwrap_or(30 * GB),
            shares_space: bytes.is_none(),
        }
    }

    fn hs(relay_machine: &str, local: &str, port: u32, d: Option<DiskOnHost>) -> pb::HostSpace {
        pb::HostSpace {
            relay_machine: relay_machine.into(),
            local_space: local.into(),
            direct_port: port,
            disk: d.map(|d| pb::HostSpaceDisk {
                bytes: d.bytes,
                allocated: d.allocated,
                shares_space: d.shares_space,
            }),
            ..Default::default()
        }
    }

    fn space(id: &str, host: &str, on_host: Option<&str>) -> RemoteSpace {
        RemoteSpace {
            id: id.into(),
            host: host.into(),
            on_host: on_host.map(Into::into),
            power_state: "stopped".into(),
        }
    }

    #[test]
    fn only_measured_spaces_come_back_from_an_answer() {
        let d = host_disks(&[
            hs("space-qa-1234", "local:qa", 0, Some(disk(Some(12 * GB)))),
            hs("space-ci-5678", "local:ci", 0, None),
        ]);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].local_space, "local:qa");
        assert_eq!(d[0].disk.bytes, Some(12 * GB));
    }

    #[test]
    fn spaces_are_matched_by_how_this_device_reaches_them() {
        let relayed = HostDisk {
            relay_machine: "space-qa-1234".into(),
            local_space: "local:qa".into(),
            direct_port: 0,
            disk: disk(Some(GB)),
        };
        assert!(is_space("relay:space-qa-1234", None, &relayed));
        assert!(!is_space("relay:space-other-99", None, &relayed));
        assert!(!is_space("local:qa", None, &relayed));
        // A Space a host reached by its address made: the id it recorded
        // there wins, then the port it forwards.
        let direct = HostDisk {
            relay_machine: String::new(),
            local_space: "local:web".into(),
            direct_port: 40007,
            disk: disk(Some(GB)),
        };
        assert!(is_space(
            "direct:10.0.0.2:40007",
            Some("local:web"),
            &direct
        ));
        assert!(!is_space(
            "direct:10.0.0.2:40007",
            Some("local:api"),
            &direct
        ));
        assert!(is_space("direct:m-4f2a9c.via.cua:40007", None, &direct));
        assert!(!is_space("direct:m-4f2a9c.via.cua:40008", None, &direct));
        // No relay machine and no port: nothing to match by.
        assert!(!is_space("relay:space-qa-1234", None, &direct));
    }

    #[test]
    fn the_merge_keys_each_size_by_this_devices_id_and_skips_silent_computers() {
        let answers = HashMap::from([
            (
                "mac-mini".to_string(),
                host_disks(&[
                    hs("space-qa-1234", "local:qa", 0, Some(disk(Some(19 * GB)))),
                    hs("space-big-5678", "local:big", 0, Some(disk(None))),
                ]),
            ),
            (
                "direct:10.0.0.2:3211".to_string(),
                host_disks(&[hs("", "local:web", 40007, Some(disk(Some(4 * GB))))]),
            ),
        ]);
        let spaces = [
            space("relay:space-qa-1234", "mac-mini", None),
            space("relay:space-big-5678", "mac-mini", None),
            // Its computer measured nothing for it (a container).
            space("relay:space-tiny-0000", "mac-mini", None),
            space(
                "direct:10.0.0.2:40007",
                "direct:10.0.0.2:3211",
                Some("local:web"),
            ),
            // Its computer did not answer (offline, or older than this).
            space("relay:space-far-1111", "linux-box", None),
        ];
        let got = merge(&spaces, &answers);
        let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "relay:space-qa-1234",
                "relay:space-big-5678",
                "direct:10.0.0.2:40007"
            ]
        );
        assert_eq!(got[0].disk_on_host.unwrap().bytes, Some(19 * GB));
        assert_eq!(got[0].power_state, "stopped");
        // Only its disk is known.
        assert_eq!((got[0].cpu_allocated, got[0].memory_allocated), (0, 0));
        assert!(got[0].usage.is_none());
        let big = got[1].disk_on_host.unwrap();
        assert_eq!(
            (big.bytes, big.allocated, big.shares_space),
            (None, 30 * GB, true)
        );
        assert_eq!(got[2].disk_on_host.unwrap().bytes, Some(4 * GB));
    }

    #[test]
    fn each_computer_is_asked_at_most_once_a_minute_and_retried_sooner_after_a_miss() {
        let r = RemoteDisks::default();
        let t0 = Instant::now();
        let hosts = BTreeSet::from(["a".to_string(), "b".to_string()]);
        assert_eq!(r.due(&hosts, t0), ["a", "b"]);
        // Still asking: not again.
        assert!(r.due(&hosts, t0 + Duration::from_secs(90)).is_empty());
        r.answered("a", Some(vec![]));
        r.answered("b", None);
        assert_eq!(r.known().len(), 1);
        assert!(r.due(&hosts, t0 + Duration::from_secs(5)).is_empty());
        assert_eq!(r.due(&hosts, t0 + RETRY_AFTER), ["b"]);
        assert_eq!(r.due(&hosts, t0 + REMOTE_DISK_EVERY), ["a"]);
        // A computer that runs none of your Spaces any more is forgotten.
        r.answered("a", Some(vec![]));
        r.due(&BTreeSet::from(["b".to_string()]), t0 + REMOTE_DISK_EVERY);
        assert!(!r.known().contains_key("a"));
    }
}
