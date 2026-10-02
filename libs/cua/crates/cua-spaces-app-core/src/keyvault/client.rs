// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Keyvault page's broker client (moved from the Tauri app's
//! `src-tauri/src/keyvault.rs` so every shell runs the same one).
//!
//! The shells are first-party clients of the broker `cua daemon` hosts on
//! `$CUA_HOME/keyvault.sock`, and nothing more. Every operation is an
//! existing broker request (`cua_keyvault::ipc::Request`). No shell captures
//! credentials, reads secret values (the broker has no "reveal"), or runs its
//! own Touch ID prompt for these actions: the daemon asks for user presence
//! itself, so a click alone can never widen access.
//!
//! Identity is the kernel's: the broker checks the calling process's code
//! signature. A build that is not signed by Cua is not first party and the
//! broker refuses to list items to it; the page says so.

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use cua_keyvault::Zeroizing;
use cua_keyvault::broker::{
    ApproveOptions, InitRequest, Inventory, ItemPage, LockOutcome, Status, UnlockRequest,
};
use cua_keyvault::ipc::{Request, VerificationView};
use cua_keyvault::model::Grant;
use serde::Serialize;
use serde_json::Value;

use super::wire::{KeyvaultOverview, KvCommand, KvFavicon, KvGrant, KvInventory, KvItem};

/// How many audit entries the page reads.
pub const AUDIT_TAIL: usize = 200;

/// A failure talking to the broker: the broker's error code
/// (`cua_keyvault::ipc::error_code`) or a transport code (`not_running`,
/// `impostor`, `connect`, `unsupported`, `internal`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KvFailure {
    /// Code.
    pub code: String,
    /// Message.
    pub message: String,
}

impl KvFailure {
    /// From a broker error. The message is the sentence the page shows:
    /// the broker's words without the error kind in front ("invalid: ...").
    pub fn from_error(e: &cua_keyvault::Error) -> Self {
        use cua_keyvault::Error as E;
        let text = match e {
            E::Invalid(m)
            | E::Forbidden(m)
            | E::Unsupported(m)
            | E::PresenceFailed(m)
            | E::Denied(m)
            | E::NotFound(m)
            | E::Capability(m)
            | E::RateLimited(m) => m.clone(),
            other => other.to_string(),
        };
        let mut chars = text.chars();
        let message = match chars.next() {
            Some(c) => c.to_uppercase().chain(chars).collect(),
            None => text,
        };
        Self {
            code: cua_keyvault::ipc::error_code(e).to_string(),
            message,
        }
    }
}

impl std::fmt::Display for KvFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for KvFailure {}

/// One request/response exchange with the broker.
#[async_trait::async_trait]
pub trait KvTransport: Send + Sync {
    /// Sends `req` and returns the broker's result.
    async fn call(&self, req: Request) -> Result<Value, KvFailure>;
    /// Whether the server was verified as Cua-signed at connect.
    fn server_verified(&self) -> bool;
}

/// The production transport: a fresh verified connection per request.
pub struct SocketTransport {
    path: PathBuf,
}

impl SocketTransport {
    /// The socket in a Cua home (`$CUA_HOME/keyvault.sock`).
    pub fn in_cua_home(cua_home: &Path) -> Self {
        Self {
            path: cua_home.join("keyvault.sock"),
        }
    }

    /// A socket at `path`.
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait::async_trait]
impl KvTransport for SocketTransport {
    async fn call(&self, req: Request) -> Result<Value, KvFailure> {
        #[cfg(unix)]
        {
            use cua_keyvault::client::{ConnectError, KeyvaultClient, ServerCheck};
            let mut client = KeyvaultClient::connect(&self.path, ServerCheck::default_for_build())
                .await
                .map_err(|e| match e {
                    ConnectError::NotRunning(_) => KvFailure {
                        code: "not_running".into(),
                        message: "The Cua daemon is not running, so the Keyvault is unavailable."
                            .into(),
                    },
                    ConnectError::Impostor { who, .. } => KvFailure {
                        code: "impostor".into(),
                        message: format!(
                            "The process serving the Keyvault ({who}) is not signed by Cua, so \
                             Cua will not talk to it and the Keyvault is off. Use the signed Cua \
                             Spaces app and the cua it ships; local and ad hoc signed builds \
                             cannot use the Keyvault."
                        ),
                    },
                    ConnectError::Other(m) => KvFailure {
                        code: "connect".into(),
                        message: m,
                    },
                })?;
            client
                .call(&req)
                .await
                .map_err(|e| KvFailure::from_error(&e))
        }
        #[cfg(not(unix))]
        {
            let _ = req;
            Err(KvFailure {
                code: "unsupported".into(),
                message: format!(
                    "The Keyvault socket ({}) is not available on this platform yet.",
                    self.path.display()
                ),
            })
        }
    }

    fn server_verified(&self) -> bool {
        #[cfg(unix)]
        {
            matches!(
                cua_keyvault::client::ServerCheck::default_for_build(),
                cua_keyvault::client::ServerCheck::Require(_)
            )
        }
        #[cfg(not(unix))]
        {
            false
        }
    }
}

/// An in-process transport over a broker with a fixed caller: the same
/// dispatch the socket server runs. Tests drive the commands against a real
/// broker without a signed binary.
pub struct DirectTransport {
    /// The broker.
    pub broker: Arc<cua_keyvault::Broker>,
    /// The caller it sees.
    pub caller: cua_keyvault::CallerIdentity,
}

#[async_trait::async_trait]
impl KvTransport for DirectTransport {
    async fn call(&self, req: Request) -> Result<Value, KvFailure> {
        let resp = cua_keyvault::ipc::dispatch(&self.broker, &self.caller, req).await;
        if resp.ok {
            Ok(resp.result.unwrap_or(Value::Null))
        } else {
            let e = resp
                .error
                .map(cua_keyvault::ipc::wire_to_error)
                .unwrap_or_else(|| cua_keyvault::Error::Backend("unknown error".into()));
            Err(KvFailure::from_error(&e))
        }
    }

    fn server_verified(&self) -> bool {
        true
    }
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, KvFailure> {
    serde_json::from_value(v).map_err(|e| KvFailure {
        code: "internal".into(),
        message: format!("unexpected Keyvault reply: {e}"),
    })
}

/// A broker record as the core's mirror type.
fn mirror<T: Serialize, U: serde::de::DeserializeOwned>(v: &T) -> Result<U, KvFailure> {
    decode(serde_json::to_value(v).map_err(|e| KvFailure {
        code: "internal".into(),
        message: e.to_string(),
    })?)
}

/// What a command returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum KvOutcome {
    /// Nothing to show.
    Done,
    /// The recovery key, shown once and never stored.
    RecoveryKey {
        /// Key.
        key: Option<String>,
    },
    /// Items locked or unlocked. Identity providers always ask: they are in
    /// `skipped`, never changed.
    Locked {
        /// Items whose lock changed.
        changed: Vec<String>,
        /// Items left locked (identity providers).
        skipped: Vec<String>,
    },
    /// Items deleted.
    Deleted {
        /// Items deleted.
        count: u32,
        /// Copies wiped in Spaces.
        wiped: Vec<String>,
    },
    /// The browse window is open until then, Unix ms.
    Browsing {
        /// When it closes.
        until_ms: u64,
    },
    /// Grants revoked.
    Revoked {
        /// Count.
        count: u32,
    },
    /// Imports wiped.
    Wiped {
        /// Import ids.
        imports: Vec<String>,
    },
    /// The grant an approval minted.
    Granted {
        /// Grant.
        grant: KvGrant,
    },
}

/// The page commands, over any [`KvTransport`].
#[derive(Clone)]
pub struct KeyvaultCommands {
    transport: Arc<dyn KvTransport>,
}

impl KeyvaultCommands {
    /// Over `transport`.
    pub fn new(transport: Arc<dyn KvTransport>) -> Self {
        Self { transport }
    }

    /// The daemon's socket in `cua_home`.
    pub fn for_cua_home(cua_home: &Path) -> Self {
        Self::new(Arc::new(SocketTransport::in_cua_home(cua_home)))
    }

    async fn typed<T: serde::de::DeserializeOwned>(&self, req: Request) -> Result<T, KvFailure> {
        decode(self.transport.call(req).await?)
    }

    /// Everything the page shows. Never fails: an unavailable Keyvault is a
    /// state of the page, with the broker's reason.
    pub async fn overview(&self) -> KeyvaultOverview {
        let mut out = KeyvaultOverview {
            server_verified: self.transport.server_verified(),
            ..Default::default()
        };
        let status: Status = match self.typed(Request::Status).await {
            Ok(s) => s,
            Err(f) => {
                out.availability = f.code.clone();
                out.message = Some(f.message);
                return out;
            }
        };
        let unavailable = if !status.initialized {
            Some((
                "no_vault",
                "It keeps the sessions teleport moves, encrypted on this computer.".to_string(),
            ))
        } else if !status.caller_first_party {
            Some((
                "not_first_party",
                format!(
                    "The Keyvault only shows items to apps signed by Cua. It sees this app as {}.",
                    status.caller_display
                ),
            ))
        } else if !status.unlocked {
            Some(("locked", "Unlock it to see and approve items.".to_string()))
        } else {
            None
        };
        match mirror(&status) {
            Ok(s) => out.status = Some(s),
            Err(f) => out.partial_errors.push(format!("Status: {}", f.message)),
        }
        if let Some((code, message)) = unavailable {
            out.availability = code.into();
            out.message = Some(message);
            return out;
        }
        match self.items().await {
            Ok((items, names_visible, total)) => {
                out.items = items;
                out.names_visible = names_visible;
                out.items_total = total;
            }
            Err(f) => {
                out.availability = f.code;
                out.message = Some(f.message);
                return out;
            }
        }
        out.availability = "ready".into();
        macro_rules! section {
            ($field:ident, $req:expr, $what:literal) => {
                match self.typed::<Value>($req).await.and_then(decode) {
                    Ok(v) => out.$field = v,
                    Err(f) => out.partial_errors.push(format!("{}: {}", $what, f.message)),
                }
            };
        }
        section!(pending, Request::ListPending, "Pending requests");
        section!(grants, Request::ListGrants, "Grants");
        section!(rules, Request::ListRules, "Unattended rules");
        section!(deliveries, Request::ListDeliveries, "Deliveries");
        section!(
            audit,
            Request::Audit {
                limit: Some(AUDIT_TAIL)
            },
            "Audit log"
        );
        match self
            .typed::<VerificationView>(Request::VerifyAudit)
            .await
            .and_then(|v| mirror(&v))
        {
            Ok(v) => out.audit_verification = Some(v),
            Err(f) => out
                .partial_errors
                .push(format!("Audit verification: {}", f.message)),
        }
        out
    }

    /// Every item, page by page: whether their names are visible, and how
    /// many the vault holds.
    /// Site icons from the vault (empty while the browse window is closed).
    /// Not secret; fetched apart from the overview so views stay small.
    pub async fn favicons(&self) -> Vec<KvFavicon> {
        self.typed::<Vec<KvFavicon>>(Request::ListFavicons)
            .await
            .unwrap_or_default()
    }

    async fn items(&self) -> Result<(Vec<KvItem>, bool, u32), KvFailure> {
        let mut all: Vec<KvItem> = Vec::new();
        loop {
            let page: ItemPage = self
                .typed(Request::ListItems {
                    offset: all.len(),
                    limit: None,
                })
                .await?;
            let visible = page.names_visible;
            let total = page.total as u32;
            let empty = page.items.is_empty();
            for i in &page.items {
                all.push(mirror(i)?);
            }
            if empty || all.len() as u32 >= total {
                return Ok((all, visible, total));
            }
        }
    }

    async fn init(&self, req: InitRequest) -> Result<Option<String>, KvFailure> {
        let v = self.transport.call(Request::Init(req)).await?;
        Ok(v.get("recovery_key")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    /// Creates the vault (the OS key store protector when `os_protector`)
    /// with a recovery key returned once. The daemon asks for presence, and
    /// refuses at once, without asking, when it cannot use the OS key store
    /// (`status.os_protector_available`).
    pub async fn setup(&self, os_protector: bool) -> Result<Option<String>, KvFailure> {
        self.init(InitRequest {
            os_protector,
            passphrase: None,
            recovery_key: true,
        })
        .await
    }

    /// Creates the vault with a passphrase protector and a recovery key
    /// (returned once). The passphrase goes only to the broker, over the
    /// verified socket; it is never logged or kept, and every copy here is
    /// zeroized.
    pub async fn setup_with_passphrase(
        &self,
        passphrase: Zeroizing<String>,
    ) -> Result<Option<String>, KvFailure> {
        self.init(InitRequest {
            os_protector: false,
            passphrase: Some(passphrase.as_str().to_string()),
            recovery_key: true,
        })
        .await
    }

    /// Unlocks with the OS key store protector.
    pub async fn unlock(&self) -> Result<(), KvFailure> {
        self.transport
            .call(Request::Unlock(UnlockRequest::default()))
            .await
            .map(|_| ())
    }

    /// Unlocks with the passphrase (sent only to the broker; zeroized here).
    pub async fn unlock_with_passphrase(
        &self,
        passphrase: Zeroizing<String>,
    ) -> Result<(), KvFailure> {
        self.transport
            .call(Request::Unlock(UnlockRequest {
                passphrase: Some(passphrase.as_str().to_string()),
                recovery_key: None,
            }))
            .await
            .map(|_| ())
    }

    /// Locks the vault.
    pub async fn lock(&self) -> Result<(), KvFailure> {
        self.transport.call(Request::Lock).await.map(|_| ())
    }

    /// The kill switch. On is always allowed and revokes every outstanding
    /// token; off makes the daemon ask for Touch ID or the login password.
    pub async fn set_disabled(&self, disabled: bool) -> Result<(), KvFailure> {
        self.transport
            .call(Request::SetDisabled { disabled })
            .await
            .map(|_| ())
    }

    /// Auto-wipe of delivered copies. On only shortens what new copies
    /// keep; off keeps them until wiped, so the daemon asks for presence.
    pub async fn set_auto_wipe(&self, on: bool) -> Result<(), KvFailure> {
        self.transport
            .call(Request::SetAutoWipe { on })
            .await
            .map(|_| ())
    }

    /// Locks or unlocks `item_ids` together. Locking only narrows. Unlocking
    /// allows unattended access (any agent with the Cua Spaces MCP may have
    /// the item written into a connected Space), so the daemon asks for
    /// presence once for the whole batch. Identity providers always ask and
    /// come back in `skipped`.
    pub async fn set_locked(
        &self,
        item_ids: &[String],
        locked: bool,
    ) -> Result<LockOutcome, KvFailure> {
        if item_ids.is_empty() {
            return Ok(LockOutcome::default());
        }
        self.typed(Request::SetLocked {
            ids: item_ids.to_vec(),
            locked,
        })
        .await
    }

    /// [`Self::set_locked`] under the name the Tauri app's command uses
    /// (`unattended` is "unlocked"). Returns no items: the page re-reads.
    pub async fn set_unattended(
        &self,
        item_ids: &[String],
        unattended: bool,
    ) -> Result<Vec<KvItem>, KvFailure> {
        self.set_locked(item_ids, !unattended).await?;
        Ok(vec![])
    }

    /// Deletes `item_ids` and wipes every live copy of them in Spaces.
    /// Returns the copies wiped.
    pub async fn delete_items(&self, item_ids: &[String]) -> Result<Vec<String>, KvFailure> {
        if item_ids.is_empty() {
            return Ok(vec![]);
        }
        self.typed(Request::DeleteItems {
            ids: item_ids.to_vec(),
        })
        .await
    }

    /// "Never ask again" on the unlock prompt.
    pub async fn set_skip_unlock_prompt(&self, on: bool) -> Result<(), KvFailure> {
        self.transport
            .call(Request::SetSkipUnlockPrompt { on })
            .await
            .map(|_| ())
    }

    /// Opens the browse window (the daemon asks for presence): item names
    /// show for a few minutes. Returns when it closes, Unix ms.
    pub async fn browse(&self) -> Result<u64, KvFailure> {
        let v = self.transport.call(Request::Browse).await?;
        Ok(v.get("browse_until_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0))
    }

    /// Closes the browse window.
    pub async fn end_browse(&self) -> Result<(), KvFailure> {
        self.transport.call(Request::EndBrowse).await.map(|_| ())
    }

    /// What `app` holds, per domain with counts (the daemon asks for
    /// presence when the browse window is closed). Never values.
    pub async fn inventory(
        &self,
        app: &str,
        profile: Option<String>,
    ) -> Result<KvInventory, KvFailure> {
        let inv: Inventory = self
            .typed(Request::Inventory {
                app: app.into(),
                profile,
            })
            .await?;
        mirror(&inv)
    }

    /// Imports a browser's saved passwords (`browser`: `chrome`), one item
    /// per site, `sites` empty for every saved site. The daemon asks for
    /// presence; the passwords stay sealed and are used only to sign in.
    pub async fn import_passwords(
        &self,
        browser: &str,
        profile: Option<String>,
        sites: Vec<String>,
    ) -> Result<cua_keyvault::broker::ImportReport, KvFailure> {
        self.typed(Request::ImportPasswords(
            cua_keyvault::broker::PasswordImportSpec {
                app: browser.into(),
                profile,
                sites,
            },
        ))
        .await
    }

    /// Revokes one grant, or every grant with `*`.
    pub async fn revoke_grant(&self, id: &str) -> Result<usize, KvFailure> {
        self.typed(Request::RevokeGrant { id: id.into() }).await
    }

    /// Removes an unattended rule.
    pub async fn remove_rule(&self, id: &str) -> Result<(), KvFailure> {
        self.transport
            .call(Request::RemoveRule { id: id.into() })
            .await
            .map(|_| ())
    }

    /// Wipes every live delivery on `target`.
    pub async fn release(&self, target: &str) -> Result<Vec<String>, KvFailure> {
        self.typed(Request::Release {
            target: target.into(),
        })
        .await
    }

    /// Approves a pending request for `items` (`None`: as asked). The
    /// daemon asks for presence.
    pub async fn approve(
        &self,
        request_id: &str,
        items: Option<Vec<String>>,
    ) -> Result<KvGrant, KvFailure> {
        let grant: Grant = self
            .typed(Request::Approve {
                request_id: request_id.into(),
                options: ApproveOptions {
                    items,
                    ..ApproveOptions::default()
                },
            })
            .await?;
        mirror(&grant)
    }

    /// Denies a pending request.
    pub async fn deny(&self, request_id: &str) -> Result<(), KvFailure> {
        self.transport
            .call(Request::Deny {
                request_id: request_id.into(),
            })
            .await
            .map(|_| ())
    }

    /// Runs one page action.
    pub async fn execute(&self, command: &KvCommand) -> Result<KvOutcome, KvFailure> {
        Ok(match command {
            KvCommand::Setup => KvOutcome::RecoveryKey {
                key: self
                    .setup(cfg!(any(target_os = "macos", target_os = "windows")))
                    .await?,
            },
            KvCommand::Unlock => {
                self.unlock().await?;
                KvOutcome::Done
            }
            KvCommand::SetDisabled { disabled } => {
                self.set_disabled(*disabled).await?;
                KvOutcome::Done
            }
            KvCommand::SetAutoWipe { on } => {
                self.set_auto_wipe(*on).await?;
                KvOutcome::Done
            }
            KvCommand::SetUnattended {
                item_ids,
                unattended,
            } => {
                let out = self.set_locked(item_ids, !*unattended).await?;
                KvOutcome::Locked {
                    changed: out.changed,
                    skipped: out.skipped,
                }
            }
            KvCommand::SetLocked { item_ids, locked } => {
                let out = self.set_locked(item_ids, *locked).await?;
                KvOutcome::Locked {
                    changed: out.changed,
                    skipped: out.skipped,
                }
            }
            KvCommand::DeleteItems { item_ids } => KvOutcome::Deleted {
                count: item_ids.len() as u32,
                wiped: self.delete_items(item_ids).await?,
            },
            KvCommand::SetSkipUnlockPrompt { on } => {
                self.set_skip_unlock_prompt(*on).await?;
                KvOutcome::Done
            }
            KvCommand::Browse => KvOutcome::Browsing {
                until_ms: self.browse().await?,
            },
            KvCommand::EndBrowse => {
                self.end_browse().await?;
                KvOutcome::Done
            }
            KvCommand::RevokeGrant { id } => KvOutcome::Revoked {
                count: self.revoke_grant(id).await? as u32,
            },
            KvCommand::RemoveRule { id } => {
                self.remove_rule(id).await?;
                KvOutcome::Done
            }
            KvCommand::Release { target } => KvOutcome::Wiped {
                imports: self.release(target).await?,
            },
            KvCommand::Approve { request_id, items } => KvOutcome::Granted {
                grant: self.approve(request_id, items.clone()).await?,
            },
            KvCommand::Deny { request_id } => {
                self.deny(request_id).await?;
                KvOutcome::Done
            }
        })
    }
}
