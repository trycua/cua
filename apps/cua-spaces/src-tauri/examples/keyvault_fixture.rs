// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Fake Keyvault data for docs captures of the Keyvault page. Test tooling
//! only: it never reads a host app, a browser profile or any keychain.
//!
//! ```text
//! keyvault_fixture seed <vault dir> <passphrase>     offline: a passphrase-only vault with fake items
//! keyvault_fixture unlock <socket> <passphrase>       first party: unlock through the daemon
//! keyvault_fixture request <socket> <label> <target> <reason>
//!                                                     any caller: file an access request
//! keyvault_fixture answer <socket> <allow|deny> <n>   first party: answer the n-th pending request
//! keyvault_fixture unattended <socket> <key|site>     first party: unlock an item (allow unattended access)
//! keyvault_fixture use <socket> <label> <target>      third party: request, wait up to 60 s, then
//!                                                     teleport with the granted token
//! ```
//!
//! Every item's payload is the literal bytes `FIXTURE-NOT-A-SECRET`. The
//! daemon it talks to must be a debug build started with
//! `CUA_KEYVAULT_TEST_REQUIREMENT` naming this binary (and the app) as first
//! party, and `CUA_KEYVAULT_ALLOW_UNVERIFIED_DAEMON=1` lets this debug client
//! talk to that unsigned daemon.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cua_keyvault::broker::{AccessRequest, Decision, Selector, TeleportRequest, UnlockRequest};
use cua_keyvault::client::{KeyvaultClient, ServerCheck};
use cua_keyvault::ipc::Request;
use cua_keyvault::model::{ItemMeta, ItemPayload};
use cua_keyvault::protector::PassphraseProtector;
use cua_keyvault::record::{self, CookieRecord};
use cua_keyvault::rollback::FileGenerationAnchor;
use cua_keyvault::Vault;

/// One fake item: a site's session cookie, or (no site) a file of the app.
fn fake(app: &str, site: Option<&str>, name: &str) -> (ItemMeta, ItemPayload) {
    let provider = app.to_ascii_lowercase();
    let new = match site {
        Some(s) => record::cookie_record(&CookieRecord {
            creation_utc: None,
            expires_utc: 0,
            host_key: format!(".{s}"),
            http_only: true,
            last_update_utc: None,
            name: name.into(),
            partition_key: None,
            path: "/".into(),
            priority: None,
            same_site: -1,
            secure: true,
            source_port: None,
            source_scheme: None,
            // "FIXTURE-NOT-A-SECRET"
            value: b"FIXTURE-NOT-A-SECRET".to_vec(),
        })
        .unwrap(),
        None => record::file_record(name, 0o600, b"FIXTURE-NOT-A-SECRET").unwrap(),
    };
    let (mut meta, payload) = new.into_item(&provider, app, "Default", "full");
    meta.identity_provider = site.is_some_and(cua_keyvault::model::is_identity_provider);
    (meta, payload)
}

fn seed(dir: &Path, passphrase: &str) -> Result<(), String> {
    let anchor = dir
        .parent()
        .ok_or("vault dir has no parent")?
        .join("keyvault.generation");
    let p = PassphraseProtector::new(passphrase);
    let mut v = Vault::create(dir, &[&p])
        .and_then(|v| v.with_anchor(Box::new(FileGenerationAnchor::new(anchor))))
        .map_err(|e| e.to_string())?;
    let items = vec![
        fake("Chrome", Some("github.com"), "user_session"),
        fake("Chrome", Some("github.com"), "logged_in"),
        fake("Chrome", Some("linear.app"), "linear_session"),
        fake("Firefox", Some("accounts.google.com"), "SID"),
        fake("Slack", None, "storage/root-state.json"),
    ];
    v.upsert_items(items).map_err(|e| e.to_string())?;
    Ok(())
}

async fn client(sock: &Path) -> Result<KeyvaultClient, String> {
    KeyvaultClient::connect(sock, ServerCheck::default_for_build())
        .await
        .map_err(|e| e.to_string())
}

async fn item_by_label(c: &mut KeyvaultClient, label: &str) -> Result<String, String> {
    // Names show only inside the browse window.
    c.browse().await.map_err(|e| e.to_string())?;
    let page = c.list_items().await.map_err(|e| e.to_string())?;
    page.items
        .into_iter()
        .find(|i| i.key == label || i.domain.as_deref().is_some_and(|d| d.contains(label)))
        .map(|i| i.id)
        .ok_or_else(|| format!("no item matching {label:?}"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).cloned().ok_or(format!("missing argument {i}"));
    match arg(0)?.as_str() {
        "seed" => seed(&PathBuf::from(arg(1)?), &arg(2)?),
        "unlock" => client(Path::new(&arg(1)?))
            .await?
            .call(&Request::Unlock(UnlockRequest {
                passphrase: Some(arg(2)?),
                recovery_key: None,
            }))
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        "request" => {
            // Third parties cannot list items, so the label names a site.
            let mut c = client(Path::new(&arg(1)?)).await?;
            let site = arg(2)?;
            let p = c
                .request_access(AccessRequest {
                    selectors: vec![Selector::Site {
                        app: "chrome".into(),
                        site,
                    }],
                    targets: vec![arg(3)?],
                    reason: arg(4)?,
                    claimed_name: Some("OpenKoalaBots".into()),
                    duration_secs: Some(900),
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            println!("{}", p.id);
            Ok(())
        }
        "answer" => {
            let mut c = client(Path::new(&arg(1)?)).await?;
            let n: usize = arg(3)?.parse().map_err(|_| "n is a number")?;
            let pending = c.list_pending().await.map_err(|e| e.to_string())?;
            let id = pending.get(n).ok_or("no such pending request")?.id.clone();
            if arg(2)? == "allow" {
                c.approve(&id, Default::default())
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            } else {
                c.deny(&id).await.map_err(|e| e.to_string())
            }
        }
        "unattended" => {
            let mut c = client(Path::new(&arg(1)?)).await?;
            let id = item_by_label(&mut c, &arg(2)?).await?;
            // Unlocked means "allow unattended access"; the daemon asks for
            // presence.
            c.set_locked(vec![id], false)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        "use" => {
            let mut c = client(Path::new(&arg(1)?)).await?;
            let p = c
                .request_access(AccessRequest {
                    selectors: vec![Selector::Site {
                        app: "chrome".into(),
                        site: arg(2)?,
                    }],
                    targets: vec![arg(3)?],
                    reason: "fixture".into(),
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            println!("{}", p.id);
            let mut decision = Decision::Pending;
            for _ in 0..30 {
                decision = c
                    .await_decision(&p.id, Duration::from_secs(2))
                    .await
                    .map_err(|e| e.to_string())?;
                if !matches!(decision, Decision::Pending) {
                    break;
                }
            }
            match decision {
                Decision::Granted { token, items, .. } => {
                    // Aim the token at a different Space than the grant names:
                    // the broker refuses it, and the audit log records that.
                    let out = c
                        .teleport(TeleportRequest {
                            token: Some(token),
                            items,
                            target: arg(4).unwrap_or_else(|_| "another-space".into()),
                        })
                        .await;
                    println!("{out:?}");
                    Ok(())
                }
                other => Err(format!("{other:?}")),
            }
        }
        other => Err(format!("unknown command {other}")),
    }
}
