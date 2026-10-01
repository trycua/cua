//! Sign in to Cua, and the one credential store every cua client shares.
//!
//! - **Login**: browser sign-in with OAuth authorization code + PKCE and a
//!   `127.0.0.1` loopback redirect (RFC 8252), or the device authorization
//!   grant (RFC 8628) for machines without a local browser. Browser sign-in
//!   first checks that the identity provider accepts the loopback redirect
//!   for the client and falls back to the device flow when it does not, so
//!   it works before and after that client change lands.
//! - **Store**: the OS credential vault on Windows and on macOS for builds
//!   signed by Cua (service `run.cua.ai`, account `cua-cli`), a 0600
//!   `~/.cua/credentials.json` elsewhere (Linux, and unsigned or ad-hoc
//!   macOS builds, which macOS treats as a new app on every rebuild so
//!   "Always Allow" never sticks) or with `CUA_CREDENTIAL_STORE=file`.
//!   `CUA_CREDENTIAL_STORE=keychain` forces the vault. The cua CLI, the SDK
//!   (`Cua.auth()`), `cua daemon` and the Spaces app all use it; the Spaces
//!   app's former `~/.cua/spaces-session.json` is migrated on first use.
//!   Named secrets (the client device key) stay one per machine across
//!   builds: a Cua-signed build moves a file-backed build's copy into the
//!   vault, and an unsigned macOS build reads the vault's copy when it has
//!   none (never writing it back to a file).
//! - **Session**: a refreshing access token. Every call reads the store, so
//!   a sign-out or a token another process rotated is seen at once instead
//!   of being refreshed twice.
//!
//! Configuration: `CUA_OIDC_ISSUER`, `CUA_OIDC_CLIENT_ID`; test hooks
//! `CUA_OIDC_POLL_UNIT_MS`, `CUA_OIDC_REDIRECT_PORT`,
//! `CUA_OIDC_LOGIN_TIMEOUT_SECS`.

use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Default OIDC issuer (`CUA_OIDC_ISSUER`).
pub const DEFAULT_OIDC_ISSUER: &str = "https://auth.cua.ai/realms/cyclops-cs";
/// Public OAuth client of the cua CLI and apps (`CUA_OIDC_CLIENT_ID`).
pub const DEFAULT_CLIENT_ID: &str = "cua-cli";
const DEFAULT_SCOPE: &str = "openid profile offline_access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// Refresh this long before expiry.
const REFRESH_SKEW_SECS: i64 = 60;
#[cfg(any(target_os = "macos", target_os = "windows"))]
const KEYRING_SERVICE: &str = "run.cua.ai";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const KEYRING_ACCOUNT: &str = "cua-cli";
/// The Spaces app's former session file under `~/.cua`.
pub const LEGACY_SPACES_SESSION: &str = "spaces-session.json";

/// Errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// No stored session.
    #[error("not logged in; run 'cua auth login'")]
    NotLoggedIn,
    /// The identity provider refused (expired or revoked session, denied
    /// sign-in, bad grant).
    #[error("{0}")]
    Unauthenticated(String),
    /// Network or protocol failure.
    #[error("{0}")]
    Http(String),
    /// The user did not finish in time.
    #[error("{0}")]
    Timeout(String),
    /// The requested flow is not available.
    #[error("{0}")]
    Unsupported(String),
    /// Bad input.
    #[error("{0}")]
    InvalidArgument(String),
    /// The credential store failed.
    #[error("credential store: {0}")]
    Store(String),
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

fn http_err(e: impl std::fmt::Display) -> Error {
    Error::Http(e.to_string())
}

fn store_err(e: impl std::fmt::Display) -> Error {
    Error::Store(e.to_string())
}

// ------------------------------------------------------------ credentials

/// Tokens issued to the public client. The JSON shape is shared with the
/// former Python CLI, so an existing keychain entry keeps working.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    /// Access token.
    pub access_token: String,
    /// Refresh token.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// RFC 3339 expiry of the access token.
    pub expires_at: String,
    /// Token type.
    #[serde(default = "bearer")]
    pub token_type: String,
    /// Granted scope.
    #[serde(default)]
    pub scope: Option<String>,
    /// ID token, when issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
}

fn bearer() -> String {
    "Bearer".into()
}

/// Who is signed in (from unverified token claims; display only).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Identity {
    /// `preferred_username`.
    pub username: Option<String>,
    /// `email`.
    pub email: Option<String>,
    /// `name`.
    pub name: Option<String>,
    /// `sub`.
    pub subject: Option<String>,
}

impl Identity {
    /// One display string: email, else username, else subject.
    pub fn display(&self) -> Option<String> {
        self.email
            .clone()
            .or_else(|| self.username.clone())
            .or_else(|| self.subject.clone())
    }
}

impl Credentials {
    /// Expiry.
    pub fn expires(&self) -> Result<chrono::DateTime<chrono::Utc>> {
        chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map_err(|_| {
                Error::Unauthenticated("stored Cua credentials are invalid; log in again".into())
            })
    }

    /// Whether the access token is expired or within the refresh skew.
    pub fn needs_refresh(&self) -> bool {
        self.expires()
            .map(|e| e <= chrono::Utc::now() + chrono::Duration::seconds(REFRESH_SKEW_SECS))
            .unwrap_or(true)
    }

    /// From an OAuth token endpoint response.
    pub fn from_token_response(v: &serde_json::Value) -> Result<Self> {
        let access = v["access_token"].as_str().ok_or_else(|| {
            Error::Unauthenticated("the identity provider returned no access token".into())
        })?;
        let expires_in = v["expires_in"].as_i64().unwrap_or(300).max(0);
        let s = |k: &str| v[k].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        Ok(Self {
            access_token: access.into(),
            refresh_token: s("refresh_token"),
            expires_at: (chrono::Utc::now() + chrono::Duration::seconds(expires_in)).to_rfc3339(),
            token_type: s("token_type").unwrap_or_else(bearer),
            scope: s("scope"),
            id_token: s("id_token"),
        })
    }

    /// The signed-in identity (ID token claims, else access token claims).
    pub fn identity(&self) -> Identity {
        let claims = self
            .id_token
            .as_deref()
            .and_then(jwt_claims)
            .or_else(|| jwt_claims(&self.access_token))
            .unwrap_or_default();
        let s = |k: &str| {
            claims[k]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Identity {
            username: s("preferred_username"),
            email: s("email"),
            name: s("name"),
            subject: s("sub"),
        }
    }
}

/// Unverified JWT claims (informational only).
pub fn jwt_claims(token: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

// ------------------------------------------------------------------ store

/// `~/.cua` (or `$CUA_HOME`).
pub fn cua_home() -> PathBuf {
    cua_home::cua_home()
}

/// Where credentials live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Store {
    /// A 0600 JSON file.
    File(PathBuf),
    /// The OS credential vault.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    Keyring,
    /// A stand-in for the OS credential vault in tests
    /// (`CUA_CREDENTIAL_STORE=test-keychain:<dir>`): a file under `<dir>`
    /// that behaves like the vault (its session marker says `keychain`) and
    /// counts every read in `<dir>/reads`.
    TestKeychain(PathBuf),
}

/// The session marker file under the cua home: a non-secret record that a
/// session is stored, so callers that must not touch the OS credential
/// vault implicitly (the default sandbox listing) know whether it holds one.
pub const SESSION_MARKER: &str = "session.json";

/// What the session marker says (no token material).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMarker {
    /// `keychain` (the OS credential vault) or `file`.
    pub store: String,
    /// The signed-in account (email or username), when known.
    #[serde(default)]
    pub account: Option<String>,
    /// When the stored access token expires (a refresh token may outlive it).
    pub expires_at: String,
}

/// The session marker, when one is written.
pub fn read_session_marker() -> Option<SessionMarker> {
    let raw = std::fs::read_to_string(Store::from_env().marker_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Whether a signed-in session may be stored, decided WITHOUT reading the
/// OS credential vault: the file store's file exists, or the session
/// marker says the vault holds one. `CUA_FLEET_SESSION=0` says no. Implicit
/// callers check this before reading the session; explicit ones read it
/// (and a successful read writes the marker for sessions stored before the
/// marker existed).
pub fn may_have_session() -> bool {
    if std::env::var("CUA_FLEET_SESSION").is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    }) {
        return false;
    }
    let store = Store::from_env();
    if let Store::File(p) = &store
        && p.is_file()
    {
        return true;
    }
    read_session_marker().is_some_and(|m| m.store == store.kind())
}

/// Cua's Apple Developer team (the one `cua_keyvault::caller::CUA_TEAM_ID`
/// names).
pub const CUA_TEAM_ID: &str = "YCK386LBJ7";

/// Whether this process is signed with Cua's Developer ID. Only then does
/// the default store use the macOS keychain: the keychain remembers "Always
/// Allow" by code signature, and an unsigned or ad-hoc build (every
/// development build) has a new one on each rebuild, so macOS would ask again
/// for every build.
#[cfg(target_os = "macos")]
pub fn signed_by_cua() -> bool {
    use security_framework::os::macos::code_signing::{Flags, SecCode, SecRequirement};
    static SIGNED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SIGNED.get_or_init(|| {
        let Ok(requirement) =
            format!("anchor apple generic and certificate leaf[subject.OU] = \"{CUA_TEAM_ID}\"")
                .parse::<SecRequirement>()
        else {
            return false;
        };
        SecCode::for_self(Flags::NONE)
            .and_then(|code| code.check_validity(Flags::NONE, &requirement))
            .is_ok()
    })
}

impl Store {
    /// `CUA_CREDENTIAL_STORE` (`file` | `keychain`) or the default: the OS
    /// vault on Windows and for Cua-signed macOS builds, else the file.
    pub fn from_env() -> Self {
        let file = || Store::File(cua_home().join("credentials.json"));
        let setting = std::env::var("CUA_CREDENTIAL_STORE").unwrap_or_default();
        if let Some(dir) = setting.strip_prefix("test-keychain:") {
            return Store::TestKeychain(PathBuf::from(dir));
        }
        match setting.as_str() {
            "file" => file(),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            "keychain" | "keyring" => Store::Keyring,
            #[cfg(target_os = "macos")]
            _ if signed_by_cua() => Store::Keyring,
            #[cfg(target_os = "windows")]
            _ => Store::Keyring,
            #[cfg(not(target_os = "windows"))]
            _ => file(),
        }
    }

    /// `keychain` (the OS credential vault, or its test stand-in) or `file`.
    pub fn kind(&self) -> &'static str {
        match self {
            Store::File(_) => "file",
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => "keychain",
            Store::TestKeychain(_) => "keychain",
        }
    }

    /// Where this store's session marker lives: next to a credentials file,
    /// else under the cua home.
    pub fn marker_path(&self) -> PathBuf {
        match self {
            Store::File(p) => p
                .parent()
                .map(|d| d.join(SESSION_MARKER))
                .unwrap_or_else(|| cua_home().join(SESSION_MARKER)),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => cua_home().join(SESSION_MARKER),
            Store::TestKeychain(_) => cua_home().join(SESSION_MARKER),
        }
    }

    /// Writes the session marker for `c` (best effort: a marker that
    /// cannot be written only means the default listing skips the cloud).
    fn mark(&self, c: &Credentials) {
        let id = c.identity();
        let marker = SessionMarker {
            store: self.kind().into(),
            account: id.email.or(id.username),
            expires_at: c.expires_at.clone(),
        };
        if let Ok(raw) = serde_json::to_vec(&marker) {
            let _ = write_private(&self.marker_path(), &raw);
        }
    }

    fn unmark(&self) {
        let _ = std::fs::remove_file(self.marker_path());
    }

    /// Human description.
    pub fn describe(&self) -> String {
        match self {
            Store::File(p) => format!("file {}", p.display()),
            Store::TestKeychain(d) => format!("test keychain {}", d.display()),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => format!(
                "OS credential vault (service {KEYRING_SERVICE}, account {KEYRING_ACCOUNT})"
            ),
        }
    }

    /// Loads credentials (`None` when there are none).
    pub fn load(&self) -> Result<Option<Credentials>> {
        let raw = match self {
            Store::File(p) => match std::fs::read_to_string(p) {
                Ok(s) => Some(s),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(store_err(e)),
            },
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => match keyring_entry()?.get_password() {
                Ok(s) => Some(s),
                Err(keyring::Error::NoEntry) => None,
                Err(e) => return Err(store_err(e)),
            },
            Store::TestKeychain(d) => {
                let reads = d.join("reads");
                let n: u64 = std::fs::read_to_string(&reads)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                std::fs::create_dir_all(d).map_err(store_err)?;
                std::fs::write(&reads, (n + 1).to_string()).map_err(store_err)?;
                std::fs::read_to_string(d.join("vault.json")).ok()
            }
        };
        let Some(raw) = raw else {
            // Nothing stored: a stale marker (a session removed by another
            // client) goes too.
            self.unmark();
            return Ok(None);
        };
        let c: Credentials = serde_json::from_str(&raw).map_err(|_| {
            Error::Unauthenticated("stored Cua credentials are invalid; log in again".into())
        })?;
        c.expires()?;
        // Sessions stored before the marker existed get one on first read.
        if !self.marker_path().is_file() {
            self.mark(&c);
        }
        Ok(Some(c))
    }

    /// Saves credentials.
    pub fn save(&self, c: &Credentials) -> Result<()> {
        let raw = serde_json::to_string(c).map_err(store_err)?;
        match self {
            Store::File(p) => write_private(p, raw.as_bytes()).map_err(store_err)?,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => keyring_entry()?.set_password(&raw).map_err(store_err)?,
            Store::TestKeychain(d) => {
                write_private(&d.join("vault.json"), raw.as_bytes()).map_err(store_err)?
            }
        }
        self.mark(c);
        Ok(())
    }

    /// Removes credentials; returns whether any existed.
    pub fn clear(&self) -> Result<bool> {
        self.unmark();
        match self {
            Store::File(p) => match std::fs::remove_file(p) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(store_err(e)),
            },
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => match keyring_entry()?.delete_credential() {
                Ok(()) => Ok(true),
                Err(keyring::Error::NoEntry) => Ok(false),
                Err(e) => Err(store_err(e)),
            },
            Store::TestKeychain(d) => match std::fs::remove_file(d.join("vault.json")) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(store_err(e)),
            },
        }
    }

    /// Where a named secret lives in a file-backed store: next to the
    /// credentials file.
    fn secret_path(&self, name: &str) -> Result<PathBuf> {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
            return Err(Error::InvalidArgument(format!("secret name {name:?}")));
        }
        Ok(match self {
            Store::File(p) => p
                .parent()
                .map(|d| d.join(name))
                .unwrap_or_else(|| cua_home().join(name)),
            Store::TestKeychain(d) => d.join(name),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => cua_home().join(name),
        })
    }

    /// Where a vault-backed store finds a named secret that a file-backed
    /// build of cua kept: `<cua_home>/<name>` (an unsigned or ad-hoc build
    /// on this machine uses the file store). `None` for the file store, and
    /// in a test process for the user's real cua home (a test never reads
    /// or moves the user's secrets).
    fn file_secret_to_migrate(&self, name: &str, home: &Path) -> Option<PathBuf> {
        match self {
            Store::File(_) => None,
            _ => {
                let path = home.join(name);
                // Never the store's own copy (a test vault inside the home).
                if self.secret_path(name).ok()? == path {
                    return None;
                }
                cua_home::guard_write(&path).ok()?;
                Some(path)
            }
        }
    }

    /// Whether this file store reads a named secret it lacks from the OS
    /// credential vault: only the default store of the user's real cua home
    /// (`~/.cua/credentials.json`, no `CUA_CREDENTIAL_STORE`, no temporary
    /// `CUA_HOME`) of an unsigned macOS build, outside tests.
    /// A Cua-signed build of the same cua moved the secret there; reading it
    /// back (macOS may ask once) keeps this device one device across builds,
    /// and it is never copied back into a file.
    fn reads_vault_fallback(&self) -> bool {
        let Store::File(path) = self else {
            return false;
        };
        vault_fallback_applies(
            path,
            &cua_home().join("credentials.json"),
            std::env::var("CUA_CREDENTIAL_STORE").ok().as_deref(),
            cua_home::is_test_process(),
            cua_home::is_under_real_home(path),
        ) && cfg!(target_os = "macos")
    }

    fn read_secret_text(&self, name: &str) -> Result<Option<String>> {
        let path = self.secret_path(name)?;
        match self {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => match secret_entry(name)?.get_password() {
                Ok(s) => Ok(Some(s)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(store_err(e)),
            },
            _ => read_optional(&path),
        }
    }

    /// Reads the named secret from the OS credential vault for an unsigned
    /// build (see [`Store::reads_vault_fallback`]). A refusal (the user
    /// denied the keychain prompt) reads as nothing.
    fn read_vault_fallback(&self, name: &str) -> Option<String> {
        if !self.reads_vault_fallback() {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            secret_entry(name).ok()?.get_password().ok()
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = name;
            None
        }
    }

    /// Loads the named secret (for example the client device key) from the
    /// same vault as the session: the OS credential vault, or a 0600 file
    /// next to the credentials file.
    ///
    /// Builds of cua that differ only in signing keep one secret: a vault
    /// store that lacks it moves in the file a file-backed build left in
    /// the cua home (and deletes the file), and the default file store of
    /// an unsigned macOS build that lacks it reads the vault's copy.
    pub fn load_secret(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.load_secret_in(name, &cua_home())
    }

    /// [`Store::load_secret`] with the cua home given.
    fn load_secret_in(&self, name: &str, home: &Path) -> Result<Option<Vec<u8>>> {
        let mut text = self.read_secret_text(name)?;
        if text.is_none() {
            if let Some(legacy) = self.file_secret_to_migrate(name, home) {
                text = self.move_into_vault(name, &legacy)?;
            } else {
                text = self.read_vault_fallback(name);
            }
        }
        text.map(|t| decode_secret(name, &t)).transpose()
    }

    /// Moves the secret at `legacy` into this vault store, returning it.
    fn move_into_vault(&self, name: &str, legacy: &Path) -> Result<Option<String>> {
        let Some(text) = read_optional(legacy)? else {
            return Ok(None);
        };
        // Only a readable secret moves; a broken file stays for inspection.
        decode_secret(name, &text)?;
        self.write_secret_text(name, text.trim())?;
        std::fs::remove_file(legacy).map_err(store_err)?;
        Ok(Some(text))
    }

    /// Another copy of the named secret this machine keeps apart from the
    /// store's own: the file a file-backed build left (for a vault store),
    /// or the vault's copy (for an unsigned macOS build's default file
    /// store). `None` when there is none or it is the one
    /// [`Store::load_secret`] returns anyway.
    pub fn load_alternate_secret(&self, name: &str) -> Result<Option<Vec<u8>>> {
        self.load_alternate_secret_in(name, &cua_home())
    }

    fn load_alternate_secret_in(&self, name: &str, home: &Path) -> Result<Option<Vec<u8>>> {
        let text = if let Some(legacy) = self.file_secret_to_migrate(name, home) {
            read_optional(&legacy)?
        } else if self.read_secret_text(name)?.is_some() {
            self.read_vault_fallback(name)
        } else {
            None
        };
        Ok(text.and_then(|t| decode_secret(name, &t).ok()))
    }

    /// Makes the alternate copy (see [`Store::load_alternate_secret`]) the
    /// named secret, dropping the store's own: a vault store moves the file
    /// in; a file store deletes its file so reads fall through to the
    /// vault's copy. Nothing ever moves from the vault into a file.
    pub fn adopt_alternate_secret(&self, name: &str) -> Result<()> {
        self.adopt_alternate_secret_in(name, &cua_home())
    }

    fn adopt_alternate_secret_in(&self, name: &str, home: &Path) -> Result<()> {
        if let Some(legacy) = self.file_secret_to_migrate(name, home) {
            if read_optional(&legacy)?.is_some() {
                self.clear_secret(name)?;
                self.move_into_vault(name, &legacy)?;
            }
            return Ok(());
        }
        if self.reads_vault_fallback() && self.read_vault_fallback(name).is_some() {
            self.clear_secret(name)?;
        }
        Ok(())
    }

    /// Deletes the file copy of the named secret a file-backed build left
    /// (vault stores only; an unsigned build never deletes the vault's
    /// copy).
    pub fn drop_alternate_secret(&self, name: &str) -> Result<()> {
        if let Some(legacy) = self.file_secret_to_migrate(name, &cua_home()) {
            match std::fs::remove_file(&legacy) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(store_err(e)),
            }
        }
        Ok(())
    }

    fn write_secret_text(&self, name: &str, text: &str) -> Result<()> {
        let path = self.secret_path(name)?;
        match self {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => secret_entry(name)?.set_password(text).map_err(store_err),
            _ => write_private(&path, text.as_bytes()).map_err(store_err),
        }
    }

    /// Saves the named secret (see [`Store::load_secret`]).
    pub fn save_secret(&self, name: &str, data: &[u8]) -> Result<()> {
        use base64::Engine as _;
        self.write_secret_text(
            name,
            &base64::engine::general_purpose::STANDARD.encode(data),
        )
    }

    /// Removes the named secret; returns whether it existed. (An unsigned
    /// build's file store removes only its file, never the vault's copy.)
    pub fn clear_secret(&self, name: &str) -> Result<bool> {
        let path = self.secret_path(name)?;
        match self {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            Store::Keyring => match secret_entry(name)?.delete_credential() {
                Ok(()) => Ok(true),
                Err(keyring::Error::NoEntry) => Ok(false),
                Err(e) => Err(store_err(e)),
            },
            _ => match std::fs::remove_file(&path) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(store_err(e)),
            },
        }
    }

    /// Moves the Spaces app's former `<cua_home>/spaces-session.json` into
    /// this store when the store is empty, then deletes the old file.
    /// Returns whether a session was migrated.
    pub fn migrate_legacy(&self, cua_home: &Path) -> Result<bool> {
        let legacy = cua_home.join(LEGACY_SPACES_SESSION);
        let Ok(raw) = std::fs::read_to_string(&legacy) else {
            return Ok(false);
        };
        if self.load().ok().flatten().is_some() {
            // The shared store wins; the old file is obsolete.
            let _ = std::fs::remove_file(&legacy);
            return Ok(false);
        }
        #[derive(Deserialize)]
        struct Legacy {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            id_token: Option<String>,
            expires_at: i64,
        }
        let migrated = match serde_json::from_str::<Legacy>(&raw) {
            Ok(l) => {
                let expires = chrono::DateTime::from_timestamp(l.expires_at, 0)
                    .unwrap_or_else(chrono::Utc::now);
                self.save(&Credentials {
                    access_token: l.access_token,
                    refresh_token: l.refresh_token,
                    expires_at: expires.to_rfc3339(),
                    token_type: bearer(),
                    scope: None,
                    id_token: l.id_token,
                })?;
                true
            }
            Err(_) => false,
        };
        let _ = std::fs::remove_file(&legacy);
        Ok(migrated)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn keyring_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(store_err)
}

/// Whether a file store at `path` reads missing secrets from the OS
/// credential vault: the default store of the real cua home, with no
/// `CUA_CREDENTIAL_STORE` choice, outside tests.
fn vault_fallback_applies(
    path: &Path,
    default_path: &Path,
    setting: Option<&str>,
    test_process: bool,
    under_real_home: bool,
) -> bool {
    !test_process
        && under_real_home
        && setting.is_none_or(|s| s.trim().is_empty())
        && path == default_path
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(store_err(e)),
    }
}

fn decode_secret(name: &str, text: &str) -> Result<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|_| Error::Unauthenticated(format!("stored secret {name} is invalid")))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn secret_entry(name: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, &format!("{KEYRING_ACCOUNT}.{name}")).map_err(store_err)
}

/// Writes a file readable only by the owner (0600 on Unix), atomically.
fn write_private(p: &Path, data: &[u8]) -> std::io::Result<()> {
    // A test must never write credentials into the real ~/.cua.
    cua_home::guard_write(p)?;
    cua_home::write_private(p, data)
}

// ------------------------------------------------------------------- OIDC

/// The issuer's endpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    /// Token endpoint.
    pub token_endpoint: String,
    /// Authorization endpoint (browser sign-in).
    pub authorization_endpoint: Option<String>,
    /// Device authorization endpoint.
    pub device_authorization_endpoint: Option<String>,
    /// Revocation endpoint.
    pub revocation_endpoint: Option<String>,
}

/// OIDC client settings.
#[derive(Clone, Debug)]
pub struct Oidc {
    /// Issuer URL (no trailing slash).
    pub issuer: String,
    /// Public client id.
    pub client_id: String,
    /// Requested scope.
    pub scope: String,
    http: reqwest::Client,
}

fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

impl Oidc {
    /// Explicit issuer and client.
    pub fn new(issuer: impl Into<String>, client_id: impl Into<String>) -> Self {
        ensure_crypto_provider();
        Self {
            issuer: issuer.into().trim_end_matches('/').to_string(),
            client_id: client_id.into(),
            scope: DEFAULT_SCOPE.into(),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(60))
                .user_agent(concat!("cua-auth/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("http client"),
        }
    }

    /// From `CUA_OIDC_ISSUER` / `CUA_OIDC_CLIENT_ID` or the defaults.
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| {
            std::env::var(k)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| d.to_string())
        };
        Self::new(
            get("CUA_OIDC_ISSUER", DEFAULT_OIDC_ISSUER),
            get("CUA_OIDC_CLIENT_ID", DEFAULT_CLIENT_ID),
        )
    }

    async fn form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, serde_json::Value)> {
        let r = self
            .http
            .post(url)
            .header("accept", "application/json")
            .form(form)
            .send()
            .await
            .map_err(http_err)?;
        let status = r.status().as_u16();
        let text = r.text().await.map_err(http_err)?;
        let v = serde_json::from_str(&text)
            .unwrap_or_else(|_| serde_json::json!({ "error_description": text }));
        Ok((status, v))
    }

    /// Fetches the discovery document.
    pub async fn discover(&self) -> Result<Discovery> {
        let url = format!("{}/.well-known/openid-configuration", self.issuer);
        let r = self.http.get(&url).send().await.map_err(http_err)?;
        if !r.status().is_success() {
            return Err(Error::Http(format!(
                "OIDC discovery failed (HTTP {})",
                r.status().as_u16()
            )));
        }
        let v: serde_json::Value = r.json().await.map_err(http_err)?;
        let s = |k: &str| v[k].as_str().map(str::to_string);
        Ok(Discovery {
            token_endpoint: s("token_endpoint").ok_or_else(incomplete)?,
            authorization_endpoint: s("authorization_endpoint"),
            device_authorization_endpoint: s("device_authorization_endpoint"),
            revocation_endpoint: s("revocation_endpoint"),
        })
    }

    /// Exchanges the refresh token.
    pub async fn refresh(&self, c: &Credentials) -> Result<Credentials> {
        let Some(rt) = &c.refresh_token else {
            return Err(Error::Unauthenticated(
                "your session cannot be refreshed; run 'cua auth login' again".into(),
            ));
        };
        let d = self.discover().await?;
        let (status, v) = self
            .form(
                &d.token_endpoint,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", rt),
                    ("client_id", &self.client_id),
                ],
            )
            .await?;
        if status != 200 {
            let msg = describe(&v, "your session expired; run 'cua auth login' again");
            // 4xx from the token endpoint means the grant is dead; anything
            // else may be transient.
            return Err(if (400..500).contains(&status) {
                Error::Unauthenticated(msg)
            } else {
                Error::Http(msg)
            });
        }
        let mut n = Credentials::from_token_response(&v)?;
        if n.refresh_token.is_none() {
            n.refresh_token = c.refresh_token.clone();
        }
        if n.id_token.is_none() {
            n.id_token = c.id_token.clone();
        }
        Ok(n)
    }

    /// Revokes the session at the issuer; `Ok(false)` when unsupported.
    pub async fn revoke(&self, c: &Credentials) -> Result<bool> {
        let d = self.discover().await?;
        let Some(url) = d.revocation_endpoint else {
            return Ok(false);
        };
        let token = c.refresh_token.as_deref().unwrap_or(&c.access_token);
        let (status, _) = self
            .form(&url, &[("token", token), ("client_id", &self.client_id)])
            .await?;
        Ok((200..300).contains(&status))
    }
}

fn incomplete() -> Error {
    Error::Http("the OIDC issuer returned an incomplete discovery document".into())
}

fn describe(v: &serde_json::Value, fallback: &str) -> String {
    v["error_description"]
        .as_str()
        .or_else(|| v["error"].as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

// ------------------------------------------------------------------ login

/// How to sign in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flow {
    /// Browser + PKCE when the provider accepts the loopback redirect, else
    /// the device flow.
    #[default]
    Auto,
    /// Browser + PKCE only (an error when unavailable).
    Browser,
    /// Device code.
    Device,
}

impl Flow {
    /// `auto`, `browser` (`pkce`), `device`.
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim() {
            "" | "auto" => Ok(Flow::Auto),
            "browser" | "pkce" => Ok(Flow::Browser),
            "device" | "remote" => Ok(Flow::Device),
            other => Err(Error::InvalidArgument(format!(
                "unknown login flow {other:?} (auto, browser or device)"
            ))),
        }
    }
}

/// Which flow a login is using.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Open `url` in a browser on this machine; the loopback redirect
    /// finishes the sign-in.
    Browser,
    /// Open `url` on any device and enter `user_code`.
    Device,
}

enum Pending {
    Pkce {
        listener: tokio::net::TcpListener,
        verifier: String,
        state: String,
        redirect: String,
    },
    Device {
        device_code: String,
        interval: u64,
        expires_in: u64,
    },
}

/// A started sign-in: show `url` (and `user_code`) to the user, then await
/// [`PendingLogin::complete`].
pub struct PendingLogin {
    /// Flow in use.
    pub method: Method,
    /// The URL to open.
    pub url: String,
    /// Device flow: the code to enter.
    pub user_code: Option<String>,
    /// Why the flow differs from the one asked for (fallback), if it does.
    pub note: Option<String>,
    oidc: Oidc,
    token_endpoint: String,
    pending: Pending,
}

fn b64url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random_token(n: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; n];
    rand::rng().fill_bytes(&mut b);
    b64url(&b)
}

/// RFC 7636 S256 challenge for `verifier`.
pub fn pkce_challenge(verifier: &str) -> String {
    use sha2::Digest;
    b64url(&sha2::Sha256::digest(verifier.as_bytes()))
}

/// Starts a sign-in.
pub async fn begin_login(oidc: &Oidc, flow: Flow) -> Result<PendingLogin> {
    let d = oidc.discover().await?;
    if flow != Flow::Device {
        match begin_pkce(oidc, &d).await {
            Ok(p) => return Ok(p),
            Err(why) if flow == Flow::Auto => {
                let mut p = begin_device(oidc, &d).await?;
                p.note = Some(why);
                return Ok(p);
            }
            Err(why) => {
                return Err(Error::Unsupported(format!(
                    "browser sign-in is unavailable: {why}"
                )));
            }
        }
    }
    begin_device(oidc, &d).await
}

/// Browser sign-in, or `Err(reason)` when the provider will not accept it.
async fn begin_pkce(oidc: &Oidc, d: &Discovery) -> Result<PendingLogin, String> {
    let auth_ep = d
        .authorization_endpoint
        .clone()
        .ok_or("the issuer has no authorization endpoint")?;
    let port: u16 = std::env::var("CUA_OIDC_REDIRECT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("cannot listen on 127.0.0.1 for the sign-in redirect: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let verifier = random_token(48);
    let state = random_token(24);
    let mut url = url::Url::parse(&auth_ep).map_err(|e| format!("authorization endpoint: {e}"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &oidc.client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("scope", &oidc.scope)
        .append_pair("state", &state)
        .append_pair("code_challenge", &pkce_challenge(&verifier))
        .append_pair("code_challenge_method", "S256");
    let url = url.to_string();
    // Preflight: an unregistered redirect URI gets an error page (Keycloak:
    // 400 "Invalid parameter: redirect_uri"); a valid request gets the login
    // page or a redirect.
    let r = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("could not reach the identity provider: {e}"))?;
    if !(r.status().is_success() || r.status().is_redirection()) {
        return Err(format!(
            "browser sign-in is not enabled for this client yet (HTTP {}); using a device code instead",
            r.status().as_u16()
        ));
    }
    Ok(PendingLogin {
        method: Method::Browser,
        url,
        user_code: None,
        note: None,
        oidc: oidc.clone(),
        token_endpoint: d.token_endpoint.clone(),
        pending: Pending::Pkce {
            listener,
            verifier,
            state,
            redirect,
        },
    })
}

async fn begin_device(oidc: &Oidc, d: &Discovery) -> Result<PendingLogin> {
    let Some(device_ep) = &d.device_authorization_endpoint else {
        return Err(incomplete());
    };
    let (status, v) = oidc
        .form(
            device_ep,
            &[("client_id", &oidc.client_id), ("scope", &oidc.scope)],
        )
        .await?;
    if status != 200 {
        return Err(Error::Unauthenticated(describe(
            &v,
            "could not start device authorization",
        )));
    }
    let device_code = v["device_code"].as_str().unwrap_or_default().to_string();
    let user_code = v["user_code"].as_str().unwrap_or_default().to_string();
    let verify = v["verification_uri_complete"]
        .as_str()
        .or_else(|| v["verification_uri"].as_str())
        .unwrap_or_default()
        .to_string();
    if device_code.is_empty() || verify.is_empty() {
        return Err(Error::Http(
            "the identity provider returned an invalid device authorization response".into(),
        ));
    }
    Ok(PendingLogin {
        method: Method::Device,
        url: verify,
        user_code: (!user_code.is_empty()).then_some(user_code),
        note: None,
        oidc: oidc.clone(),
        token_endpoint: d.token_endpoint.clone(),
        pending: Pending::Device {
            device_code,
            interval: v["interval"].as_u64().unwrap_or(5).max(1),
            expires_in: v["expires_in"].as_u64().unwrap_or(600).clamp(1, 3600),
        },
    })
}

impl PendingLogin {
    /// Waits for the user to finish and returns the tokens (not yet saved).
    pub async fn complete(self) -> Result<Credentials> {
        match self.pending {
            Pending::Pkce {
                listener,
                verifier,
                state,
                redirect,
            } => {
                let secs = std::env::var("CUA_OIDC_LOGIN_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(300);
                let code = tokio::time::timeout(
                    Duration::from_secs(secs),
                    await_callback(&listener, &state),
                )
                .await
                .map_err(|_| {
                    Error::Timeout(
                        "browser sign-in was not completed in time; try `cua auth login --remote`"
                            .into(),
                    )
                })??;
                let (status, v) = self
                    .oidc
                    .form(
                        &self.token_endpoint,
                        &[
                            ("grant_type", "authorization_code"),
                            ("code", &code),
                            ("redirect_uri", &redirect),
                            ("client_id", &self.oidc.client_id),
                            ("code_verifier", &verifier),
                        ],
                    )
                    .await?;
                if status != 200 {
                    return Err(Error::Unauthenticated(describe(
                        &v,
                        &format!("token request failed (HTTP {status})"),
                    )));
                }
                Credentials::from_token_response(&v)
            }
            Pending::Device {
                device_code,
                mut interval,
                expires_in,
            } => {
                let unit = std::env::var("CUA_OIDC_POLL_UNIT_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(Duration::from_millis)
                    .unwrap_or(Duration::from_secs(1));
                let deadline = tokio::time::Instant::now() + unit * expires_in as u32;
                // Hard bound on iterations, independent of the clock.
                for _ in 0..(expires_in / interval + 20) {
                    let (status, v) = self
                        .oidc
                        .form(
                            &self.token_endpoint,
                            &[
                                ("grant_type", DEVICE_GRANT),
                                ("device_code", &device_code),
                                ("client_id", &self.oidc.client_id),
                            ],
                        )
                        .await?;
                    if status == 200 {
                        return Credentials::from_token_response(&v);
                    }
                    match v["error"].as_str() {
                        Some("authorization_pending") => {}
                        Some("slow_down") => interval += 5,
                        _ => {
                            return Err(Error::Unauthenticated(describe(
                                &v,
                                &format!("token request failed (HTTP {status})"),
                            )));
                        }
                    }
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(unit * interval as u32).await;
                }
                Err(Error::Timeout(
                    "device authorization expired before it was completed".into(),
                ))
            }
        }
    }
}

const CALLBACK_OK: &str = "<!doctype html><meta charset=utf-8><title>Cua</title><body style=\"font-family:system-ui;margin:4em\"><h1>Signed in to Cua</h1><p>You can close this tab and return to the app.</p></body>";

/// Escapes text for an HTML body (the callback page echoes the provider's
/// error description, which arrives in the redirect's query string).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Serves the loopback redirect until a request carries our `state` and a
/// code (or an error). Bounded in connections and bytes.
async fn await_callback(listener: &tokio::net::TcpListener, state: &str) -> Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for _ in 0..64 {
        let (mut sock, _) = listener.accept().await.map_err(http_err)?;
        let mut buf = Vec::new();
        let mut tmp = [0u8; 2048];
        // Request line and headers only (GET has no body); 16 KiB cap.
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
            match tokio::time::timeout(Duration::from_secs(10), sock.read(&mut tmp)).await {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                Ok(Ok(n)) => buf.extend_from_slice(&tmp[..n]),
            }
        }
        let head = String::from_utf8_lossy(&buf).to_string();
        let target = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_string();
        let parsed = url::Url::parse(&format!("http://127.0.0.1{target}")).ok();
        let respond = |status: &str, body: &str| {
            format!(
                "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        };
        let Some(u) = parsed.filter(|u| u.path() == "/callback") else {
            let _ = sock
                .write_all(respond("404 Not Found", "not found").as_bytes())
                .await;
            continue;
        };
        let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
        if q.get("state").map(String::as_str) != Some(state) {
            let _ = sock
                .write_all(
                    respond(
                        "400 Bad Request",
                        "Sign-in state mismatch; start again with cua auth login.",
                    )
                    .as_bytes(),
                )
                .await;
            continue;
        }
        if let Some(e) = q.get("error") {
            let msg = q
                .get("error_description")
                .cloned()
                .unwrap_or_else(|| e.clone());
            let _ = sock
                .write_all(
                    respond(
                        "400 Bad Request",
                        &format!("Sign-in failed: {}", html_escape(&msg)),
                    )
                    .as_bytes(),
                )
                .await;
            return Err(Error::Unauthenticated(format!("sign-in failed: {msg}")));
        }
        let Some(code) = q.get("code").filter(|c| !c.is_empty()).cloned() else {
            let _ = sock
                .write_all(respond("400 Bad Request", "missing code").as_bytes())
                .await;
            continue;
        };
        let _ = sock
            .write_all(respond("200 OK", CALLBACK_OK).as_bytes())
            .await;
        let _ = sock.shutdown().await;
        return Ok(code);
    }
    Err(Error::Unauthenticated(
        "too many unexpected requests on the sign-in callback".into(),
    ))
}

// ---------------------------------------------------------------- session

/// The shared, refreshing user session.
pub struct Session {
    oidc: Oidc,
    store: Store,
    cached: tokio::sync::Mutex<Option<Credentials>>,
}

impl Session {
    /// On `store`, using `oidc` for refresh and revocation.
    pub fn new(oidc: Oidc, store: Store) -> Self {
        Self {
            oidc,
            store,
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// `Oidc::from_env()` and `Store::from_env()`, after migrating the Spaces
    /// app's former session file.
    pub fn from_env() -> Self {
        let store = Store::from_env();
        let _ = store.migrate_legacy(&cua_home());
        Self::new(Oidc::from_env(), store)
    }

    /// The store.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The OIDC client.
    pub fn oidc(&self) -> &Oidc {
        &self.oidc
    }

    /// The stored credentials (no network).
    pub fn credentials(&self) -> Result<Option<Credentials>> {
        self.store.load()
    }

    /// Who is signed in (no network).
    pub fn identity(&self) -> Option<Identity> {
        self.store.load().ok().flatten().map(|c| c.identity())
    }

    /// Starts a sign-in.
    pub async fn begin_login(&self, flow: Flow) -> Result<PendingLogin> {
        begin_login(&self.oidc, flow).await
    }

    /// Stores freshly issued credentials.
    pub async fn install(&self, c: Credentials) -> Result<Identity> {
        self.store.save(&c)?;
        let id = c.identity();
        *self.cached.lock().await = Some(c);
        Ok(id)
    }

    /// A valid access token, refreshed (and persisted) when it expires
    /// within a minute or `force`. `Error::NotLoggedIn` without a session;
    /// a refresh the provider refuses clears the session.
    pub async fn access_token(&self, force: bool) -> Result<String> {
        let mut guard = self.cached.lock().await;
        // The store is the source of truth: another process may have signed
        // out, or refreshed and rotated the tokens. A store that cannot be
        // read right now falls back to the last good copy.
        let stored = match self.store.load() {
            Ok(Some(c)) => c,
            Ok(None) => {
                *guard = None;
                return Err(Error::NotLoggedIn);
            }
            Err(Error::Store(e)) => guard.clone().ok_or(Error::Store(e))?,
            Err(e) => return Err(e),
        };
        let rotated = guard
            .as_ref()
            .is_some_and(|c| c.access_token != stored.access_token);
        if !stored.needs_refresh() && (!force || rotated) {
            let t = stored.access_token.clone();
            *guard = Some(stored);
            return Ok(t);
        }
        match self.oidc.refresh(&stored).await {
            Ok(n) => {
                self.store.save(&n)?;
                let t = n.access_token.clone();
                *guard = Some(n);
                Ok(t)
            }
            Err(Error::Unauthenticated(m)) => {
                // Another process may have refreshed (and rotated the
                // refresh token) while ours was in flight, so the provider
                // refused the one we sent. Its fresh session is in the store
                // now: use it rather than signing the user out. Only a store
                // that still holds the refused session is cleared.
                match self.store.load() {
                    Ok(Some(current)) if current != stored => {
                        if !current.needs_refresh() {
                            let t = current.access_token.clone();
                            *guard = Some(current);
                            return Ok(t);
                        }
                        *guard = None;
                        Err(Error::Unauthenticated(m))
                    }
                    Ok(Some(_)) => {
                        *guard = None;
                        let _ = self.store.clear();
                        Err(Error::Unauthenticated(m))
                    }
                    _ => {
                        *guard = None;
                        Err(Error::Unauthenticated(m))
                    }
                }
            }
            Err(e) => Err(e),
        }
    }

    /// Revokes (best effort) and clears the session. Returns whether a
    /// session existed and whether the issuer confirmed revocation.
    pub async fn logout(&self) -> Result<(bool, bool)> {
        let c = self.store.load().ok().flatten();
        *self.cached.lock().await = None;
        let Some(c) = c else {
            let _ = self.store.clear();
            return Ok((false, false));
        };
        let revoked = self.oidc.revoke(&c).await.unwrap_or(false);
        self.store.clear()?;
        Ok((true, revoked))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cargo test binary is ad-hoc signed, like every development build:
    /// it is not Cua-signed, so it never defaults to the macOS keychain
    /// (which would prompt again for every rebuild).
    #[cfg(target_os = "macos")]
    #[test]
    fn an_unsigned_build_is_not_cua_signed() {
        assert!(!signed_by_cua());
    }

    #[test]
    fn callback_error_text_is_html_escaped() {
        assert_eq!(
            html_escape("<script>x('a\"b')</script>&"),
            "&lt;script&gt;x(&#39;a&quot;b&#39;)&lt;/script&gt;&amp;"
        );
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_appendix_b() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let v = random_token(48);
        assert!(v.len() >= 43 && v.len() <= 128, "{}", v.len());
    }

    #[test]
    fn credentials_round_trip_python_format() {
        let raw = r#"{"access_token":"a","refresh_token":"r","expires_at":"2026-08-12T22:36:31.604247+00:00","token_type":"Bearer","scope":null}"#;
        let c: Credentials = serde_json::from_str(raw).unwrap();
        assert!(c.expires().is_ok());
        assert_eq!(c.refresh_token.as_deref(), Some("r"));
        assert!(c.needs_refresh());
    }

    #[test]
    fn named_secrets_live_next_to_the_session_owner_only() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::File(d.path().join("credentials.json"));
        assert_eq!(store.load_secret("device-key").unwrap(), None);
        store.save_secret("device-key", b"\x00key\xff").unwrap();
        assert_eq!(
            store.load_secret("device-key").unwrap().as_deref(),
            Some(&b"\x00key\xff"[..])
        );
        let path = d.path().join("device-key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Names cannot leave the store's directory.
        assert!(store.save_secret("../x", b"x").is_err());
        assert!(store.load_secret("").is_err());
        assert!(store.clear_secret("device-key").unwrap());
        assert!(!store.clear_secret("device-key").unwrap());
        assert!(!path.exists());
        // The test keychain keeps them in its directory.
        let t = Store::TestKeychain(d.path().join("vault"));
        t.save_secret("device-key", b"k").unwrap();
        assert!(d.path().join("vault/device-key").is_file());
    }

    #[test]
    fn a_vault_store_moves_in_the_key_a_file_backed_build_left() {
        let home = tempfile::tempdir().unwrap();
        let vault = Store::TestKeychain(home.path().join("vault"));
        let file = Store::File(home.path().join("credentials.json"));
        // The unsigned build stored its device key as a file.
        file.save_secret("device-key", b"unsigned-key").unwrap();
        let legacy = home.path().join("device-key");
        assert!(legacy.is_file());
        // The signed build finds it, moves it into its vault and deletes
        // the file: one key, in the stronger store.
        assert_eq!(
            vault
                .load_secret_in("device-key", home.path())
                .unwrap()
                .as_deref(),
            Some(&b"unsigned-key"[..])
        );
        assert!(!legacy.exists());
        assert!(home.path().join("vault/device-key").is_file());
        assert_eq!(
            vault
                .load_secret_in("device-key", home.path())
                .unwrap()
                .as_deref(),
            Some(&b"unsigned-key"[..])
        );
        // Nothing left to adopt.
        assert_eq!(
            vault
                .load_alternate_secret_in("device-key", home.path())
                .unwrap(),
            None
        );
    }

    #[test]
    fn a_vault_store_adopts_the_files_key_only_when_asked() {
        let home = tempfile::tempdir().unwrap();
        let vault = Store::TestKeychain(home.path().join("vault"));
        let file = Store::File(home.path().join("credentials.json"));
        vault.save_secret("device-key", b"signed-key").unwrap();
        file.save_secret("device-key", b"unsigned-key").unwrap();
        // Both builds made a key: the vault's own wins on load, the file's
        // is the alternate.
        assert_eq!(
            vault
                .load_secret_in("device-key", home.path())
                .unwrap()
                .as_deref(),
            Some(&b"signed-key"[..])
        );
        assert_eq!(
            vault
                .load_alternate_secret_in("device-key", home.path())
                .unwrap()
                .as_deref(),
            Some(&b"unsigned-key"[..])
        );
        // Adopting moves the file's key in and deletes the file.
        vault
            .adopt_alternate_secret_in("device-key", home.path())
            .unwrap();
        assert_eq!(
            vault
                .load_secret_in("device-key", home.path())
                .unwrap()
                .as_deref(),
            Some(&b"unsigned-key"[..])
        );
        assert!(!home.path().join("device-key").exists());
        // A broken file is left alone and not moved.
        std::fs::write(home.path().join("device-key"), "not base64!").unwrap();
        vault.clear_secret("device-key").unwrap();
        assert!(vault.load_secret_in("device-key", home.path()).is_err());
        assert!(home.path().join("device-key").exists());
        // A test vault inside the home never treats its own copy as a file
        // build's.
        let inner = Store::TestKeychain(home.path().to_path_buf());
        assert_eq!(
            inner.file_secret_to_migrate("device-key", home.path()),
            None
        );
        // A file store never migrates.
        assert_eq!(file.file_secret_to_migrate("device-key", home.path()), None);
    }

    #[test]
    fn only_the_default_file_store_outside_tests_reads_the_vault() {
        let d = Path::new("/h/.cua/credentials.json");
        assert!(vault_fallback_applies(d, d, None, false, true));
        assert!(vault_fallback_applies(d, d, Some(" "), false, true));
        assert!(!vault_fallback_applies(d, d, Some("file"), false, true));
        assert!(!vault_fallback_applies(d, d, None, true, true));
        // A temporary CUA_HOME (a test harness, a sandbox) never does.
        assert!(!vault_fallback_applies(d, d, None, false, false));
        assert!(!vault_fallback_applies(
            Path::new("/elsewhere/credentials.json"),
            d,
            None,
            false,
            true
        ));
        // This test process never does.
        assert!(!Store::File(cua_home().join("credentials.json")).reads_vault_fallback());
    }

    #[test]
    fn flows_parse() {
        assert_eq!(Flow::parse("pkce").unwrap(), Flow::Browser);
        assert_eq!(Flow::parse("").unwrap(), Flow::Auto);
        assert_eq!(Flow::parse("device").unwrap(), Flow::Device);
        assert!(Flow::parse("x").is_err());
    }

    #[test]
    fn legacy_spaces_session_migrates_into_an_empty_store_once() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::File(d.path().join("credentials.json"));
        std::fs::write(
            d.path().join(LEGACY_SPACES_SESSION),
            r#"{"access_token":"at","refresh_token":"rt","expires_at":4102444800,"email":"a@b.c"}"#,
        )
        .unwrap();
        assert!(store.migrate_legacy(d.path()).unwrap());
        assert!(!d.path().join(LEGACY_SPACES_SESSION).exists());
        let c = store.load().unwrap().unwrap();
        assert_eq!(c.refresh_token.as_deref(), Some("rt"));
        assert!(!c.needs_refresh());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(d.path().join("credentials.json")).unwrap();
            assert_eq!(m.permissions().mode() & 0o777, 0o600);
        }
        // With a session already stored, the legacy file is just dropped.
        std::fs::write(
            d.path().join(LEGACY_SPACES_SESSION),
            r#"{"access_token":"other","expires_at":1}"#,
        )
        .unwrap();
        assert!(!store.migrate_legacy(d.path()).unwrap());
        assert_eq!(store.load().unwrap().unwrap().access_token, "at");
        assert!(!d.path().join(LEGACY_SPACES_SESSION).exists());
        assert!(!store.migrate_legacy(d.path()).unwrap());
    }

    #[test]
    fn identity_prefers_the_id_token() {
        use base64::Engine;
        let e = |v: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
        let at = format!("h.{}.s", e(r#"{"preferred_username":"u"}"#));
        let it = format!("h.{}.s", e(r#"{"email":"u@x.y","sub":"s1"}"#));
        let mut c = Credentials::from_token_response(
            &serde_json::json!({"access_token": at, "expires_in": 60}),
        )
        .unwrap();
        assert_eq!(c.identity().display().as_deref(), Some("u"));
        c.id_token = Some(it);
        assert_eq!(c.identity().display().as_deref(), Some("u@x.y"));
    }
}
