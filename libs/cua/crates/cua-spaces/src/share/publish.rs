//! Spaces of this computer on the owner's account: while the computer is
//! signed in and sharing, every running local Space with a working
//! cua-spacesd is registered on the relay (the same owner-only
//! [`Spaces::relay_register`] a person can run by hand), so the phone and the
//! owner's other computers list and open it with no extra step. One pass
//! ([`Spaces::publish_to_relay`]) is the whole rule, and the daemon runs it
//! whenever a Space is created, started, stopped, changed or deleted and every
//! few seconds besides, so it covers a Space made by any path (the app, `cua`,
//! an SDK, MCP, an agent's child, a compat layer) and the Spaces that were
//! already here when this release arrived.
//!
//! - **Listed**: a `local:` Space that is running, whose cua-spacesd can join
//!   the relay, that has not opted out (`--no-relay`; a child follows its
//!   parent) and whose network lets it reach the relay.
//! - **Not listed**: Spaces added by address (not made by Cua), Spaces a
//!   computer provides (already on the relay under their host), Spaces in the
//!   owner's cloud, and everything while `relay.spaces` / `CUA_RELAY_SPACES`
//!   is off or the host stopped sharing.
//! - **Taken off** again: when the Space stops, is deleted (`delete` already
//!   does), opts out, or the switch goes off; when the owner signs out the
//!   Space's driver leaves the relay at once (no account, no calls), and the
//!   next sign-in lists it again. A Space the owner shared with other people
//!   stays as shared.
//!
//! Nothing new is stored: the machine token the relay mints stays inside the
//! Space's driver, as for any registered Space, and the row is the owner's
//! alone until they share it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::{Result, SpaceId, Spaces};

/// Woken when something that decides what is listed changed (a Space was
/// created, started, stopped, deleted or opted out), so the daemon's loop
/// runs a pass now instead of at its next tick.
pub static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// How long a Space that could not be listed is left alone.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// How long a Space whose cua-spacesd is still waking is left alone.
static WAKING_RETRY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10_000);

pub(super) fn set_waking_retry(d: Duration) {
    WAKING_RETRY_MS.store(d.as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
}
/// Calls to the relay in flight at once.
const AT_ONCE: usize = 4;
/// Most registrations and removals one pass starts; the rest wait for the
/// next pass ([`PublishReport::more`]).
const PER_PASS: usize = 24;
/// A Space younger than this is not listed yet, so one made and deleted
/// within moments (a job, a test, a burst of throwaway Spaces) never reaches
/// the relay.
const SETTLE: Duration = Duration::from_secs(5);
/// While Spaces are being made (the newest is younger than this), the pass
/// leaves the relay alone, so a burst of creates has the machine to itself;
/// it goes on after this much quiet, or once a Space has waited
/// [`BUSY_LIMIT`].
const BUSY: Duration = Duration::from_secs(3);
/// The longest a Space waits for a burst of creates to end.
const BUSY_LIMIT: Duration = Duration::from_secs(60);
/// How long a registered Space is given to dial the relay before it is
/// registered again.
const DIAL_GRACE: Duration = Duration::from_secs(90);
/// When nothing changed here, the relay is asked once in this long (plus up
/// to a fifth) whether every listed Space is still online.
const VERIFY_EVERY: Duration = Duration::from_secs(300);
/// Longest pause after the relay failed.
const BREAKER_MAX: Duration = Duration::from_secs(300);

/// The relay failed: no calls until `open_until`, longer each time in a row.
#[derive(Default)]
struct Breaker {
    fails: u32,
    open_until: Option<Instant>,
}

/// `+-20%` so many computers do not move together.
pub fn jitter(d: Duration) -> Duration {
    d.mul_f64(0.8 + rand::random::<f64>() * 0.4)
}

/// How long to stop calling the relay after its `fails`th failure in a row:
/// 5 s doubling to 5 min, with jitter, and never less than the relay's own
/// `Retry-After`.
pub fn breaker_wait(fails: u32, retry_after: Option<Duration>) -> Duration {
    let base = Duration::from_secs(5)
        .saturating_mul(1u32 << fails.saturating_sub(1).min(6))
        .min(BREAKER_MAX);
    jitter(base).max(retry_after.unwrap_or_default().min(BREAKER_MAX))
}

/// The wait the relay asked for in an error, and whether the error is the
/// relay failing (so calls pause) rather than a Space being refused.
fn relay_trouble(e: &crate::Error) -> Option<Option<Duration>> {
    match e {
        crate::Error::Relay(h) if h.is_transient() => Some(match h {
            cua_host::Error::Throttled { retry_after_secs } => {
                Some(Duration::from_secs(*retry_after_secs))
            }
            _ => None,
        }),
        _ => None,
    }
}

struct Memo {
    /// Spaces not tried again before this time.
    retry_at: BTreeMap<String, Instant>,
    /// Spaces whose driver already left the relay for want of an account.
    left: BTreeSet<String>,
    /// The Spaces that were to be listed when the last pass finished
    /// without trouble; `None` while the last pass left work.
    settled: Option<BTreeSet<String>>,
    /// Registered Spaces that have until then to dial the relay.
    dialing: BTreeMap<String, Instant>,
    /// When the relay is asked next even if nothing changed.
    verify_at: Instant,
    breaker: Breaker,
}

impl Default for Memo {
    fn default() -> Self {
        Memo {
            retry_at: Default::default(),
            left: Default::default(),
            settled: None,
            dialing: Default::default(),
            verify_at: Instant::now(),
            breaker: Default::default(),
        }
    }
}

static MEMO: LazyLock<Mutex<BTreeMap<PathBuf, Memo>>> = LazyLock::new(Default::default);

fn memo<T>(home: &std::path::Path, f: impl FnOnce(&mut Memo) -> T) -> T {
    let mut all = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    f(all.entry(home.to_path_buf()).or_default())
}

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublishReport {
    /// Spaces listed on the relay by this pass.
    pub published: Vec<String>,
    /// Spaces taken off the relay by this pass.
    pub unpublished: Vec<String>,
    /// Spaces that could not be listed now, with why (tried again later).
    pub failed: Vec<(String, String)>,
    /// More is waiting (the per-pass limit, or Spaces too new to list):
    /// run the next pass soon, not at the usual interval.
    pub more: bool,
}

const STATUS_FILE: &str = "relay-publish.json";
/// How long what the daemon last failed to list is still told.
const STATUS_FRESH: u64 = 600;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Saved {
    at: u64,
    failed: BTreeMap<String, String>,
}

fn load_saved(home: &std::path::Path) -> Saved {
    std::fs::read(home.join(STATUS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Where this computer's running Spaces stand on the account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublishStatus {
    /// Why nothing is listed at all, when that is so.
    pub off: Option<Off>,
    /// Running Spaces that did not opt out.
    pub running: usize,
    /// Of those, listed on the account.
    pub listed: usize,
    /// Stopped Spaces (listed once they run).
    pub stopped: usize,
    /// Running Spaces that are not listed, with why (name, reason).
    pub not_listed: Vec<(String, String)>,
}

impl Off {
    /// Why, in words for the person.
    pub fn why(self) -> &'static str {
        match self {
            Off::NoAccount => "no cua.ai account is set up on this computer",
            Off::SignedOut => {
                "you are signed out of cua.ai (sign in from the app or run `cua auth login`)"
            }
            Off::Setting => "the relay.spaces setting (or CUA_RELAY_SPACES) is off",
            Off::NotSharing => {
                "this computer stopped sharing (turn sharing on in the app or run `cua host start`)"
            }
        }
    }
}

impl PublishStatus {
    /// Plain lines for `cua spaces ls` and `cua host status`; none when all
    /// is well and there is nothing to say.
    pub fn notes(&self) -> Vec<String> {
        let mut out = Vec::new();
        let plural = |n: usize| if n == 1 { "Space is" } else { "Spaces are" };
        if let Some(off) = self.off {
            if self.running > 0 {
                out.push(format!(
                    "{} running {} not on your account: {}.",
                    self.running,
                    plural(self.running),
                    off.why()
                ));
            }
            return out;
        }
        if !self.not_listed.is_empty() {
            let items: Vec<String> = self
                .not_listed
                .iter()
                .map(|(n, w)| format!("{n} ({w})"))
                .collect();
            out.push(format!(
                "{} running {} not on your account: {}.",
                self.not_listed.len(),
                plural(self.not_listed.len()),
                items.join("; ")
            ));
        }
        if self.stopped > 0 {
            out.push(format!(
                "{} stopped {} on your account once running.",
                self.stopped,
                if self.stopped == 1 {
                    "Space shows"
                } else {
                    "Spaces show"
                }
            ));
        }
        out
    }
}

/// Why nothing is listed now, or `None` when Spaces may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Off {
    /// No relay account is configured.
    NoAccount,
    /// Nobody is signed in.
    SignedOut,
    /// `relay.spaces` / `CUA_RELAY_SPACES` is off.
    Setting,
    /// This computer stopped sharing (`cua host stop`, or paused signed out).
    NotSharing,
}

impl Spaces {
    fn relay_spaces_setting(&self) -> bool {
        cua_sandbox_core::settings::Settings::load_with(self.home_dir().join("config.toml"), |k| {
            std::env::var(k).ok()
        })
        .map(|s| s.relay_spaces())
        .unwrap_or(true)
    }

    /// Whether this computer lists its Spaces on the account now, or why not.
    pub async fn relay_publishing(&self) -> std::result::Result<(), Off> {
        let Some(account) = self.relay_account() else {
            return Err(Off::NoAccount);
        };
        if !self.relay_spaces_setting() {
            return Err(Off::Setting);
        }
        // A computer set up to share that stopped (or paused until the owner
        // signs in again) lists nothing; one never set up lists its Spaces
        // once signed in.
        if let Ok(Some(c)) = cua_host::Host::new(self.home_dir()).config()
            && (!c.sharing || c.paused_signed_out)
        {
            return Err(Off::NotSharing);
        }
        if !account.tokens.signed_in().await {
            return Err(Off::SignedOut);
        }
        Ok(())
    }

    /// The Spaces another of the owner's computers asked this one to make:
    /// each is on the relay already, under this computer.
    fn provided_here(&self) -> BTreeSet<String> {
        cua_host::provided::load_provided(&cua_host::Host::new(self.home_dir()).paths().dir)
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.local_space)
            .collect()
    }

    /// Whether the local Space `info` is on, as the runtime last recorded it
    /// (any kind: a macOS VM, a QEMU VM, a container, a microVM). A Space
    /// whose state is not recorded is neither on nor off.
    fn recorded_running(&self, info: &crate::SpaceInfo) -> bool {
        matches!(
            SpaceId::parse(&info.id),
            Ok(SpaceId::Local { name })
                if self.inner.sandboxes.power_state(&name) == Some(cua_sandbox_core::PowerState::Running)
        )
    }

    fn is_off(&self, info: &crate::SpaceInfo) -> bool {
        matches!(
            SpaceId::parse(&info.id),
            Ok(SpaceId::Local { name })
                if matches!(
                    self.inner.sandboxes.power_state(&name),
                    Some(cua_sandbox_core::PowerState::Stopped | cua_sandbox_core::PowerState::Suspended)
                )
        )
    }

    /// Whether `info` (a running local Space) is one to list: it can join
    /// the relay and its network lets it reach it.
    fn listable(
        &self,
        info: &crate::SpaceInfo,
        meta: &BTreeMap<String, crate::registry::SpaceMeta>,
    ) -> bool {
        self.recorded_running(info) && self.listable_ignoring_state(info, meta)
    }

    /// [`Self::listable`] without the running state (a stopped Space that
    /// could be listed is still taken off).
    fn listable_ignoring_state(
        &self,
        info: &crate::SpaceInfo,
        meta: &BTreeMap<String, crate::registry::SpaceMeta>,
    ) -> bool {
        if !info.id.starts_with("local:") {
            return false;
        }
        if !info
            .features
            .iter()
            .any(|f| f == super::RELAY_ATTACH_FEATURE)
        {
            return false;
        }
        !Self::opted_out(&info.id, meta) && !Self::network_blocks_relay(&info.id, meta)
    }

    /// Whether the Space `id` (or a parent) opted out of the account's relay
    /// (`--no-relay`, `cua.relay=off`) or is hidden.
    fn opted_out(id: &str, meta: &BTreeMap<String, crate::registry::SpaceMeta>) -> bool {
        let mut at = Some(id.to_string());
        let mut seen = BTreeSet::new();
        while let Some(id) = at {
            if !seen.insert(id.clone()) {
                break;
            }
            let m = meta.get(&id);
            if m.is_some_and(|m| {
                !m.relay.is_on()
                    || m.labels.contains_key(crate::registry::HIDDEN_LABEL)
                    || m.labels.get(crate::registry::RELAY_LABEL).is_some_and(|v| {
                        crate::registry::RelaySetting::parse(v).is_ok_and(|s| !s.is_on())
                    })
            }) {
                return true;
            }
            at = m.and_then(|m| m.parent.clone());
        }
        false
    }

    /// The Space dials the relay itself: with no network it never reaches
    /// it (`custom` starts from `internet` or `none`).
    fn network_blocks_relay(id: &str, meta: &BTreeMap<String, crate::registry::SpaceMeta>) -> bool {
        use cua_vmm::network::NetworkPreset as P;
        meta.get(id)
            .and_then(|m| m.network.as_ref())
            .is_some_and(|n| {
                n.preset == P::None || (n.preset == P::Custom && n.custom_base == P::None)
            })
    }

    /// Where each local Space of `infos` stands on the account: `on` (listed
    /// there now) or `off` (it stays off: `--no-relay`, a parent's `off`, or
    /// `relay.spaces` off). A Space not in the map is neither (not running,
    /// or not one this computer lists); `cua spaces ls` shows it.
    pub fn relay_states(&self, infos: &[crate::SpaceInfo]) -> BTreeMap<String, &'static str> {
        let meta = self.inner.registry.all_meta();
        let setting = self.relay_spaces_setting();
        let attached = self.attached();
        let mut out = BTreeMap::new();
        let provided = self.provided_here();
        for info in infos
            .iter()
            .filter(|i| i.id.starts_with("local:") && !provided.contains(&i.id))
        {
            let mut able = info.clone();
            able.features = vec![super::RELAY_ATTACH_FEATURE.into()];
            if !setting || !self.listable_ignoring_state(&able, &meta) {
                out.insert(info.id.clone(), "off");
            } else if attached.get(&info.id).is_some_and(|a| a.auto) {
                out.insert(info.id.clone(), "on");
            }
        }
        out
    }

    /// One pass: lists what should be listed and takes off what should not.
    /// Built for a relay that is shared by many computers:
    ///
    /// - Nothing that changed here means no call at all (the relay is asked
    ///   once in five minutes, with jitter, whether the listed Spaces are
    ///   still online).
    /// - A pass reads the relay's directory once, then registers or removes
    ///   at most [`PER_PASS`] Spaces, [`AT_ONCE`] at a time, without waiting
    ///   for any of them to come online; the rest wait for the next pass.
    /// - A Space younger than [`SETTLE`] is not listed yet, so one made and
    ///   deleted within moments never reaches the relay.
    /// - When the relay fails (429, 5xx, not reached) calls stop for a
    ///   growing, jittered time that is never shorter than its `Retry-After`;
    ///   Spaces keep being made here meanwhile.
    pub async fn publish_to_relay(&self) -> Result<PublishReport> {
        let report = self.publish_pass().await?;
        self.record_outcome(&report);
        Ok(report)
    }

    async fn publish_pass(&self) -> Result<PublishReport> {
        let mut report = PublishReport::default();
        let home = self.home_dir().to_path_buf();
        let gate = self.relay_publishing().await;
        let all = self.attached();
        let auto: Vec<String> = all
            .iter()
            .filter(|(_, a)| a.auto)
            .map(|(s, _)| s.clone())
            .collect();

        // Not publishing now: what this pass listed leaves.
        if let Err(off) = gate {
            if auto.is_empty() {
                return Ok(report);
            }
            if off == Off::SignedOut || off == Off::NoAccount {
                // No account to call the relay with: the driver stops
                // dialing, and the next sign-in lists the Space again.
                for space in auto {
                    if memo(&home, |m| m.left.insert(space.clone())) {
                        self.detach_attached_space(&space).await;
                    }
                }
                memo(&home, |m| {
                    m.settled = None;
                    m.dialing.clear();
                });
                return Ok(report);
            }
            if memo(&home, |m| {
                m.breaker.open_until.is_some_and(|t| t > Instant::now())
            }) {
                return Ok(report);
            }
            let rows = self.owner_rows().await;
            let rows = match rows {
                Ok(r) => r,
                Err(e) => {
                    self.relay_failed(&home, &e);
                    return Ok(report);
                }
            };
            let removals = self.remove_all(&auto, &rows, &mut report).await;
            report.more = removals;
            memo(&home, |m| m.settled = None);
            return Ok(report);
        }
        memo(&home, |m| m.left.clear());

        // The relay failed lately: no calls yet.
        if memo(&home, |m| {
            m.breaker.open_until.is_some_and(|t| t > Instant::now())
        }) {
            return Ok(report);
        }

        let infos = self.list()?;
        let meta = self.inner.registry.all_meta();
        let provided = self.provided_here();
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut young = false;
        let want: Vec<&crate::SpaceInfo> = infos
            .iter()
            .filter(|i| !provided.contains(&i.id) && self.listable(i, &meta))
            .filter(|i| {
                let born = meta.get(&i.id).map_or(0, |m| m.created_at_unix);
                let settled = born == 0 || now_unix.saturating_sub(born) >= SETTLE.as_secs();
                young |= !settled;
                settled
            })
            .collect();
        let wanted: BTreeSet<String> = want.iter().map(|i| i.id.clone()).collect();

        // Spaces are being made right now: listing them waits for a gap (but
        // not for more than a minute).
        let newest = meta
            .iter()
            .filter(|(id, _)| id.starts_with("local:"))
            .map(|(_, m)| m.created_at_unix)
            .max()
            .unwrap_or(0);
        let waited_long = want.iter().any(|i| {
            let born = meta.get(&i.id).map_or(0, |m| m.created_at_unix);
            !all.contains_key(&i.id)
                && born != 0
                && now_unix.saturating_sub(born) >= BUSY_LIMIT.as_secs()
        });
        if newest != 0
            && now_unix.saturating_sub(newest) < BUSY.as_secs()
            && !waited_long
            && !wanted.is_empty()
        {
            report.more = true;
            return Ok(report);
        }

        // Nothing listed and nothing to list: no call.
        if wanted.is_empty() && auto.is_empty() {
            memo(&home, |m| {
                m.settled = (!young).then(|| wanted.clone());
                m.verify_at = Instant::now() + jitter(VERIFY_EVERY);
            });
            report.more = young;
            return Ok(report);
        }

        // Nothing changed since a pass that finished: no call.
        let unchanged = memo(&home, |m| {
            m.settled.as_ref() == Some(&wanted) && Instant::now() < m.verify_at
        });
        if unchanged && !young {
            return Ok(report);
        }

        // The account may have changed since a Space was listed: its row
        // belongs to the other account, so it is listed again as new.
        let account = self.relay()?;
        let subject = account.account_subject().await.unwrap_or_default();
        for space in &auto {
            let owner = all
                .get(space)
                .map(|a| a.account.clone())
                .unwrap_or_default();
            if !subject.is_empty() && !owner.is_empty() && owner != subject {
                self.detach_attached_space(space).await;
                let _ = self.update_attached(|now| {
                    now.remove(space);
                });
            }
        }
        let all = self.attached();

        // One read of the directory serves the whole pass.
        let rows = match self.owner_rows().await {
            Ok(r) => r,
            Err(e) => {
                self.relay_failed(&home, &e);
                report.failed.push((String::new(), e.to_string()));
                return Ok(report);
            }
        };

        // Off: automatically listed Spaces that stopped, opted out or went.
        let mut leaving = Vec::new();
        for space in all.iter().filter(|(_, a)| a.auto).map(|(s, _)| s) {
            if wanted.contains(space) {
                continue;
            }
            // A Space whose state is not known now is left as it is; one
            // that is gone, stopped or opted out leaves.
            let gone = match infos.iter().find(|i| &i.id == space) {
                None => true,
                Some(i) => {
                    self.is_off(i)
                        || !meta.get(space).is_none_or(|m| m.relay.is_on())
                        || !self.listable_ignoring_state(i, &meta)
                }
            };
            if gone {
                leaving.push(space.clone());
            }
        }

        // On: Spaces that are not online and not on their way (or whose row
        // does not name this computer as host yet: it is registered again).
        let host_id = self.own_host_id();
        let now = Instant::now();
        let due: Vec<String> = want
            .iter()
            .map(|i| i.id.clone())
            .filter(|id| {
                let online = all
                    .get(id)
                    .and_then(|a| rows.get(&a.machine))
                    .is_some_and(|m| {
                        m.online && m.role == "owner" && super::host_matches(m, host_id.as_deref())
                    });
                let (waiting, dialing) = memo(&home, |m| {
                    (
                        m.retry_at.get(id).is_some_and(|t| *t > now),
                        m.dialing.get(id).is_some_and(|t| *t > now),
                    )
                });
                !online && !waiting && !dialing
            })
            .collect();

        let mut budget = PER_PASS;
        let leave_now: Vec<String> = leaving.iter().take(budget).cloned().collect();
        budget -= leave_now.len();
        let join_now: Vec<String> = due.iter().take(budget).cloned().collect();
        let over = leaving.len() + due.len() > PER_PASS;

        let tripped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rows = Arc::new(rows);
        use futures_util::StreamExt as _;
        let removed: Vec<(String, bool)> = futures_util::stream::iter(leave_now)
            .map(|space| {
                let (rows, tripped, this) = (rows.clone(), tripped.clone(), self.clone());
                async move {
                    if tripped.load(std::sync::atomic::Ordering::SeqCst) {
                        return (space, false);
                    }
                    let machine = this.attached().get(&space).map(|a| a.machine.clone());
                    let shared = machine.as_ref().and_then(|m| rows.get(m)).is_some_and(|m| {
                        !m.allow.is_empty() || !m.viewers.is_empty() || m.team.is_some()
                    });
                    let ok = this.unpublish_with(&space, shared).await;
                    if !ok {
                        tripped.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    (space, ok)
                }
            })
            .buffer_unordered(AT_ONCE)
            .collect()
            .await;
        let mut trouble = false;
        for (space, ok) in removed {
            if ok {
                report.unpublished.push(space);
            } else {
                // Shared Spaces stay (not trouble); a failed call is.
                trouble |= self.attached().get(&space).is_some_and(|a| a.auto);
            }
        }

        let results: Vec<(String, Option<Result<()>>)> = futures_util::stream::iter(join_now)
            .map(|id| {
                let (tripped, subject, this) = (tripped.clone(), subject.clone(), self.clone());
                async move {
                    if trouble || tripped.load(std::sync::atomic::Ordering::SeqCst) {
                        return (id, None);
                    }
                    let fresh = !this.attached().contains_key(&id);
                    let r = this
                        .ensure_shared_machine_with(&id, false)
                        .await
                        .map(|_| ());
                    if let Err(e) = &r
                        && relay_trouble(e).is_some()
                    {
                        tripped.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    if r.is_ok() && fresh {
                        // Listed by this pass, not by hand: this pass also
                        // takes it off again.
                        let _ = this.update_attached(|now| {
                            if let Some(a) = now.get_mut(&id) {
                                a.auto = true;
                                a.account = subject.clone();
                            }
                        });
                        this.audit_share("publish", &id, "automatic");
                    }
                    (id, Some(r))
                }
            })
            .buffer_unordered(AT_ONCE)
            .collect()
            .await;
        let mut skipped = false;
        let mut relay_error: Option<(crate::Error, Option<Duration>)> = None;
        for (id, r) in results {
            match r {
                None => skipped = true,
                Some(Ok(())) => {
                    memo(&home, |m| {
                        m.retry_at.remove(&id);
                        m.dialing.insert(id.clone(), Instant::now() + DIAL_GRACE);
                    });
                    report.published.push(id);
                }
                Some(Err(e)) => {
                    tracing::debug!(space = %id, error = %e, "not listed on the relay now");
                    if let Some(wait) = relay_trouble(&e) {
                        let msg = e.to_string();
                        relay_error = Some((e, wait));
                        report.failed.push((id, msg));
                    } else {
                        let after = if super::is_waking(&e) {
                            Duration::from_millis(
                                WAKING_RETRY_MS.load(std::sync::atomic::Ordering::Relaxed),
                            )
                        } else {
                            RETRY_AFTER
                        };
                        memo(&home, |m| {
                            m.retry_at.insert(id.clone(), Instant::now() + after)
                        });
                        report.failed.push((id, e.to_string()));
                    }
                }
            }
        }
        if trouble && relay_error.is_none() {
            // A removal failed against the relay.
            relay_error = Some((crate::Error::invalid("removal failed"), None));
        }
        match relay_error {
            Some((e, wait)) => {
                memo(&home, |m| {
                    m.breaker.fails = m.breaker.fails.saturating_add(1);
                    m.breaker.open_until =
                        Some(Instant::now() + breaker_wait(m.breaker.fails, wait));
                    m.settled = None;
                });
                tracing::warn!(error = %e, "the relay is failing; Spaces are listed again later");
                report.more = false;
            }
            None => {
                // A Space waiting for its retry is not finished: the next
                // passes must come back to it, not rest until the verify.
                let retrying = memo(&home, |m| {
                    want.iter()
                        .any(|i| m.retry_at.get(&i.id).is_some_and(|t| *t > Instant::now()))
                });
                let finished = !over && !skipped && !young && !retrying && report.failed.is_empty();
                memo(&home, |m| {
                    m.breaker = Breaker::default();
                    m.dialing.retain(|_, t| *t > Instant::now());
                    if finished {
                        m.settled = Some(wanted);
                        m.verify_at = Instant::now() + jitter(VERIFY_EVERY);
                    } else {
                        m.settled = None;
                    }
                });
                report.more = over || young;
            }
        }
        Ok(report)
    }

    /// The owner's rows of the relay directory, by machine id: the pass's one
    /// read.
    async fn owner_rows(&self) -> Result<BTreeMap<String, crate::relay::RelayMachine>> {
        let account = self.relay()?;
        let token = account.tokens.access_token().await?;
        let rows = account.client().await?.machines(&token).await?;
        Ok(rows.into_iter().map(|m| (m.id.clone(), m)).collect())
    }

    /// Notes a failed call to the relay: no more calls for a while.
    fn relay_failed(&self, home: &std::path::Path, e: &crate::Error) {
        let wait = relay_trouble(e).flatten();
        memo(home, |m| {
            m.breaker.fails = m.breaker.fails.saturating_add(1);
            m.breaker.open_until = Some(Instant::now() + breaker_wait(m.breaker.fails, wait));
            m.settled = None;
        });
        tracing::warn!(error = %e, "the relay is failing; Spaces are listed again later");
    }

    /// Takes every Space of `spaces` off (the setting went off, or sharing
    /// stopped), at most [`PER_PASS`] at a time. True when some are left.
    async fn remove_all(
        &self,
        spaces: &[String],
        rows: &BTreeMap<String, crate::relay::RelayMachine>,
        report: &mut PublishReport,
    ) -> bool {
        use futures_util::StreamExt as _;
        let batch: Vec<String> = spaces.iter().take(PER_PASS).cloned().collect();
        let out: Vec<(String, bool)> = futures_util::stream::iter(batch)
            .map(|space| {
                let this = self.clone();
                let shared = this
                    .attached()
                    .get(&space)
                    .and_then(|a| rows.get(&a.machine))
                    .is_some_and(|m| {
                        !m.allow.is_empty() || !m.viewers.is_empty() || m.team.is_some()
                    });
                async move {
                    let ok = this.unpublish_with(&space, shared).await;
                    (space, ok)
                }
            })
            .buffer_unordered(AT_ONCE)
            .collect()
            .await;
        for (space, ok) in out {
            if ok {
                report.unpublished.push(space);
            }
        }
        spaces.len() > PER_PASS
    }

    /// Takes a Space this pass listed off the relay: its driver leaves and
    /// its row goes (one call). A Space shared with other people (`shared`,
    /// from the directory) stays on the relay as shared: the owner decides
    /// that. `true` when it left.
    async fn unpublish_with(&self, space: &str, shared: bool) -> bool {
        let Some(machine) = self.attached().get(space).map(|a| a.machine.clone()) else {
            return false;
        };
        if shared {
            let _ = self.update_attached(|now| {
                if let Some(a) = now.get_mut(space) {
                    a.auto = false;
                }
            });
            return false;
        }
        match self.detach(space, &machine).await {
            Ok(()) => {
                memo(self.home_dir(), |m| m.dialing.remove(space));
                self.audit_share("unpublish", space, "automatic");
                true
            }
            Err(e) => {
                tracing::debug!(space, error = %e, "not taken off the relay now");
                false
            }
        }
    }

    /// Remembers, for `cua spaces ls` and `cua host status` (other
    /// processes), which Spaces the last passes could not list and why.
    fn record_outcome(&self, report: &PublishReport) {
        let home = self.home_dir();
        let mut saved = load_saved(home);
        let before = saved.failed.clone();
        let clean = memo(home, |m| m.settled.is_some() && m.breaker.fails == 0);
        if clean {
            saved.failed.clear();
        }
        for id in report.published.iter().chain(&report.unpublished) {
            saved.failed.remove(id);
        }
        for (id, why) in &report.failed {
            saved.failed.insert(id.clone(), why.clone());
        }
        if saved.failed != before {
            saved.at = unix_now();
            let _ = std::fs::write(
                home.join(STATUS_FILE),
                serde_json::to_vec(&saved).unwrap_or_default(),
            );
        }
    }

    /// Where this computer's running Spaces stand on the account, in plain
    /// words for the person: which are not listed and why (nothing is
    /// silent). Reads the same rules a pass follows.
    pub async fn relay_publish_status(&self, infos: &[crate::SpaceInfo]) -> PublishStatus {
        let off = self.relay_publishing().await.err();
        let meta = self.inner.registry.all_meta();
        let attached = self.attached();
        let provided = self.provided_here();
        let saved = load_saved(self.home_dir());
        let fresh = unix_now().saturating_sub(saved.at) < STATUS_FRESH;
        let mut st = PublishStatus {
            off,
            ..Default::default()
        };
        for info in infos
            .iter()
            .filter(|i| i.id.starts_with("local:") && !provided.contains(&i.id))
        {
            if self.is_off(info) {
                st.stopped += 1;
                continue;
            }
            if !self.recorded_running(info) || Self::opted_out(&info.id, &meta) {
                continue;
            }
            st.running += 1;
            if attached.get(&info.id).is_some_and(|a| a.auto) {
                st.listed += 1;
                continue;
            }
            if off.is_some() {
                continue;
            }
            let why = if !info
                .features
                .iter()
                .any(|f| f == super::RELAY_ATTACH_FEATURE)
            {
                crate::space::unreported_feature_limitation(
                    &info.spacesd_version,
                    super::RELAY_ATTACH_FEATURE,
                )
            } else if Self::network_blocks_relay(&info.id, &meta) {
                "its network cannot reach the relay".to_string()
            } else if let Some(e) = saved.failed.get(&info.id).filter(|_| fresh) {
                format!("not listed yet: {e}")
            } else if let Some(e) = saved.failed.get("").filter(|_| fresh) {
                format!("the relay did not answer: {e}")
            } else {
                "waiting for the next check".to_string()
            };
            st.not_listed.push((info.shown_name().to_string(), why));
        }
        st
    }

    /// Makes a hidden Space ([`crate::registry::HIDDEN_LABEL`]) visible: the
    /// apps list it from now on and the next publish pass puts it on the
    /// owner's account (`set_on_relay` on). The daemon calls it when a
    /// person writes into the Space's thread. `true` when it was hidden.
    pub async fn reveal(&self, space: &str) -> Result<bool> {
        let id = self.resolve(space)?;
        let canonical = id.to_string();
        let hidden = self
            .inner
            .registry
            .all_meta()
            .get(&canonical)
            .is_some_and(|m| m.labels.contains_key(crate::registry::HIDDEN_LABEL));
        if !hidden {
            return Ok(false);
        }
        self.inner.registry.update_meta(&canonical, |m| {
            m.labels.remove(crate::registry::HIDDEN_LABEL);
        })?;
        self.set_on_relay(space, true).await?;
        Ok(true)
    }

    /// Opts a Space in or out of the account's relay (`--no-relay`). Off
    /// takes it off at once when this computer is signed in.
    pub async fn set_on_relay(&self, space: &str, on: bool) -> Result<crate::SpaceInfo> {
        let id = self.resolve(space)?;
        if !matches!(id, SpaceId::Local { .. }) {
            return Err(crate::Error::invalid(format!(
                "{id} is not a Space on this computer; only those are listed on your account by Cua"
            )));
        }
        let canonical = id.to_string();
        let setting = if on {
            crate::registry::RelaySetting::On
        } else {
            crate::registry::RelaySetting::Off
        };
        self.inner.registry.update_meta(&canonical, |m| {
            m.relay = setting;
            // The label that opted it out at create (`--label cua.relay=off`)
            // would otherwise keep it off for good.
            if on {
                m.labels.remove(crate::registry::RELAY_LABEL);
            }
        })?;
        memo(self.home_dir(), |m| m.settled = None);
        CHANGED.notify_waiters();
        if !on {
            let _ = self.publish_to_relay().await;
        }
        self.info(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_after_a_failure_doubles_honors_retry_after_and_is_capped() {
        for _ in 0..50 {
            let w = |n, r| breaker_wait(n, r).as_secs_f64();
            assert!((4.0..=6.0).contains(&w(1, None)), "{}", w(1, None));
            assert!((8.0..=12.0).contains(&w(2, None)));
            assert!((16.0..=24.0).contains(&w(3, None)));
            assert!(w(30, None) <= 360.0, "capped");
            assert!(w(30, None) >= 240.0);
            // The relay's own Retry-After is a floor.
            assert!(w(1, Some(Duration::from_secs(90))) >= 90.0);
            // ...but an absurd one does not park the pass for hours.
            assert!(w(1, Some(Duration::from_secs(86_400))) <= 300.0);
        }
    }

    #[test]
    fn jitter_stays_within_a_fifth() {
        for _ in 0..200 {
            let j = jitter(Duration::from_secs(15)).as_secs_f64();
            assert!((12.0..=18.0).contains(&j), "{j}");
        }
    }

    #[test]
    fn only_a_failing_relay_trips_the_breaker() {
        let transient = crate::Error::Relay(cua_host::Error::Throttled {
            retry_after_secs: 7,
        });
        assert_eq!(
            relay_trouble(&transient),
            Some(Some(Duration::from_secs(7)))
        );
        let down = crate::Error::Relay(cua_host::Error::Relay("HTTP 503: gone".into()));
        assert_eq!(relay_trouble(&down), Some(None));
        let refused = crate::Error::Relay(cua_host::Error::PermissionDenied("no".into()));
        assert_eq!(relay_trouble(&refused), None);
        assert_eq!(relay_trouble(&crate::Error::invalid("x")), None);
    }
}
