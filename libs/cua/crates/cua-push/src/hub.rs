// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The registry and a provider together.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::event::PushEvent;
use crate::provider::{ProviderError, PushProvider};
use crate::registry::{Registry, RegistryError};

/// Events remembered to send each once.
const SEEN_MAX: usize = 2048;

/// What became of one event.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Delivery {
    /// Phones the alert was handed to.
    pub sent: usize,
    /// Phones whose token the provider said is gone (their registration
    /// was removed).
    pub dropped: usize,
    /// Phones the provider could not reach now.
    pub failed: usize,
    /// Why nothing was sent, when nothing was: `duplicate`,
    /// `preferences_off`, `no_phones`, `not_configured`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<&'static str>,
}

/// The events sent lately, as `(account, event id)`: in order, and as a set.
#[derive(Default)]
struct Seen {
    order: VecDeque<(String, String)>,
    set: std::collections::HashSet<(String, String)>,
}

pub struct PushHub {
    pub registry: Registry,
    provider: Arc<dyn PushProvider>,
    seen: Mutex<Seen>,
}

impl PushHub {
    pub fn new(registry: Registry, provider: Arc<dyn PushProvider>) -> PushHub {
        PushHub {
            registry,
            provider,
            seen: Mutex::default(),
        }
    }

    pub fn provider_name(&self) -> &'static str {
        self.provider.name()
    }

    /// Whether `(account, id)` is new; remembers it.
    fn first_time(&self, account: &str, id: &str) -> bool {
        let key = (account.to_string(), id.to_string());
        let mut seen = self.seen.lock().expect("push seen");
        if seen.set.contains(&key) {
            return false;
        }
        seen.set.insert(key.clone());
        seen.order.push_back(key);
        while seen.order.len() > SEEN_MAX {
            if let Some(old) = seen.order.pop_front() {
                seen.set.remove(&old);
            }
        }
        true
    }

    /// Forgets that `(account, id)` was seen, so a send that did not reach
    /// anyone can be tried again.
    fn forget(&self, account: &str, id: &str) {
        let key = (account.to_string(), id.to_string());
        let mut seen = self.seen.lock().expect("push seen");
        if seen.set.remove(&key) {
            seen.order.retain(|k| *k != key);
        }
    }

    /// Sends `event` to the phones of `account` that want it.
    pub async fn notify(&self, account: &str, event: &PushEvent) -> Delivery {
        if !self.registry.preferences(account).wants(event.kind) {
            return Delivery {
                skipped: Some("preferences_off"),
                ..Default::default()
            };
        }
        let phones: Vec<_> = self
            .registry
            .phones(account)
            .into_iter()
            .filter(|p| self.provider.serves(p) && p.takes(event.kind))
            .collect();
        if phones.is_empty() {
            return Delivery {
                skipped: Some("no_phones"),
                ..Default::default()
            };
        }
        if !self.first_time(account, &event.id) {
            return Delivery {
                skipped: Some("duplicate"),
                ..Default::default()
            };
        }
        let payload = event.payload();
        let mut out = Delivery::default();
        let mut not_configured = false;
        for phone in &phones {
            match self.provider.send(phone, &payload).await {
                Ok(()) => out.sent += 1,
                Err(ProviderError::InvalidToken) => {
                    if self
                        .registry
                        .unregister_token(account, &phone.token)
                        .unwrap_or(false)
                    {
                        out.dropped += 1;
                    }
                }
                Err(ProviderError::NotConfigured) => not_configured = true,
                Err(e) => {
                    tracing::warn!(provider = self.provider.name(), error = %e, "push not sent");
                    out.failed += 1;
                }
            }
        }
        if out.sent == 0 {
            // Nobody got it: a retry of the same event may try again.
            self.forget(account, &event.id);
            if not_configured {
                out.skipped = Some("not_configured");
            }
        }
        out
    }
}

/// A registry error as a short code for the wire.
pub fn error_code(e: &RegistryError) -> &'static str {
    match e {
        RegistryError::Invalid(_) => "invalid",
        RegistryError::Limit(_) => "limit",
        RegistryError::NotFound => "not_found",
        RegistryError::Storage(_) => "storage",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use crate::provider::{MockProvider, NoProvider};
    use crate::registry::{Platform, PreferencesPatch};

    fn ev(id: &str, kind: EventKind) -> PushEvent {
        PushEvent {
            id: id.into(),
            kind,
            space: Some("relay:abc".into()),
            entry: Some(3),
            agent: Some("Claude Code".into()),
            scope_choices: vec![],
            at_ms: 0,
        }
    }

    fn tok(n: u8) -> String {
        format!("{:02x}", n).repeat(32)
    }

    fn add(h: &PushHub, acct: &str, n: u8) {
        h.registry
            .register(
                acct,
                &format!("dev_{n:08}"),
                Platform::Ios,
                &tok(n),
                "app",
                false,
                "",
                None,
                "en",
                true,
            )
            .unwrap();
    }

    fn hub(mock: &Arc<MockProvider>) -> PushHub {
        PushHub::new(Registry::in_memory(), mock.clone())
    }

    #[tokio::test]
    async fn an_event_reaches_each_phone_once() {
        let mock = Arc::new(MockProvider::new());
        let h = hub(&mock);
        add(&h, "a", 1);
        add(&h, "a", 2);
        let d = h.notify("a", &ev("n1", EventKind::ApprovalNeeded)).await;
        assert_eq!(d.sent, 2);
        let again = h.notify("a", &ev("n1", EventKind::ApprovalNeeded)).await;
        assert_eq!((again.sent, again.skipped), (0, Some("duplicate")));
        assert_eq!(mock.sent().len(), 2);
        assert_eq!(mock.sent()[0].1.title, "Claude Code needs you");
    }

    #[tokio::test]
    async fn another_account_gets_nothing() {
        let mock = Arc::new(MockProvider::new());
        let h = hub(&mock);
        add(&h, "a", 1);
        let d = h.notify("b", &ev("n1", EventKind::TaskFailed)).await;
        assert_eq!(d.skipped, Some("no_phones"));
        assert!(mock.sent().is_empty());
    }

    #[tokio::test]
    async fn preferences_turn_kinds_and_everything_off() {
        let mock = Arc::new(MockProvider::new());
        let h = hub(&mock);
        add(&h, "a", 1);
        h.registry
            .set_preferences(
                "a",
                &PreferencesPatch {
                    task_finished: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            h.notify("a", &ev("n1", EventKind::TaskFinished))
                .await
                .skipped,
            Some("preferences_off")
        );
        assert_eq!(
            h.notify("a", &ev("n2", EventKind::TaskFailed)).await.sent,
            1
        );
        h.registry
            .set_preferences(
                "a",
                &PreferencesPatch {
                    enabled: Some(false),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            h.notify("a", &ev("n3", EventKind::ApprovalNeeded))
                .await
                .skipped,
            Some("preferences_off")
        );
        assert_eq!(mock.sent().len(), 1);
    }

    #[tokio::test]
    async fn a_dead_token_is_dropped_and_a_busy_provider_can_be_retried() {
        let mock = Arc::new(MockProvider::new());
        let h = hub(&mock);
        add(&h, "a", 1);
        add(&h, "a", 2);
        mock.fail_token(&tok(1), ProviderError::InvalidToken);
        let d = h.notify("a", &ev("n1", EventKind::AgentBlocked)).await;
        assert_eq!((d.sent, d.dropped), (1, 1));
        assert_eq!(h.registry.phones("a").len(), 1);

        let mock = Arc::new(MockProvider::new());
        let h = hub(&mock);
        add(&h, "a", 5);
        mock.fail_token(&tok(5), ProviderError::Transient("503".into()));
        let d = h.notify("a", &ev("n1", EventKind::TaskFailed)).await;
        assert_eq!((d.sent, d.failed), (0, 1));
        // The same event is tried again when the daemon reports it again.
        let d = h.notify("a", &ev("n1", EventKind::TaskFailed)).await;
        assert_eq!(d.failed, 1);
        assert_ne!(d.skipped, Some("duplicate"));
    }

    #[tokio::test]
    async fn without_a_provider_registrations_are_kept_and_the_event_says_so() {
        let h = PushHub::new(Registry::in_memory(), Arc::new(NoProvider));
        add(&h, "a", 1);
        let d = h.notify("a", &ev("n1", EventKind::TaskFinished)).await;
        assert_eq!(d.skipped, Some("not_configured"));
        assert_eq!(h.registry.phones("a").len(), 1);
        assert_eq!(h.provider_name(), "none");
    }
}
