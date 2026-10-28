//! The Spaces this computer is creating, published through its relay
//! machine record next to its [`summary`](super::summary), so your other
//! computers show "Creating macOS on Mac mini… 40%" while it happens
//! (CUA-2349).
//!
//! One `meta` key, [`META_KEY`], holds compact JSON of at most
//! [`MAX_LEN`] characters (the relay's value limit):
//! `{"v":1,"t":<unix s>,"s":[[name,os,phase,percent],...]}`, at most
//! [`MAX_SHOWN`] Spaces, names cut to [`NAME_CHARS`]. No ids, paths, images
//! or tokens. The key is cleared (an empty value) when nothing is being
//! created.
//!
//! The daemon publishes at once when a create starts, when its phase
//! changes, when it finishes or fails, and when its percent moved by
//! [`PERCENT_STEP`] or [`PERCENT_EVERY`] passed; and again every
//! [`HEARTBEAT`] while one runs. A reader drops a value older than
//! [`STALE_AFTER`]: a computer that stopped mid-create (its daemon crashed)
//! does not show a Space being created forever.
//!
//! The relay shows a machine's `meta` to everyone who can see the machine,
//! so people a computer is shared with see its creates too.

use super::summary::MAX_LEN;
use cua_sandbox_core::progress::{Phase, Progress};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// The relay machine meta key.
pub const META_KEY: &str = "cua.new";
/// The format.
pub const VERSION: u32 = 1;
/// At most this many Spaces are published (the rest are left out).
pub const MAX_SHOWN: usize = 4;
/// A name longer than this is cut.
pub const NAME_CHARS: usize = 32;
/// A published value older than this is ignored.
pub const STALE_AFTER: Duration = Duration::from_secs(120);
/// While a create runs, published again at least this often.
pub const HEARTBEAT: Duration = Duration::from_secs(60);
/// A percent change this big publishes at once ...
pub const PERCENT_STEP: u8 = 10;
/// ... any other once this long passed since the last publish.
pub const PERCENT_EVERY: Duration = Duration::from_secs(5);

/// One Space being created, as published.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Creating {
    /// Its name.
    pub name: String,
    /// `macos`, `linux`, `windows`, or empty when unknown.
    pub os: String,
    /// The create's phase ([`Phase::as_str`]).
    pub phase: String,
    /// How far along the whole create is, 0 to 100.
    pub percent: u8,
}

/// A Space another of your computers is creating.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatingElsewhere {
    /// The computer's relay machine id.
    pub machine_id: String,
    /// The computer's name.
    pub machine_name: String,
    /// The Space's name.
    pub name: String,
    /// `macos`, `linux`, `windows`, or empty when unknown.
    pub os: String,
    /// The create's phase ([`Phase::as_str`]).
    pub phase: String,
    /// How far along the whole create is, 0 to 100.
    pub percent: u8,
    /// When the computer published it, unix ms.
    pub updated_at_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    v: u32,
    /// Published at, unix seconds.
    t: u64,
    /// `[name, os, phase, percent]` each.
    s: Vec<(String, String, String, u8)>,
}

/// `creates` (at `at_secs`) as the value of [`META_KEY`]: empty when there
/// are none (which clears the key). At most [`MAX_SHOWN`], as many as fit
/// in [`MAX_LEN`].
pub fn encode(creates: &[Creating], at_secs: u64) -> String {
    if creates.is_empty() {
        return String::new();
    }
    let mut wire = Wire {
        v: VERSION,
        t: at_secs,
        s: Vec::new(),
    };
    let mut out = String::new();
    for c in creates.iter().take(MAX_SHOWN) {
        let name: String = c
            .name
            .chars()
            .filter(|ch| !ch.is_control())
            .take(NAME_CHARS)
            .collect();
        let os: String =
            c.os.chars()
                .filter(char::is_ascii_alphanumeric)
                .take(12)
                .collect();
        let phase: String = c
            .phase
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .take(24)
            .collect();
        wire.s.push((name, os, phase, c.percent.min(100)));
        let next = serde_json::to_string(&wire).expect("strings and numbers serialize");
        if next.chars().count() > MAX_LEN {
            wire.s.pop();
            break;
        }
        out = next;
    }
    out
}

/// The Spaces the computer `machine_id` named `machine_name` published as
/// being created, or none for text that is not a value of a version this
/// build knows, or one older than [`STALE_AFTER`] at `now_ms`.
pub fn decode(
    machine_id: &str,
    machine_name: &str,
    value: &str,
    now_ms: u64,
) -> Vec<CreatingElsewhere> {
    let Ok(w) = serde_json::from_str::<Wire>(value) else {
        return Vec::new();
    };
    let at_ms = w.t.saturating_mul(1000);
    if w.v != VERSION || now_ms.saturating_sub(at_ms) > STALE_AFTER.as_millis() as u64 {
        return Vec::new();
    }
    w.s.into_iter()
        .take(MAX_SHOWN)
        .filter(|(name, ..)| !name.is_empty())
        .map(|(name, os, phase, percent)| CreatingElsewhere {
            machine_id: machine_id.into(),
            machine_name: machine_name.into(),
            name,
            os,
            phase,
            percent: percent.min(100),
            updated_at_ms: at_ms,
        })
        .collect()
}

/// How far along a whole create is at `p`, 0 to 99 (100 is ready): each
/// phase has a band, and a download moves through its own by how much is
/// done.
pub fn percent(p: &Progress) -> u8 {
    let (from, to) = match p.phase {
        Phase::Preparing => (0, 5),
        Phase::Pulling => (5, 60),
        Phase::Creating => (60, 70),
        Phase::Booting => (70, 85),
        Phase::WaitingForServices => (85, 95),
        Phase::Connecting => (95, 99),
        Phase::Ready => return 100,
    };
    let fraction = p.fraction.or_else(|| {
        p.bytes
            .filter(|b| b.total > 0)
            .map(|b| b.done as f64 / b.total as f64)
    });
    let within = fraction.unwrap_or(0.0).clamp(0.0, 1.0);
    (f64::from(from) + f64::from(to - from) * within).floor() as u8
}

struct Entry {
    creating: Creating,
    /// The percent last published, and when.
    published: Option<(u8, Instant)>,
}

/// The creates running on this computer (one per runtime), and the signal
/// that wakes the publisher.
#[derive(Default)]
pub(crate) struct Board {
    entries: Mutex<BTreeMap<String, Entry>>,
    wake: Notify,
}

impl Board {
    /// A create of `name` (`os`) started under the key `id`: published at once.
    pub(crate) fn start(&self, id: &str, name: &str, os: &str) {
        let creating = Creating {
            name: name.into(),
            os: os.into(),
            phase: Phase::Preparing.as_str().into(),
            percent: 0,
        };
        self.entries.lock().expect("creates").insert(
            id.into(),
            Entry {
                creating,
                published: None,
            },
        );
        self.wake.notify_one();
    }

    /// The create `id` reported `p`. Wakes the publisher when the phase
    /// changed, or the percent moved by [`PERCENT_STEP`], or by anything
    /// once [`PERCENT_EVERY`] passed since the last publish.
    pub(crate) fn report(&self, id: &str, p: &Progress) {
        if p.phase == Phase::Ready {
            return;
        }
        let mut entries = self.entries.lock().expect("creates");
        let Some(e) = entries.get_mut(id) else { return };
        let phase = p.phase.as_str();
        let pct = percent(p).max(if e.creating.phase == phase {
            e.creating.percent
        } else {
            0
        });
        let phase_changed = e.creating.phase != phase;
        e.creating.phase = phase.into();
        e.creating.percent = pct;
        let due = phase_changed
            || match e.published {
                None => true,
                Some((last, at)) => {
                    pct >= last.saturating_add(PERCENT_STEP)
                        || (pct != last && at.elapsed() >= PERCENT_EVERY)
                }
            };
        drop(entries);
        if due {
            self.wake.notify_one();
        }
    }

    /// The create `id` finished or failed: it leaves the list at once.
    pub(crate) fn finish(&self, id: &str) {
        if self.entries.lock().expect("creates").remove(id).is_some() {
            self.wake.notify_one();
        }
    }

    /// What to publish now, oldest first; marks each as published.
    pub(crate) fn take(&self) -> Vec<Creating> {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("creates");
        entries
            .values_mut()
            .map(|e| {
                e.published = Some((e.creating.percent, now));
                e.creating.clone()
            })
            .collect()
    }

    /// Whether a create runs (the publisher keeps a heartbeat while one does).
    pub(crate) fn busy(&self) -> bool {
        !self.entries.lock().expect("creates").is_empty()
    }

    /// Waits until something should be published.
    pub(crate) async fn changed(&self) {
        self.wake.notified().await
    }
}

/// Removes a create from the [`Board`] when dropped: a create that returns,
/// fails, is cancelled or is dropped leaves the list.
pub(crate) struct OnBoard {
    pub(crate) board: std::sync::Arc<super::Hub>,
    pub(crate) id: String,
}

impl Drop for OnBoard {
    fn drop(&mut self) {
        self.board.creates.finish(&self.id);
    }
}

impl crate::Spaces {
    /// The Spaces your other computers are creating, from what each
    /// published in its relay machine record ([`decode`]). Reads the
    /// machine list already fetched: no relay call of its own. A Space the
    /// list already has as a row (one a computer makes for this device,
    /// listed under it from the start) is left out.
    pub fn creating_elsewhere(&self) -> Vec<CreatingElsewhere> {
        let own = self.this_relay_machine_id();
        let listed = self.list().unwrap_or_default();
        let now = super::now_ms();
        let cache = self.inner.relay_cache.lock().expect("relay cache");
        cache
            .iter()
            .filter(|m| own.as_deref() != Some(m.id.as_str()))
            .filter_map(|m| Some((m, m.meta.get(META_KEY)?)))
            .flat_map(|(m, value)| decode(&m.id, &m.name, value, now))
            .filter(|c| {
                !listed
                    .iter()
                    .any(|i| i.host == c.machine_id && i.name == c.name)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, os: &str, phase: &str, percent: u8) -> Creating {
        Creating {
            name: name.into(),
            os: os.into(),
            phase: phase.into(),
            percent,
        }
    }

    #[test]
    fn a_value_round_trips() {
        let now = 1_760_000_000u64;
        let s = encode(&[c("design-review", "macos", "pulling", 40)], now);
        let back = decode("m-1", "Mac mini", &s, now * 1000 + 3_000);
        assert_eq!(
            back,
            vec![CreatingElsewhere {
                machine_id: "m-1".into(),
                machine_name: "Mac mini".into(),
                name: "design-review".into(),
                os: "macos".into(),
                phase: "pulling".into(),
                percent: 40,
                updated_at_ms: now * 1000,
            }]
        );
    }

    #[test]
    fn nothing_being_created_clears_the_key() {
        assert_eq!(encode(&[], 1), "");
        assert!(decode("m", "", "", 0).is_empty());
    }

    #[test]
    fn the_largest_value_fits_the_relay_limit() {
        let long = "x".repeat(200);
        let many: Vec<Creating> = (0..20)
            .map(|_| c(&long, "windows", "waiting_for_services", 100))
            .collect();
        let s = encode(&many, u64::MAX);
        assert!(!s.is_empty());
        assert!(
            s.chars().count() <= MAX_LEN,
            "{} chars: {s}",
            s.chars().count()
        );
        assert!(s.chars().all(|ch| !ch.is_control()));
        // Names are cut, and only what fits is kept.
        let back = decode("m", "", &s, u64::MAX);
        assert!(
            !back.is_empty() && back.len() <= MAX_SHOWN,
            "{}",
            back.len()
        );
        assert!(back.iter().all(|b| b.name.chars().count() == NAME_CHARS));
    }

    #[test]
    fn a_value_with_quotes_and_unicode_still_fits() {
        let odd = "\"é\\".repeat(40);
        let four: Vec<Creating> = (0..4).map(|_| c(&odd, "macos", "booting", 70)).collect();
        let s = encode(&four, 1_760_000_000);
        assert!(s.chars().count() <= MAX_LEN, "{} chars", s.chars().count());
        assert!(!decode("m", "", &s, 1_760_000_000_000).is_empty());
    }

    #[test]
    fn at_most_max_shown_are_published() {
        let many: Vec<Creating> = (0..9)
            .map(|i| c(&format!("s{i}"), "linux", "booting", 70))
            .collect();
        let back = decode("m", "", &encode(&many, 10), 10_000);
        assert_eq!(back.len(), MAX_SHOWN);
        assert_eq!(back[0].name, "s0");
    }

    #[test]
    fn a_stale_value_is_ignored() {
        let s = encode(&[c("a", "linux", "booting", 70)], 1_000);
        assert_eq!(decode("m", "", &s, 1_000_000 + 119_000).len(), 1);
        assert!(decode("m", "", &s, 1_000_000 + 121_000).is_empty());
    }

    #[test]
    fn text_that_is_not_a_known_value_is_ignored() {
        assert!(decode("m", "", "not json", 0).is_empty());
        assert!(
            decode(
                "m",
                "",
                r#"{"v":99,"t":0,"s":[["a","linux","booting",1]]}"#,
                0
            )
            .is_empty()
        );
    }

    #[test]
    fn percent_moves_through_each_phase() {
        let p = |phase| Progress::phase(phase);
        assert_eq!(percent(&p(Phase::Preparing)), 0);
        assert_eq!(percent(&p(Phase::Pulling).fraction(0.5)), 32);
        assert_eq!(percent(&p(Phase::Booting)), 70);
        assert_eq!(percent(&p(Phase::Connecting)), 95);
        assert_eq!(percent(&p(Phase::Ready)), 100);
    }

    #[test]
    fn the_board_wakes_on_start_phase_and_finish_and_throttles_percent() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let board = Board::default();
            let woke = |b: &Board| {
                let fut = b.changed();
                futures_util::FutureExt::now_or_never(Box::pin(fut)).is_some()
            };
            board.start("local:a", "a", "macos");
            assert!(woke(&board), "a start publishes at once");
            assert_eq!(board.take()[0].phase, "preparing");
            board.report("local:a", &Progress::phase(Phase::Pulling).fraction(0.01));
            assert!(woke(&board), "a phase change publishes at once");
            assert_eq!(board.take()[0].percent, 5);
            board.report("local:a", &Progress::phase(Phase::Pulling).fraction(0.05));
            assert!(!woke(&board), "a small move waits");
            board.report("local:a", &Progress::phase(Phase::Pulling).fraction(0.3));
            assert!(woke(&board), "ten points publish at once");
            board.take();
            board.finish("local:a");
            assert!(woke(&board), "a finish publishes at once");
            assert!(!board.busy());
            assert!(board.take().is_empty());
        });
    }
}

#[cfg(test)]
mod publish_tests {
    //! The daemon's publisher against a relay (`cua_host::testing::FakeRelay`):
    //! a create is published at once, and so is a new Space count.

    use super::*;
    use crate::resources::summary;
    use crate::{RelayAccount, Spaces};
    use cua_host::testing::FakeRelay;
    use std::sync::Arc;

    const MACHINE: &str = "aaaa000000000002";

    async fn host(relay: &FakeRelay, home: &std::path::Path) -> Spaces {
        relay.add_account("owner-token", "user-1", Some("ada@example.com"));
        cua_host::RelayClient::new(&relay.url)
            .unwrap()
            .register(
                "owner-token",
                &cua_host::relay::RegisterRequest {
                    id: MACHINE.into(),
                    name: "Mac mini".into(),
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .unwrap();
        std::fs::create_dir_all(home.join("host")).unwrap();
        std::fs::write(
            home.join("host/config.json"),
            serde_json::json!({
                "mode": "relay",
                "name": "Mac mini",
                "machine_id": MACHINE,
                "runner": "process",
                "driver_bin": "/nonexistent/cua-spacesd",
            })
            .to_string(),
        )
        .unwrap();
        Spaces::builder()
            .home(home)
            .relay(RelayAccount::new(
                &relay.url,
                Arc::new(crate::relay::StaticToken("owner-token".into())),
            ))
            .build()
    }

    /// Waits (up to `secs`) for the machine's meta to satisfy `ok`.
    async fn until(
        relay: &FakeRelay,
        secs: u64,
        ok: impl Fn(&std::collections::BTreeMap<String, String>) -> bool,
    ) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if ok(&relay.machine(MACHINE).unwrap().meta) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_create_is_published_at_once_and_cleared_when_it_ends() {
        let relay = FakeRelay::start().await;
        let home = tempfile::tempdir().unwrap();
        let spaces = host(&relay, home.path()).await;
        let _publisher = spaces.spawn_resources_publisher();
        let hub = spaces.inner.resources.clone();

        // Long before the first timed publish (20 s after start).
        hub.creates
            .start("local:design-review", "design-review", "macos");
        assert!(
            until(&relay, 5, |m| m
                .get(META_KEY)
                .is_some_and(|v| v.contains("design-review")))
            .await,
            "the create was not published at once"
        );
        // The phase moved on: published again at once.
        hub.creates
            .report("local:design-review", &Progress::phase(Phase::Booting));
        assert!(
            until(&relay, 5, |m| m
                .get(META_KEY)
                .is_some_and(|v| v.contains("booting")))
            .await
        );

        // Another device reads it from the machine list.
        let laptop_home = tempfile::tempdir().unwrap();
        let laptop = Spaces::builder()
            .home(laptop_home.path())
            .relay(RelayAccount::new(
                &relay.url,
                Arc::new(crate::relay::StaticToken("owner-token".into())),
            ))
            .build();
        laptop.relay_machines().await.unwrap();
        let seen = laptop.creating_elsewhere();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(
            (
                seen[0].machine_name.as_str(),
                seen[0].name.as_str(),
                seen[0].os.as_str()
            ),
            ("Mac mini", "design-review", "macos")
        );
        assert_eq!(seen[0].percent, 70);
        // The computer creating it does not list its own create as elsewhere.
        spaces.relay_machines().await.unwrap();
        assert!(spaces.creating_elsewhere().is_empty());

        // Finished: the key goes at once.
        hub.creates.finish("local:design-review");
        assert!(until(&relay, 5, |m| !m.contains_key(META_KEY)).await);
        laptop.relay_machines().await.unwrap();
        assert!(laptop.creating_elsewhere().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_space_count_is_published_without_waiting_for_the_timer() {
        let relay = FakeRelay::start().await;
        let home = tempfile::tempdir().unwrap();
        let spaces = host(&relay, home.path()).await;
        let _publisher = spaces.spawn_resources_publisher();
        // A first publish (a create that came and went).
        let hub = spaces.inner.resources.clone();
        hub.creates.start("local:a", "a", "linux");
        hub.creates.finish("local:a");
        let count = |m: &std::collections::BTreeMap<String, String>| {
            m.get(summary::META_KEY)
                .and_then(|v| summary::decode(MACHINE, "", true, v))
                .map(|c| c.spaces_count)
        };
        assert!(until(&relay, 5, |m| count(m) == Some(0)).await);

        // A Space appears in the registry: published well before the
        // five-minute interval.
        spaces
            .registry()
            .upsert(
                cua_proto::daemon::v1::Space {
                    id: "local:a".into(),
                    name: "a".into(),
                    os: "linux".into(),
                    ..Default::default()
                },
                Default::default(),
            )
            .unwrap();
        assert!(
            until(&relay, 20, |m| count(m) == Some(1)).await,
            "the new count was not published"
        );
    }
}
