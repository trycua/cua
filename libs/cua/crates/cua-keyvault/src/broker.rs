// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The broker: every Keyvault operation, with its authorization.
//!
//! The IPC server calls exactly one broker method per request, passing the
//! kernel-verified [`CallerIdentity`] of the connection. Capture (reading a
//! host app) and delivery (uploading to a Space) are behind [`Backend`] so
//! the daemon supplies the real ones and tests supply fakes; OS user
//! presence (Touch ID or the login password) is behind [`UserPresence`].
//!
//! Authorization summary (design.md 5.4):
//!
//! | Operation | Who |
//! |-----------|-----|
//! | status, request_access, await_decision, teleport (token or rule), release own | any verified caller |
//! | list, describe, audit, grants, rules, pending, disable, revoke, deny, lock, delete | first party |
//! | init, import, approve, add rule, widen policy, enable, unlock (presence policy), interactive teleport | first party + user presence |

use cua_telemetry::events::{KeyvaultAction, KeyvaultMethod};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::audit::{AuditEntry, AuditEvent, Verification};
use crate::caller::CallerIdentity;
use crate::capability::{Action, Capability, Caveat, VerifyContext};
use crate::crypto::{self, SecretKey};
use crate::model::{
    DEFAULT_GRANT_SECS, DEFAULT_RULE_SECS, Delivery, Grant, ItemKind, ItemMeta, ItemPayload,
    ItemPolicy, ItemSummary, MAX_TTL_SECS, RuleCaller, Settings, UnattendedRule, UnlockPolicy,
};
use crate::policy::{self, Authority};
use crate::protector::{
    PassphraseProtector, Protector, ProtectorKind, RecoveryKey, RecoveryProtector,
};
use crate::store::Vault;
use crate::{Error, Result};

/// Pending consent requests per caller (per budget bucket).
pub const MAX_PENDING_PER_CALLER: usize = 3;
/// Access requests per caller (per budget bucket) per minute.
pub const MAX_REQUESTS_PER_MINUTE: usize = 10;
/// Pending consent requests across every caller: an absolute ceiling on how
/// many prompts can be queued at once, so a burst is bounded even if it comes
/// from several distinct verified identities (red-team F9, D1).
pub const MAX_PENDING_GLOBAL: usize = 8;
/// Access requests across every caller per minute: a global consent-prompt
/// rate limit on top of the per-caller one (red-team F9, D1).
pub const MAX_REQUESTS_PER_MINUTE_GLOBAL: usize = 30;
/// The single shared budget bucket every unsigned, ad hoc, `Unknown` or
/// otherwise unverified caller draws from, so spawning many copies of itself at
/// many paths (each a distinct fingerprint) cannot multiply its prompt
/// allowance (red-team F9).
pub const UNTRUSTED_BUDGET: &str = "untrusted:shared";
/// Pending requests expire after this long.
pub const PENDING_TTL_MS: u64 = 10 * 60 * 1000;

/// The rate/pending budget bucket a caller draws from. Team-signed,
/// OS-verified callers (and first parties) each get their own bucket, keyed by
/// their stable fingerprint. Every unverified caller shares one bucket, so it
/// cannot manufacture fresh budgets by relaunching at new paths (red-team F9).
fn budget_key(caller: &CallerIdentity) -> String {
    if caller.first_party || caller.is_verified() {
        caller.fingerprint()
    } else {
        UNTRUSTED_BUDGET.to_string()
    }
}

/// OS user presence: Touch ID, Apple Watch or the login password on macOS.
/// Blocking; the broker calls it on a blocking thread.
pub trait UserPresence: Send + Sync {
    /// Asks the user to confirm `reason`. `Ok` only when they did.
    fn confirm(&self, reason: &str) -> Result<()>;
}

/// A presence gate for tests: always yes or always no, recording reasons.
#[derive(Default)]
pub struct FakePresence {
    /// Answer.
    pub allow: std::sync::atomic::AtomicBool,
    /// Reasons asked, in order.
    pub asked: std::sync::Mutex<Vec<String>>,
}

impl FakePresence {
    /// A gate that answers `allow`.
    pub fn new(allow: bool) -> Self {
        Self {
            allow: std::sync::atomic::AtomicBool::new(allow),
            asked: Default::default(),
        }
    }

    /// Changes the answer.
    pub fn set(&self, allow: bool) {
        self.allow.store(allow, std::sync::atomic::Ordering::SeqCst);
    }
}

impl UserPresence for FakePresence {
    fn confirm(&self, reason: &str) -> Result<()> {
        self.asked.lock().unwrap().push(reason.to_string());
        if self.allow.load(std::sync::atomic::Ordering::SeqCst) {
            Ok(())
        } else {
            Err(Error::PresenceFailed("declined (test)".into()))
        }
    }
}

/// Which vault items (or items to import) a request is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Selector {
    /// An existing item.
    Item {
        /// Item id.
        id: String,
    },
    /// One site of a browser (`app`: `chrome`, `firefox`).
    Site {
        /// Provider id.
        app: String,
        /// Registrable domain (`github.com`).
        site: String,
        /// Account, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
    /// A whole app session (`slack`, `discord`, `claude-code`).
    App {
        /// Provider id.
        app: String,
    },
    /// The saved password for a site (any browser), used to sign in with
    /// [`Broker::login`]; never delivered.
    Login {
        /// Registrable domain (`github.com`) or a host under it.
        site: String,
    },
}

impl Selector {
    fn matches(&self, item: &ItemMeta) -> bool {
        match self {
            Selector::Item { id } => &item.id == id,
            Selector::Site { app, site, account } => {
                item.kind == ItemKind::BrowserSite
                    && &item.provider_id == app
                    && item.site.as_deref() == Some(site.as_str())
                    && (account.is_none() || item.account == *account)
            }
            Selector::App { app } => item.kind == ItemKind::AppSession && &item.provider_id == app,
            Selector::Login { site } => {
                item.kind == ItemKind::SitePasswords
                    && item.site.as_deref().is_some_and(|s| host_in_site(site, s))
            }
        }
    }
}

/// Cookie minimization at capture time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CookieFilter {
    /// Keep only session cookies (drop every persistent cookie).
    #[serde(default)]
    pub session_only: bool,
    /// Drop persistent cookies that expire more than 30 days out (long-lived
    /// refresh tokens), keeping short sessions.
    #[serde(default)]
    pub drop_long_lived: bool,
}

/// One site to import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteChoice {
    /// Registrable domain.
    pub site: String,
    /// Also carry the site's localStorage / IndexedDB origins.
    #[serde(default)]
    pub include_storage: bool,
    /// Also carry the site's saved passwords (a separate item; needs
    /// `confirm_passwords`).
    #[serde(default)]
    pub include_passwords: bool,
}

/// What to import from a host app.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportSpec {
    /// Provider id (`chrome`, `firefox`, `slack`).
    pub app: String,
    /// Browser profile (name or path); `None` is the default profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Sites (browsers). Empty with `whole_app: false` imports nothing.
    #[serde(default)]
    pub sites: Vec<SiteChoice>,
    /// The whole app session (Electron apps and CLIs).
    #[serde(default)]
    pub whole_app: bool,
    /// Cookie minimization.
    #[serde(default)]
    pub cookies: CookieFilter,
    /// The user confirmed moving saved passwords (second confirmation).
    #[serde(default)]
    pub confirm_passwords: bool,
    /// An explicit bundle-relative path selection for `whole_app` (any
    /// provider, not only a browser): the same `rel_path`s a manifest's
    /// items carry and a direct (non-Keyvault) teleport's consent screen
    /// already lets a caller pick. `None` keeps the existing default (a
    /// browser's login-only selection, or everything the provider offers);
    /// `Some` is used exactly as given, even if empty (which imports
    /// nothing from that half of the capture). Lets a caller that already
    /// computed its own selection -- [`Broker::import_and_teleport`], used
    /// by `Space::teleport` and `cua teleport push` -- route through the
    /// same capture the Keyvault's own import uses, instead of a second,
    /// independent selection mechanism.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

/// One importable thing, as the inventory shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    /// Kind.
    pub kind: ItemKind,
    /// Site (browsers) or `None` (whole app).
    #[serde(default)]
    pub site: Option<String>,
    /// Account or workspace names, when detectable.
    #[serde(default)]
    pub accounts: Vec<String>,
    /// Label.
    pub label: String,
    /// Contents, without values.
    pub summary: ItemSummary,
    /// Warnings (device-bound sessions and so on).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// What a host app offers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    /// Provider id.
    pub provider_id: String,
    /// App display name.
    pub app_display: String,
    /// Profile read.
    #[serde(default)]
    pub profile: Option<String>,
    /// Candidates. Nothing is selected by default.
    pub candidates: Vec<Candidate>,
    /// Notes.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// A captured item, ready to store.
#[derive(Clone, Debug)]
pub struct Captured {
    /// Metadata (`id`, `rev`, digest and timestamps are assigned by the
    /// store).
    pub meta: ItemMeta,
    /// Payload.
    pub payload: ItemPayload,
}

/// A completed delivery.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryOutcome {
    /// Receiver import id.
    pub import_id: String,
    /// What the receiver imported.
    #[serde(default)]
    pub imported: Vec<String>,
    /// What it skipped.
    #[serde(default)]
    pub skipped: Vec<String>,
    /// Whether the app was launched.
    #[serde(default)]
    pub launched: bool,
}

/// Where a direct teleport ([`Broker::import_and_teleport_with_progress`])
/// is, so the UI can say what is happening (and why macOS asks for the
/// Keychain) instead of showing a bare progress bar. Never carries a value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum TeleportStage {
    /// Reading the app's sign-in on this machine (the OS may ask for
    /// Keychain access).
    Reading,
    /// Sealing what was read into the vault (the review's "Save to
    /// Keyvault").
    Saving,
    /// Packing the bundle for the Space.
    Packing,
    /// Uploading it: bytes the Space has of the total.
    Uploading {
        /// Bytes sent.
        done: u64,
        /// Bytes in all.
        total: u64,
    },
    /// The Space is importing it.
    Importing,
}

/// Receives [`TeleportStage`]s (from any task).
pub type StageSink = Arc<dyn Fn(TeleportStage) + Send + Sync>;

/// Capture and delivery, supplied by the daemon (and by fakes in tests).
#[async_trait::async_trait]
pub trait Backend: Send + Sync {
    /// Resolves a target Space name to a STABLE, immutable identity: a
    /// sandbox or Fleet-claim id, or a pinned receiver public key, that a
    /// rename or re-create cannot forge. Grants and rules pin this id at
    /// approval time and delivery re-resolves and verifies it, so a name that
    /// now maps to a different Space fails closed (red-team F1). Fails when the
    /// target does not resolve to a Space the operator controls.
    ///
    /// The default returns the name unchanged, which gives no rebinding
    /// protection: a real daemon backend MUST override it with the true
    /// immutable id. Returning the name is only sound for a backend where the
    /// name already is the immutable id.
    fn resolve_target(&self, name: &str) -> Result<String> {
        Ok(name.to_string())
    }
    /// Lists what `app` offers (read-only; never values). Blocking.
    fn inventory(&self, app: &str, profile: Option<&str>) -> Result<Inventory>;
    /// Captures the chosen items. Blocking. Called only after user presence.
    fn capture(&self, spec: &ImportSpec) -> Result<Vec<Captured>>;
    /// Builds one bundle from same-provider payloads and imports it into
    /// `target`; the receiver wipes it on its own at `expires_ms`.
    async fn deliver(
        &self,
        target: &str,
        provider_id: &str,
        payloads: Vec<ItemPayload>,
        expires_ms: u64,
    ) -> Result<DeliveryOutcome>;
    /// [`Self::deliver`], telling `stage` when it packs, uploads (with
    /// bytes) and the Space imports. The default reports nothing.
    async fn deliver_with_progress(
        &self,
        target: &str,
        provider_id: &str,
        payloads: Vec<ItemPayload>,
        expires_ms: u64,
        stage: StageSink,
    ) -> Result<DeliveryOutcome> {
        let _ = stage;
        self.deliver(target, provider_id, payloads, expires_ms)
            .await
    }
    /// Wipes an earlier import from `target`.
    async fn wipe(&self, target: &str, import_id: &str) -> Result<Vec<String>>;
    /// Tells the UI a consent request is waiting (open the Keyvault page).
    fn notify_consent(&self, _pending: &PendingView) {}
    /// Reads a browser profile's saved passwords, one
    /// [`ItemKind::SitePasswords`] item per site whose payload holds only
    /// [`crate::model::LOGINS_ENTRY`]. Blocking. Called only after user
    /// presence.
    fn capture_passwords(&self, _spec: &PasswordImportSpec) -> Result<Vec<Captured>> {
        Err(Error::Unsupported(
            "this Keyvault cannot import saved passwords".into(),
        ))
    }
    /// Signs in to `fill.url` in `target`'s browser with the saved login:
    /// types the username and password into the page's login form through
    /// the Space's cua-driver and submits it. Must refuse when the page's
    /// origin is not `fill.origin`, and must never echo the password.
    async fn fill_login(&self, _target: &str, _fill: &LoginFill) -> Result<LoginFilled> {
        Err(Error::Unsupported(
            "this Keyvault cannot sign in to sites in a Space".into(),
        ))
    }
}

/// Saved passwords to import from a browser profile.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasswordImportSpec {
    /// Browser provider id (`chrome`).
    pub app: String,
    /// Profile name or path; `None`: the default profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Only these sites (registrable domains); empty: every saved site.
    #[serde(default)]
    pub sites: Vec<String>,
}

/// Which browser tab of the target Space to sign in (cua-driver browser
/// ids, from the caller's own `get_browser_state`). All optional: without a
/// tab the Keyvault opens its own browser in the Space.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserRef {
    /// cua-driver lifecycle session label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// cua-driver browser target id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    /// cua-driver tab id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
}

/// What the backend fills. `Debug` never prints the password; the password
/// is wiped on drop.
#[derive(Clone)]
pub struct LoginFill {
    /// The page to sign in on.
    pub url: String,
    /// The origin the saved login belongs to; the page must be on it.
    pub origin: String,
    /// Username.
    pub username: String,
    /// Password.
    pub password: zeroize::Zeroizing<String>,
    /// The tab to fill, when the caller named one.
    pub browser: BrowserRef,
    /// Lets this fill proceed over a relay connection that predates
    /// end-to-end sealing (S1), even though the relay could read the
    /// password in transit. `false` by default: such a Space refuses the
    /// fill instead, with [`crate::Error::Forbidden`].
    pub relay_plaintext_ack: bool,
}

impl std::fmt::Debug for LoginFill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginFill")
            .field("url", &self.url)
            .field("origin", &self.origin)
            .field("username", &crate::model::mask_username(&self.username))
            .field("password", &"<redacted>")
            .field("browser", &self.browser)
            .finish()
    }
}

/// What a fill did (never the password).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginFilled {
    /// The form was submitted.
    pub submitted: bool,
    /// The tab's URL after the fill.
    #[serde(default)]
    pub page_url: String,
    /// The browser tab it filled (so the caller can keep using it).
    #[serde(default)]
    pub browser: BrowserRef,
}

/// A site-login request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginRequest {
    /// Capability token (third parties); `None` uses a rule or, for first
    /// parties, interactive presence.
    #[serde(default)]
    pub token: Option<String>,
    /// The page to sign in on (`https://github.com/login`).
    pub url: String,
    /// Target Space.
    pub target: String,
    /// Which saved username, when the site has several.
    #[serde(default)]
    pub username: Option<String>,
    /// The named agent asking (audit wording only).
    #[serde(default)]
    pub agent: Option<String>,
    /// The tab to fill.
    #[serde(default)]
    pub browser: BrowserRef,
    /// See [`LoginFill::relay_plaintext_ack`].
    #[serde(default)]
    pub relay_plaintext_ack: bool,
}

/// What a site login did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginOutcome {
    /// Site (registrable domain) of the saved login.
    pub site: String,
    /// The origin signed in to.
    pub origin: String,
    /// The username, masked (`a***@example.test`).
    pub username_hint: String,
    /// The vault item used.
    pub item: String,
    /// How it was authorized.
    pub authority: String,
    /// The fill.
    pub filled: LoginFilled,
}

/// `scheme://host[:port]` of an http(s) URL, lowercased, default port
/// dropped; `None` otherwise.
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?.to_ascii_lowercase();
    if authority.is_empty() {
        return None;
    }
    let default_port = if scheme == "https" { ":443" } else { ":80" };
    let authority = authority
        .strip_suffix(default_port)
        .unwrap_or(&authority)
        .to_string();
    Some(format!("{scheme}://{authority}"))
}

/// The host of an origin (`https://a.b:8443` -> `a.b`).
pub fn origin_host(origin: &str) -> String {
    let authority = origin.split_once("://").map(|x| x.1).unwrap_or(origin);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default().to_string();
    }
    authority.split(':').next().unwrap_or_default().to_string()
}

/// Whether `host` (or a site name) is `site` or a subdomain of it.
fn host_in_site(host: &str, site: &str) -> bool {
    let (h, s) = (
        host.trim().to_ascii_lowercase(),
        site.trim().to_ascii_lowercase(),
    );
    !s.is_empty() && (h == s || h.ends_with(&format!(".{s}")))
}

/// A request for access, from any caller.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRequest {
    /// What.
    pub selectors: Vec<Selector>,
    /// Where (Space names).
    pub targets: Vec<String>,
    /// Actions (default: teleport).
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Requested lifetime (default 15 minutes).
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// Requested uses (default 1).
    #[serde(default)]
    pub uses: Option<u32>,
    /// Why (free text from the app; shown as unverified).
    #[serde(default)]
    pub reason: String,
    /// The app's claimed name (shown as unverified).
    #[serde(default)]
    pub claimed_name: Option<String>,
    /// The named agent this request is for (`ada`), shown in the consent
    /// text. Claimed by the caller, so it narrows the wording, never the
    /// authority: the grant is still bound to the verified caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

/// A pending request, as the consent UI sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingView {
    /// Request id.
    pub id: String,
    /// Verified caller.
    pub caller: CallerIdentity,
    /// Its fingerprint.
    pub caller_fp: String,
    /// One-line verified identity.
    pub caller_display: String,
    /// The request.
    pub request: AccessRequest,
    /// Existing items it resolves to.
    pub items: Vec<ItemMeta>,
    /// Selectors with no item yet (approval imports them).
    pub needs_import: Vec<Selector>,
    /// Created, Unix ms.
    pub created_ms: u64,
}

/// How the user narrows a request when approving.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApproveOptions {
    /// Keep only these item ids.
    #[serde(default)]
    pub items: Option<Vec<String>>,
    /// Keep only these targets.
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    /// Lifetime.
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// Uses (`0`: unlimited until expiry).
    #[serde(default)]
    pub uses: Option<u32>,
    /// Import missing site selectors with these options.
    #[serde(default)]
    pub cookies: CookieFilter,
    /// Include storage for imported sites.
    #[serde(default)]
    pub include_storage: bool,
    /// For a whole-app (`Selector::App`) request: the exact bundle-relative
    /// paths to capture, same meaning as [`ImportSpec::paths`]. `None` keeps
    /// the existing default (a provider's login-only selection). This is
    /// the approver's OWN choice, set when a human reviews and widens what
    /// an agent's `teleport_app` request asked for (shown to them as
    /// `would_send` when they decide) -- never the requester's own
    /// unverified `include`, which only ever reaches this call as text on
    /// the consent screen.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

/// The answer to a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Decision {
    /// Waiting for the user.
    Pending,
    /// Allowed.
    Granted {
        /// The capability token (bound to the requesting caller).
        token: String,
        /// Grant id.
        grant_id: String,
        /// Items covered.
        items: Vec<String>,
        /// Targets covered.
        targets: Vec<String>,
        /// Expiry, Unix ms.
        not_after_ms: u64,
    },
    /// Declined (or expired).
    Denied {
        /// Why.
        reason: String,
    },
}

/// Whether the caller already confirmed user presence for the WHOLE call
/// this `teleport_inner` is part of (see [`Broker::import_and_teleport`]),
/// so an `Authority::Interactive` delivery must not ask a second time. The
/// public [`Broker::teleport`] always passes [`Self::NotConfirmed`]; its
/// behavior is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresenceAlready {
    Confirmed,
    NotConfirmed,
}

/// A teleport request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeleportRequest {
    /// Capability token (third parties); `None` uses a rule or, for first
    /// parties, interactive presence.
    #[serde(default)]
    pub token: Option<String>,
    /// Item ids.
    pub items: Vec<String>,
    /// Target Space.
    pub target: String,
}

/// What a teleport did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeleportOutcome {
    /// One delivery per provider.
    pub deliveries: Vec<DeliveryOutcome>,
    /// Item ids delivered.
    pub items: Vec<String>,
    /// How it was authorized.
    pub authority: String,
    /// When the target wipes the copy (`0`: only when wiped).
    pub expires_ms: u64,
}

/// Vault status (safe for any caller).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Keyvault version.
    pub version: String,
    /// A vault exists.
    pub initialized: bool,
    /// It is unlocked.
    pub unlocked: bool,
    /// The kill switch is on.
    pub disabled: bool,
    /// The caller is first party.
    pub caller_first_party: bool,
    /// The caller as the vault sees it.
    pub caller_display: String,
    /// Items (first party only; `0` otherwise).
    pub items: usize,
    /// Pending requests (first party only).
    pub pending: usize,
    /// Unlock policy (first party only).
    #[serde(default)]
    pub unlock_policy: Option<UnlockPolicy>,
    /// Delivered copies wipe themselves after their TTL (first party only;
    /// off by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_wipe: Option<bool>,
    /// This daemon can create the OS key store protector (the macOS Keychain
    /// or Windows Credential Manager): the platform has one, this daemon may
    /// use it, and (macOS) it is the signed Cua daemon. False for a debug
    /// daemon serving a test identity, which never touches the login
    /// keychain. Setup offers a passphrase instead.
    #[serde(default)]
    pub os_protector_available: bool,
    /// A passphrase protector can always be created.
    #[serde(default)]
    pub passphrase_available: bool,
    /// The protectors that can unlock this vault now (first party only):
    /// the enrolled ones, minus the OS key store when this daemon cannot use
    /// it. Empty when there is no vault.
    #[serde(default)]
    pub unlock_protectors: Vec<ProtectorKind>,
}

/// Creating a vault. Its `Debug` never prints the passphrase, and the
/// passphrase is zeroized on drop.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct InitRequest {
    /// Enroll the OS protector (Keychain / Credential Manager).
    #[serde(default)]
    pub os_protector: bool,
    /// Enroll a passphrase protector.
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Create a recovery key (shown once).
    #[serde(default)]
    pub recovery_key: bool,
}

impl std::fmt::Debug for InitRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitRequest")
            .field("os_protector", &self.os_protector)
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field("recovery_key", &self.recovery_key)
            .finish()
    }
}

impl Drop for InitRequest {
    fn drop(&mut self) {
        if let Some(p) = self.passphrase.as_mut() {
            p.zeroize();
        }
    }
}

/// Unlocking with a credential (OS protector when both are `None`). Its
/// `Debug` is redacted and both credentials are zeroized on drop.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct UnlockRequest {
    /// Passphrase.
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Recovery key.
    #[serde(default)]
    pub recovery_key: Option<String>,
}

impl std::fmt::Debug for UnlockRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UnlockRequest(<redacted>)")
    }
}

impl Drop for UnlockRequest {
    fn drop(&mut self) {
        if let Some(p) = self.passphrase.as_mut() {
            p.zeroize();
        }
        if let Some(r) = self.recovery_key.as_mut() {
            r.zeroize();
        }
    }
}

/// A rule as the user writes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSpec {
    /// Items.
    pub items: Vec<String>,
    /// Targets (`*`: any).
    pub targets: Vec<String>,
    /// Caller fingerprints (from grants, pending requests or `status`).
    pub callers: Vec<RuleCaller>,
    /// Lifetime (default 7 days, max 90).
    #[serde(default)]
    pub duration_secs: Option<u64>,
    /// Note.
    #[serde(default)]
    pub note: String,
}

/// Broker configuration.
#[derive(Clone, Debug)]
pub struct BrokerConfig {
    /// Vault directory.
    pub dir: PathBuf,
    /// Explicit keychain for the OS protector (tests); `None`: default.
    pub keychain_path: Option<PathBuf>,
    /// Whether this platform has an OS protector the broker may use.
    pub os_protector: bool,
}

struct Pending {
    view: PendingView,
    /// The budget bucket this request was charged to (red-team F9).
    budget: String,
}

#[derive(Default)]
struct State {
    vault: Option<Vault>,
    pending: BTreeMap<String, Pending>,
    decided: HashMap<String, (String, Decision)>,
    nonces: HashMap<String, u64>,
    requests: HashMap<String, VecDeque<u64>>,
    /// Consent-request timestamps across every caller (global rate limit).
    global_requests: VecDeque<u64>,
    presence_until_ms: u64,
    /// Callers verified during this run (rules may name them).
    seen: HashMap<String, CallerIdentity>,
}

/// The broker.
pub struct Broker {
    cfg: BrokerConfig,
    backend: Arc<dyn Backend>,
    presence: Arc<dyn UserPresence>,
    /// Capability root: random per run, so tokens die with the process.
    root: SecretKey,
    state: tokio::sync::Mutex<State>,
    decided: tokio::sync::Notify,
    /// Where teleport and consent counts go (the process-wide client
    /// unless replaced with [`Broker::with_telemetry`]).
    telemetry: cua_telemetry::Telemetry,
    /// Why this daemon cannot create the OS key store protector (`None`: it
    /// can). Computed once: the daemon's own signature does not change.
    os_enroll_block: std::sync::OnceLock<Option<String>>,
}

fn uid() -> Result<String> {
    crypto::random_id()
}

/// The anti-rollback generation anchor for this platform (red-team F5). On
/// macOS with the OS protector it is a dedicated login-keychain item, kept
/// outside the vault directory so a directory restore cannot roll it back.
/// Otherwise it is a file sibling to the vault directory: a weaker portable
/// floor that still catches partial restores.
fn generation_anchor(cfg: &BrokerConfig) -> Box<dyn crate::rollback::GenerationAnchor> {
    #[cfg(target_os = "macos")]
    if cfg.os_protector {
        return Box::new(crate::macos::KeychainGenerationAnchor::new(
            cfg.keychain_path.clone(),
        ));
    }
    let path = cfg
        .dir
        .parent()
        .map(|p| p.join("keyvault.generation"))
        .unwrap_or_else(|| cfg.dir.with_extension("generation"));
    Box::new(crate::rollback::FileGenerationAnchor::new(path))
}

fn ev(kind: &str, caller: &CallerIdentity, decision: &str) -> AuditEvent {
    AuditEvent {
        kind: kind.into(),
        actor: caller.display(),
        caller_fp: caller.fingerprint(),
        item: None,
        target: None,
        decision: decision.into(),
        detail: String::new(),
    }
}

impl Broker {
    /// A broker over `cfg.dir`. Opens (but does not unlock) an existing vault.
    pub fn new(
        cfg: BrokerConfig,
        backend: Arc<dyn Backend>,
        presence: Arc<dyn UserPresence>,
    ) -> Result<Self> {
        let vault = if Vault::exists(&cfg.dir) {
            Some(Vault::open(&cfg.dir)?.with_anchor(generation_anchor(&cfg))?)
        } else {
            None
        };
        Ok(Self {
            cfg,
            backend,
            presence,
            root: SecretKey::generate()?,
            state: tokio::sync::Mutex::new(State {
                vault,
                ..Default::default()
            }),
            decided: tokio::sync::Notify::new(),
            telemetry: cua_telemetry::global().clone(),
            os_enroll_block: std::sync::OnceLock::new(),
        })
    }

    /// Records telemetry through `t` instead of the process-wide client
    /// (tests, embedders with their own client).
    pub fn with_telemetry(mut self, t: cua_telemetry::Telemetry) -> Self {
        self.telemetry = t;
        self
    }

    /// Resolves every target name to its immutable id, failing closed if any
    /// does not resolve (red-team F1).
    fn resolve_target_ids(&self, names: &[String]) -> Result<BTreeMap<String, String>> {
        let mut out = BTreeMap::new();
        for n in names {
            let id = self.backend.resolve_target(n)?;
            if id.is_empty() {
                return Err(Error::Invalid(format!(
                    "target {n:?} did not resolve to a Space identity"
                )));
            }
            out.insert(n.clone(), id);
        }
        Ok(out)
    }

    /// Why this daemon cannot use the OS key store at all (`None`: it can).
    /// Pure configuration, no OS call: a daemon configured without it (a
    /// debug daemon serving a test identity, a test fixture) or a platform
    /// without one.
    fn os_use_block(&self) -> Option<String> {
        if !self.cfg.os_protector {
            return Some(
                "this Cua daemon does not use the OS key store (a development daemon with a \
                 test identity never touches the login keychain)"
                    .into(),
            );
        }
        if cfg!(not(any(target_os = "macos", target_os = "windows"))) {
            return Some("this platform has no OS key store protector yet".into());
        }
        None
    }

    /// Why this daemon cannot create a new OS key store protector (`None`: it
    /// can). On macOS only the signed Cua daemon may create one (red-team
    /// F2), so this is checked here, before any presence prompt, rather
    /// than discovered after the user confirmed.
    fn os_enroll_block(&self) -> Option<String> {
        if let Some(why) = self.os_use_block() {
            return Some(why);
        }
        self.os_enroll_block
            .get_or_init(|| {
                #[cfg(target_os = "macos")]
                {
                    crate::protector::require_signed_os_protector(&crate::macos::self_signing())
                        .err()
                        .map(|_| {
                            "only the signed Cua daemon can create the OS key store protector"
                                .into()
                        })
                }
                #[cfg(not(target_os = "macos"))]
                {
                    None
                }
            })
            .clone()
    }

    fn os_unavailable(why: String) -> Error {
        Error::Unsupported(format!(
            "{why}; use a passphrase (in Cua, or `cua keyvault init --passphrase`)"
        ))
    }

    fn os_protector(&self) -> Result<Box<dyn Protector>> {
        if let Some(why) = self.os_use_block() {
            return Err(Self::os_unavailable(why));
        }
        crate::protector::os_protector(self.cfg.keychain_path.clone())
    }

    async fn confirm(&self, reason: String) -> Result<()> {
        let p = self.presence.clone();
        tokio::task::spawn_blocking(move || p.confirm(&reason))
            .await
            .map_err(|e| Error::Backend(format!("presence prompt task: {e}")))?
    }

    fn audit(st: &mut State, event: AuditEvent) {
        if let Some(v) = st.vault.as_mut()
            && let Err(e) = v.audit(event)
        {
            tracing::warn!(error = %e, "keyvault audit append failed");
        }
    }

    /// Appends an audit event that must land: an access the log cannot
    /// record does not happen, so no delivery is ever invisible to the user.
    fn audit_required(st: &mut State, event: AuditEvent) -> Result<()> {
        let v = st
            .vault
            .as_mut()
            .ok_or_else(|| Error::NoVault("the Keyvault is not set up".into()))?;
        v.audit(event).map(|_| ()).map_err(|e| {
            tracing::warn!(error = %e, "keyvault audit append failed; refusing");
            Error::Corrupt(format!(
                "the Keyvault could not record this access in its audit log, so it was refused: {e}"
            ))
        })
    }

    fn vault_mut(st: &mut State) -> Result<&mut Vault> {
        let v = st
            .vault
            .as_mut()
            .ok_or_else(|| Error::NoVault("the Keyvault is not set up".into()))?;
        if !v.is_unlocked() {
            return Err(Error::Locked);
        }
        Ok(v)
    }

    fn settings(st: &State) -> Result<Settings> {
        Ok(st
            .vault
            .as_ref()
            .ok_or_else(|| Error::NoVault("the Keyvault is not set up".into()))?
            .meta()?
            .settings
            .clone())
    }

    /// Under the `presence` unlock policy, first-party access to vault
    /// contents needs a recent presence confirmation.
    async fn ensure_session(&self, caller: &CallerIdentity) -> Result<()> {
        let (policy, until, minutes) = {
            let st = self.state.lock().await;
            let s = Self::settings(&st)?;
            (s.unlock_policy, st.presence_until_ms, s.auto_lock_minutes)
        };
        if policy == UnlockPolicy::Auto || crate::now_ms() < until {
            return Ok(());
        }
        self.confirm(format!("Unlock the Cua Keyvault for {}", caller.display()))
            .await?;
        let mut st = self.state.lock().await;
        st.presence_until_ms = crate::now_ms() + u64::from(minutes.max(1)) * 60_000;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------

    /// Status, for any verified caller.
    pub async fn status(&self, caller: &CallerIdentity) -> Status {
        let st = self.state.lock().await;
        let (initialized, unlocked) = match &st.vault {
            Some(v) => (true, v.is_unlocked()),
            None => (false, false),
        };
        let meta = st.vault.as_ref().and_then(|v| v.meta().ok());
        let disabled = meta.map(|m| m.settings.disabled).unwrap_or(false);
        let fp = caller.first_party;
        Status {
            version: env!("CARGO_PKG_VERSION").into(),
            initialized,
            unlocked,
            disabled,
            caller_first_party: fp,
            caller_display: caller.display(),
            items: if fp {
                meta.map(|m| m.items.len()).unwrap_or(0)
            } else {
                0
            },
            pending: if fp { st.pending.len() } else { 0 },
            unlock_policy: if fp {
                meta.map(|m| m.settings.unlock_policy)
            } else {
                None
            },
            auto_wipe: if fp {
                meta.map(|m| m.settings.auto_wipe)
            } else {
                None
            },
            os_protector_available: self.os_enroll_block().is_none(),
            passphrase_available: true,
            unlock_protectors: match (&st.vault, fp) {
                (Some(v), true) => {
                    let os_ok = self.os_use_block().is_none();
                    let mut kinds: Vec<ProtectorKind> = Vec::new();
                    for r in &v.header().protectors {
                        if (os_ok || !r.kind.unattended()) && !kinds.contains(&r.kind) {
                            kinds.push(r.kind);
                        }
                    }
                    kinds
                }
                _ => Vec::new(),
            },
        }
    }

    /// Creates the vault (first party + presence). Returns the recovery key
    /// to show once.
    pub async fn init(&self, caller: &CallerIdentity, req: InitRequest) -> Result<Option<String>> {
        let method = if req.os_protector {
            KeyvaultMethod::OsKeyStore
        } else if req.passphrase.is_some() {
            KeyvaultMethod::Passphrase
        } else {
            KeyvaultMethod::RecoveryKey
        };
        let r = self.init_inner(caller, req).await;
        self.record_action(KeyvaultAction::Setup, method, &r);
        r
    }

    async fn init_inner(
        &self,
        caller: &CallerIdentity,
        mut req: InitRequest,
    ) -> Result<Option<String>> {
        policy::require_first_party(caller, "creating the Keyvault")?;
        if self.state.lock().await.vault.is_some() {
            return Err(Error::Invalid("the Keyvault already exists".into()));
        }
        // Everything that can refuse the request is checked before the
        // presence prompt, so an unusable choice fails fast and the user is
        // never asked for Touch ID for a setup that cannot happen.
        if !req.os_protector && req.passphrase.is_none() && !req.recovery_key {
            return Err(Error::Invalid(
                "choose at least one protector (OS key store or passphrase)".into(),
            ));
        }
        if req.os_protector
            && let Some(why) = self.os_enroll_block()
        {
            return Err(Self::os_unavailable(why));
        }
        if let Some(p) = &req.passphrase {
            crate::protector::check_new_passphrase(p)?;
        }
        self.confirm(format!("Create the Cua Keyvault ({})", caller.display()))
            .await?;
        let mut boxes: Vec<Box<dyn Protector>> = Vec::new();
        if req.os_protector {
            boxes.push(self.os_protector()?);
        }
        if let Some(p) = req.passphrase.take() {
            boxes.push(Box::new(PassphraseProtector::new(p)));
        }
        let recovery = if req.recovery_key {
            let k = RecoveryKey::generate()?;
            boxes.push(Box::new(RecoveryProtector(k.clone())));
            Some(k)
        } else {
            None
        };
        let refs: Vec<&dyn Protector> = boxes.iter().map(|b| b.as_ref()).collect();
        let dir = self.cfg.dir.clone();
        let vault = Vault::create(&dir, &refs)?.with_anchor(generation_anchor(&self.cfg))?;
        let mut st = self.state.lock().await;
        st.vault = Some(vault);
        let mut e = ev("vault.create", caller, "ok");
        e.detail = format!("protectors={}", refs.len());
        Self::audit(&mut st, e);
        Ok(recovery.map(|k| k.reveal().to_string()))
    }

    /// Unlocks with the OS protector (no credential) or a passphrase or
    /// recovery key. First party.
    pub async fn unlock(&self, caller: &CallerIdentity, req: UnlockRequest) -> Result<()> {
        let method = match (&req.passphrase, &req.recovery_key) {
            (Some(_), _) => KeyvaultMethod::Passphrase,
            (None, Some(_)) => KeyvaultMethod::RecoveryKey,
            (None, None) => KeyvaultMethod::OsKeyStore,
        };
        let r = self.unlock_inner(caller, req).await;
        self.record_action(KeyvaultAction::Unlock, method, &r);
        r
    }

    async fn unlock_inner(&self, caller: &CallerIdentity, req: UnlockRequest) -> Result<()> {
        policy::require_first_party(caller, "unlocking the Keyvault")?;
        let protector: Box<dyn Protector> = match (&req.passphrase, &req.recovery_key) {
            (Some(p), _) => Box::new(PassphraseProtector::new(p.clone())),
            (None, Some(r)) => Box::new(RecoveryProtector(RecoveryKey::parse(r)?)),
            (None, None) => self.os_protector()?,
        };
        drop(req);
        let mut st = self.state.lock().await;
        let v = st
            .vault
            .as_mut()
            .ok_or_else(|| Error::NoVault("the Keyvault is not set up".into()))?;
        if v.is_unlocked() {
            return Ok(());
        }
        if !v
            .header()
            .protectors
            .iter()
            .any(|r| r.kind == protector.kind())
        {
            return Err(Error::Unsupported(match protector.kind() {
                ProtectorKind::Passphrase => {
                    "this Keyvault has no passphrase; unlock it with the OS key store".into()
                }
                ProtectorKind::Recovery => "this Keyvault has no recovery key".into(),
                _ => "this Keyvault has no OS key store protector; unlock it with its passphrase"
                    .into(),
            }));
        }
        match v.unlock(protector.as_ref()) {
            Ok(()) => {
                let now = crate::now_ms();
                let _ = v.update_meta(|m| {
                    policy::prune(m, now);
                    Ok(())
                });
                let mut e = ev("vault.unlock", caller, "ok");
                e.detail = format!("{:?}", protector.kind());
                Self::audit(&mut st, e);
                Ok(())
            }
            Err(err) => {
                let mut e = ev("vault.unlock", caller, "deny");
                e.detail = format!("{:?}", protector.kind());
                Self::audit(&mut st, e);
                Err(err)
            }
        }
    }

    /// Unlocks with the OS protector at daemon start, when there is one.
    pub async fn auto_unlock(&self) -> Result<bool> {
        let mut st = self.state.lock().await;
        let Some(v) = st.vault.as_mut() else {
            return Ok(false);
        };
        if v.is_unlocked() {
            return Ok(true);
        }
        let has_os = v.header().protectors.iter().any(|r| r.kind.unattended());
        if !has_os || !self.cfg.os_protector {
            return Ok(false);
        }
        let p = self.os_protector()?;
        v.unlock(p.as_ref())?;
        Ok(true)
    }

    /// Locks (first party).
    pub async fn lock(&self, caller: &CallerIdentity) -> Result<()> {
        let r = self.lock_inner(caller).await;
        self.record_action(KeyvaultAction::Lock, KeyvaultMethod::None, &r);
        r
    }

    /// `cua_keyvault_action`: the action, how it was unlocked and whether it
    /// worked. Nothing about the vault's contents.
    fn record_action<T>(&self, a: KeyvaultAction, m: KeyvaultMethod, r: &Result<T>) {
        self.telemetry
            .capture(cua_telemetry::events::keyvault_action(
                a,
                m,
                if r.is_ok() {
                    cua_telemetry::Outcome::Ok
                } else {
                    cua_telemetry::Outcome::Error
                },
            ));
    }

    async fn lock_inner(&self, caller: &CallerIdentity) -> Result<()> {
        policy::require_first_party(caller, "locking the Keyvault")?;
        let mut st = self.state.lock().await;
        Self::audit(&mut st, ev("vault.lock", caller, "ok"));
        if let Some(v) = st.vault.as_mut() {
            v.lock();
        }
        st.presence_until_ms = 0;
        Ok(())
    }

    /// The kill switch. Disabling is always allowed for first parties,
    /// revokes every outstanding token and wipes every live delivery;
    /// enabling needs user presence.
    pub async fn set_disabled(&self, caller: &CallerIdentity, disabled: bool) -> Result<()> {
        policy::require_first_party(caller, "the global disable switch")?;
        if !disabled {
            self.confirm(format!(
                "Turn the Cua Keyvault back on ({})",
                caller.display()
            ))
            .await?;
        }
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        v.update_meta(|m| {
            m.settings.disabled = disabled;
            if disabled {
                m.epoch += 1;
            }
            Ok(())
        })?;
        if disabled {
            st.pending.clear();
        }
        Self::audit(
            &mut st,
            ev(
                if disabled {
                    "vault.disable"
                } else {
                    "vault.enable"
                },
                caller,
                "ok",
            ),
        );
        drop(st);
        if disabled {
            // Stop sharing now: wipe every live copy in every Space, not only
            // future teleports. Best effort (each wipe and failure is
            // audited); a copy that cannot be wiped still expires by its TTL.
            let _ = self.wipe_where(caller, |_| true).await;
        }
        Ok(())
    }

    /// Changes settings other than the kill switch (first party; moving to
    /// the weaker `auto` unlock policy needs presence).
    pub async fn set_unlock_policy(
        &self,
        caller: &CallerIdentity,
        unlock_policy: UnlockPolicy,
        auto_lock_minutes: Option<u32>,
    ) -> Result<()> {
        policy::require_first_party(caller, "changing Keyvault settings")?;
        if unlock_policy == UnlockPolicy::Auto {
            self.confirm("Let the Cua Keyvault unlock automatically".into())
                .await?;
        }
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        v.update_meta(|m| {
            m.settings.unlock_policy = unlock_policy;
            if let Some(n) = auto_lock_minutes {
                m.settings.auto_lock_minutes = n.clamp(1, 24 * 60);
            }
            Ok(())
        })?;
        st.presence_until_ms = 0;
        Self::audit(&mut st, ev("settings.update", caller, "ok"));
        Ok(())
    }

    /// Auto-wipe (first party): on, copies delivered from now on wipe
    /// themselves after their item's TTL; off, they stay until wiped.
    /// Turning it off keeps copies longer, so it needs presence. Copies
    /// already delivered keep the expiry they were delivered with.
    pub async fn set_auto_wipe(&self, caller: &CallerIdentity, on: bool) -> Result<()> {
        policy::require_first_party(caller, "changing Keyvault settings")?;
        let current = {
            let st = self.state.lock().await;
            Self::settings(&st)?.auto_wipe
        };
        if current == on {
            return Ok(());
        }
        if !on {
            self.confirm("Keep Keyvault access in Spaces until you wipe it".into())
                .await?;
        }
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        v.update_meta(|m| {
            m.settings.auto_wipe = on;
            Ok(())
        })?;
        let mut e = ev("settings.update", caller, "ok");
        e.detail = format!("auto_wipe={on}");
        Self::audit(&mut st, e);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Items
    // ------------------------------------------------------------------

    /// Every item's coarse metadata (first party). Never payloads, and never
    /// the cookie names, domains or storage origins: bulk enumeration returns
    /// only labels and counts, so an injected or confused-deputy first-party
    /// client cannot harvest the user's whole logged-in map without presence
    /// (red-team F17). Per-item detail comes from [`Broker::describe_item`],
    /// which requires presence.
    pub async fn list_items(&self, caller: &CallerIdentity) -> Result<Vec<ItemMeta>> {
        policy::require_first_party(caller, "listing Keyvault items")?;
        self.ensure_session(caller).await?;
        let mut st = self.state.lock().await;
        Ok(Self::vault_mut(&mut st)?
            .meta()?
            .items
            .values()
            .map(|i| i.redacted())
            .collect())
    }

    /// One item's full metadata, including cookie names and domains and storage
    /// origins (first party + user presence). This detail set is a sensitive
    /// map of the user's logged-in life, so it is gated on presence even for a
    /// first party, whose address space may have been injected (red-team
    /// F17/F3). Never returns payload values.
    pub async fn describe_item(&self, caller: &CallerIdentity, id: &str) -> Result<ItemMeta> {
        policy::require_first_party(caller, "reading item detail")?;
        let label = {
            let mut st = self.state.lock().await;
            Self::vault_mut(&mut st)?
                .meta()?
                .items
                .get(id)
                .map(|i| i.label.clone())
                .ok_or_else(|| Error::NotFound(format!("item {id}")))?
        };
        self.confirm(format!(
            "Show the saved cookie names and domains for {label} ({})",
            caller.display()
        ))
        .await?;
        let mut st = self.state.lock().await;
        Self::vault_mut(&mut st)?
            .meta()?
            .items
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("item {id}")))
    }

    /// What a host app offers, per site (first party; reads metadata only).
    pub async fn inventory(
        &self,
        caller: &CallerIdentity,
        app: &str,
        profile: Option<&str>,
    ) -> Result<Inventory> {
        policy::require_first_party(caller, "reading host apps")?;
        let b = self.backend.clone();
        let (app, profile) = (app.to_string(), profile.map(str::to_string));
        tokio::task::spawn_blocking(move || b.inventory(&app, profile.as_deref()))
            .await
            .map_err(|e| Error::Backend(e.to_string()))?
    }

    /// Imports sites or an app session into the vault (first party +
    /// presence). Each site becomes its own item.
    pub async fn import(&self, caller: &CallerIdentity, spec: ImportSpec) -> Result<Vec<ItemMeta>> {
        let r = self.import_inner(caller, spec).await;
        self.record_action(KeyvaultAction::Import, KeyvaultMethod::None, &r);
        r
    }

    async fn import_inner(
        &self,
        caller: &CallerIdentity,
        spec: ImportSpec,
    ) -> Result<Vec<ItemMeta>> {
        policy::require_first_party(caller, "importing into the Keyvault")?;
        {
            let st = self.state.lock().await;
            if Self::settings(&st)?.disabled {
                return Err(Error::Disabled);
            }
        }
        let reason = import_reason(&spec, caller);
        self.confirm(reason).await?;
        self.import_confirmed(caller, spec).await
    }

    async fn import_confirmed(
        &self,
        caller: &CallerIdentity,
        spec: ImportSpec,
    ) -> Result<Vec<ItemMeta>> {
        let captured = self.capture_confirmed(spec).await?;
        self.store_captured(caller, captured).await
    }

    /// The capture half of [`Self::import_confirmed`] (after presence):
    /// checks the selection, then reads it on a blocking thread.
    async fn capture_confirmed(&self, spec: ImportSpec) -> Result<Vec<Captured>> {
        if spec.sites.iter().any(|s| s.include_passwords) && !spec.confirm_passwords {
            return Err(Error::Invalid(
                "saved passwords need the explicit second confirmation (confirm_passwords)".into(),
            ));
        }
        if spec.sites.is_empty() && !spec.whole_app {
            return Err(Error::Invalid(
                "nothing selected: choose sites, or the whole app session".into(),
            ));
        }
        let b = self.backend.clone();
        tokio::task::spawn_blocking(move || b.capture(&spec))
            .await
            .map_err(|e| Error::Backend(e.to_string()))?
    }

    /// Captures `spec` and immediately delivers every resulting item to
    /// `target`, as ONE first-party action behind a SINGLE presence
    /// confirmation covering both halves. This is what a direct (non-MCP)
    /// teleport -- `Space::teleport`, `cua teleport push`, the Tauri and
    /// SwiftUI apps -- uses instead of exporting and uploading a bundle on
    /// its own: the capture, the audited authorization and the delivery all
    /// happen inside the Keyvault, so a direct teleport gets the same audit
    /// trail, kill switch and auto-wipe setting a Keyvault-saved session
    /// already has.
    ///
    /// `save`: when `false` (the common case -- "teleport this app", not
    /// "save this session"), every item this call captured is forgotten
    /// from the vault once delivery completes: crypto-shredded like
    /// [`Self::delete_item`], but WITHOUT wiping the delivery just made --
    /// "don't save" means "don't keep this for later reuse", not "undo what
    /// was just sent". The capture, the delivery and the forgetting are all
    /// still in the audit log. `save: true` is the review sheet's "Save to
    /// Keyvault" (or `cua keyvault import-session`): the item stays, so the
    /// user can later grant an agent access to it.
    ///
    /// Fails closed: a caller that is not first party, or a disabled vault,
    /// never reaches the backend at all (same as [`Self::import`] and
    /// [`Self::teleport`] individually).
    pub async fn import_and_teleport(
        &self,
        caller: &CallerIdentity,
        spec: ImportSpec,
        target: String,
        save: bool,
    ) -> Result<TeleportOutcome> {
        self.import_and_teleport_with_progress(caller, spec, target, save, None)
            .await
    }

    /// [`Self::import_and_teleport`], telling `stage` where it is: reading
    /// (when the OS may ask for Keychain access), saving (`save` only),
    /// packing, uploading and importing.
    pub async fn import_and_teleport_with_progress(
        &self,
        caller: &CallerIdentity,
        spec: ImportSpec,
        target: String,
        save: bool,
        stage: Option<StageSink>,
    ) -> Result<TeleportOutcome> {
        let tell = |s: TeleportStage| {
            if let Some(f) = &stage {
                f(s);
            }
        };
        policy::require_first_party(caller, "teleporting from this machine")?;
        {
            let st = self.state.lock().await;
            if Self::settings(&st)?.disabled {
                return Err(Error::Disabled);
            }
        }
        validate_target(&target)?;
        let reason = format!(
            "Teleport {} to {} ({})",
            describe_spec(&spec),
            target,
            caller.display()
        );
        self.confirm(reason).await?;
        tell(TeleportStage::Reading);
        let captured = self.capture_confirmed(spec).await?;
        if save {
            tell(TeleportStage::Saving);
        }
        let items = self.store_captured(caller, captured).await?;
        let item_ids: Vec<String> = items.iter().map(|m| m.id.clone()).collect();
        if item_ids.is_empty() {
            return Err(Error::Invalid(
                "nothing was captured to teleport (empty selection)".into(),
            ));
        }
        let started = std::time::Instant::now();
        let app = items
            .first()
            .map(|m| m.provider_id.clone())
            .unwrap_or_default();
        let n = item_ids.len() as u64;
        let req = TeleportRequest {
            token: None,
            items: item_ids.clone(),
            target: target.clone(),
        };
        let result = self
            .teleport_inner(caller, req, PresenceAlready::Confirmed, stage.clone())
            .await;
        crate::telemetry::teleport(&self.telemetry, caller, &app, started, &result, n);
        if !save {
            for id in &item_ids {
                if let Err(e) = self.forget_item(caller, id).await {
                    tracing::warn!(
                        item = %id,
                        error = %e,
                        "keyvault: could not forget an unsaved item after teleport"
                    );
                }
            }
        }
        result
    }

    /// Crypto-shreds `id`'s vault record (like [`Self::delete_item`]) but
    /// never wipes a delivery it already made: used only by
    /// [`Self::import_and_teleport`]'s `save: false`, where the point is to
    /// forget the vault copy while leaving what was just delivered in place.
    async fn forget_item(&self, caller: &CallerIdentity, id: &str) -> Result<()> {
        policy::require_first_party(caller, "forgetting an unsaved Keyvault item")?;
        let mut st = self.state.lock().await;
        Self::vault_mut(&mut st)?.delete_item(id)?;
        let mut e = ev("item.delete", caller, "ok");
        e.item = Some(id.into());
        e.detail = "not saved (import_and_teleport save=false)".into();
        Self::audit(&mut st, e);
        Ok(())
    }

    /// Imports a browser's saved passwords into the vault, one
    /// [`ItemKind::SitePasswords`] item per site (first party + presence).
    /// The passwords stay sealed: they are used through [`Broker::login`]
    /// and never delivered.
    pub async fn import_passwords(
        &self,
        caller: &CallerIdentity,
        spec: PasswordImportSpec,
    ) -> Result<Vec<ItemMeta>> {
        let r = self.import_passwords_inner(caller, spec).await;
        self.record_action(KeyvaultAction::Import, KeyvaultMethod::None, &r);
        r
    }

    async fn import_passwords_inner(
        &self,
        caller: &CallerIdentity,
        spec: PasswordImportSpec,
    ) -> Result<Vec<ItemMeta>> {
        policy::require_first_party(caller, "importing saved passwords")?;
        if spec.app.trim().is_empty() {
            return Err(Error::Invalid("name the browser to import from".into()));
        }
        {
            let st = self.state.lock().await;
            if Self::settings(&st)?.disabled {
                return Err(Error::Disabled);
            }
        }
        let what = if spec.sites.is_empty() {
            "every saved password".to_string()
        } else {
            format!("the saved passwords for {}", spec.sites.join(", "))
        };
        self.confirm(format!(
            "Import {what} from {} into the Cua Keyvault ({})",
            spec.app,
            caller.display()
        ))
        .await?;
        let b = self.backend.clone();
        let spec2 = spec.clone();
        let captured = tokio::task::spawn_blocking(move || b.capture_passwords(&spec2))
            .await
            .map_err(|e| Error::Backend(e.to_string()))??;
        self.store_captured(caller, captured).await
    }

    async fn store_captured(
        &self,
        caller: &CallerIdentity,
        captured: Vec<Captured>,
    ) -> Result<Vec<ItemMeta>> {
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        let mut out = Vec::new();
        for c in captured {
            // Re-importing the same site replaces the old item's payload and
            // keeps its id, policy and grants.
            let existing = v
                .meta()?
                .items
                .values()
                .find(|i| {
                    i.provider_id == c.meta.provider_id
                        && i.kind == c.meta.kind
                        && i.site == c.meta.site
                        && i.account == c.meta.account
                        && i.source == c.meta.source
                })
                .map(|i| i.id.clone());
            let id = match existing {
                Some(id) => {
                    v.replace_payload(&id, &c.payload)?;
                    let summary = c.meta.summary.clone();
                    let warnings = c.meta.warnings.clone();
                    v.update_meta(|m| {
                        let it = m.items.get_mut(&id).expect("exists");
                        it.summary = summary;
                        it.warnings = warnings;
                        Ok(())
                    })?;
                    id
                }
                None => v.put_item(c.meta, &c.payload)?,
            };
            out.push(v.meta()?.items[&id].clone());
        }
        for it in &out {
            let mut e = ev("item.import", caller, "ok");
            e.item = Some(it.id.clone());
            Self::audit(&mut st, e);
        }
        Ok(out)
    }

    /// Deletes an item and wipes its live deliveries (first party).
    pub async fn delete_item(&self, caller: &CallerIdentity, id: &str) -> Result<Vec<String>> {
        policy::require_first_party(caller, "deleting Keyvault items")?;
        let wiped = self
            .wipe_where(caller, |d| d.items.iter().any(|i| i == id))
            .await?;
        let mut st = self.state.lock().await;
        Self::vault_mut(&mut st)?.delete_item(id)?;
        let mut e = ev("item.delete", caller, "ok");
        e.item = Some(id.into());
        Self::audit(&mut st, e);
        Ok(wiped)
    }

    /// Sets an item's policy (first party; widening needs presence).
    pub async fn set_item_policy(
        &self,
        caller: &CallerIdentity,
        id: &str,
        mut new: ItemPolicy,
    ) -> Result<ItemMeta> {
        policy::require_first_party(caller, "changing item policies")?;
        new.ttl_secs = new.ttl_secs.clamp(60, MAX_TTL_SECS);
        let (old, label) = {
            let mut st = self.state.lock().await;
            let it = Self::vault_mut(&mut st)?
                .meta()?
                .items
                .get(id)
                .cloned()
                .ok_or_else(|| Error::NotFound(format!("item {id}")))?;
            (it.policy.clone(), it.label.clone())
        };
        if old.widened_by(&new) {
            self.confirm(format!("Widen the Keyvault policy for {label}"))
                .await?;
        }
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        let out = v.update_meta(|m| {
            let it = m
                .items
                .get_mut(id)
                .ok_or_else(|| Error::NotFound(format!("item {id}")))?;
            it.policy = new.clone();
            if !new.unattended {
                // Rules that relied on it lose the item.
                for r in &mut m.rules {
                    r.items.retain(|i| i != id);
                }
                m.rules.retain(|r| !r.items.is_empty());
            }
            Ok(it.clone())
        })?;
        let mut e = ev("item.policy", caller, "ok");
        e.item = Some(id.into());
        Self::audit(&mut st, e);
        Ok(out)
    }

    // ------------------------------------------------------------------
    // Consent
    // ------------------------------------------------------------------

    /// Asks for access (any verified caller). Returns the request id.
    pub async fn request_access(
        &self,
        caller: &CallerIdentity,
        req: AccessRequest,
    ) -> Result<PendingView> {
        let r = self.request_access_inner(caller, req).await;
        if r.is_ok() {
            crate::telemetry::consent(
                &self.telemetry,
                cua_telemetry::events::ConsentDecision::Requested,
            );
        }
        r
    }

    async fn request_access_inner(
        &self,
        caller: &CallerIdentity,
        mut req: AccessRequest,
    ) -> Result<PendingView> {
        if req.selectors.is_empty() || req.targets.is_empty() {
            return Err(Error::Invalid(
                "name at least one item or site and one target".into(),
            ));
        }
        if req.selectors.len() > 32 || req.targets.len() > 8 || req.reason.len() > 500 {
            return Err(Error::Invalid("request too large".into()));
        }
        for t in &req.targets {
            validate_target(t)?;
        }
        if req.actions.is_empty() {
            req.actions = vec![Action::Teleport];
        }
        let fp = caller.fingerprint();
        let budget = budget_key(caller);
        let now = crate::now_ms();
        let mut st = self.state.lock().await;
        if Self::settings(&st)?.disabled {
            return Err(Error::Disabled);
        }
        // Expire stale requests.
        let before = st.pending.len();
        st.pending
            .retain(|_, p| now.saturating_sub(p.view.created_ms) < PENDING_TTL_MS);
        crate::telemetry::expired(&self.telemetry, before - st.pending.len());
        // Global consent-prompt rate limit (across every caller): bounds the
        // total prompt volume even when several distinct identities each stay
        // under their own budget (red-team F9).
        while st
            .global_requests
            .front()
            .is_some_and(|t| now.saturating_sub(*t) > 60_000)
        {
            st.global_requests.pop_front();
        }
        if st.global_requests.len() >= MAX_REQUESTS_PER_MINUTE_GLOBAL {
            return Err(Error::RateLimited(
                "too many consent requests right now; try again in a minute".into(),
            ));
        }
        // Per-budget rate limit. Every unverified caller shares one budget
        // bucket, so relaunching at new paths cannot mint fresh allowances.
        let window = st.requests.entry(budget.clone()).or_default();
        while window
            .front()
            .is_some_and(|t| now.saturating_sub(*t) > 60_000)
        {
            window.pop_front();
        }
        if window.len() >= MAX_REQUESTS_PER_MINUTE {
            return Err(Error::RateLimited(
                "too many access requests; try again in a minute".into(),
            ));
        }
        // Global pending ceiling.
        if st.pending.len() >= MAX_PENDING_GLOBAL {
            return Err(Error::RateLimited(
                "too many consent requests are already waiting for the user".into(),
            ));
        }
        if st.pending.values().filter(|p| p.budget == budget).count() >= MAX_PENDING_PER_CALLER {
            return Err(Error::RateLimited(
                "this app already has requests waiting for the user".into(),
            ));
        }
        st.requests
            .entry(budget.clone())
            .or_default()
            .push_back(now);
        st.global_requests.push_back(now);
        let v = Self::vault_mut(&mut st)?;
        let meta = v.meta()?;
        let mut items: Vec<ItemMeta> = Vec::new();
        let mut needs_import = Vec::new();
        for s in &req.selectors {
            let found: Vec<&ItemMeta> = meta.items.values().filter(|i| s.matches(i)).collect();
            if found.is_empty() {
                match s {
                    Selector::Item { id } => return Err(Error::NotFound(format!("item {id}"))),
                    Selector::Login { site } => {
                        return Err(Error::NotFound(format!(
                            "no saved password for {site} in the Keyvault (import it in Cua first)"
                        )));
                    }
                    other => needs_import.push(other.clone()),
                }
            }
            for f in found {
                if !items.iter().any(|i| i.id == f.id) {
                    items.push(f.clone());
                }
            }
        }
        let view = PendingView {
            id: uid()?,
            caller_fp: fp,
            caller_display: caller.display(),
            caller: caller.clone(),
            request: req,
            items,
            needs_import,
            created_ms: now,
        };
        st.pending.insert(
            view.id.clone(),
            Pending {
                view: view.clone(),
                budget,
            },
        );
        let login = view.request.actions.contains(&Action::Login);
        let mut e = ev(
            if login {
                "login.request"
            } else {
                "consent.request"
            },
            caller,
            "pending",
        );
        e.target = view.request.targets.first().cloned();
        e.detail = match (&view.request.agent, login) {
            (Some(a), true) => format!("request={} agent={a}", view.id),
            _ => format!("request={}", view.id),
        };
        Self::audit(&mut st, e);
        drop(st);
        // The first-party consent UI gets the full view (with cookie names and
        // domains) via notify_consent / list_pending.
        self.backend.notify_consent(&view);
        // The requesting caller may be an unverified third party; do not hand it
        // the exact cookie names and domains of the user's existing items
        // (red-team F17). It only needs the request id and its own selection.
        let mut caller_view = view;
        caller_view.items = caller_view.items.iter().map(|i| i.redacted()).collect();
        Ok(caller_view)
    }

    /// Waits up to `timeout` for the user's answer to the caller's own
    /// request. A granted token is handed over once.
    pub async fn await_decision(
        &self,
        caller: &CallerIdentity,
        request_id: &str,
        timeout: Duration,
    ) -> Result<Decision> {
        let fp = caller.fingerprint();
        let deadline = tokio::time::Instant::now() + timeout.min(Duration::from_secs(120));
        // Bounded: each pass either returns or waits for a notification or
        // the deadline.
        for _ in 0..10_000 {
            {
                let mut st = self.state.lock().await;
                if let Some((owner, _)) = st.decided.get(request_id) {
                    if owner != &fp {
                        return Err(Error::Forbidden("not your request".into()));
                    }
                    let (_, d) = st.decided.remove(request_id).expect("present");
                    return Ok(d);
                }
                match st.pending.get(request_id) {
                    Some(p) if p.view.caller_fp != fp => {
                        return Err(Error::Forbidden("not your request".into()));
                    }
                    Some(p)
                        if crate::now_ms().saturating_sub(p.view.created_ms) >= PENDING_TTL_MS =>
                    {
                        st.pending.remove(request_id);
                        return Ok(Decision::Denied {
                            reason: "the request expired".into(),
                        });
                    }
                    Some(_) => {}
                    None => return Err(Error::NotFound(format!("request {request_id}"))),
                }
            }
            let notified = self.decided.notified();
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(Decision::Pending);
            }
        }
        Ok(Decision::Pending)
    }

    /// Pending requests (first party: the consent UI).
    pub async fn list_pending(&self, caller: &CallerIdentity) -> Result<Vec<PendingView>> {
        policy::require_first_party(caller, "listing consent requests")?;
        let now = crate::now_ms();
        let mut st = self.state.lock().await;
        let before = st.pending.len();
        st.pending
            .retain(|_, p| now.saturating_sub(p.view.created_ms) < PENDING_TTL_MS);
        crate::telemetry::expired(&self.telemetry, before - st.pending.len());
        Ok(st.pending.values().map(|p| p.view.clone()).collect())
    }

    /// Declines a request (first party; no presence needed).
    pub async fn deny(&self, caller: &CallerIdentity, request_id: &str) -> Result<()> {
        let r = self.deny_inner(caller, request_id).await;
        if r.is_ok() {
            crate::telemetry::consent(
                &self.telemetry,
                cua_telemetry::events::ConsentDecision::Denied,
            );
        }
        r
    }

    async fn deny_inner(&self, caller: &CallerIdentity, request_id: &str) -> Result<()> {
        policy::require_first_party(caller, "answering consent requests")?;
        let mut st = self.state.lock().await;
        let p = st
            .pending
            .remove(request_id)
            .ok_or_else(|| Error::NotFound(format!("request {request_id}")))?;
        st.decided.insert(
            request_id.into(),
            (
                p.view.caller_fp.clone(),
                Decision::Denied {
                    reason: "the user declined".into(),
                },
            ),
        );
        let login = p.view.request.actions.contains(&Action::Login);
        let mut e = ev(
            if login {
                "login.denied"
            } else {
                "consent.deny"
            },
            caller,
            "deny",
        );
        e.target = p.view.request.targets.first().cloned();
        e.detail = format!("request={request_id} requester={}", p.view.caller_fp);
        Self::audit(&mut st, e);
        drop(st);
        self.decided.notify_waiters();
        Ok(())
    }

    /// Approves a request (first party + user presence), optionally
    /// narrowed, importing missing sites first. Mints the requester's token.
    pub async fn approve(
        &self,
        caller: &CallerIdentity,
        request_id: &str,
        opts: ApproveOptions,
    ) -> Result<Grant> {
        let r = self.approve_inner(caller, request_id, opts).await;
        if r.is_ok() {
            crate::telemetry::consent(
                &self.telemetry,
                cua_telemetry::events::ConsentDecision::Approved,
            );
        }
        r
    }

    async fn approve_inner(
        &self,
        caller: &CallerIdentity,
        request_id: &str,
        opts: ApproveOptions,
    ) -> Result<Grant> {
        policy::require_first_party(caller, "approving consent requests")?;
        let view = {
            let st = self.state.lock().await;
            if Self::settings(&st)?.disabled {
                return Err(Error::Disabled);
            }
            st.pending
                .get(request_id)
                .map(|p| p.view.clone())
                .ok_or_else(|| Error::NotFound(format!("request {request_id}")))?
        };
        let targets: Vec<String> = match &opts.targets {
            Some(t) => view
                .request
                .targets
                .iter()
                .filter(|x| t.contains(x))
                .cloned()
                .collect(),
            None => view.request.targets.clone(),
        };
        if targets.is_empty() {
            return Err(Error::Invalid("no target left after narrowing".into()));
        }
        // Red-team F1: pin every approved target to the immutable id it
        // resolves to right now. Delivery re-resolves and refuses a changed id.
        let target_ids = self.resolve_target_ids(&targets)?;
        let secs = policy::clamp_grant_secs(
            opts.duration_secs
                .or(view.request.duration_secs)
                .unwrap_or(DEFAULT_GRANT_SECS),
        );
        let uses = match opts.uses.or(view.request.uses) {
            Some(0) => None,
            Some(n) => Some(n.min(1000)),
            None => Some(1),
        };
        let what = describe_selection(&view);
        let reason = if view.request.actions.contains(&Action::Login) {
            let who = match &view.request.agent {
                Some(a) => format!("agent {a} ({})", view.caller_display),
                None => view.caller_display.clone(),
            };
            let sites: Vec<String> = view.items.iter().filter_map(|i| i.site.clone()).collect();
            format!(
                "Allow {who} to sign in to {} in {} ({})",
                if sites.is_empty() {
                    what.clone()
                } else {
                    sites.join(", ")
                },
                describe_targets(&targets, &target_ids),
                match uses {
                    Some(1) => "once".to_string(),
                    Some(n) => format!("{n} times"),
                    None => format!("for {}", human_secs(secs)),
                }
            )
        } else {
            format!(
                "Allow {} to teleport {} to {} for {}",
                view.caller_display,
                what,
                describe_targets(&targets, &target_ids),
                human_secs(secs)
            )
        };
        self.confirm(reason).await?;

        // Import what the request named but the vault does not hold yet.
        let mut item_ids: Vec<String> = view.items.iter().map(|i| i.id.clone()).collect();
        for spec in imports_for(&view.needs_import, &opts) {
            for it in self.import_confirmed(caller, spec).await? {
                if !item_ids.contains(&it.id) {
                    item_ids.push(it.id);
                }
            }
        }
        if let Some(keep) = &opts.items {
            item_ids.retain(|i| keep.contains(i));
        }
        if item_ids.is_empty() {
            return Err(Error::Invalid("no items left to grant".into()));
        }
        let now = crate::now_ms();
        let mut st = self.state.lock().await;
        if st.pending.remove(request_id).is_none() {
            return Err(Error::NotFound(format!(
                "request {request_id} (already answered)"
            )));
        }
        let v = Self::vault_mut(&mut st)?;
        {
            let meta = v.meta()?;
            for i in &item_ids {
                let it = meta
                    .items
                    .get(i)
                    .ok_or_else(|| Error::NotFound(format!("item {i}")))?;
                for t in &targets {
                    if !it.policy.allows_target(t) {
                        return Err(Error::Forbidden(format!(
                            "item {} may not go to {t} (its policy)",
                            it.label
                        )));
                    }
                }
            }
        }
        let grant = Grant {
            id: uid()?,
            request_id: request_id.into(),
            caller_fp: view.caller_fp.clone(),
            caller_display: view.caller_display.clone(),
            items: item_ids.clone(),
            targets: targets.clone(),
            target_ids: target_ids.clone(),
            actions: view.request.actions.clone(),
            created_ms: now,
            not_after_ms: now + secs * 1000,
            uses_left: uses,
            revoked: false,
            agent: view.request.agent.clone(),
        };
        let epoch = v.update_meta(|m| {
            m.grants.push(grant.clone());
            Ok(m.epoch)
        })?;
        let token = self.mint(&grant, epoch)?;
        st.decided.insert(
            request_id.into(),
            (
                view.caller_fp.clone(),
                Decision::Granted {
                    token: token.encode(),
                    grant_id: grant.id.clone(),
                    items: item_ids,
                    targets,
                    not_after_ms: grant.not_after_ms,
                },
            ),
        );
        let mut e = ev("consent.allow", caller, "allow");
        e.detail = format!(
            "request={request_id} grant={} requester={} secs={secs}",
            grant.id, view.caller_fp
        );
        Self::audit(&mut st, e);
        drop(st);
        self.decided.notify_waiters();
        Ok(grant)
    }

    fn mint(&self, grant: &Grant, epoch: u64) -> Result<Capability> {
        let actions: BTreeSet<Action> = grant.actions.iter().copied().collect();
        Ok(Capability::mint(
            &self.root,
            &uid()?,
            &[
                Caveat::Grant(grant.id.clone()),
                Caveat::Caller(grant.caller_fp.clone()),
                Caveat::Actions(actions),
                Caveat::Items(grant.items.iter().cloned().collect()),
                Caveat::Targets(grant.targets.iter().cloned().collect()),
                // Bind the token to the immutable ids the grant was pinned to
                // (red-team F1). Delivery resolves the name and it must match.
                Caveat::TargetIds(grant.target_ids.values().cloned().collect()),
                Caveat::NotAfter(grant.not_after_ms / 1000),
                Caveat::Epoch(epoch),
            ],
        ))
    }

    /// Grants (first party).
    pub async fn list_grants(&self, caller: &CallerIdentity) -> Result<Vec<Grant>> {
        policy::require_first_party(caller, "listing grants")?;
        let mut st = self.state.lock().await;
        Ok(Self::vault_mut(&mut st)?.meta()?.grants.clone())
    }

    /// Revokes a grant, or every grant with `"*"` (first party). Revoking
    /// all also bumps the epoch, killing every outstanding token.
    pub async fn revoke_grant(&self, caller: &CallerIdentity, id: &str) -> Result<usize> {
        policy::require_first_party(caller, "revoking grants")?;
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        let n = v.update_meta(|m| {
            let mut n = 0;
            for g in &mut m.grants {
                if (id == "*" || g.id == id) && !g.revoked {
                    g.revoked = true;
                    n += 1;
                }
            }
            if id == "*" {
                m.epoch += 1;
            }
            Ok(n)
        })?;
        if n == 0 && id != "*" {
            return Err(Error::NotFound(format!("grant {id}")));
        }
        let mut e = ev("grant.revoke", caller, "ok");
        e.detail = format!("grant={id} count={n}");
        Self::audit(&mut st, e);
        Ok(n)
    }

    // ------------------------------------------------------------------
    // Rules
    // ------------------------------------------------------------------

    /// Rules (first party).
    pub async fn list_rules(&self, caller: &CallerIdentity) -> Result<Vec<UnattendedRule>> {
        policy::require_first_party(caller, "listing rules")?;
        let mut st = self.state.lock().await;
        Ok(Self::vault_mut(&mut st)?.meta()?.rules.clone())
    }

    /// Adds an unattended rule (first party + presence). Callers must be
    /// team-signed identities the vault has seen verified: a fingerprint
    /// from a grant or a pending request.
    pub async fn add_rule(
        &self,
        caller: &CallerIdentity,
        spec: RuleSpec,
    ) -> Result<UnattendedRule> {
        policy::require_first_party(caller, "adding unattended rules")?;
        for t in &spec.targets {
            if t != "*" {
                validate_target(t)?;
            }
        }
        let now = crate::now_ms();
        // Red-team F1: pin each concretely-named target to its immutable id
        // now. `*` cannot be pinned and stays an explicit "any Space" opt-in.
        let concrete: Vec<String> = spec.targets.iter().filter(|t| *t != "*").cloned().collect();
        let target_ids = self.resolve_target_ids(&concrete)?;
        let mut rule = UnattendedRule {
            id: uid()?,
            items: spec.items.clone(),
            targets: spec.targets.clone(),
            target_ids,
            callers: spec.callers.clone(),
            created_ms: now,
            not_after_ms: now + spec.duration_secs.unwrap_or(DEFAULT_RULE_SECS) * 1000,
            enabled: true,
            note: spec.note.clone(),
        };
        {
            let mut st = self.state.lock().await;
            let known = self.known_rule_callers(&mut st)?;
            let meta = Self::vault_mut(&mut st)?.meta()?.clone();
            policy::validate_rule(&mut rule, &meta, now)?;
            for c in &mut rule.callers {
                match known.get(&c.fp) {
                    Some(id) if policy::rule_eligible(id) => c.display = id.display(),
                    Some(_) => {
                        return Err(Error::Forbidden(format!(
                            "{} is not team-signed; unattended rules need a signed app",
                            c.fp
                        )));
                    }
                    None => {
                        return Err(Error::Invalid(format!(
                            "unknown caller {}: it must have asked the Keyvault before (see grants or pending)",
                            c.fp
                        )));
                    }
                }
            }
        }
        let who: Vec<String> = rule.callers.iter().map(|c| c.display.clone()).collect();
        self.confirm(format!(
            "Let {} teleport {} item(s) to {} without asking, for {}",
            who.join(", "),
            rule.items.len(),
            rule.targets.join(", "),
            human_secs((rule.not_after_ms - now) / 1000)
        ))
        .await?;
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        v.update_meta(|m| {
            m.rules.push(rule.clone());
            Ok(())
        })?;
        let mut e = ev("rule.add", caller, "ok");
        e.detail = format!("rule={}", rule.id);
        Self::audit(&mut st, e);
        Ok(rule)
    }

    /// Callers the vault has verified in this run or recorded in grants.
    fn known_rule_callers(&self, st: &mut State) -> Result<HashMap<String, CallerIdentity>> {
        let mut out = HashMap::new();
        for p in st.pending.values() {
            out.insert(p.view.caller_fp.clone(), p.view.caller.clone());
        }
        // Grants store the display only; rebuild a signed identity marker
        // from the pending/seen cache when available.
        for (fp, id) in st.seen_callers() {
            out.entry(fp).or_insert(id);
        }
        Ok(out)
    }

    /// Removes (or disables) a rule (first party).
    pub async fn remove_rule(&self, caller: &CallerIdentity, id: &str) -> Result<()> {
        policy::require_first_party(caller, "removing rules")?;
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        v.update_meta(|m| {
            let before = m.rules.len();
            m.rules.retain(|r| r.id != id);
            if before == m.rules.len() {
                return Err(Error::NotFound(format!("rule {id}")));
            }
            Ok(())
        })?;
        let mut e = ev("rule.remove", caller, "ok");
        e.detail = format!("rule={id}");
        Self::audit(&mut st, e);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Teleport and release
    // ------------------------------------------------------------------

    /// Delivers items into a target, authorized by a token, a rule, or (for
    /// first parties) user presence right now.
    pub async fn teleport(
        &self,
        caller: &CallerIdentity,
        req: TeleportRequest,
    ) -> Result<TeleportOutcome> {
        let started = std::time::Instant::now();
        let items = req.items.len() as u64;
        // The provider (a public catalog id such as `chrome`) of the first
        // item, read without changing anything; `other` when unknown.
        let app = {
            let st = self.state.lock().await;
            st.vault
                .as_ref()
                .filter(|v| v.is_unlocked())
                .and_then(|v| v.meta().ok())
                .and_then(|m| {
                    req.items
                        .first()
                        .and_then(|i| m.items.get(i))
                        .map(|it| it.provider_id.clone())
                })
                .unwrap_or_default()
        };
        let r = self
            .teleport_inner(caller, req, PresenceAlready::NotConfirmed, None)
            .await;
        crate::telemetry::teleport(&self.telemetry, caller, &app, started, &r, items);
        r
    }

    async fn teleport_inner(
        &self,
        caller: &CallerIdentity,
        req: TeleportRequest,
        presence: PresenceAlready,
        stage: Option<StageSink>,
    ) -> Result<TeleportOutcome> {
        validate_target(&req.target)?;
        if req.items.is_empty() {
            return Err(Error::Invalid("no items".into()));
        }
        let fp = caller.fingerprint();
        let now = crate::now_ms();
        // Red-team F1: resolve the requested name to its immutable id now, so
        // authorization and delivery both act on the id the user approved, not
        // a name an attacker may have re-pointed to their own Space.
        let target_id = self.backend.resolve_target(&req.target)?;
        if target_id.is_empty() {
            return Err(Error::Invalid(format!(
                "target {:?} did not resolve to a Space identity",
                req.target
            )));
        }
        // Phase 1 (locked): authorize and read payloads.
        let (authority, groups, labels, ttl, auto_wipe) = {
            let mut st = self.state.lock().await;
            let disabled = Self::settings(&st)?.disabled;
            if disabled {
                let mut e = ev("teleport.deny", caller, "deny");
                e.target = Some(req.target.clone());
                e.detail = "kill switch".into();
                Self::audit(&mut st, e);
                return Err(Error::Disabled);
            }
            let presence_soft_locked = {
                let s = Self::settings(&st)?;
                s.unlock_policy == UnlockPolicy::Presence && now >= st.presence_until_ms
            };
            let v = Self::vault_mut(&mut st)?;
            let meta = v.meta()?.clone();
            for i in &req.items {
                let it = meta
                    .items
                    .get(i)
                    .ok_or_else(|| Error::NotFound(format!("item {i}")))?;
                if !it.policy.allows_target(&req.target) {
                    return Err(Error::Forbidden(format!(
                        "item {i} may not go to {} (its policy)",
                        req.target
                    )));
                }
            }
            let authority = if let Some(tok) = &req.token {
                let cap = Capability::decode(tok)?;
                let grants = meta.grants.clone();
                let live = move |g: &str| grants.iter().any(|x| x.id == g && x.is_live(now));
                let mut grant = None;
                for i in &req.items {
                    let nonces = &mut st.nonces;
                    let mut consume = |n: &str| nonces.insert(n.to_string(), now).is_none();
                    let mut ctx = VerifyContext {
                        now: now / 1000,
                        caller: &fp,
                        action: Action::Teleport,
                        item: i,
                        target: Some(&req.target),
                        target_id: Some(&target_id),
                        epoch: meta.epoch,
                        grant_is_live: &live,
                        consume_nonce: &mut consume,
                    };
                    match cap.verify(&self.root, &mut ctx) {
                        Ok(g) => grant = Some(g),
                        Err(err) => {
                            let mut e = ev("teleport.deny", caller, "deny");
                            e.item = Some(i.clone());
                            e.target = Some(req.target.clone());
                            e.detail = err.to_string();
                            Self::audit(&mut st, e);
                            return Err(err);
                        }
                    }
                }
                Authority::Grant(grant.expect("items is non-empty"))
            } else if let Some(rule_id) = req
                .items
                .iter()
                .map(|i| {
                    policy::matching_rule(&meta, &fp, i, &req.target, now).map(|r| r.id.clone())
                })
                .collect::<Option<Vec<_>>>()
                .and_then(|ids| ids.into_iter().next())
            {
                if presence_soft_locked {
                    return Err(Error::Locked);
                }
                // Red-team F1: a concretely-named rule target is pinned to an
                // immutable id; refuse if that id changed. `*` targets carry no
                // pin (an explicit "any Space" opt-in).
                if let Some(pinned) = meta
                    .rules
                    .iter()
                    .find(|r| r.id == rule_id)
                    .and_then(|r| r.target_ids.get(&req.target))
                    && pinned != &target_id
                {
                    let mut e = ev("teleport.deny", caller, "deny");
                    e.target = Some(req.target.clone());
                    e.detail = "rule target identity changed (rebinding)".into();
                    Self::audit(&mut st, e);
                    return Err(Error::Forbidden(
                        "the rule's target identity changed since it was authored; refusing (rebinding)"
                            .into(),
                    ));
                }
                Authority::Rule(rule_id)
            } else if caller.first_party {
                Authority::Interactive
            } else {
                let mut e = ev("teleport.deny", caller, "deny");
                e.target = Some(req.target.clone());
                e.detail = "no token or rule".into();
                Self::audit(&mut st, e);
                return Err(Error::Forbidden(
                    "request access first (the user approves it in Cua)".into(),
                ));
            };
            let v = Self::vault_mut(&mut st)?;
            if let Authority::Grant(g) = &authority {
                v.update_meta(|m| {
                    if let Some(gr) = m.grants.iter_mut().find(|x| &x.id == g)
                        && let Some(n) = gr.uses_left.as_mut()
                    {
                        *n = n.saturating_sub(1);
                    }
                    Ok(())
                })?;
            }
            // Group by provider, adding what is already live on the target
            // for that provider (one live import per target and provider;
            // the new one supersedes the old).
            let mut groups: BTreeMap<String, (Vec<String>, Vec<Delivery>)> = BTreeMap::new();
            for i in &req.items {
                let p = meta.items[i].provider_id.clone();
                groups.entry(p).or_default().0.push(i.clone());
            }
            for d in meta
                .deliveries
                .iter()
                .filter(|d| d.live(now) && d.target == req.target)
            {
                if let Some(g) = groups.get_mut(&d.provider_id) {
                    for i in &d.items {
                        if meta.items.contains_key(i) && !g.0.contains(i) {
                            g.0.push(i.clone());
                        }
                    }
                    g.1.push(d.clone());
                }
            }
            let ttl = req
                .items
                .iter()
                .map(|i| meta.items[i].policy.ttl_secs)
                .min()
                .unwrap_or(crate::model::DEFAULT_TTL_SECS);
            let auto_wipe = meta.settings.auto_wipe;
            let mut out = Vec::new();
            for (provider, (ids, superseded)) in groups {
                let mut payloads = Vec::new();
                for i in &ids {
                    // Saved logins stay sealed: only the site-login fill
                    // reads them, never a delivery.
                    let p = v.read_payload(i)?.deliverable();
                    if !p.entries.is_empty() {
                        payloads.push(p);
                    }
                }
                if !payloads.is_empty() {
                    out.push((provider, ids, payloads, superseded));
                }
            }
            if out.is_empty() {
                return Err(Error::Invalid(
                    "these items hold only saved passwords, which sign in through site login \
                     and are never delivered"
                        .into(),
                ));
            }
            let labels: Vec<String> = req
                .items
                .iter()
                .map(|i| meta.items[i].label.clone())
                .collect();
            (authority, out, labels, ttl, auto_wipe)
        };
        if authority == Authority::Interactive && presence == PresenceAlready::NotConfirmed {
            self.confirm(format!(
                "Teleport {} to {} [{}] ({})",
                labels.join(", "),
                req.target,
                short_id(&target_id),
                caller.display()
            ))
            .await?;
        }
        // Record the authorization before anything leaves the vault, and
        // refuse when the log cannot take it (grants, rules and interactive
        // alike), so no delivery is ever missing from the audit log.
        {
            let mut st = self.state.lock().await;
            let mut e = ev("teleport.authorize", caller, "allow");
            e.target = Some(req.target.clone());
            e.detail = format!("{} items={}", authority.describe(), req.items.len());
            Self::audit_required(&mut st, e)?;
        }
        // Phase 2 (unlocked): supersede, deliver.
        // Auto-wipe off (the default): the copy stays until it is wiped.
        let expires_ms = crate::model::delivery_expiry(auto_wipe, now, ttl);
        let mut outcome = TeleportOutcome {
            authority: authority.describe(),
            expires_ms,
            items: req.items.clone(),
            ..Default::default()
        };
        let mut delivered = Vec::new();
        for (provider, ids, payloads, superseded) in groups {
            for d in &superseded {
                // Best effort: a failed wipe of the superseded import still
                // leaves its own TTL to clean up.
                let _ = self.backend.wipe(&req.target, &d.import_id).await;
            }
            let res = match &stage {
                Some(sink) => {
                    self.backend
                        .deliver_with_progress(
                            &req.target,
                            &provider,
                            payloads,
                            expires_ms,
                            sink.clone(),
                        )
                        .await
                }
                None => {
                    self.backend
                        .deliver(&req.target, &provider, payloads, expires_ms)
                        .await
                }
            };
            match res {
                Ok(o) => {
                    delivered.push((provider, ids, o.clone(), superseded));
                    outcome.deliveries.push(o);
                }
                Err(err) => {
                    let mut st = self.state.lock().await;
                    let mut e = ev("teleport.deliver", caller, "error");
                    e.target = Some(req.target.clone());
                    e.detail = format!("{} {err}", authority.describe());
                    Self::audit(&mut st, e);
                    return Err(err);
                }
            }
        }
        // Phase 3 (locked): record; if the kill switch flipped meanwhile, wipe.
        let mut st = self.state.lock().await;
        let disabled_now = Self::settings(&st).map(|s| s.disabled).unwrap_or(true);
        let v = Self::vault_mut(&mut st)?;
        v.update_meta(|m| {
            for (provider, ids, o, superseded) in &delivered {
                for d in &mut m.deliveries {
                    if superseded.iter().any(|s| s.import_id == d.import_id) {
                        d.wiped = true;
                    }
                }
                m.deliveries.push(Delivery {
                    import_id: o.import_id.clone(),
                    target: req.target.clone(),
                    provider_id: provider.clone(),
                    items: ids.clone(),
                    caller_fp: fp.clone(),
                    delivered_ms: now,
                    expires_ms,
                    wiped: false,
                });
            }
            Ok(())
        })?;
        for i in &req.items {
            let mut e = ev("teleport.deliver", caller, "allow");
            e.item = Some(i.clone());
            e.target = Some(req.target.clone());
            e.detail = authority.describe();
            Self::audit(&mut st, e);
        }
        drop(st);
        if disabled_now {
            let _ = self.wipe_where(caller, |d| d.target == req.target).await;
            return Err(Error::Disabled);
        }
        Ok(outcome)
    }

    /// Signs in to a site in a target Space with a saved password. The
    /// Keyvault picks the saved login for the page's exact origin and has
    /// the backend fill and submit the form through the Space's cua-driver;
    /// the password never leaves the daemon except as keystrokes into that
    /// page, and never appears in a result, error or audit entry.
    ///
    /// Authorization is the same as teleport's, with the `login` action:
    /// a third party needs a capability token from an approved request
    /// (one use by default) or a user-authored unattended rule; a first
    /// party confirms with user presence each time.
    pub async fn login(&self, caller: &CallerIdentity, req: LoginRequest) -> Result<LoginOutcome> {
        validate_target(&req.target)?;
        let origin = origin_of(&req.url).ok_or_else(|| {
            Error::Invalid(format!(
                "{:?} is not an http(s) page to sign in on",
                req.url
            ))
        })?;
        let host = origin_host(&origin);
        let fp = caller.fingerprint();
        let now = crate::now_ms();
        let target_id = self.backend.resolve_target(&req.target)?;
        if target_id.is_empty() {
            return Err(Error::Invalid(format!(
                "target {:?} did not resolve to a Space identity",
                req.target
            )));
        }
        let denied = |st: &mut State, detail: String| {
            let mut e = ev("login.denied", caller, "deny");
            e.target = Some(req.target.clone());
            e.detail = detail;
            Self::audit(st, e);
        };
        // Phase 1 (locked): find the item, authorize, pick the login.
        let (authority, item_id, site, login, approved_agent) = {
            let mut st = self.state.lock().await;
            if Self::settings(&st)?.disabled {
                denied(&mut st, format!("origin={origin} kill switch"));
                return Err(Error::Disabled);
            }
            let presence_soft_locked = {
                let s = Self::settings(&st)?;
                s.unlock_policy == UnlockPolicy::Presence && now >= st.presence_until_ms
            };
            let v = Self::vault_mut(&mut st)?;
            let meta = v.meta()?.clone();
            let candidates: Vec<&ItemMeta> = meta
                .items
                .values()
                .filter(|i| {
                    i.kind == ItemKind::SitePasswords
                        && i.site.as_deref().is_some_and(|s| host_in_site(&host, s))
                        && i.policy.allows_target(&req.target)
                })
                .collect();
            if candidates.is_empty() {
                denied(&mut st, format!("origin={origin} no saved password"));
                return Err(Error::NotFound(format!(
                    "no saved password for {host} may be used in {} (import it in Cua first)",
                    req.target
                )));
            }
            // Authorize against the first candidate the caller may use.
            let mut chosen: Option<(Authority, String)> = None;
            let mut last_err: Option<Error> = None;
            for it in &candidates {
                if let Some(tok) = &req.token {
                    let cap = Capability::decode(tok)?;
                    let grants = meta.grants.clone();
                    let live = move |g: &str| grants.iter().any(|x| x.id == g && x.is_live(now));
                    let nonces = &mut st.nonces;
                    let mut consume = |n: &str| nonces.insert(n.to_string(), now).is_none();
                    let mut ctx = VerifyContext {
                        now: now / 1000,
                        caller: &fp,
                        action: Action::Login,
                        item: &it.id,
                        target: Some(&req.target),
                        target_id: Some(&target_id),
                        epoch: meta.epoch,
                        grant_is_live: &live,
                        consume_nonce: &mut consume,
                    };
                    match cap.verify(&self.root, &mut ctx) {
                        Ok(g) => {
                            chosen = Some((Authority::Grant(g), it.id.clone()));
                            break;
                        }
                        Err(e) => last_err = Some(e),
                    }
                } else if let Some(rule) =
                    policy::matching_rule(&meta, &fp, &it.id, &req.target, now)
                {
                    if presence_soft_locked {
                        return Err(Error::Locked);
                    }
                    if let Some(pinned) = rule.target_ids.get(&req.target)
                        && pinned != &target_id
                    {
                        last_err = Some(Error::Forbidden(
                            "the rule's target identity changed since it was authored; refusing (rebinding)"
                                .into(),
                        ));
                        continue;
                    }
                    chosen = Some((Authority::Rule(rule.id.clone()), it.id.clone()));
                    break;
                } else if caller.first_party {
                    chosen = Some((Authority::Interactive, it.id.clone()));
                    break;
                }
            }
            let Some((authority, item_id)) = chosen else {
                let err = last_err.unwrap_or_else(|| {
                    Error::Forbidden(
                        "request access first (the user approves each sign-in in Cua)".into(),
                    )
                });
                denied(&mut st, format!("origin={origin} {err}"));
                return Err(err);
            };
            let v = Self::vault_mut(&mut st)?;
            let logins = v.read_payload(&item_id)?.logins()?;
            let pick = logins.iter().find(|l| {
                l.origin == origin
                    && req
                        .username
                        .as_deref()
                        .is_none_or(|u| u.eq_ignore_ascii_case(&l.username))
            });
            let Some(login) = pick.cloned() else {
                drop(logins);
                denied(
                    &mut st,
                    format!("origin={origin} no saved login for this origin"),
                );
                return Err(Error::NotFound(format!(
                    "no saved login for {origin}{}",
                    req.username
                        .as_deref()
                        .map(|u| format!(" and user {}", crate::model::mask_username(u)))
                        .unwrap_or_default()
                )));
            };
            drop(logins);
            if let Authority::Grant(g) = &authority {
                let v = Self::vault_mut(&mut st)?;
                v.update_meta(|m| {
                    if let Some(gr) = m.grants.iter_mut().find(|x| &x.id == g)
                        && let Some(n) = gr.uses_left.as_mut()
                    {
                        *n = n.saturating_sub(1);
                    }
                    Ok(())
                })?;
            }
            let site = meta.items[&item_id].site.clone().unwrap_or_default();
            // The agent the user approved (the grant's) names the use; a
            // caller cannot relabel an approved sign-in as another agent's.
            let approved_agent = match &authority {
                Authority::Grant(g) => meta
                    .grants
                    .iter()
                    .find(|x| &x.id == g)
                    .and_then(|x| x.agent.clone()),
                _ => None,
            };
            (authority, item_id, site, login, approved_agent)
        };
        let hint = crate::model::mask_username(&login.username);
        if authority == Authority::Interactive {
            self.confirm(format!(
                "Sign in to {site} as {hint} in {} [{}] ({})",
                req.target,
                short_id(&target_id),
                caller.display()
            ))
            .await?;
        }
        let agent = approved_agent
            .as_deref()
            .or(req.agent.as_deref())
            .map(|a| format!(" agent={a}"))
            .unwrap_or_default();
        {
            let mut st = self.state.lock().await;
            let mut e = ev("login.authorize", caller, "allow");
            e.item = Some(item_id.clone());
            e.target = Some(req.target.clone());
            e.detail = format!(
                "{} origin={origin} user={hint}{agent}",
                authority.describe()
            );
            Self::audit_required(&mut st, e)?;
        }
        let fill = LoginFill {
            url: req.url.clone(),
            origin: origin.clone(),
            username: login.username.clone(),
            password: zeroize::Zeroizing::new(login.password.clone()),
            browser: req.browser.clone(),
            relay_plaintext_ack: req.relay_plaintext_ack,
        };
        drop(login);
        let res = self.backend.fill_login(&req.target, &fill).await;
        drop(fill);
        let mut st = self.state.lock().await;
        match res {
            Ok(filled) => {
                let mut e = ev("login.fill", caller, "allow");
                e.item = Some(item_id.clone());
                e.target = Some(req.target.clone());
                e.detail = format!(
                    "{} origin={origin} user={hint} submitted={}{agent}",
                    authority.describe(),
                    filled.submitted
                );
                Self::audit(&mut st, e);
                Ok(LoginOutcome {
                    site,
                    origin,
                    username_hint: hint,
                    item: item_id,
                    authority: authority.describe(),
                    filled,
                })
            }
            Err(err) => {
                let mut e = ev("login.fill", caller, "error");
                e.item = Some(item_id);
                e.target = Some(req.target.clone());
                e.detail = format!("{} origin={origin} {err}{agent}", authority.describe());
                Self::audit(&mut st, e);
                Err(err)
            }
        }
    }

    /// Wipes live deliveries to `target` (all of them for first parties;
    /// only the caller's own for third parties).
    pub async fn release(&self, caller: &CallerIdentity, target: &str) -> Result<Vec<String>> {
        let fp = caller.fingerprint();
        let first = caller.first_party;
        self.wipe_where(caller, |d| {
            d.target == target && (first || d.caller_fp == fp)
        })
        .await
    }

    async fn wipe_where(
        &self,
        caller: &CallerIdentity,
        pred: impl Fn(&Delivery) -> bool,
    ) -> Result<Vec<String>> {
        let live: Vec<Delivery> = {
            let mut st = self.state.lock().await;
            let v = Self::vault_mut(&mut st)?;
            v.meta()?
                .deliveries
                .iter()
                .filter(|d| !d.wiped && pred(d))
                .cloned()
                .collect()
        };
        let mut wiped_ids = Vec::new();
        let mut errors = Vec::new();
        for d in &live {
            match self.backend.wipe(&d.target, &d.import_id).await {
                Ok(_) => wiped_ids.push(d.import_id.clone()),
                Err(e) => errors.push(format!("{}: {e}", d.target)),
            }
        }
        let mut st = self.state.lock().await;
        let v = Self::vault_mut(&mut st)?;
        v.update_meta(|m| {
            for d in &mut m.deliveries {
                if wiped_ids.contains(&d.import_id) {
                    d.wiped = true;
                }
            }
            Ok(())
        })?;
        for d in live.iter().filter(|d| wiped_ids.contains(&d.import_id)) {
            let mut e = ev("target.wipe", caller, "ok");
            e.target = Some(d.target.clone());
            e.detail = format!("import={}", d.import_id);
            Self::audit(&mut st, e);
        }
        if !errors.is_empty() {
            let mut e = ev("target.wipe", caller, "error");
            e.detail = errors.join("; ");
            Self::audit(&mut st, e);
            return Err(Error::Backend(format!(
                "could not wipe every delivery (the target's own expiry still applies): {}",
                errors.join("; ")
            )));
        }
        Ok(wiped_ids)
    }

    /// Live deliveries (first party).
    pub async fn list_deliveries(&self, caller: &CallerIdentity) -> Result<Vec<Delivery>> {
        policy::require_first_party(caller, "listing deliveries")?;
        let mut st = self.state.lock().await;
        Ok(Self::vault_mut(&mut st)?.meta()?.deliveries.clone())
    }

    // ------------------------------------------------------------------
    // Audit
    // ------------------------------------------------------------------

    /// Recent audit entries (first party).
    pub async fn audit_tail(
        &self,
        caller: &CallerIdentity,
        limit: usize,
    ) -> Result<Vec<AuditEntry>> {
        policy::require_first_party(caller, "reading the audit log")?;
        let st = self.state.lock().await;
        st.vault
            .as_ref()
            .ok_or_else(|| Error::NoVault("the Keyvault is not set up".into()))?
            .audit_tail(limit.min(10_000))
    }

    /// Verifies the audit chain (first party).
    pub async fn verify_audit(&self, caller: &CallerIdentity) -> Result<Verification> {
        policy::require_first_party(caller, "verifying the audit log")?;
        let mut st = self.state.lock().await;
        Self::vault_mut(&mut st)?.verify_audit()
    }

    /// Records a refused connection (called by the IPC server).
    pub async fn record_rejected(&self, detail: &str) {
        let mut st = self.state.lock().await;
        let e = AuditEvent {
            kind: "caller.reject".into(),
            actor: String::new(),
            caller_fp: String::new(),
            item: None,
            target: None,
            decision: "deny".into(),
            detail: detail.chars().take(200).collect(),
        };
        Self::audit(&mut st, e);
    }

    /// Remembers a verified caller (so rules can name it later).
    pub async fn remember_caller(&self, caller: &CallerIdentity) {
        let mut st = self.state.lock().await;
        st.remember(caller);
    }
}

impl State {
    fn remember(&mut self, caller: &CallerIdentity) {
        if self.seen.len() > 256 {
            self.seen.clear();
        }
        self.seen.insert(caller.fingerprint(), caller.clone());
    }

    fn seen_callers(&self) -> Vec<(String, CallerIdentity)> {
        self.seen
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Space names: 1 to 128 characters of `[A-Za-z0-9._:@-]`.
pub fn validate_target(t: &str) -> Result<()> {
    let ok = !t.is_empty()
        && t.len() <= 128
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b':' | b'@'));
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("bad target name {t:?}")))
    }
}

/// A short, human-comparable form of an immutable target id for prompts.
fn short_id(id: &str) -> String {
    if id.len() <= 12 {
        id.to_string()
    } else {
        format!("{}…", &id[..12])
    }
}

/// Names with their pinned ids, for the consent prompt (red-team F1: the human
/// sees the immutable identity, not only the mutable name).
fn describe_targets(names: &[String], ids: &BTreeMap<String, String>) -> String {
    names
        .iter()
        .map(|n| match ids.get(n) {
            Some(id) => format!("{n} [{}]", short_id(id)),
            None => n.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn human_secs(secs: u64) -> String {
    match secs {
        s if s < 120 => format!("{s} seconds"),
        s if s < 7200 => format!("{} minutes", s / 60),
        s if s < 172_800 => format!("{} hours", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

fn describe_selection(view: &PendingView) -> String {
    let mut parts: Vec<String> = view.items.iter().map(|i| i.label.clone()).collect();
    for s in &view.needs_import {
        match s {
            Selector::Site { app, site, .. } => parts.push(format!("{site} ({app}, import)")),
            Selector::App { app } => parts.push(format!("the whole {app} session (import)")),
            Selector::Item { id } => parts.push(id.clone()),
            Selector::Login { site } => parts.push(format!("the saved password for {site}")),
        }
    }
    if parts.len() > 4 {
        let n = parts.len() - 3;
        parts.truncate(3);
        parts.push(format!("{n} more"));
    }
    parts.join(", ")
}

/// What `spec` selects, for a confirmation prompt (`"the whole chrome
/// session"`, `"github.com, example.com from chrome INCLUDING SAVED
/// PASSWORDS"`). Shared by [`import_reason`] and
/// [`Broker::import_and_teleport`]'s reason so the two prompts describe a
/// spec identically.
fn describe_spec(spec: &ImportSpec) -> String {
    let what = if spec.whole_app {
        format!("the whole {} session", spec.app)
    } else {
        let sites: Vec<&str> = spec.sites.iter().map(|s| s.site.as_str()).collect();
        format!("{} from {}", sites.join(", "), spec.app)
    };
    let pw = if spec.sites.iter().any(|s| s.include_passwords) {
        " INCLUDING SAVED PASSWORDS"
    } else {
        ""
    };
    format!("{what}{pw}")
}

fn import_reason(spec: &ImportSpec, caller: &CallerIdentity) -> String {
    format!(
        "Import {} into the Cua Keyvault ({})",
        describe_spec(spec),
        caller.display()
    )
}

fn imports_for(needs: &[Selector], opts: &ApproveOptions) -> Vec<ImportSpec> {
    let mut by_app: BTreeMap<String, ImportSpec> = BTreeMap::new();
    for s in needs {
        match s {
            Selector::Site { app, site, .. } => {
                let spec = by_app.entry(app.clone()).or_insert_with(|| ImportSpec {
                    app: app.clone(),
                    cookies: opts.cookies,
                    ..Default::default()
                });
                spec.sites.push(SiteChoice {
                    site: site.clone(),
                    include_storage: opts.include_storage,
                    include_passwords: false,
                });
            }
            Selector::App { app } => {
                by_app
                    .entry(format!("{app}#app"))
                    .or_insert_with(|| ImportSpec {
                        app: app.clone(),
                        whole_app: true,
                        paths: opts.paths.clone(),
                        ..Default::default()
                    });
            }
            Selector::Item { .. } | Selector::Login { .. } => {}
        }
    }
    by_app.into_values().collect()
}
