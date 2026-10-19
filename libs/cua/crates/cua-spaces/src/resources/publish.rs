//! A computer that is registered on the relay publishes its summary
//! ([`summary`]) into its own machine record, so your other computers show
//! it from the machine list they already fetch.
//!
//! Rates: once a minute while someone is connected to the machine
//! ([`summary::WATCHED_EVERY`]; the relay's answer to each publish says who
//! is), every five minutes otherwise ([`summary::IDLE_EVERY`]), and at once
//! when the low-disk level changes. A relay that predates summaries drops
//! the key; the publisher then tries again only hourly.

use super::{ResourcesEvent, summary};
use crate::{Result, Spaces};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long after the daemon starts the first summary goes out (so the
/// first sample has settled).
const FIRST_PUBLISH_DELAY: Duration = Duration::from_secs(20);
/// How long a relay that dropped the summary is left alone.
const UNSUPPORTED_BACKOFF: Duration = Duration::from_secs(3600);

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
        let token = relay.tokens.access_token().await?;
        let client = relay.client().await?;
        let reply = client
            .patch(
                &token,
                &machine,
                &cua_host::MachinePatch {
                    meta: Some(BTreeMap::from([(
                        summary::META_KEY.to_string(),
                        value.clone(),
                    )])),
                    ..Default::default()
                },
            )
            .await?;
        if reply.meta.get(summary::META_KEY) != Some(&value) {
            return Ok(Published::Unsupported);
        }
        Ok(Published::Sent {
            watchers: reply.clients.len(),
        })
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
            loop {
                tokio::select! {
                    event = watch.next() => match event {
                        None => return,
                        // The level changed: say so at once.
                        Some(ResourcesEvent::LowDisk(_)) => next = Instant::now(),
                        Some(_) => {}
                    },
                    _ = tokio::time::sleep_until(next.into()) => {
                        let Some(inner) = weak.upgrade() else { return };
                        let spaces = Spaces { inner };
                        let wait = match spaces.publish_resources().await {
                            Ok(Published::Sent { watchers }) if watchers > 0 => summary::WATCHED_EVERY,
                            Ok(Published::Unsupported) => UNSUPPORTED_BACKOFF,
                            Ok(_) => summary::IDLE_EVERY,
                            Err(e) => {
                                tracing::debug!(error = %e, "publishing resources failed");
                                summary::IDLE_EVERY
                            }
                        };
                        next = Instant::now() + wait;
                    }
                }
            }
        })
    }
}
