//! This device as a client of the cua.ai account on a relay.
//!
//! A device that lists or reaches the account's machines through the relay
//! holds a P-256 key (in the OS keychain for Cua-signed builds, else a 0600
//! file; see [`KeySlot`]) and is enrolled once with a second factor:
//!
//! - a fresh interactive sign-in enrolls it right away (`cua auth login` or
//!   the app's sign-in registers this device, creating its key on first
//!   use);
//! - otherwise it shows a one-time code that an enrolled device confirms
//!   (`cua devices approve <code>`, or the Cua Spaces app). Relays that
//!   predate sign-in enrollment for every device enroll only an account's
//!   first device by sign-in and show the code for the others.
//!
//! The device reports its machine id ([`crate::machine`]), so a new key of
//! the same machine replaces the old record instead of adding a device. Two
//! builds of cua on one machine share one key: the slot moves or reads the
//! other build's copy, and [`DeviceAuth::enroll`] adopts whichever of the
//! two keys the relay already enrolled and retires the other.
//!
//! Enrollment lasts the relay's TTL (30 days by default); then one approval
//! or a fresh sign-in re-verifies the device. In between, [`DeviceAuth::session`] proves the
//! key to the relay without prompts, so unattended agents keep working.
//!
//! Hosting never enrolls a device: a hosted machine joins with its machine
//! token only and cannot reach the account's other machines.

use crate::relay::{
    AccountTokens, AuditEvent, DeviceListing, DeviceState, DeviceView, Enrollment, RelayClient,
};
use crate::{Error, Result};
use base64::Engine as _;
use ring::signature::KeyPair as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// The name of the device key in a credential store.
pub const DEVICE_KEY_SECRET: &str = "device-key";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Message a device signs to register itself (the relay's
/// `register_message`).
pub fn register_message(device: &str, ts: u64) -> String {
    format!("cua-device-register/v1\n{device}\n{ts}")
}

/// Message a device signs to open a session (the relay's
/// `session_message`).
pub fn session_message(device: &str, ts: u64) -> String {
    format!("cua-device-session/v1\n{device}\n{ts}")
}

/// Where the device key (PKCS#8) is kept.
pub trait KeySlot: Send + Sync {
    /// The stored key, if any.
    fn load(&self) -> Result<Option<Vec<u8>>>;
    /// Stores the key.
    fn save(&self, pkcs8: &[u8]) -> Result<()>;
    /// Deletes the key; returns whether one existed.
    fn clear(&self) -> Result<bool>;
    /// Another key this machine keeps for the device apart from the slot's
    /// own: the copy a build of cua that stores it elsewhere made. None by
    /// default.
    fn alternate(&self) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    /// Makes the alternate key this slot's key (moving it into the stronger
    /// store), dropping the current one.
    fn adopt_alternate(&self) -> Result<()> {
        Ok(())
    }
    /// Forgets the alternate key where the slot may (a file another build
    /// left; never the OS vault's copy).
    fn drop_alternate(&self) -> Result<()> {
        Ok(())
    }
}

/// A 0600 file (base64 PKCS#8), written atomically.
#[derive(Clone, Debug)]
pub struct FileKeySlot(pub PathBuf);

impl KeySlot for FileKeySlot {
    fn load(&self) -> Result<Option<Vec<u8>>> {
        match std::fs::read_to_string(&self.0) {
            Ok(text) => base64::engine::general_purpose::STANDARD
                .decode(text.trim())
                .map(Some)
                .map_err(|_| Error::Internal(format!("{} is not a device key", self.0.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, pkcs8: &[u8]) -> Result<()> {
        if let Some(parent) = self.0.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = base64::engine::general_purpose::STANDARD.encode(pkcs8);
        cua_home::write_private(&self.0, text.as_bytes())?;
        Ok(())
    }

    fn clear(&self) -> Result<bool> {
        match std::fs::remove_file(&self.0) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

/// The device key in the same vault as the cua.ai session (the OS keychain
/// for Cua-signed builds, else a 0600 file next to the credentials file),
/// shared by the `cua` CLI, the daemon and the Cua Spaces app.
impl KeySlot for cua_auth::Store {
    fn load(&self) -> Result<Option<Vec<u8>>> {
        self.load_secret(DEVICE_KEY_SECRET)
            .map_err(|e| Error::Internal(e.to_string()))
    }

    fn save(&self, pkcs8: &[u8]) -> Result<()> {
        self.save_secret(DEVICE_KEY_SECRET, pkcs8)
            .map_err(|e| Error::Internal(e.to_string()))
    }

    fn clear(&self) -> Result<bool> {
        self.clear_secret(DEVICE_KEY_SECRET)
            .map_err(|e| Error::Internal(e.to_string()))
    }

    fn alternate(&self) -> Result<Option<Vec<u8>>> {
        self.load_alternate_secret(DEVICE_KEY_SECRET)
            .map_err(|e| Error::Internal(e.to_string()))
    }

    fn adopt_alternate(&self) -> Result<()> {
        self.adopt_alternate_secret(DEVICE_KEY_SECRET)
            .map_err(|e| Error::Internal(e.to_string()))
    }

    fn drop_alternate(&self) -> Result<()> {
        self.drop_alternate_secret(DEVICE_KEY_SECRET)
            .map_err(|e| Error::Internal(e.to_string()))
    }
}

/// An in-memory slot (tests, embedding).
#[derive(Default)]
pub struct MemoryKeySlot(std::sync::Mutex<Option<Vec<u8>>>);

impl KeySlot for MemoryKeySlot {
    fn load(&self) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().expect("slot").clone())
    }

    fn save(&self, pkcs8: &[u8]) -> Result<()> {
        *self.0.lock().expect("slot") = Some(pkcs8.to_vec());
        Ok(())
    }

    fn clear(&self) -> Result<bool> {
        Ok(self.0.lock().expect("slot").take().is_some())
    }
}

/// The device's P-256 key.
pub struct DeviceKey {
    pair: ring::signature::EcdsaKeyPair,
}

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceKey").field("id", &self.id()).finish()
    }
}

const ALG: &ring::signature::EcdsaSigningAlgorithm =
    &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;

impl DeviceKey {
    /// A new key and its PKCS#8 encoding.
    pub fn generate() -> Result<(Self, Vec<u8>)> {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(ALG, &rng)
            .map_err(|_| Error::Internal("cannot generate a device key".into()))?;
        let key = Self::from_pkcs8(pkcs8.as_ref())?;
        Ok((key, pkcs8.as_ref().to_vec()))
    }

    /// Parses a PKCS#8 key.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self> {
        let rng = ring::rand::SystemRandom::new();
        ring::signature::EcdsaKeyPair::from_pkcs8(ALG, pkcs8, &rng)
            .map(|pair| Self { pair })
            .map_err(|_| Error::Internal("the stored device key is invalid".into()))
    }

    /// The public key (uncompressed point, base64url).
    pub fn public_key(&self) -> String {
        b64().encode(self.pair.public_key().as_ref())
    }

    /// The device id the relay derives from the public key.
    pub fn id(&self) -> String {
        let digest = ring::digest::digest(&ring::digest::SHA256, self.pair.public_key().as_ref());
        format!("dev_{}", &hex::encode(digest.as_ref())[..24])
    }

    /// A fixed-size ECDSA signature of `message`, base64url.
    pub fn sign(&self, message: &str) -> Result<String> {
        let rng = ring::rand::SystemRandom::new();
        self.pair
            .sign(&rng, message.as_bytes())
            .map(|sig| b64().encode(sig.as_ref()))
            .map_err(|_| Error::Internal("cannot sign with the device key".into()))
    }
}

/// This device as a client of the account on one relay.
pub struct DeviceAuth {
    relay: RelayClient,
    tokens: Arc<dyn AccountTokens>,
    slot: Arc<dyn KeySlot>,
    name: String,
    machine_id: std::sync::OnceLock<Option<String>>,
    session: tokio::sync::Mutex<Option<(String, u64)>>,
    pending_file: Option<PathBuf>,
}

/// The one-time code this device last showed, remembered (in
/// `~/.cua/device-pending.json`) so `cua devices status` can show it again
/// and `cua devices enroll` reuses it instead of re-registering: a new
/// registration replaces the code and silently invalidates the one the user
/// may be typing on the other device.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PendingCode {
    /// Relay base URL.
    pub relay: String,
    /// `dev_…`.
    pub device_id: String,
    /// The one-time code.
    pub code: String,
    /// Unix seconds the code expires at.
    pub expires_at: u64,
}

/// A pending code's lifetime when the relay does not say.
const DEFAULT_CODE_TTL_SECS: u64 = 600;
/// Do not reuse a code that expires sooner than this.
const CODE_REUSE_MARGIN_SECS: u64 = 60;
/// A sign-in at most this old may still enroll by registering again (the
/// relay's default bootstrap window).
const FRESH_SIGN_IN_SECS: u64 = 600;

/// The unverified `auth_time` claim of a JWT access token (display and
/// local decisions only; the relay verifies).
fn token_auth_time(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let bytes = b64().decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("auth_time")?.as_u64()
}

impl std::fmt::Debug for DeviceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceAuth")
            .field("relay", &self.relay.base())
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// Renew a device session this long before it expires.
const SESSION_MARGIN_SECS: u64 = 60;

impl DeviceAuth {
    /// This device on `relay_url`, its key in `slot`, shown as `name`.
    pub fn new(
        relay_url: &str,
        tokens: Arc<dyn AccountTokens>,
        slot: Arc<dyn KeySlot>,
        name: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            relay: RelayClient::new(relay_url)?,
            tokens,
            slot,
            name: name.into(),
            machine_id: std::sync::OnceLock::new(),
            session: tokio::sync::Mutex::new(None),
            pending_file: None,
        })
    }

    /// Remembers this device's pending code in `path` (see
    /// [`PendingCode`]).
    pub fn with_pending_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.pending_file = Some(path.into());
        self
    }

    /// The unexpired pending code this device last showed on this relay.
    pub fn pending_code(&self) -> Option<PendingCode> {
        let path = self.pending_file.as_ref()?;
        let p: PendingCode = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let id = self.device_id().ok()??;
        (p.relay == self.relay.base()
            && p.device_id == id
            && p.expires_at > now_secs() + CODE_REUSE_MARGIN_SECS)
            .then_some(p)
    }

    fn remember_pending(&self, enrollment: &Enrollment) {
        let Some(path) = &self.pending_file else {
            return;
        };
        // A test never writes the user's real ~/.cua.
        if cua_home::guard_write(path).is_err() {
            return;
        }
        let result = match (&enrollment.device.state, &enrollment.code) {
            (DeviceState::Pending, Some(code)) => {
                let p = PendingCode {
                    relay: self.relay.base().to_string(),
                    device_id: enrollment.device.id.clone(),
                    code: code.clone(),
                    expires_at: enrollment
                        .device
                        .code_expires
                        .unwrap_or_else(|| now_secs() + DEFAULT_CODE_TTL_SECS),
                };
                serde_json::to_vec_pretty(&p)
                    .map_err(std::io::Error::other)
                    .and_then(|bytes| cua_home::write_private(path, &bytes))
            }
            _ => match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        };
        if let Err(e) = result {
            tracing::debug!(error = %e, "could not remember the pending device code");
        }
    }

    /// Reports `machine_id` instead of this machine's
    /// ([`crate::machine::machine_id`]); `None` reports none.
    pub fn with_machine_id(self, machine_id: Option<String>) -> Self {
        let _ = self.machine_id.set(machine_id);
        self
    }

    fn machine_id(&self) -> Option<String> {
        self.machine_id
            .get_or_init(crate::machine::machine_id)
            .clone()
    }

    /// The relay base URL.
    pub fn relay_url(&self) -> &str {
        self.relay.base()
    }

    /// The stored key, if this device has one.
    pub fn key(&self) -> Result<Option<DeviceKey>> {
        self.slot
            .load()?
            .map(|pkcs8| DeviceKey::from_pkcs8(&pkcs8))
            .transpose()
    }

    /// This device's id, if it has a key.
    pub fn device_id(&self) -> Result<Option<String>> {
        Ok(self.key()?.map(|k| k.id()))
    }

    fn key_or_create(&self) -> Result<DeviceKey> {
        if let Some(key) = self.key()? {
            return Ok(key);
        }
        let (key, pkcs8) = DeviceKey::generate()?;
        self.slot.save(&pkcs8)?;
        Ok(key)
    }

    fn alternate_key(&self) -> Result<Option<DeviceKey>> {
        Ok(self
            .slot
            .alternate()?
            .and_then(|pkcs8| DeviceKey::from_pkcs8(&pkcs8).ok()))
    }

    /// Registers this device (creating its key on first use). Right after a
    /// fresh sign-in it is enrolled at once; otherwise the result carries a
    /// one-time code to confirm from an enrolled device.
    ///
    /// When this machine keeps two keys (two builds of cua), the one the
    /// relay already enrolled wins and the other is retired, so the machine
    /// stays one device. A key the relay revoked (or replaced) can never
    /// enroll again, so it is replaced by a new one.
    ///
    /// While this device still waits with an unexpired code it remembers
    /// (see [`DeviceAuth::with_pending_file`]) and the sign-in is not fresh
    /// enough to enroll by itself, the code is shown again instead of
    /// registering anew (which would replace it).
    pub async fn enroll(&self) -> Result<Enrollment> {
        if let Some(p) = self.pending_code() {
            let token = self.tokens.access_token().await?;
            let fresh = token_auth_time(&token)
                .is_some_and(|t| now_secs().saturating_sub(t) <= FRESH_SIGN_IN_SECS);
            if !fresh
                && let Err(e) = self.session().await
                && refused_state(&e) == Some(DeviceState::Pending)
            {
                return Ok(Enrollment {
                    device: DeviceView {
                        id: p.device_id,
                        name: self.name.clone(),
                        state: DeviceState::Pending,
                        code_expires: Some(p.expires_at),
                        current: true,
                        ..Default::default()
                    },
                    code: Some(p.code),
                    ..Default::default()
                });
            }
        }
        self.register_this(&self.name).await
    }

    /// After an interactive sign-in: registers this device so the fresh
    /// sign-in enrolls (or re-verifies) it without an approval. A device
    /// with a key keeps the name the relay knows; a new one gets its key
    /// now and registers under this device's name, so signing in on a new
    /// computer is enough to use it (a relay that does not take the
    /// sign-in as a second factor leaves it pending with a code).
    pub async fn enroll_after_sign_in(&self) -> Result<Enrollment> {
        let name = if self.key()?.is_none() && self.alternate_key()?.is_none() {
            self.name.clone()
        } else {
            String::new()
        };
        self.register_this(&name).await
    }

    async fn register_this(&self, name: &str) -> Result<Enrollment> {
        let token = self.tokens.access_token().await?;
        *self.session.lock().await = None;
        let primary = self.key()?;
        let alternate = self
            .alternate_key()?
            .filter(|a| primary.as_ref().map(DeviceKey::id) != Some(a.id()));
        // With two keys, keep the one the relay enrolled.
        let mut adopted = false;
        let mut other = None;
        if let Some(alt) = &alternate {
            let primary_ok = match &primary {
                Some(k) => self.open_session(&token, k).await.is_ok(),
                None => false,
            };
            if !primary_ok && self.open_session(&token, alt).await.is_ok() {
                self.slot.adopt_alternate()?;
                adopted = true;
                other = primary.as_ref().map(DeviceKey::id);
            } else {
                other = Some(alt.id());
            }
        }
        let key = self.key_or_create()?;
        let enrollment = match self.register_key(&token, &key, name).await {
            Err(Error::PermissionDenied(m))
                if m.contains("revoked") || m.contains("replaced by") =>
            {
                let (key, pkcs8) = DeviceKey::generate()?;
                self.slot.save(&pkcs8)?;
                self.register_key(&token, &key, name).await?
            }
            other => other?,
        };
        *self.session.lock().await = None;
        self.remember_pending(&enrollment);
        if enrollment.device.state == DeviceState::Enrolled
            && let Some(other) =
                other.filter(|o| o != &enrollment.device.id && !enrollment.superseded.contains(o))
        {
            // The machine's other key is not used any more: retire its
            // record (best effort; the relay may have replaced it already).
            if let Ok((relay, token)) = self.enrolled_client().await
                && let Err(e) = relay.revoke_device(&token, &other).await
            {
                tracing::debug!(error = %e, device = %other, "could not retire the other key");
            }
            if !adopted {
                self.slot.drop_alternate()?;
            }
        }
        Ok(enrollment)
    }

    async fn register_key(&self, token: &str, key: &DeviceKey, name: &str) -> Result<Enrollment> {
        let ts = now_secs();
        let body = serde_json::json!({
            "public_key": key.public_key(),
            "name": name,
            "platform": crate::device_platform(),
            "machine_id": self.machine_id(),
            "ts": ts,
            "sig": key.sign(&register_message(&key.id(), ts))?,
            "bootstrap": true,
        });
        self.relay.register_device(token, &body).await
    }

    async fn open_session(&self, token: &str, key: &DeviceKey) -> Result<(String, u64)> {
        let ts = now_secs();
        let body = serde_json::json!({
            "device_id": key.id(),
            "ts": ts,
            "sig": key.sign(&session_message(&key.id(), ts))?,
        });
        let session = self.relay.device_session(token, &body).await?;
        Ok((session.session, session.expires_at))
    }

    /// A device session token (cached until shortly before it expires).
    /// Fails when this device has no key or is not enrolled (pending,
    /// re-verification due, revoked).
    pub async fn session(&self) -> Result<String> {
        let mut cached = self.session.lock().await;
        if let Some((token, expires)) = cached.as_ref()
            && *expires > now_secs() + SESSION_MARGIN_SECS
        {
            return Ok(token.clone());
        }
        let key = self.key()?.ok_or_else(|| {
            Error::PermissionDenied(
                "this device is not enrolled for your cua.ai account (run `cua devices enroll`)"
                    .into(),
            )
        })?;
        let token = self.tokens.access_token().await?;
        let session = self.open_session(&token, &key).await?;
        *cached = Some(session.clone());
        Ok(session.0)
    }

    /// [`DeviceAuth::session`] for automatic callers: `None` (the relay
    /// then decides, e.g. during its grace period) when this device has no
    /// session.
    pub async fn try_session(&self) -> Option<String> {
        match self.session().await {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::debug!(error = %e, "no device session");
                None
            }
        }
    }

    /// Forgets the cached session (after the relay refused it).
    pub async fn reset_session(&self) {
        *self.session.lock().await = None;
    }

    /// A relay client carrying this device's current (required) session,
    /// for a one-off, best-effort call that is not worth retrying on its
    /// own (see [`DeviceAuth::with_session_retry`] for calls that are).
    async fn enrolled_client(&self) -> Result<(RelayClient, String)> {
        let session = self.session().await?;
        let token = self.tokens.access_token().await?;
        Ok((self.relay.clone().with_device_session(Some(session)), token))
    }

    /// Runs `call` with this device's current (required) session, retrying
    /// once with a freshly-signed one if the relay refuses the first
    /// attempt specifically over the session (see [`is_session_rejection`]):
    /// most commonly, the relay restarted since this session was opened.
    /// Sessions are never persisted server-side (every relay restart
    /// forgets them all at once), so a cached session can look locally
    /// valid -- not yet past [`SESSION_MARGIN_SECS`] of its claimed
    /// expiry -- while the relay has already forgotten it. A device that
    /// genuinely has no key, or is genuinely not enrolled, fails
    /// identically on the retry (its session fails for the same reason
    /// again), so this never masks a real enrollment problem: it only
    /// costs one extra round trip, and only right after a restart.
    async fn with_session_retry<T, Fut>(
        &self,
        call: impl Fn(RelayClient, String) -> Fut,
    ) -> Result<T>
    where
        Fut: std::future::Future<Output = Result<T>>,
    {
        let token = self.tokens.access_token().await?;
        let session = self.session().await?;
        match call(
            self.relay.clone().with_device_session(Some(session)),
            token.clone(),
        )
        .await
        {
            Err(e) if is_session_rejection(&e) => {
                self.reset_session().await;
                let session = self.session().await?;
                call(self.relay.clone().with_device_session(Some(session)), token).await
            }
            other => other,
        }
    }

    /// [`DeviceAuth::with_session_retry`] for automatic callers that, like
    /// [`DeviceAuth::try_session`], carry on without a session when this
    /// device has none (the relay then decides, e.g. during its grace
    /// period): retries only when a session was actually sent and refused,
    /// never when there was none to begin with.
    async fn with_optional_session_retry<T, Fut>(
        &self,
        call: impl Fn(RelayClient, String) -> Fut,
    ) -> Result<T>
    where
        Fut: std::future::Future<Output = Result<T>>,
    {
        let token = self.tokens.access_token().await?;
        let session = self.try_session().await;
        let had_session = session.is_some();
        let relay = self.relay.clone().with_device_session(session);
        match call(relay, token.clone()).await {
            Err(e) if had_session && is_session_rejection(&e) => {
                self.reset_session().await;
                let relay = self
                    .relay
                    .clone()
                    .with_device_session(self.try_session().await);
                call(relay, token).await
            }
            other => other,
        }
    }

    /// Approves the device showing `code` (or the device `device_id`) from
    /// this enrolled device.
    pub async fn approve(&self, code: Option<&str>, device_id: Option<&str>) -> Result<DeviceView> {
        let code = code.map(str::to_owned);
        let device_id = device_id.map(str::to_owned);
        self.with_session_retry(move |relay, token| {
            let code = code.clone();
            let device_id = device_id.clone();
            async move {
                relay
                    .approve_device(&token, code.as_deref(), device_id.as_deref())
                    .await
            }
        })
        .await
    }

    /// The account's devices (this one marked `current`).
    pub async fn devices(&self) -> Result<Vec<DeviceView>> {
        Ok(self.listing().await?.devices)
    }

    /// The account's devices (this one marked `current`) and the end of the
    /// relay's grace period.
    pub async fn listing(&self) -> Result<DeviceListing> {
        self.with_optional_session_retry(|relay, token| async move {
            relay.device_listing(&token).await
        })
        .await
    }

    /// Renames a device.
    pub async fn rename(&self, id: &str, name: &str) -> Result<DeviceView> {
        let id = id.to_owned();
        let name = name.to_owned();
        self.with_session_retry(move |relay, token| {
            let id = id.clone();
            let name = name.clone();
            async move { relay.rename_device(&token, &id, &name).await }
        })
        .await
    }

    /// Revokes a device. Revoking this device also deletes its key.
    pub async fn revoke(&self, id: &str) -> Result<DeviceView> {
        let revoked = self
            .with_session_retry({
                let id = id.to_owned();
                move |relay, token| {
                    let id = id.clone();
                    async move { relay.revoke_device(&token, &id).await }
                }
            })
            .await?;
        if self.device_id()?.as_deref() == Some(id) {
            self.slot.clear()?;
            self.reset_session().await;
        }
        Ok(revoked)
    }

    /// The account's audit log, newest last.
    pub async fn audit(&self, limit: usize) -> Result<Vec<AuditEvent>> {
        self.with_optional_session_retry(move |relay, token| async move {
            relay.audit(&token, limit).await
        })
        .await
    }
}

/// The state a relay's refusal to open a device session reports: pending
/// (waiting for approval), re-verification due, or revoked. `None` for any
/// other error (an unknown device is `Error::NotFound`).
pub fn refused_state(e: &Error) -> Option<DeviceState> {
    let Error::PermissionDenied(m) = e else {
        return None;
    };
    if m.contains("revoked") {
        Some(DeviceState::Revoked)
    } else if m.contains("waiting for approval") || m.contains("not enrolled yet") {
        Some(DeviceState::Pending)
    } else if m.contains("re-verif") || m.contains("enrollment expired") {
        Some(DeviceState::Expired)
    } else {
        None
    }
}

/// Whether `e` is the relay refusing a request specifically because of
/// this device's session -- not a sign-in problem, not some other 403 --
/// matching the two message shapes `cua-relay`'s device API emits for it:
/// `device_api::NOT_ENROLLED` (no live session, grace period over) and
/// `enrolled()`'s "do this from an enrolled device" (a session was sent
/// but the relay does not recognize it). Both cover the common case of a
/// relay restart, which forgets every live session at once since none are
/// persisted (see `cua_relay::devices::DeviceStore`): a session opened
/// moments before still looks locally valid, so the next call sends it,
/// and the relay refuses it with one of these two messages.
fn is_session_rejection(e: &Error) -> bool {
    matches!(e, Error::PermissionDenied(msg)
        if msg.contains("not enrolled") || msg.contains("from an enrolled device"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_state_reads_the_relays_messages() {
        let denied = |m: &str| Error::PermissionDenied(m.into());
        for (m, want) in [
            (
                "this device is waiting for approval from an enrolled device",
                DeviceState::Pending,
            ),
            (
                "this device is not enrolled yet: sign in again (`cua auth login`) to enroll it",
                DeviceState::Pending,
            ),
            (
                "this device needs re-verification: approve it from an enrolled device",
                DeviceState::Expired,
            ),
            (
                "this device's enrollment expired after a period without use: sign in again",
                DeviceState::Expired,
            ),
            ("this device was revoked", DeviceState::Revoked),
        ] {
            assert_eq!(refused_state(&denied(m)), Some(want), "{m}");
        }
        assert_eq!(refused_state(&Error::NotFound("relay: gone".into())), None);
        assert_eq!(refused_state(&denied("forbidden")), None);
    }

    #[test]
    fn token_auth_time_reads_the_unverified_claim() {
        let claims = b64().encode(br#"{"sub":"u","auth_time":1700000000}"#);
        assert_eq!(
            token_auth_time(&format!("h.{claims}.s")),
            Some(1_700_000_000)
        );
        assert_eq!(token_auth_time("opaque-token"), None);
    }
}
