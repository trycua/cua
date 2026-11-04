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
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::{Result, SpaceId, Spaces};

/// Woken when something that decides what is listed changed (a Space was
/// created, started, stopped, deleted or opted out), so the daemon's loop
/// runs a pass now instead of at its next tick.
pub static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// How long a Space that could not be listed is left alone.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// Registrations running at once in one pass.
const AT_ONCE: usize = 4;

#[derive(Default)]
struct Memo {
    /// Spaces not tried again before this time.
    retry_at: BTreeMap<String, Instant>,
    /// Spaces whose driver already left the relay for want of an account.
    left: BTreeSet<String>,
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
        // Opted out itself or through a parent.
        let mut at = Some(info.id.clone());
        let mut seen = BTreeSet::new();
        while let Some(id) = at {
            if !seen.insert(id.clone()) {
                break;
            }
            let m = meta.get(&id);
            if m.is_some_and(|m| {
                !m.relay.is_on()
                    || m.labels.get(crate::registry::RELAY_LABEL).is_some_and(|v| {
                        crate::registry::RelaySetting::parse(v).is_ok_and(|s| !s.is_on())
                    })
            }) {
                return false;
            }
            at = m.and_then(|m| m.parent.clone());
        }
        // The Space dials the relay itself: with no network it never reaches
        // it (`custom` starts from `internet` or `none`).
        use cua_vmm::network::NetworkPreset as P;
        let no_network = meta
            .get(&info.id)
            .and_then(|m| m.network.as_ref())
            .is_some_and(|n| {
                n.preset == P::None || (n.preset == P::Custom && n.custom_base == P::None)
            });
        !no_network
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
    /// Idempotent and safe to run often; a Space that fails is left alone
    /// for a minute.
    pub async fn publish_to_relay(&self) -> Result<PublishReport> {
        let mut report = PublishReport::default();
        let home = self.home_dir().to_path_buf();
        let gate = self.relay_publishing().await;
        let all = self.attached();
        let auto: Vec<String> = all
            .iter()
            .filter(|(_, a)| a.auto)
            .map(|(s, _)| s.clone())
            .collect();
        let infos = self.list()?;
        let meta = self.inner.registry.all_meta();

        // Not publishing now: what this pass listed leaves.
        if let Err(off) = gate {
            for space in auto {
                if off == Off::SignedOut || off == Off::NoAccount {
                    // No account to call the relay with: the driver stops
                    // dialing, and the next sign-in lists the Space again.
                    if memo(&home, |m| m.left.insert(space.clone())) {
                        self.detach_attached_space(&space).await;
                    }
                } else if self.unpublish(&space).await {
                    report.unpublished.push(space);
                }
            }
            return Ok(report);
        }
        memo(&home, |m| m.left.clear());

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
                let mut now = self.attached();
                now.remove(space);
                let _ = self.save_attached(&now);
            }
        }
        let all = self.attached();

        let provided = self.provided_here();
        let want: Vec<&crate::SpaceInfo> = infos
            .iter()
            .filter(|i| !provided.contains(&i.id) && self.listable(i, &meta))
            .collect();
        let wanted: BTreeSet<&str> = want.iter().map(|i| i.id.as_str()).collect();

        // Off: automatically listed Spaces that stopped, opted out or went.
        for space in all.iter().filter(|(_, a)| a.auto).map(|(s, _)| s) {
            if wanted.contains(space.as_str()) {
                continue;
            }
            // A Space that is starting or whose state is not known now is
            // left as it is; one that is gone, stopped or opted out leaves.
            let still = infos.iter().find(|i| &i.id == space);
            let gone = match still {
                None => true,
                Some(i) => {
                    self.is_off(i)
                        || !meta.get(space).is_none_or(|m| m.relay.is_on())
                        || !self.listable_ignoring_state(i, &meta)
                }
            };
            if gone && self.unpublish(space).await {
                report.unpublished.push(space.clone());
            }
        }

        // On: running Spaces that are not listed (or whose driver left).
        let token = account.tokens.access_token().await?;
        let rows: BTreeMap<String, crate::relay::RelayMachine> = account
            .client()
            .await?
            .machines(&token)
            .await
            .map(|v| v.into_iter().map(|m| (m.id.clone(), m)).collect())
            .unwrap_or_default();
        let due: Vec<String> = want
            .iter()
            .map(|i| i.id.clone())
            .filter(|id| {
                let online = all
                    .get(id)
                    .and_then(|a| rows.get(&a.machine))
                    .is_some_and(|m| m.online && m.role == "owner");
                let waiting = memo(&home, |m| {
                    m.retry_at.get(id).is_some_and(|t| *t > Instant::now())
                });
                !online && !waiting
            })
            .collect();
        use futures_util::StreamExt as _;
        let results: Vec<(String, Result<()>)> = futures_util::stream::iter(due)
            .map(|id| async move {
                let fresh = !self.attached().contains_key(&id);
                let r = self.ensure_shared_machine(&id).await.map(|_| ());
                if r.is_ok() && fresh {
                    // Listed by this pass, not by hand: this pass also takes
                    // it off again.
                    let account = subject_of(&self.relay_account()).await;
                    let _ = self.update_attached(|now| {
                        if let Some(a) = now.get_mut(&id) {
                            a.auto = true;
                            a.account = account;
                        }
                    });
                    self.audit_share("publish", &id, "automatic");
                }
                (id, r)
            })
            .buffer_unordered(AT_ONCE)
            .collect()
            .await;
        for (id, r) in results {
            match r {
                Ok(()) => {
                    memo(&home, |m| m.retry_at.remove(&id));
                    report.published.push(id);
                }
                Err(e) => {
                    tracing::debug!(space = %id, error = %e, "not listed on the relay now");
                    memo(&home, |m| {
                        m.retry_at.insert(id.clone(), Instant::now() + RETRY_AFTER)
                    });
                    report.failed.push((id, e.to_string()));
                }
            }
        }
        Ok(report)
    }

    /// Takes a Space this pass listed off the relay: its driver leaves and
    /// its row goes. A Space shared with other people stays on the relay as
    /// shared (the owner decides that), and nothing is changed when the relay
    /// cannot be reached now (it is tried again). `true` when it left.
    async fn unpublish(&self, space: &str) -> bool {
        let Some(machine) = self.attached().get(space).map(|a| a.machine.clone()) else {
            return false;
        };
        if let Ok(account) = self.relay()
            && let Ok(token) = account.tokens.access_token().await
            && let Ok(client) = account.client().await
            && let Ok(m) = client.machine(&token, &machine).await
            && (!m.allow.is_empty() || !m.viewers.is_empty() || m.team.is_some())
        {
            let mut now = self.attached();
            if let Some(a) = now.get_mut(space) {
                a.auto = false;
            }
            let _ = self.save_attached(&now);
            return false;
        }
        match self.detach(space, &machine).await {
            Ok(()) => {
                self.audit_share("unpublish", space, "automatic");
                true
            }
            Err(e) => {
                tracing::debug!(space, error = %e, "not taken off the relay now");
                false
            }
        }
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
        self.inner
            .registry
            .update_meta(&canonical, |m| m.relay = setting)?;
        CHANGED.notify_waiters();
        if !on {
            let _ = self.publish_to_relay().await;
        }
        self.info(&id)
    }
}

async fn subject_of(account: &Option<crate::relay::RelayAccount>) -> String {
    match account {
        Some(a) => a.account_subject().await.unwrap_or_default(),
        None => String::new(),
    }
}
