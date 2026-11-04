//! A computer that is registered on the relay publishes its summary
//! ([`summary`]) into its own machine record, so your other computers show
//! it from the machine list they already fetch.
//!
//! Rates: once a minute while someone is connected to the machine
//! ([`summary::WATCHED_EVERY`]; the relay's answer to each publish says who
//! is), every five minutes otherwise ([`summary::IDLE_EVERY`]), and at once
//! when the low-disk level or the number of Spaces changes. A relay that
//! predates summaries drops the key; the publisher then tries again only
//! hourly.
//!
//! The same publish carries the Spaces being created here ([`creates`]): at
//! once when one starts, changes phase, moves on, finishes or fails, and
//! every [`creates::HEARTBEAT`] while one runs.

use super::{ResourcesEvent, creates, summary};
use crate::{Result, Spaces};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long after the daemon starts the first summary goes out (so the
/// first sample has settled).
const FIRST_PUBLISH_DELAY: Duration = Duration::from_secs(20);
/// How long a relay that dropped the summary is left alone.
const UNSUPPORTED_BACKOFF: Duration = Duration::from_secs(3600);
/// How often the number of this computer's Spaces is compared with the one
/// last published (a local read).
const COUNT_EVERY: Duration = Duration::from_secs(5);

/// What a publish did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Published {
    /// This install is not a relay machine, or no account is signed in:
    /// nothing to publish into.
    NotAHost,
    /// The relay answered without the summary (an older relay).
    Unsupported,
    /// Published. `watchers` are the clients connected to the machine now.
    Sent {
        /// Clients connected to this machine.
        watchers: usize,
        /// The number of Spaces it published.
        spaces: u32,
    },
}

impl Spaces {
    /// Publishes this computer's summary into its relay machine record.
    pub async fn publish_resources(&self) -> Result<Published> {
        let (Some(machine), Some(relay)) = (self.this_relay_machine_id(), self.relay_account())
        else {
            return Ok(Published::NotAHost);
        };
        let snapshot = self.resources(false).await?;
        let value = summary::encode(&snapshot.this_mac);
        // An empty value clears the key: nothing is being created here.
        let making = creates::encode(&self.inner.resources.creates.take(), super::now_ms() / 1000);
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let reply = client
            .patch(
                &token,
                &machine,
                &cua_host::MachinePatch {
                    meta: Some(BTreeMap::from([
                        (summary::META_KEY.to_string(), value.clone()),
                        (creates::META_KEY.to_string(), making),
                    ])),
                    ..Default::default()
                },
            )
            .await?;
        if reply.meta.get(summary::META_KEY) != Some(&value) {
            return Ok(Published::Unsupported);
        }
        Ok(Published::Sent {
            watchers: reply.clients.len(),
            spaces: snapshot.this_mac.spaces_count,
        })
    }

    /// How many of this computer's own Spaces the registry lists now (what
    /// the summary counts).
    fn own_space_count(&self) -> u32 {
        let n = self
            .list()
            .unwrap_or_default()
            .iter()
            .filter(|i| i.id.starts_with("local:") && i.host.is_empty())
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// Keeps this computer's summary published until the runtime is
    /// dropped (the daemon starts it once). The task is also a hidden
    /// watcher, which is what keeps the sampler running every five minutes
    /// when no app is open.
    pub fn spawn_resources_publisher(&self) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(&self.inner);
        let mut watch = self.watch_resources(false);
        tokio::spawn(async move {
            let mut next = Instant::now() + FIRST_PUBLISH_DELAY;
            // What the last publish said, until the first one went out.
            let mut published: Option<u32> = None;
            let mut count_at = Instant::now() + COUNT_EVERY;
            loop {
                let Some(hub) = weak.upgrade().map(|i| i.resources.clone()) else {
                    return;
                };
                tokio::select! {
                    event = watch.next() => match event {
                        None => return,
                        // The level changed: say so at once.
                        Some(ResourcesEvent::LowDisk(_)) => next = Instant::now(),
                        Some(_) => {}
                    },
                    // A create started, moved on or ended: at once.
                    _ = hub.creates.changed() => next = Instant::now(),
                    _ = tokio::time::sleep_until(count_at.into()) => {
                        count_at = Instant::now() + COUNT_EVERY;
                        let Some(inner) = weak.upgrade() else { return };
                        let spaces = Spaces { inner };
                        // A Space was added or removed: at once (after the
                        // first publish, which is due anyway).
                        if published.is_some_and(|n| n != spaces.own_space_count()) {
                            next = Instant::now();
                        }
                    }
                    _ = tokio::time::sleep_until(next.into()) => {
                        let Some(inner) = weak.upgrade() else { return };
                        let spaces = Spaces { inner };
                        let wait = match spaces.publish_resources().await {
                            Ok(Published::Sent { watchers, spaces: n }) => {
                                published = Some(n);
                                if watchers > 0 { summary::WATCHED_EVERY } else { summary::IDLE_EVERY }
                            }
                            Ok(Published::Unsupported) => UNSUPPORTED_BACKOFF,
                            Ok(_) => summary::IDLE_EVERY,
                            Err(e) => {
                                tracing::debug!(error = %e, "publishing resources failed");
                                summary::IDLE_EVERY
                            }
                        };
                        // While a create runs, its progress stays fresh.
                        let wait = if hub.creates.busy() { wait.min(creates::HEARTBEAT) } else { wait };
                        next = Instant::now() + wait;
                    }
                }
            }
        })
    }
}
