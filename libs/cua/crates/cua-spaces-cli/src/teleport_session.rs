// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault broker behind the MCP `teleport_browser_session` tool (see
//! `cua_cli::teleport_session` for the tool and its contract): a fresh IPC
//! connection per call to the Keyvault the daemon hosts.

use std::time::Duration;

use cua_cli::teleport_session::{
    AccessRequest, Broker, BrokerError, Decision, GrantedItem, requires_cua_app,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// The real Keyvault broker (unix): a fresh IPC connection per call. The grant
// and its capability token are bound to the caller's verified fingerprint, not
// to a connection, so reconnecting is sound.
// ---------------------------------------------------------------------------

/// The Keyvault IPC broker: converts between the tool's own vocabulary and
/// `cua_keyvault::broker::*`, and never mints consent itself.
pub struct KeyvaultBroker;

#[async_trait::async_trait]
impl Broker for KeyvaultBroker {
    async fn request_access(&self, request: &AccessRequest) -> Result<String, BrokerError> {
        let mut client = connect().await?;
        let pending = client
            .request_access(to_kv_access_request(request))
            .await
            .map_err(kv_error)?;
        Ok(pending.id)
    }

    async fn await_decision(&self, id: &str, timeout: Duration) -> Result<Decision, BrokerError> {
        let mut client = connect().await?;
        let decision = client.await_decision(id, timeout).await.map_err(kv_error)?;
        // A grant returns only item ids. Pair each with its registrable domain
        // from the vault metadata so `complete` can keep delivering only the
        // requested sites; unknown ids fall back to the id (fail closed: they
        // will not match any requested site).
        let site_of = match &decision {
            cua_keyvault::broker::Decision::Granted { .. } => {
                // Names (domains) are only listed inside the browse window,
                // which the daemon opens with Touch ID.
                client.browse().await.map_err(kv_error)?;
                let page = client.list_items().await.map_err(kv_error)?;
                page.items
                    .into_iter()
                    .filter_map(|m| {
                        m.domain
                            .as_deref()
                            .map(cua_keyvault::record::site_of)
                            .map(|s| (m.id, s))
                    })
                    .collect()
            }
            _ => std::collections::HashMap::new(),
        };
        Ok(cli_decision(decision, &site_of))
    }

    async fn teleport(&self, token: &str, item: &str, target: &str) -> Result<Value, BrokerError> {
        let mut client = connect().await?;
        let outcome = client
            .teleport(cua_keyvault::broker::TeleportRequest {
                token: Some(token.to_string()),
                items: vec![item.to_string()],
                target: target.to_string(),
                include_passwords: false,
                launch: false,
            })
            .await
            .map_err(kv_error)?;
        Ok(serde_json::to_value(outcome).unwrap_or(Value::Null))
    }
}

/// Connects to the default Keyvault socket; any connect failure means there is
/// no reachable, verified Keyvault, so the tool asks for the Cua app.
async fn connect() -> Result<cua_keyvault::client::KeyvaultClient, BrokerError> {
    cua_keyvault::client::KeyvaultClient::connect_default()
        .await
        .map_err(connect_error)
}

/// A [`SiteSelector`] request becomes a Keyvault `Site`-selector request for the
/// teleport action, asking for one use per site (each site is one teleport).
fn to_kv_access_request(request: &AccessRequest) -> cua_keyvault::broker::AccessRequest {
    cua_keyvault::broker::AccessRequest {
        selectors: request
            .selectors
            .iter()
            .map(|s| cua_keyvault::broker::Selector::Site {
                app: s.app.clone(),
                site: s.site.clone(),
            })
            .collect(),
        targets: request.targets.clone(),
        actions: vec![cua_keyvault::capability::Action::Teleport],
        duration_secs: Some(request.duration.as_secs()),
        uses: Some(request.selectors.len() as u32),
        reason: request.reason.clone(),
        claimed_name: None,
        agent: None,
    }
}

/// Converts a Keyvault decision into the tool's own [`Decision`], pairing each
/// granted item id with its site from `site_of`.
fn cli_decision(
    decision: cua_keyvault::broker::Decision,
    site_of: &std::collections::HashMap<String, String>,
) -> Decision {
    match decision {
        cua_keyvault::broker::Decision::Pending => Decision::Pending,
        cua_keyvault::broker::Decision::Denied { reason } => Decision::Denied(reason),
        cua_keyvault::broker::Decision::Granted { token, items, .. } => Decision::Granted {
            token,
            items: items
                .into_iter()
                .map(|id| {
                    let site = site_of.get(&id).cloned().unwrap_or_else(|| id.clone());
                    GrantedItem { id, site }
                })
                .collect(),
        },
    }
}

/// Every connect failure (nothing listening, an impostor, or any other error)
/// means no reachable/verified Keyvault: ask for the Cua app.
fn connect_error(_e: cua_keyvault::client::ConnectError) -> BrokerError {
    requires_cua_app()
}

/// A live Keyvault returned an error: surface its wire code and message.
fn kv_error(e: cua_keyvault::Error) -> BrokerError {
    BrokerError::Keyvault {
        code: cua_keyvault::ipc::error_code(&e).to_string(),
        message: e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_cli::teleport_session::{DEFAULT_DURATION, SiteSelector, complete};
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        requests: Mutex<Vec<AccessRequest>>,
        decision: Mutex<Option<Decision>>,
        delivered: Mutex<Vec<(String, String, String)>>,
    }

    #[async_trait::async_trait]
    impl Broker for Fake {
        async fn request_access(&self, r: &AccessRequest) -> Result<String, BrokerError> {
            self.requests.lock().unwrap().push(r.clone());
            Ok("req-1".into())
        }
        async fn await_decision(&self, id: &str, _: Duration) -> Result<Decision, BrokerError> {
            assert_eq!(id, "req-1");
            Ok(self
                .decision
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Decision::Pending))
        }
        async fn teleport(
            &self,
            token: &str,
            item: &str,
            target: &str,
        ) -> Result<Value, BrokerError> {
            self.delivered
                .lock()
                .unwrap()
                .push((token.into(), item.into(), target.into()));
            Ok(json!({"import_id": format!("imp-{item}")}))
        }
    }

    /// The Keyvault conversion layer (unix): a keyvault-style granted decision
    /// pairs each item id with its site, and `complete` then teleports exactly
    /// once per requested site and never for an unrequested one.
    #[tokio::test]
    async fn keyvault_grant_delivers_only_requested_sites() {
        use std::collections::HashMap;
        // The vault knows both items' sites, but only github.com was requested.
        let site_of = HashMap::from([
            ("i-gh".to_string(), "github.com".to_string()),
            ("i-x".to_string(), "example.org".to_string()),
        ]);
        let kv = cua_keyvault::broker::Decision::Granted {
            token: "cuakv1.tok".into(),
            grant_id: "g-1".into(),
            items: vec!["i-gh".into(), "i-x".into()],
            targets: vec!["local:web".into()],
            not_after_ms: 0,
        };
        // Conversion pairs id -> site.
        let converted = cli_decision(kv, &site_of);
        assert_eq!(
            converted,
            Decision::Granted {
                token: "cuakv1.tok".into(),
                items: vec![
                    GrantedItem {
                        id: "i-gh".into(),
                        site: "github.com".into()
                    },
                    GrantedItem {
                        id: "i-x".into(),
                        site: "example.org".into()
                    },
                ],
            }
        );
        // Run it through `complete` with only github.com requested.
        let fake = Fake::default();
        *fake.decision.lock().unwrap() = Some(converted);
        let requested = vec!["github.com".to_string()];
        let done = complete(&fake, "local:web", "chrome", &requested, "req-1")
            .await
            .unwrap();
        assert_eq!(done["moved"], true);
        // Exactly one teleport, for the github.com item, none for example.org.
        assert_eq!(
            *fake.delivered.lock().unwrap(),
            [(
                "cuakv1.tok".to_string(),
                "i-gh".to_string(),
                "local:web".to_string()
            )]
        );
        assert!(!done.to_string().contains("cuakv1"), "token never leaves");
    }

    /// A connect failure maps to `requires_cua_app`, and a live-Keyvault error
    /// keeps its wire code.
    #[test]
    fn connect_and_keyvault_errors_map() {
        use cua_keyvault::client::ConnectError;
        for e in [
            ConnectError::NotRunning("/x/keyvault.sock".into()),
            ConnectError::Impostor {
                path: "/x/keyvault.sock".into(),
                who: "some other process".into(),
            },
            ConnectError::Other("boom".into()),
        ] {
            let mapped = connect_error(e);
            assert_eq!(mapped.kind(), "requires_cua_app");
            assert!(mapped.message().contains("https://cua.ai/download"));
        }
        match kv_error(cua_keyvault::Error::Denied("the user declined".into())) {
            BrokerError::Keyvault { code, .. } => assert_eq!(code, "denied"),
            other => panic!("expected a keyvault error, got {other:?}"),
        }
    }

    /// The tool's site selectors become Keyvault `Site` selectors for the
    /// teleport action, asking for one use per site.
    #[test]
    fn access_request_conversion() {
        let req = AccessRequest {
            selectors: vec![
                SiteSelector {
                    app: "chrome".into(),
                    site: "github.com".into(),
                },
                SiteSelector {
                    app: "chrome".into(),
                    site: "news.ycombinator.com".into(),
                },
            ],
            targets: vec!["local:web".into()],
            duration: DEFAULT_DURATION,
            reason: "log in".into(),
        };
        let kv = to_kv_access_request(&req);
        assert_eq!(kv.targets, ["local:web"]);
        assert_eq!(kv.uses, Some(2));
        assert_eq!(kv.duration_secs, Some(DEFAULT_DURATION.as_secs()));
        assert_eq!(kv.actions, vec![cua_keyvault::capability::Action::Teleport]);
        assert_eq!(
            kv.selectors,
            vec![
                cua_keyvault::broker::Selector::Site {
                    app: "chrome".into(),
                    site: "github.com".into(),
                },
                cua_keyvault::broker::Selector::Site {
                    app: "chrome".into(),
                    site: "news.ycombinator.com".into(),
                },
            ]
        );
    }
}
