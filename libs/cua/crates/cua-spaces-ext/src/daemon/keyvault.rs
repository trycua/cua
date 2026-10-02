// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Cua Keyvault, hosted in the daemon's own hardened process.
//!
//! The daemon is the one owner of teleport (design.md section 0). This module
//! wires the merged [`cua_keyvault`] broker into the running daemon:
//!
//! - [`DaemonBackend`] is the broker's capture and delivery [`Backend`]. It
//!   reads host apps through the `cua-teleport` providers and delivers over
//!   the daemon's already authenticated spacesd channel
//!   (`TeleportService.ImportSession` with a TTL, and `WipeImport` on
//!   release). It resolves a target Space name to its immutable id so a grant
//!   binds to the id, not the name (red-team F1).
//! - [`Keyvault`] holds the [`Broker`] and serves `$CUA_HOME/keyvault.sock`
//!   for external first-party clients (the SDK and the CLI).
//! - [`McpSessionBroker`] adapts the in-process broker to the
//!   [`cua_spaces::teleport_broker::SessionBroker`] seam the daemon-hosted
//!   `teleport_app` MCP tool calls. The MCP caller is an unverified third
//!   party: it always needs a live consent (Touch ID) or a matching
//!   unattended rule, and the broker (never the tool) performs the delivery.
//!
//! Nothing here mints its own approval or ever returns raw secret values.

use std::io::Cursor;
use std::sync::Arc;

use crate::teleport::{AppSessions, TeleportScope};
use base64::Engine as _;
use cua_keyvault::broker::{
    AccessRequest, Backend, Broker, BrokerConfig, Captured, CookieFilter, DeliveryOutcome,
    ImportSpec, Inventory, LoginFill, LoginFilled, PasswordImportSpec, Selector, TeleportRequest,
    TeleportStage, UserPresence,
};
use cua_keyvault::caller::{CallerIdentity, Signing};
use cua_keyvault::model::{
    ItemKind, ItemMeta, ItemPayload, ItemPolicy, ItemSummary, LoginRecord, PayloadEntry,
};
use cua_keyvault::{Error as KvError, Result as KvResult};
use cua_spaces::Spaces;
use cua_spacesd_client::pb;
use cua_teleport::bundle::{BundleReader, BundleWriter};
use cua_teleport::{BundleSource, SendOptions, TransferScope};
// The reserved `cookies.json` bundle entry: decrypted cookies ride sealed in
// the vault payload exactly like this, then travel unchanged into the
// delivered bundle (never stripped like `LOGINS_ENTRY`, since cookies ARE
// what restores the signed-in session -- the receiver's Chrome importer
// re-encrypts them under the DESTINATION's own Safe Storage key).
use cua_teleport_bundle::cookies::{COOKIES_ENTRY, CookieItem};
use sha2::{Digest, Sha256};

/// Base64 for [`PayloadEntry`] bytes (values stay sealed in the vault; only
/// their base64 form crosses the in-process Backend boundary).
const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

fn backend_err(context: &str, e: impl std::fmt::Display) -> KvError {
    KvError::Backend(format!("{context}: {e}"))
}

fn scope_str(scope: TransferScope) -> &'static str {
    match scope {
        TransferScope::TabsOnly => "tabs",
        TransferScope::FullProfile => "full",
    }
}

fn scope_of(s: &str) -> TransferScope {
    match s {
        "tabs" => TransferScope::TabsOnly,
        _ => TransferScope::FullProfile,
    }
}

/// The broker's capture and delivery backend, backed by the daemon's Spaces
/// registry and teleport providers.
pub struct DaemonBackend {
    spaces: Spaces,
    sessions: Arc<AppSessions>,
    /// The host the saved-password reader acts on (the Keychain or
    /// libsecret read, the home directory): the real host in production, a
    /// `FakeHost` in tests.
    host: Arc<dyn cua_teleport::HostEffects>,
    /// Decrypt saved passwords with this platform's scheme (default: this
    /// machine's).
    platform: cua_teleport::Platform,
}

impl DaemonBackend {
    /// A backend over the daemon's Spaces registry and teleport providers.
    pub fn new(spaces: Spaces, sessions: Arc<AppSessions>) -> Self {
        Self {
            spaces,
            sessions,
            host: cua_teleport::default_host(),
            platform: cua_teleport::Platform::current(),
        }
    }

    /// Reads saved passwords through `host` instead of the real machine.
    pub fn with_host(mut self, host: Arc<dyn cua_teleport::HostEffects>) -> Self {
        self.host = host;
        self
    }

    /// Decrypts saved passwords with `platform`'s scheme.
    pub fn with_platform(mut self, platform: cua_teleport::Platform) -> Self {
        self.platform = platform;
        self
    }

    fn password_reader(&self, profile: Option<String>) -> cua_teleport::passwords::ChromePasswords {
        cua_teleport::passwords::ChromePasswords::new(self.host.clone())
            .with_platform(self.platform)
            .with_profile(profile)
    }

    fn cookie_reader(
        &self,
        profile: Option<String>,
    ) -> cua_teleport::browser_cookies::ChromeCookies {
        cua_teleport::browser_cookies::ChromeCookies::new(self.host.clone())
            .with_platform(self.platform)
            .with_profile(profile)
    }

    /// The immutable id a target name resolves to right now (sync: the
    /// registry read needs no network). Returns the canonical Space id, which
    /// a rename cannot forge.
    fn immutable_id(&self, name: &str) -> KvResult<String> {
        let list = self
            .spaces
            .list()
            .map_err(|e| backend_err("list Spaces", e))?;
        // An exact canonical-id match wins (already the immutable id).
        if let Some(s) = list.iter().find(|s| s.id == name) {
            return Ok(s.id.clone());
        }
        let mut by_name = list.iter().filter(|s| s.name == name);
        match (by_name.next(), by_name.next()) {
            (Some(one), None) => Ok(one.id.clone()),
            (None, _) => Err(KvError::NotFound(format!("Space {name:?}"))),
            (Some(_), Some(_)) => Err(KvError::Invalid(format!(
                "the Space name {name:?} is ambiguous; use its id"
            ))),
        }
    }

    /// Connects to a target Space, verifying its immutable id still matches
    /// the pin (defense in depth for F1: the broker already checks the id it
    /// resolved at approval, and delivery re-verifies here).
    async fn open_target(&self, name: &str) -> KvResult<cua_spaces::Space> {
        let pinned = self.immutable_id(name)?;
        let space = self
            .spaces
            .space(name)
            .await
            .map_err(|e| backend_err("open Space", e))?;
        if space.id().to_string() != pinned {
            return Err(KvError::Forbidden(format!(
                "target {name:?} now resolves to a different Space; refusing (rebinding)"
            )));
        }
        Ok(space)
    }

    /// Exports `app`'s selection into vault payload entries: `override_paths`
    /// exactly (any provider, e.g. a direct teleport's own consent-screen
    /// selection routed through [`Backend::capture`] via
    /// [`cua_keyvault::broker::ImportSpec::paths`]) when given, else the
    /// existing login-only default. The provider filters at the source;
    /// values never leave sealed form.
    fn export_entries(
        &self,
        app: &str,
        override_paths: Option<&[String]>,
    ) -> KvResult<(String, Vec<PayloadEntry>)> {
        let manifest = self
            .sessions
            .manifest(app, TeleportScope::Full)
            .map_err(|e| backend_err("read manifest", e))?;
        let paths: Vec<String> = if let Some(p) = override_paths {
            p.to_vec()
        } else {
            // The smallest teleport that still leaves an in-Space agent signed
            // in; never the whole profile by default (design 5.7).
            let mut paths: Vec<String> = manifest
                .login_only_selection()
                .into_iter()
                .map(|i| i.relative_path)
                .collect();
            if paths.is_empty() {
                paths = manifest
                    .items
                    .iter()
                    .map(|i| i.relative_path.clone())
                    .collect();
            }
            paths
        };
        let approval = manifest
            .approving("vault-import", &paths, true)
            .map_err(|e| backend_err("mint approval", e))?;
        let mut buf: Vec<u8> = Vec::new();
        self.sessions
            .export(&approval, &mut buf)
            .map_err(|e| backend_err("export session", e))?;
        let reader =
            BundleReader::open(Cursor::new(buf)).map_err(|e| backend_err("open bundle", e))?;
        let scope = scope_str(reader.header().scope).to_string();
        let entries = reader
            .read_all()
            .map_err(|e| backend_err("read bundle", e))?
            .into_iter()
            .map(|e| PayloadEntry {
                rel_path: e.rel_path,
                mode: e.mode,
                data: B64.encode(&e.bytes),
            })
            .collect();
        Ok((scope, entries))
    }
}

impl DaemonBackend {
    /// The decrypted saved logins of one site.
    fn site_logins(
        &self,
        app: &str,
        profile: Option<String>,
        site: &str,
    ) -> KvResult<Vec<LoginRecord>> {
        if app != "chrome" {
            return Err(KvError::Unsupported(format!(
                "saved passwords from {app} are not read yet"
            )));
        }
        Ok(self
            .password_reader(profile)
            .read(&[site.to_string()])
            .map_err(|e| backend_err("read saved passwords", e))?
            .into_iter()
            .map(|l| LoginRecord {
                origin: l.origin.clone(),
                username: l.username.clone(),
                password: l.password.to_string(),
            })
            .collect())
    }

    /// The decrypted cookies of one site, minimized per `filter`. `None`
    /// (never an empty `Ok(vec![])` for a read that simply found nothing to
    /// keep) only when the app's cookies are not readable yet.
    fn site_cookies(
        &self,
        app: &str,
        profile: Option<String>,
        site: &str,
        filter: &CookieFilter,
    ) -> KvResult<Vec<CookieItem>> {
        if app != "chrome" {
            return Err(KvError::Unsupported(format!(
                "cookies from {app} are not read yet"
            )));
        }
        let cookies = self
            .cookie_reader(profile)
            .read(&[site.to_string()])
            .map_err(|e| backend_err("read cookies", e))?;
        // 30 days, in Chrome's own `*_utc` unit (microseconds since
        // 1601-01-01 UTC): the same "long-lived" cutoff a browser's own
        // "clear cookies older than" setting uses.
        const THIRTY_DAYS_CHROME_MICROS: i64 = 30 * 24 * 60 * 60 * 1_000_000;
        let now = chrome_epoch_now_micros();
        Ok(cookies
            .into_iter()
            .filter(|c| {
                let is_session = c.expires_utc == 0;
                if filter.session_only && !is_session {
                    return false;
                }
                if filter.drop_long_lived
                    && !is_session
                    && c.expires_utc > now + THIRTY_DAYS_CHROME_MICROS
                {
                    return false;
                }
                true
            })
            .map(|c| CookieItem {
                host_key: c.host_key,
                name: c.name,
                value: c.value.as_bytes().to_vec(),
                path: c.path,
                expires_utc: c.expires_utc,
                is_secure: c.is_secure,
                is_httponly: c.is_httponly,
                samesite: c.samesite,
            })
            .collect())
    }
}

/// Microseconds since the Windows/Chrome epoch (1601-01-01 UTC) for the
/// current time, the unit every Chrome `*_utc` cookie column uses.
fn chrome_epoch_now_micros() -> i64 {
    const UNIX_TO_CHROME_EPOCH_MICROS: i64 = 11_644_473_600_000_000;
    let unix_micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    unix_micros + UNIX_TO_CHROME_EPOCH_MICROS
}

/// Milliseconds between the Chrome/Windows epoch (1601-01-01 UTC) and the
/// Unix epoch -- subtract this from `expires_utc / 1000` (Chrome's own unit,
/// converted to milliseconds) to get [`CookieInfo::expires_ms`] (Unix ms).
const CHROME_EPOCH_OFFSET_MS: i64 = 11_644_473_600_000;

/// A sealed `cookies.json` payload entry for `items` (see
/// [`cua_teleport_bundle::cookies`]): decrypted cookies, base64-encoded like
/// every other [`PayloadEntry`], owner-only mode. The vault seals the whole
/// payload at rest; this is only the wire shape.
fn cookies_payload_entry(items: &[CookieItem]) -> PayloadEntry {
    PayloadEntry {
        rel_path: COOKIES_ENTRY.to_string(),
        mode: 0o600,
        data: B64.encode(cua_teleport_bundle::cookies::serialize(items)),
    }
}

fn browser_meta(app: &str, site: &str, passwords: bool) -> ItemMeta {
    ItemMeta {
        id: String::new(),
        kind: if passwords {
            ItemKind::SitePasswords
        } else {
            ItemKind::BrowserSite
        },
        label: format!("{site} ({app})"),
        provider_id: app.into(),
        app_display: app.into(),
        site: Some(site.into()),
        account: None,
        source: "default".into(),
        summary: ItemSummary::default(),
        warnings: vec![],
        identity_provider: false,
        policy: ItemPolicy::default(),
        created_ms: 0,
        updated_ms: 0,
        rev: 0,
        record_digest: String::new(),
    }
}

fn app_meta(app: &str) -> ItemMeta {
    ItemMeta {
        id: String::new(),
        kind: ItemKind::AppSession,
        label: format!("{app} (whole app session)"),
        provider_id: app.into(),
        app_display: app.into(),
        site: None,
        account: None,
        source: "default".into(),
        summary: ItemSummary::default(),
        warnings: vec![],
        identity_provider: false,
        policy: ItemPolicy::default(),
        created_ms: 0,
        updated_ms: 0,
        rev: 0,
        record_digest: String::new(),
    }
}

#[async_trait::async_trait]
impl Backend for DaemonBackend {
    fn resolve_target(&self, name: &str) -> KvResult<String> {
        self.immutable_id(name)
    }

    fn inventory(&self, app: &str, _profile: Option<&str>) -> KvResult<Inventory> {
        let manifest = self
            .sessions
            .manifest(app, TeleportScope::Full)
            .map_err(|e| backend_err("read manifest", e))?;
        Ok(Inventory {
            provider_id: manifest.app.clone(),
            app_display: manifest.display_name.clone(),
            profile: None,
            candidates: vec![],
            notes: manifest.notes,
        })
    }

    fn capture(&self, spec: &ImportSpec) -> KvResult<Vec<Captured>> {
        let (scope, entries) = self.export_entries(&spec.app, spec.paths.as_deref())?;
        let payload = |kind_entries: &[PayloadEntry]| ItemPayload {
            provider_id: spec.app.clone(),
            scope: scope.clone(),
            entries: kind_entries.to_vec(),
        };
        let mut out = Vec::new();
        for s in &spec.sites {
            let mut meta = browser_meta(&spec.app, &s.site, false);
            let mut site_entries = entries.clone();
            // Cookies are the whole point of a `BrowserSite` item (model.rs:
            // "cookies, and optionally its storage"), so they ride in the
            // SAME captured item as the rest of the site's profile state,
            // not a separate sealed-only entry like saved passwords below --
            // teleport delivers them (that is how the destination arrives
            // signed in), it just never delivers them as the raw
            // Safe-Storage-encrypted `Cookies` file: they travel already
            // decrypted, in the reserved `cookies.json` entry, and the
            // receiver re-encrypts under the DESTINATION's own key.
            match self.site_cookies(&spec.app, spec.profile.clone(), &s.site, &spec.cookies) {
                Ok(cookies) => {
                    meta.summary.cookies = cookies
                        .iter()
                        .map(|c| cua_keyvault::model::CookieInfo {
                            name: c.name.clone(),
                            domain: c.host_key.clone(),
                            session: c.expires_utc == 0,
                            // `expires_utc` is Chrome's own epoch
                            // (microseconds since 1601-01-01 UTC);
                            // `CookieInfo::expires_ms` is Unix ms.
                            expires_ms: (c.expires_utc != 0)
                                .then_some(c.expires_utc / 1000 - CHROME_EPOCH_OFFSET_MS),
                        })
                        .collect();
                    if !cookies.is_empty() {
                        site_entries.push(cookies_payload_entry(&cookies));
                    }
                }
                Err(e) => meta
                    .warnings
                    .push(format!("cookies were not read for this site: {e}")),
            }
            out.push(Captured {
                meta,
                payload: payload(&site_entries),
            });
            if s.include_passwords && spec.confirm_passwords {
                let mut meta = browser_meta(&spec.app, &s.site, true);
                let mut entries = entries.clone();
                // The decrypted logins ride along sealed, for site login;
                // teleport never delivers them.
                match self.site_logins(&spec.app, spec.profile.clone(), &s.site) {
                    Ok(logins) => {
                        meta.summary.passwords = logins.len() as u32;
                        entries.push(ItemPayload::logins_entry(&logins)?);
                    }
                    Err(e) => meta
                        .warnings
                        .push(format!("saved passwords were not read for site login: {e}")),
                }
                out.push(Captured {
                    meta,
                    payload: payload(&entries),
                });
            }
        }
        if spec.whole_app {
            out.push(Captured {
                meta: app_meta(&spec.app),
                payload: payload(&entries),
            });
        }
        Ok(out)
    }

    fn capture_passwords(&self, spec: &PasswordImportSpec) -> KvResult<Vec<Captured>> {
        if spec.app != "chrome" {
            return Err(KvError::Unsupported(format!(
                "importing saved passwords from {} is not supported yet (chrome is)",
                spec.app
            )));
        }
        let logins = self
            .password_reader(spec.profile.clone())
            .read(&spec.sites)
            .map_err(|e| backend_err("read saved passwords", e))?;
        let mut by_site: std::collections::BTreeMap<String, Vec<LoginRecord>> = Default::default();
        for l in logins {
            by_site
                .entry(l.site.clone())
                .or_default()
                .push(LoginRecord {
                    origin: l.origin.clone(),
                    username: l.username.clone(),
                    password: l.password.to_string(),
                });
        }
        let mut out = Vec::new();
        for (site, logins) in by_site {
            let mut meta = browser_meta(&spec.app, &site, true);
            meta.label = format!("{site} passwords ({})", spec.app);
            meta.source = spec.profile.clone().unwrap_or_else(|| "default".into());
            meta.summary.passwords = logins.len() as u32;
            out.push(Captured {
                meta,
                payload: ItemPayload {
                    provider_id: spec.app.clone(),
                    scope: "full".into(),
                    entries: vec![ItemPayload::logins_entry(&logins)?],
                },
            });
        }
        Ok(out)
    }

    async fn fill_login(&self, target: &str, fill: &LoginFill) -> KvResult<LoginFilled> {
        let space = self.open_target(target).await?;
        crate::daemon::site_login::fill_with_driver(&space, fill).await
    }

    async fn deliver(
        &self,
        target: &str,
        provider_id: &str,
        payloads: Vec<ItemPayload>,
        expires_ms: u64,
    ) -> KvResult<DeliveryOutcome> {
        self.deliver_with_progress(target, provider_id, payloads, expires_ms, Arc::new(|_| {}))
            .await
    }

    /// Packs, uploads (with bytes) and imports, telling `stage` each step.
    async fn deliver_with_progress(
        &self,
        target: &str,
        provider_id: &str,
        payloads: Vec<ItemPayload>,
        expires_ms: u64,
        stage: cua_keyvault::broker::StageSink,
    ) -> KvResult<DeliveryOutcome> {
        let space = self.open_target(target).await?;
        stage(TeleportStage::Packing);
        let spacesd = space
            .spacesd()
            .map_err(|e| backend_err("spacesd channel", e))?;
        let scope = payloads
            .first()
            .map(|p| scope_of(&p.scope))
            .unwrap_or(TransferScope::FullProfile);
        // Reassemble one SessionBundle in memory from the sealed payloads.
        let mut writer = BundleWriter::new(Vec::new(), provider_id, provider_id, scope);
        for p in &payloads {
            for e in &p.entries {
                let bytes = B64
                    .decode(&e.data)
                    .map_err(|err| backend_err("decode payload", err))?;
                writer
                    .add_bytes(&e.rel_path, e.mode, &bytes)
                    .map_err(|err| backend_err("pack bundle", err))?;
            }
        }
        let bytes = writer
            .finish()
            .map_err(|e| backend_err("finish bundle", e))?;
        let sha = hex::encode(Sha256::digest(&bytes));
        let import_id = format!("cua-kv-{:016x}", rand::random::<u64>());
        // `sent == total` once the last chunk (the import) is on its way.
        let progress: cua_teleport::Progress = {
            let stage = stage.clone();
            Arc::new(move |done, total| {
                stage(if total > 0 && done >= total {
                    TeleportStage::Importing
                } else {
                    TeleportStage::Uploading { done, total }
                })
            })
        };
        // The env token / Fleet bearer authenticate this channel and are never
        // exposed to the MCP caller; the guest wipes on its own at expiry.
        let result = cua_teleport::upload_bundle(
            spacesd,
            &import_id,
            provider_id,
            scope,
            BundleSource::Bytes(&bytes),
            &sha,
            &SendOptions {
                launch_after: false,
                expires_at_ms: expires_ms,
                // Every delivery through here was authorized by the broker.
                broker_grant: Some("keyvault".into()),
                progress: Some(progress),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| backend_err("import session", e))?;
        Ok(DeliveryOutcome {
            import_id,
            imported: result.imported,
            skipped: result.skipped,
            launched: result.launched,
        })
    }

    async fn wipe(&self, target: &str, import_id: &str) -> KvResult<Vec<String>> {
        let space = self.open_target(target).await?;
        let resp = space
            .spacesd()
            .map_err(|e| backend_err("spacesd channel", e))?
            .teleport()
            .wipe_import(pb::WipeImportRequest {
                import_id: import_id.to_string(),
                all: false,
            })
            .await
            .map_err(|e| backend_err("wipe import", e))?
            .into_inner();
        Ok(resp.removed_paths)
    }

    fn notify_consent(&self, pending: &cua_keyvault::broker::PendingView) {
        // The Cua Spaces app opens `cua://keyvault/consent/<id>`; a headless
        // daemon logs it so the user can find the request when they open Cua.
        tracing::info!(
            request = %pending.id,
            caller = %pending.caller_display,
            "keyvault: a consent request is waiting for the user"
        );
    }
}

/// The daemon's OS user-presence gate: LocalAuthentication (Touch ID, Apple
/// Watch or the login password) via the teleport biometric helper. It is
/// constructed only in production; tests inject a fake gate.
pub struct OsPresence;

impl UserPresence for OsPresence {
    fn confirm(&self, reason: &str) -> KvResult<()> {
        cua_teleport::biometric::authorize_sensitive_export(reason)
            .map_err(|e| KvError::PresenceFailed(e.to_string()))
    }
}

/// Confirms `share_space` with the same user presence the Keyvault asks
/// for: an agent on `cua.sock` can ask to share a Space, only the user can
/// allow it.
pub struct PresenceConsent(pub Arc<dyn UserPresence>);

impl cua_spaces::share::ShareConsent for PresenceConsent {
    fn confirm(&self, reason: &str) -> std::result::Result<(), String> {
        self.0.confirm(reason).map_err(|e| e.to_string())
    }
}

/// A synthetic identity for the daemon-hosted automation surface (the MCP
/// `teleport_app` tool on `cua.sock`). It is deliberately an **unverified
/// third party**: `first_party = false` and unsigned, so it can never
/// self-approve. Its fingerprint is stable across the request and the retry
/// (design 5.5: the request id binds to the caller fingerprint), so a
/// stateless MCP retry collects the token the user approved.
pub fn automation_caller() -> CallerIdentity {
    CallerIdentity {
        pid: 0,
        uid: 0,
        path: Some("cua.daemon.mcp".into()),
        signing: Signing::Unsigned,
        first_party: false,
        os_verified: false,
        launched_by: None,
        verified_name: Some("Cua daemon automation (MCP)".into()),
    }
}

/// The Keyvault as hosted by the daemon: the in-process broker. Serving the
/// socket for external clients is [`serve_socket`], spawned from the async
/// daemon start so this can be built in a synchronous context.
pub struct Keyvault {
    broker: Arc<Broker>,
}

impl Keyvault {
    /// Builds the broker over `dir` with the given backend and presence gate.
    /// It opens (but does not unlock) any existing vault. `os_protector`
    /// enrolls the platform key store so the vault can auto-unlock for
    /// unattended rules.
    pub fn new(
        dir: std::path::PathBuf,
        backend: Arc<dyn Backend>,
        presence: Arc<dyn UserPresence>,
        os_protector: bool,
    ) -> KvResult<Self> {
        let broker = Arc::new(Broker::new(
            BrokerConfig {
                dir,
                keychain_path: None,
                os_protector,
            },
            backend,
            presence,
        )?);
        Ok(Self { broker })
    }

    /// The in-process broker handle for the MCP tools.
    pub fn broker(&self) -> Arc<Broker> {
        self.broker.clone()
    }

    /// The MCP `teleport_app` session-broker adapter (an unverified
    /// third-party caller, so a delivery always needs consent or a rule).
    pub fn session_broker(&self) -> Arc<dyn SessionBroker> {
        Arc::new(McpSessionBroker {
            broker: self.broker.clone(),
        })
    }

    /// The MCP `request_site_login` adapter (the same unverified caller, so
    /// every sign-in needs the user's approval or a rule they wrote).
    pub fn site_login_broker(&self) -> Arc<dyn cua_spaces::site_login::SiteLoginBroker> {
        Arc::new(McpSiteLoginBroker {
            broker: self.broker.clone(),
        })
    }
}

/// Debug builds only: a macOS code requirement (csreq syntax) that counts as
/// first party instead of Cua's production requirement, for tests and
/// captures that sign binaries with a throwaway identity in a temp keychain
/// (design 5.3, "Test identities"). A release daemon ignores it.
pub const TEST_REQUIREMENT_ENV: &str = "CUA_KEYVAULT_TEST_REQUIREMENT";

/// The trust policy for `debug_build` given the env override. Pure, so a test
/// can assert that a release build never honours the override.
pub fn trust_policy_for(
    debug_build: bool,
    test_requirement: Option<String>,
) -> cua_keyvault::TrustPolicy {
    match test_requirement.filter(|r| debug_build && !r.trim().is_empty()) {
        Some(req) => cua_keyvault::TrustPolicy::for_tests(req),
        None => cua_keyvault::TrustPolicy::production(),
    }
}

/// The trust policy this daemon serves the socket with.
pub fn trust_policy() -> cua_keyvault::TrustPolicy {
    trust_policy_for(
        cfg!(debug_assertions),
        std::env::var(TEST_REQUIREMENT_ENV).ok(),
    )
}

/// Whether this daemon runs with a test identity. Such a daemon never enrolls
/// or reads the OS key store (the login keychain): its vault is
/// passphrase-only with a file generation anchor.
pub fn test_identity_active() -> bool {
    trust_policy().is_test_policy
}

/// Serves `path` (`$CUA_HOME/keyvault.sock`) for external first-party clients
/// (the SDK and the CLI), verifying every peer's signature. Returns once the
/// listener stops. Spawn this from the async daemon start.
pub async fn serve_socket(broker: Arc<Broker>, path: std::path::PathBuf) {
    let policy = trust_policy();
    if policy.is_test_policy {
        tracing::warn!(
            requirement = %policy.macos_requirement,
            "keyvault: debug build serving with a TEST first-party requirement"
        );
    }
    // Under the `auto` unlock policy the OS protector opens the vault at
    // daemon start (design 5.2); a vault without one stays locked until a
    // first party unlocks it.
    match broker.auto_unlock().await {
        Ok(true) => tracing::info!("keyvault: unlocked with the OS key store"),
        Ok(false) => {}
        Err(e) => tracing::warn!(error = %e, "keyvault: stays locked"),
    }
    // A daemon this one replaced may still hold the socket for a moment
    // while it exits (a stale file is replaced at once): retry, bounded.
    #[cfg(unix)]
    {
        let mut tries = 0;
        let listener = loop {
            match cua_keyvault::ipc::bind(&path).await {
                Ok(l) => break Some(l),
                Err(e) if tries < 40 => {
                    tries += 1;
                    tracing::debug!(error = %e, "keyvault: socket busy, retrying");
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(), "keyvault: socket not served");
                    break None;
                }
            }
        };
        if let Some(listener) = listener {
            cua_keyvault::ipc::serve(listener, broker, policy).await;
        }
    }
    // The socket verifies every peer through the kernel (a Unix socket
    // feature); on other OSes only the in-process broker (the app and the
    // daemon's MCP tools) reaches the vault.
    #[cfg(not(unix))]
    {
        let _ = (broker, policy);
        tracing::info!(path = %path.display(), "keyvault: no external socket on this OS");
    }
}

/// Adapts the in-process [`Broker`] to the [`cua_spaces`] MCP teleport seam.
/// The caller is the unverified automation identity, so every delivery is
/// gated on a live consent or an audited unattended rule.
pub struct McpSessionBroker {
    broker: Arc<Broker>,
}

use cua_spaces::teleport_broker::{SessionBroker, SessionBrokerError, SessionDelivery};

fn map_kv_error(e: KvError) -> SessionBrokerError {
    SessionBrokerError {
        code: cua_keyvault::ipc::error_code(&e).to_string(),
        message: e.to_string(),
    }
}

#[async_trait::async_trait]
impl SessionBroker for McpSessionBroker {
    async fn request_access(
        &self,
        app: &str,
        target: &str,
        duration_secs: u64,
        reason: &str,
    ) -> Result<String, SessionBrokerError> {
        let caller = automation_caller();
        let pending = self
            .broker
            .request_access(
                &caller,
                AccessRequest {
                    selectors: vec![Selector::App { app: app.into() }],
                    targets: vec![target.into()],
                    duration_secs: Some(duration_secs),
                    reason: reason.into(),
                    ..Default::default()
                },
            )
            .await
            .map_err(map_kv_error)?;
        Ok(pending.id)
    }

    async fn await_and_deliver(
        &self,
        app: &str,
        target: &str,
        request_id: &str,
        timeout: std::time::Duration,
    ) -> Result<SessionDelivery, SessionBrokerError> {
        use cua_keyvault::broker::Decision;
        let caller = automation_caller();
        match self
            .broker
            .await_decision(&caller, request_id, timeout)
            .await
            .map_err(map_kv_error)?
        {
            Decision::Pending => Ok(SessionDelivery::Pending),
            Decision::Denied { reason } => Ok(SessionDelivery::Denied(reason)),
            Decision::Granted { token, items, .. } => {
                // The broker performs the delivery over the daemon's
                // authenticated channel; the tool never sees raw secrets.
                let outcome = self
                    .broker
                    .teleport(
                        &caller,
                        TeleportRequest {
                            token: Some(token),
                            items: items.clone(),
                            target: target.into(),
                        },
                    )
                    .await
                    .map_err(map_kv_error)?;
                let import_ids = outcome
                    .deliveries
                    .iter()
                    .map(|d| d.import_id.clone())
                    .collect();
                let imported = outcome
                    .deliveries
                    .iter()
                    .flat_map(|d| d.imported.iter().cloned())
                    .collect();
                Ok(SessionDelivery::Delivered {
                    app: app.into(),
                    target: target.into(),
                    items,
                    imported,
                    import_ids,
                    expires_ms: outcome.expires_ms,
                })
            }
        }
    }
}

/// Adapts the in-process [`Broker`] to the [`cua_spaces::site_login`] seam
/// behind `request_site_login`. The caller is the unverified automation
/// identity: every sign-in needs the user's approval (one use per approval)
/// or an unattended rule the user wrote.
pub struct McpSiteLoginBroker {
    broker: Arc<Broker>,
}

use cua_spaces::site_login::{
    BrowserTab, SiteLoginAsk, SiteLoginBroker, SiteLoginError, SiteLoginFilled, SiteLoginOutcome,
};

fn login_err(e: KvError) -> SiteLoginError {
    SiteLoginError {
        code: cua_keyvault::ipc::error_code(&e).to_string(),
        message: e.to_string(),
    }
}

fn site_of_url(url: &str) -> Result<String, SiteLoginError> {
    let origin = cua_keyvault::broker::origin_of(url).ok_or_else(|| SiteLoginError {
        code: "invalid".into(),
        message: format!("{url:?} is not an http(s) page"),
    })?;
    Ok(cua_teleport::passwords::site_for_host(
        &cua_keyvault::broker::origin_host(&origin),
    ))
}

#[async_trait::async_trait]
impl SiteLoginBroker for McpSiteLoginBroker {
    async fn request_login(&self, ask: &SiteLoginAsk) -> Result<String, SiteLoginError> {
        let site = site_of_url(&ask.url)?;
        let caller = automation_caller();
        let reason = match &ask.agent {
            Some(a) => format!("agent {a} asks to sign in to {site} in {}", ask.target),
            None => format!("sign in to {site} in {}", ask.target),
        };
        let pending = self
            .broker
            .request_access(
                &caller,
                AccessRequest {
                    selectors: vec![Selector::Login { site }],
                    targets: vec![ask.target.clone()],
                    actions: vec![cua_keyvault::Action::Login],
                    reason,
                    agent: ask.agent.clone(),
                    ..Default::default()
                },
            )
            .await
            .map_err(login_err)?;
        Ok(pending.id)
    }

    async fn await_and_fill(
        &self,
        request_id: &str,
        ask: &SiteLoginAsk,
        timeout: std::time::Duration,
    ) -> Result<SiteLoginOutcome, SiteLoginError> {
        use cua_keyvault::broker::{BrowserRef, Decision, LoginRequest};
        let caller = automation_caller();
        match self
            .broker
            .await_decision(&caller, request_id, timeout)
            .await
            .map_err(login_err)?
        {
            Decision::Pending => Ok(SiteLoginOutcome::Pending),
            Decision::Denied { reason } => Ok(SiteLoginOutcome::Denied(reason)),
            Decision::Granted { token, .. } => {
                let out = self
                    .broker
                    .login(
                        &caller,
                        LoginRequest {
                            token: Some(token),
                            url: ask.url.clone(),
                            target: ask.target.clone(),
                            username: ask.username.clone(),
                            agent: ask.agent.clone(),
                            browser: BrowserRef {
                                session: ask.browser.session.clone(),
                                target_id: ask.browser.target_id.clone(),
                                tab_id: ask.browser.tab_id.clone(),
                            },
                            // Threaded through for S1's relay plaintext
                            // gate (cua-spaces-ext/src/daemon/site_login.rs);
                            // not otherwise touching this file (wt/chrome-kv
                            // owns it).
                            relay_plaintext_ack: ask.relay_plaintext_ack,
                        },
                    )
                    .await
                    .map_err(login_err)?;
                Ok(SiteLoginOutcome::Filled(SiteLoginFilled {
                    site: out.site,
                    origin: out.origin,
                    username_hint: out.username_hint,
                    submitted: out.filled.submitted,
                    page_url: out.filled.page_url,
                    browser: BrowserTab {
                        session: out.filled.browser.session,
                        target_id: out.filled.browser.target_id,
                        tab_id: out.filled.browser.tab_id,
                    },
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_build_never_honours_the_test_requirement() {
        let req = Some("identifier \"com.example.test\"".to_string());
        assert!(!trust_policy_for(false, req.clone()).is_test_policy);
        assert_eq!(
            trust_policy_for(false, req.clone()),
            cua_keyvault::TrustPolicy::production()
        );
        let dbg = trust_policy_for(true, req);
        assert!(dbg.is_test_policy);
        assert_eq!(dbg.macos_requirement, "identifier \"com.example.test\"");
        assert!(!trust_policy_for(true, Some("  ".into())).is_test_policy);
        assert!(!trust_policy_for(true, None).is_test_policy);
    }
}
