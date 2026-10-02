// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Chromium-family cookies, decrypted for the Cua Keyvault.
//!
//! Chrome's `Cookies` SQLite database encrypts each row's value into
//! `encrypted_value` with the same per-machine Safe Storage scheme saved
//! passwords use (`passwords.rs`): PBKDF2-HMAC-SHA1 + AES-128-CBC, in
//! [`cua_teleport_bundle::chromium_crypto`]. A plain file copy of `Cookies`
//! (what the `chrome` teleport provider's `FullProfile` scope moves today)
//! therefore lands undecryptable bytes on any destination whose Safe Storage
//! key differs from the source's -- which is every destination, because a
//! fresh Space's Chrome (or a Chrome that has never seen that Keychain item)
//! has its own, different key. This module is the missing half: it decrypts
//! on the source so the caller (the Keyvault import) can hold plaintext
//! cookie rows in memory, seal them, and re-encrypt them for the
//! destination's own key on delivery -- never the source's key.
//!
//! [`ChromeCookies::read`] optionally restricts the result to one or more
//! sites via [`crate::cookies::host_key_in_site`] (the per-site cookie
//! filter the Keyvault review sheet's "Save to Keyvault" offers per site, not
//! the whole profile) -- this wires that filter into an actual reader for
//! the first time; the filter itself already existed and was already tested,
//! just never called from outside its own module.
//!
//! Every host effect (the Keychain read on macOS, `secret-tool` on Linux, the
//! home directory) goes through [`HostEffects`], so tests never touch the
//! machine. Nothing in this module writes a file: it only reads and decrypts.

use std::path::{Path, PathBuf};

use cua_teleport_bundle::chromium_crypto;
use cua_teleport_bundle::layout::chrome::user_data_dir_for;
use zeroize::Zeroizing;

use crate::cookies::host_key_in_site;
use crate::host::HostEffects;
use crate::safe_storage::SafeStorageKeys;
use crate::{Platform, TeleportError};

/// One decrypted cookie row. `Debug` never prints the value, and the value is
/// wiped on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct DecryptedCookie {
    /// The `host_key` column (`.github.com`, `api.github.com`, …): the
    /// domain/host this cookie was set for, exactly as Chrome stores it
    /// (leading dot preserved for domain cookies).
    pub host_key: String,
    /// Cookie name.
    pub name: String,
    /// Decrypted value.
    pub value: Zeroizing<String>,
    /// Cookie path.
    pub path: String,
    /// Expiry, `chrome_utc` microseconds since 1601-01-01 (0 for a session
    /// cookie). Carried through unchanged; the receiver writes it back
    /// verbatim, since it is Chrome's own epoch and never needs converting
    /// here.
    pub expires_utc: i64,
    pub is_secure: bool,
    pub is_httponly: bool,
    /// Chrome's `samesite` enum (-1 unspecified, 0 none, 1 lax, 2 strict).
    pub samesite: i64,
    /// Every other column of Chrome's current schema, kept when the store
    /// has it (`None`: an older Chrome without that column). They travel so
    /// the destination's row is the source's row, not a reconstruction.
    pub extra: CookieExtras,
    /// The registrable-domain site [`host_key_in_site`] grouped this row
    /// under, for a caller that filtered by site.
    pub site: String,
}

/// The cookie columns beyond the core ones, as Chrome's `cookies` table has
/// them (names as in the schema). A partitioned (CHIPS) cookie is the one
/// with a non-empty `top_frame_site_key`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CookieExtras {
    /// `creation_utc`.
    pub creation_utc: Option<i64>,
    /// `last_access_utc`.
    pub last_access_utc: Option<i64>,
    /// `last_update_utc`.
    pub last_update_utc: Option<i64>,
    /// `priority` (0 low, 1 medium, 2 high).
    pub priority: Option<i64>,
    /// `source_scheme` (0 unset, 1 non-secure, 2 secure).
    pub source_scheme: Option<i64>,
    /// `source_port` (-1 unspecified).
    pub source_port: Option<i64>,
    /// `source_type` (0 unknown, 1 http, 2 script, 3 other).
    pub source_type: Option<i64>,
    /// `has_cross_site_ancestor`.
    pub has_cross_site_ancestor: Option<i64>,
    /// `top_frame_site_key`: the top-level site a partitioned cookie is
    /// keyed to; empty for an unpartitioned cookie.
    pub top_frame_site_key: Option<String>,
}

impl CookieExtras {
    /// The partition (`top_frame_site_key`) when the cookie is partitioned.
    pub fn partition(&self) -> Option<&str> {
        self.top_frame_site_key.as_deref().filter(|k| !k.is_empty())
    }
}

/// A cookie that cannot be read, and why, so a review can grey it out
/// instead of the whole read failing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableCookie {
    /// The `host_key` column.
    pub host_key: String,
    /// The cookie name.
    pub name: String,
    /// The registrable-domain site.
    pub site: String,
    /// Why, in a sentence.
    pub reason: String,
}

/// The reason shown for a Chromium app-bound (`v20`) value.
pub const APP_BOUND_REASON: &str =
    "Chrome protects it with app-bound encryption, which only Chrome itself can unlock on this PC";

/// What one read produced: the cookies that decrypted and the ones that
/// cannot (and why).
#[derive(Debug, Default)]
pub struct CookieRead {
    /// Decrypted cookies.
    pub cookies: Vec<DecryptedCookie>,
    /// Cookies this build cannot read.
    pub unavailable: Vec<UnavailableCookie>,
}

impl DecryptedCookie {
    /// The bundle's cookie row for this cookie: every column the source had,
    /// the partition kept as `partition_key`.
    pub fn into_item(self) -> cua_teleport_bundle::cookies::CookieItem {
        let e = &self.extra;
        cua_teleport_bundle::cookies::CookieItem {
            host_key: self.host_key.clone(),
            name: self.name.clone(),
            value: self.value.as_bytes().to_vec(),
            path: self.path.clone(),
            expires_utc: self.expires_utc,
            is_secure: self.is_secure,
            is_httponly: self.is_httponly,
            samesite: self.samesite,
            extra: cua_teleport_bundle::cookies::CookieExtra {
                creation_utc: e.creation_utc,
                last_access_utc: e.last_access_utc,
                last_update_utc: e.last_update_utc,
                priority: e.priority,
                source_scheme: e.source_scheme,
                source_port: e.source_port,
                source_type: e.source_type,
                has_cross_site_ancestor: e.has_cross_site_ancestor,
                partition_key: e.partition().map(str::to_string),
            },
        }
    }
}

impl std::fmt::Debug for DecryptedCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecryptedCookie")
            .field("host_key", &self.host_key)
            .field("name", &self.name)
            .field("value", &"<redacted>")
            .field("path", &self.path)
            .field("site", &self.site)
            .finish()
    }
}

/// Decrypts one `encrypted_value` column through `keys` -- the same
/// [`SafeStorageKeys`] resolver `passwords.rs` uses, since the same Safe
/// Storage key decrypts both a profile's cookies and its saved passwords. An
/// empty `encrypted_value` with a non-empty plaintext `value` (rows Chrome
/// has not migrated to encryption, or legacy rows) is returned as-is, with
/// no key needed.
fn decrypt_cookie_value(
    keys: &mut SafeStorageKeys<'_>,
    host_key: &str,
    encrypted_value: &[u8],
    plain_value: &str,
) -> Result<String, TeleportError> {
    if encrypted_value.is_empty() {
        return Ok(plain_value.to_string());
    }
    let plain = keys.decrypt(encrypted_value)?;
    let value = strip_host_key_digest(host_key, &plain);
    String::from_utf8(value.to_vec())
        .map_err(|_| TeleportError::Provider("a decrypted cookie value is not UTF-8".into()))
}

/// Chrome 130 and later (`Cookies` meta version 24+) put SHA-256 of the
/// row's `host_key` in front of the value before encrypting it, and check it
/// on read. Strips that digest when it is there; older rows are unchanged.
fn strip_host_key_digest<'a>(host_key: &str, plain: &'a [u8]) -> &'a [u8] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(host_key.as_bytes());
    match plain.strip_prefix(digest.as_slice()) {
        Some(rest) => rest,
        None => plain,
    }
}

/// Reads and decrypts the cookies of one Chromium-family browser profile.
pub struct ChromeCookies {
    host: std::sync::Arc<dyn HostEffects>,
    platform: Platform,
    /// The catalog browser id (`"chrome"` by default); resolves the macOS
    /// Safe Storage service name via
    /// [`chromium_crypto::macos_safe_storage_service`].
    browser_id: String,
    profile_dir: Option<PathBuf>,
    profile: Option<String>,
}

impl ChromeCookies {
    /// A reader for the current platform's Chrome, `Default` profile.
    pub fn new(host: std::sync::Arc<dyn HostEffects>) -> Self {
        Self {
            host,
            platform: Platform::current(),
            browser_id: "chrome".to_string(),
            profile_dir: None,
            profile: None,
        }
    }

    /// Decrypt with `platform`'s scheme (the platform that wrote the profile).
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// Read a different Chromium-family browser's Safe Storage item (e.g.
    /// `"brave"`) instead of Chrome's. Unknown ids fail the read with a named
    /// error rather than silently falling back to Chrome's key.
    pub fn with_browser(mut self, browser_id: impl Into<String>) -> Self {
        self.browser_id = browser_id.into();
        self
    }

    /// Read this exact profile directory.
    pub fn with_profile_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.profile_dir = Some(dir.into());
        self
    }

    /// Read the named profile (`Profile 1`) or a profile path.
    pub fn with_profile(mut self, profile: Option<String>) -> Self {
        self.profile = profile.filter(|p| !p.trim().is_empty());
        self
    }

    /// The profile directory this reader uses.
    pub fn profile_dir(&self) -> Result<PathBuf, TeleportError> {
        if let Some(d) = &self.profile_dir {
            return Ok(d.clone());
        }
        let home = self.host.home_dir().ok_or_else(|| {
            TeleportError::Provider("no home directory to read the browser's profile from".into())
        })?;
        let root = home.join(user_data_dir_for(self.platform));
        Ok(match &self.profile {
            Some(p) if Path::new(p).is_absolute() || p.contains(std::path::MAIN_SEPARATOR) => {
                PathBuf::from(p)
            }
            Some(p) => root.join(p),
            None => root.join("Default"),
        })
    }

    /// Every cookie, decrypted, optionally restricted to the registrable
    /// domains in `sites` (empty reads every cookie). A row that fails to
    /// decrypt fails the whole read: a partial import would silently drop a
    /// cookie the destination needs and still report success.
    pub fn read(&self, sites: &[String]) -> Result<Vec<DecryptedCookie>, TeleportError> {
        self.read_report(sites).map(|r| r.cookies)
    }

    /// [`Self::read`], also reporting the cookies that cannot be read (a
    /// Chromium app-bound `v20` value) instead of failing the whole read.
    pub fn read_report(&self, sites: &[String]) -> Result<CookieRead, TeleportError> {
        let dir = self.profile_dir()?;
        // `Default/Cookies` or `Default/Network/Cookies`, whichever is newer.
        let db = cua_teleport_bundle::layout::chrome::cookies_store(&dir);
        if !db.is_file() {
            return Err(TeleportError::Provider(format!(
                "no cookies: {} has no Cookies database",
                dir.display()
            )));
        }
        let service = match self.platform {
            Platform::MacOS => chromium_crypto::macos_safe_storage_service(&self.browser_id)
                .ok_or_else(|| {
                    TeleportError::Provider(format!(
                        "{} is not a Chromium-family browser this build can decrypt",
                        self.browser_id
                    ))
                })?,
            // Linux keys don't depend on the browser's own service name
            // (the v10 password is fixed and v11 reads the shared "chrome"
            // libsecret application); Windows is refused before it matters.
            Platform::Linux | Platform::Windows => "",
        };
        let rows = read_rows(&db)?;
        let wanted: Vec<String> = sites
            .iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .collect();
        let mut keys = SafeStorageKeys::new(self.host.as_ref(), self.platform, service);
        let mut out = CookieRead::default();
        for r in rows {
            let site = crate::passwords::site_for_host(&r.host_key);
            if !wanted.is_empty() && !wanted.iter().any(|w| host_key_in_site(&r.host_key, w)) {
                continue;
            }
            if is_app_bound(&r.encrypted_value) {
                out.unavailable.push(UnavailableCookie {
                    host_key: r.host_key,
                    name: r.name,
                    site,
                    reason: APP_BOUND_REASON.into(),
                });
                continue;
            }
            let value = decrypt_cookie_value(&mut keys, &r.host_key, &r.encrypted_value, &r.value)?;
            out.cookies.push(DecryptedCookie {
                host_key: r.host_key,
                name: r.name,
                value: Zeroizing::new(value),
                path: r.path,
                expires_utc: r.expires_utc,
                is_secure: r.is_secure,
                is_httponly: r.is_httponly,
                samesite: r.samesite,
                extra: r.extra,
                site,
            });
        }
        Ok(out)
    }
}

/// Whether a stored value is Chromium's app-bound (`v20`) encryption: the key
/// lives in Chrome's elevation service, so nobody else can unwrap it.
pub fn is_app_bound(encrypted_value: &[u8]) -> bool {
    encrypted_value.starts_with(b"v20")
}

/// One cookie as its store lists it, without its value: what a review
/// shows (a domain's count and whether it keeps a sign-in) before anything
/// is decrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookieHostRow {
    /// The `host_key` column.
    pub host_key: String,
    /// The cookie name.
    pub name: String,
    /// Chrome's expiry (`0`: a session cookie).
    pub expires_utc: i64,
    /// The value is app-bound (`v20`) and cannot be read by anyone but
    /// Chrome.
    pub app_bound: bool,
}

impl ChromeCookies {
    /// Every cookie's host, name and expiry (plaintext columns): nothing is
    /// decrypted, so the Keychain is never asked.
    pub fn host_rows(&self) -> Result<Vec<CookieHostRow>, TeleportError> {
        let dir = self.profile_dir()?;
        let db = cua_teleport_bundle::layout::chrome::cookies_store(&dir);
        if !db.is_file() {
            return Err(TeleportError::Provider(format!(
                "no cookies: {} has no Cookies database",
                dir.display()
            )));
        }
        Ok(read_rows(&db)?
            .into_iter()
            .map(|r| CookieHostRow {
                host_key: r.host_key,
                name: r.name,
                expires_utc: r.expires_utc,
                app_bound: is_app_bound(&r.encrypted_value),
            })
            .collect())
    }
}

struct Row {
    host_key: String,
    name: String,
    value: String,
    encrypted_value: Vec<u8>,
    path: String,
    expires_utc: i64,
    is_secure: bool,
    is_httponly: bool,
    samesite: i64,
    extra: CookieExtras,
}

/// Reads every row of `db`'s `cookies` table. Selects columns by name (not
/// `SELECT *`): the core ones must exist (`samesite` has since Chrome M80),
/// and every other column of Chrome's current schema is read when this
/// Chrome's table has it, so an older Chrome still reads.
fn read_rows(db: &Path) -> Result<Vec<Row>, TeleportError> {
    // Chrome holds `Cookies` open; read a private copy so a running browser's
    // lock never blocks the import and the original is untouched.
    let tmp = tempfile_copy(db)?;
    let result = (|| -> rusqlite::Result<Vec<Row>> {
        let conn = rusqlite::Connection::open_with_flags(
            tmp.path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        let have: std::collections::HashSet<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(cookies)")?;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            names.into_iter().map(|n| n.to_ascii_lowercase()).collect()
        };
        // Optional columns, in the order the row reads them after the core 9.
        const OPTIONAL: [&str; 9] = [
            "creation_utc",
            "last_access_utc",
            "last_update_utc",
            "priority",
            "source_scheme",
            "source_port",
            "source_type",
            "has_cross_site_ancestor",
            "top_frame_site_key",
        ];
        let optional: Vec<&str> = OPTIONAL
            .iter()
            .copied()
            .filter(|c| have.contains(*c))
            .collect();
        let mut cols = String::from(
            "host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite",
        );
        for c in &optional {
            cols.push_str(", ");
            cols.push_str(c);
        }
        let mut stmt = conn.prepare(&format!("SELECT {cols} FROM cookies"))?;
        let rows = stmt.query_map([], |r| {
            let mut extra = CookieExtras::default();
            for (i, c) in optional.iter().enumerate() {
                let idx = 9 + i;
                match *c {
                    "top_frame_site_key" => extra.top_frame_site_key = r.get(idx)?,
                    other => {
                        let v: Option<i64> = r.get(idx)?;
                        match other {
                            "creation_utc" => extra.creation_utc = v,
                            "last_access_utc" => extra.last_access_utc = v,
                            "last_update_utc" => extra.last_update_utc = v,
                            "priority" => extra.priority = v,
                            "source_scheme" => extra.source_scheme = v,
                            "source_port" => extra.source_port = v,
                            "source_type" => extra.source_type = v,
                            _ => extra.has_cross_site_ancestor = v,
                        }
                    }
                }
            }
            Ok(Row {
                host_key: r.get(0)?,
                name: r.get(1)?,
                value: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                encrypted_value: r.get::<_, Option<Vec<u8>>>(3)?.unwrap_or_default(),
                path: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                expires_utc: r.get(5)?,
                is_secure: r.get::<_, i64>(6)? != 0,
                is_httponly: r.get::<_, i64>(7)? != 0,
                samesite: r.get(8)?,
                extra,
            })
        })?;
        rows.collect()
    })();
    result.map_err(|e| TeleportError::Provider(format!("reading Cookies failed: {e}")))
}

/// A private copy of `path` that is deleted on drop.
struct TempCopy(PathBuf);

impl TempCopy {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempCopy {
    fn drop(&mut self) {
        // The private directory holds the copy and its journal sidecars.
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Copies `src` and its `-wal`, `-shm` and `-journal` sidecars to a private
/// directory. Chrome may be running: a WAL-mode database keeps recent commits
/// only in `-wal`, and a rollback-journal database mid-write has a hot
/// `-journal`; copying the main file alone would miss or tear them. SQLite
/// replays/rolls back the sidecars on open, so the copy reads as a consistent
/// snapshot while the original is never opened, locked or modified.
fn tempfile_copy(src: &Path) -> Result<TempCopy, TeleportError> {
    let dir = std::env::temp_dir().join(format!("cua-cookies-{:016x}", rand::random::<u64>()));
    std::fs::create_dir(&dir)
        .map_err(|e| TeleportError::Provider(format!("temp dir for Cookies: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let dst = dir.join("Cookies");
    // Sidecars first: a checkpoint between the two copies then only makes the
    // main file newer than the log, which replays idempotently.
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut from = src.as_os_str().to_owned();
        from.push(suffix);
        let from = PathBuf::from(from);
        if from.is_file() {
            let _ = std::fs::copy(&from, dir.join(format!("Cookies{suffix}")));
        }
    }
    std::fs::copy(src, &dst).map_err(|e| {
        let _ = std::fs::remove_dir_all(&dir);
        TeleportError::Provider(format!("copying Cookies failed: {e}"))
    })?;
    Ok(TempCopy(dst))
}

/// One row for [`write_cookies_db_for_tests`] (tests and e2e fixtures).
pub struct TestCookieRow<'a> {
    pub host_key: &'a str,
    pub name: &'a str,
    pub encrypted_value: Vec<u8>,
    pub path: &'a str,
    pub expires_utc: i64,
    pub is_secure: bool,
    pub is_httponly: bool,
    pub samesite: i64,
}

/// Writes a Chrome-shaped `Cookies` database (tests and e2e fixtures).
pub fn write_cookies_db_for_tests(
    profile_dir: &Path,
    rows: &[TestCookieRow<'_>],
) -> Result<(), TeleportError> {
    write_cookies_db_at(profile_dir, &profile_dir.join("Cookies"), rows)
}

/// Like [`write_cookies_db_for_tests`], in modern Chrome's layout
/// (`Network/Cookies`, no legacy file).
pub fn write_network_cookies_db_for_tests(
    profile_dir: &Path,
    rows: &[TestCookieRow<'_>],
) -> Result<(), TeleportError> {
    write_cookies_db_at(
        profile_dir,
        &profile_dir.join("Network").join("Cookies"),
        rows,
    )
}

/// One row of Chrome's current (meta version 24) `cookies` schema, every
/// column, for [`write_modern_cookies_db_for_tests`].
pub struct ModernTestRow<'a> {
    pub core: TestCookieRow<'a>,
    pub extra: CookieExtras,
}

/// Writes `Network/Cookies` in Chrome's current schema with every column
/// (tests and e2e fixtures), so a read proves each attribute travels.
pub fn write_modern_cookies_db_for_tests(
    profile_dir: &Path,
    rows: &[ModernTestRow<'_>],
) -> Result<(), TeleportError> {
    let db = profile_dir.join("Network").join("Cookies");
    std::fs::create_dir_all(db.parent().unwrap_or(profile_dir))
        .map_err(|e| TeleportError::Provider(format!("profile dir: {e}")))?;
    let run = || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute_batch(
            "CREATE TABLE meta (key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR);
             INSERT INTO meta VALUES ('version', '24'), ('last_compatible_version', '24');
             CREATE TABLE cookies (
                creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL,
                top_frame_site_key TEXT NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL,
                encrypted_value BLOB NOT NULL, path TEXT NOT NULL, expires_utc INTEGER NOT NULL,
                is_secure INTEGER NOT NULL, is_httponly INTEGER NOT NULL,
                last_access_utc INTEGER NOT NULL, has_expires INTEGER NOT NULL,
                is_persistent INTEGER NOT NULL, priority INTEGER NOT NULL,
                samesite INTEGER NOT NULL, source_scheme INTEGER NOT NULL,
                source_port INTEGER NOT NULL, last_update_utc INTEGER NOT NULL,
                source_type INTEGER NOT NULL, has_cross_site_ancestor INTEGER NOT NULL,
                UNIQUE (host_key, top_frame_site_key, has_cross_site_ancestor, name, path,
                        source_scheme, source_port));",
        )?;
        for r in rows {
            let e = &r.extra;
            conn.execute(
                "INSERT INTO cookies (creation_utc, host_key, top_frame_site_key, name, value, \
                 encrypted_value, path, expires_utc, is_secure, is_httponly, last_access_utc, \
                 has_expires, is_persistent, priority, samesite, source_scheme, source_port, \
                 last_update_utc, source_type, has_cross_site_ancestor) VALUES (?1, ?2, ?3, ?4, \
                 '', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                rusqlite::params![
                    e.creation_utc.unwrap_or(1),
                    r.core.host_key,
                    e.top_frame_site_key.clone().unwrap_or_default(),
                    r.core.name,
                    r.core.encrypted_value,
                    r.core.path,
                    r.core.expires_utc,
                    r.core.is_secure as i64,
                    r.core.is_httponly as i64,
                    e.last_access_utc.unwrap_or(0),
                    (r.core.expires_utc != 0) as i64,
                    e.priority.unwrap_or(1),
                    r.core.samesite,
                    e.source_scheme.unwrap_or(0),
                    e.source_port.unwrap_or(-1),
                    e.last_update_utc.unwrap_or(0),
                    e.source_type.unwrap_or(0),
                    e.has_cross_site_ancestor.unwrap_or(0),
                ],
            )?;
        }
        Ok(())
    };
    run().map_err(|e| TeleportError::Provider(format!("writing the fixture Cookies: {e}")))
}

fn write_cookies_db_at(
    profile_dir: &Path,
    db: &Path,
    rows: &[TestCookieRow<'_>],
) -> Result<(), TeleportError> {
    std::fs::create_dir_all(db.parent().unwrap_or(profile_dir))
        .map_err(|e| TeleportError::Provider(format!("profile dir: {e}")))?;
    let run = || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(db)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cookies (host_key TEXT NOT NULL, name TEXT NOT NULL, \
             value TEXT NOT NULL, encrypted_value BLOB NOT NULL, path TEXT NOT NULL, \
             expires_utc INTEGER NOT NULL, is_secure INTEGER NOT NULL, is_httponly INTEGER NOT \
             NULL, samesite INTEGER NOT NULL DEFAULT -1, creation_utc INTEGER NOT NULL DEFAULT \
             0);",
        )?;
        for row in rows {
            conn.execute(
                "INSERT INTO cookies (host_key, name, value, encrypted_value, path, \
                 expires_utc, is_secure, is_httponly, samesite) VALUES (?1, ?2, '', ?3, ?4, ?5, \
                 ?6, ?7, ?8)",
                rusqlite::params![
                    row.host_key,
                    row.name,
                    row.encrypted_value,
                    row.path,
                    row.expires_utc,
                    row.is_secure as i64,
                    row.is_httponly as i64,
                    row.samesite
                ],
            )?;
        }
        Ok(())
    };
    run().map_err(|e| TeleportError::Provider(format!("writing Cookies: {e}")))
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_chrome_130_value_loses_its_host_key_digest() {
        use sha2::{Digest, Sha256};
        let mut plain = Sha256::digest(b".github.com").to_vec();
        plain.extend_from_slice(b"session=abc");
        assert_eq!(strip_host_key_digest(".github.com", &plain), b"session=abc");
        // Older rows (no digest) and another host's digest are left alone.
        assert_eq!(
            strip_host_key_digest(".github.com", b"session=abc"),
            b"session=abc"
        );
        assert_eq!(
            strip_host_key_digest("example.com", &plain),
            plain.as_slice()
        );
    }
    use super::*;
    use crate::host::FakeHost;
    #[cfg(target_os = "macos")]
    use crate::host::{EffectKind, HostOutput};
    use std::sync::Arc;

    fn v10_row(password: &[u8], rounds: u32, plain: &str) -> Vec<u8> {
        let key = chromium_crypto::derive_key(password, rounds);
        chromium_crypto::encrypt_v10(&key, plain.as_bytes())
    }

    #[test]
    fn linux_cookies_decrypt_and_filter_by_site() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        write_cookies_db_for_tests(
            &profile,
            &[
                TestCookieRow {
                    host_key: ".github.com",
                    name: "user_session",
                    encrypted_value: v10_row(
                        chromium_crypto::LINUX_V10_PASSWORD,
                        1,
                        "gh-session-abc",
                    ),
                    path: "/",
                    expires_utc: 0,
                    is_secure: true,
                    is_httponly: true,
                    samesite: 1,
                },
                TestCookieRow {
                    host_key: "api.github.com",
                    name: "_gh_sess",
                    encrypted_value: v10_row(chromium_crypto::LINUX_V10_PASSWORD, 1, "gh-api-xyz"),
                    path: "/",
                    expires_utc: 0,
                    is_secure: true,
                    is_httponly: true,
                    samesite: 1,
                },
                TestCookieRow {
                    host_key: ".gitlab.com",
                    name: "session",
                    encrypted_value: v10_row(chromium_crypto::LINUX_V10_PASSWORD, 1, "gl-session"),
                    path: "/",
                    expires_utc: 0,
                    is_secure: true,
                    is_httponly: true,
                    samesite: 1,
                },
            ],
        )
        .unwrap();

        let reader = ChromeCookies::new(Arc::new(FakeHost::new()))
            .with_platform(Platform::Linux)
            .with_profile_dir(&profile);

        let only_github = reader.read(&["github.com".into()]).unwrap();
        assert_eq!(only_github.len(), 2, "{only_github:?}");
        assert!(only_github.iter().all(|c| c.site == "github.com"));
        let values: Vec<&str> = only_github.iter().map(|c| c.value.as_str()).collect();
        assert!(values.contains(&"gh-session-abc"));
        assert!(values.contains(&"gh-api-xyz"));
        assert!(
            !format!("{only_github:?}").contains("gh-session-abc"),
            "Debug redacts"
        );

        let everything = reader.read(&[]).unwrap();
        assert_eq!(everything.len(), 3);
    }

    fn one_row() -> Vec<TestCookieRow<'static>> {
        vec![TestCookieRow {
            host_key: ".github.com",
            name: "user_session",
            encrypted_value: v10_row(chromium_crypto::LINUX_V10_PASSWORD, 1, "gh-session-abc"),
            path: "/",
            expires_utc: 0,
            is_secure: true,
            is_httponly: true,
            samesite: 1,
        }]
    }

    fn read_linux(profile: &Path) -> Result<Vec<DecryptedCookie>, TeleportError> {
        ChromeCookies::new(std::sync::Arc::new(FakeHost::new()))
            .with_platform(Platform::Linux)
            .with_profile_dir(profile)
            .read(&[])
    }

    fn modern_row<'a>(
        host: &'a str,
        name: &'a str,
        encrypted: Vec<u8>,
        extra: CookieExtras,
    ) -> ModernTestRow<'a> {
        ModernTestRow {
            core: TestCookieRow {
                host_key: host,
                name,
                encrypted_value: encrypted,
                path: "/",
                expires_utc: 13_400_000_000_000_000,
                is_secure: true,
                is_httponly: true,
                samesite: 1,
            },
            extra,
        }
    }

    /// Every column of Chrome's current schema is read, a partitioned cookie
    /// keeps its partition, and a Windows app-bound (`v20`) value is reported
    /// per cookie instead of failing the whole read.
    #[test]
    fn every_column_is_read_and_app_bound_values_are_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let ok = v10_row(chromium_crypto::LINUX_V10_PASSWORD, 1, "sess-value");
        let full = CookieExtras {
            creation_utc: Some(13_300_000_000_000_001),
            last_access_utc: Some(13_300_000_000_000_002),
            last_update_utc: Some(13_300_000_000_000_003),
            priority: Some(2),
            source_scheme: Some(2),
            source_port: Some(443),
            source_type: Some(1),
            has_cross_site_ancestor: Some(1),
            top_frame_site_key: Some("https://top.example".into()),
        };
        let mut v20 = b"v20".to_vec();
        v20.extend_from_slice(&[7u8; 40]);
        write_modern_cookies_db_for_tests(
            dir.path(),
            &[
                modern_row(".example.com", "sid", ok.clone(), full.clone()),
                modern_row(".example.com", "sid", ok, CookieExtras::default()),
                modern_row(".bank.test", "app_bound", v20, CookieExtras::default()),
            ],
        )
        .unwrap();
        let read = ChromeCookies::new(Arc::new(FakeHost::new()))
            .with_platform(Platform::Linux)
            .with_profile_dir(dir.path())
            .read_report(&[])
            .unwrap();
        assert_eq!(read.cookies.len(), 2);
        let partitioned = read
            .cookies
            .iter()
            .find(|c| c.extra.partition().is_some())
            .unwrap();
        assert_eq!(partitioned.extra, full);
        assert_eq!(partitioned.value.as_str(), "sess-value");
        let plain = read
            .cookies
            .iter()
            .find(|c| c.extra.partition().is_none())
            .unwrap();
        assert_eq!(plain.extra.priority, Some(1));
        assert_eq!(plain.extra.source_port, Some(-1));
        // The v20 value is named, with why, and nothing else fails.
        assert_eq!(read.unavailable.len(), 1);
        assert_eq!(read.unavailable[0].host_key, ".bank.test");
        assert_eq!(read.unavailable[0].site, "bank.test");
        assert!(read.unavailable[0].reason.contains("app-bound"));
        // The bundle row keeps the partition and every attribute.
        let item = partitioned.clone().into_item();
        assert_eq!(
            item.extra.partition_key.as_deref(),
            Some("https://top.example")
        );
        assert_eq!(item.extra.has_cross_site_ancestor, Some(1));
        // The plaintext-column listing flags it too, so a review can grey it.
        let rows = ChromeCookies::new(Arc::new(FakeHost::new()))
            .with_platform(Platform::Linux)
            .with_profile_dir(dir.path())
            .host_rows()
            .unwrap();
        assert_eq!(rows.iter().filter(|r| r.app_bound).count(), 1);
    }

    /// Modern Chrome (96+): only `Network/Cookies` exists.
    #[test]
    fn reads_a_modern_network_cookies_only_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Profile 1");
        write_network_cookies_db_for_tests(&profile, &one_row()).unwrap();
        assert!(!profile.join("Cookies").exists());
        let got = read_linux(&profile).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].value.as_str(), "gh-session-abc");
    }

    /// Older Chrome: only the legacy root `Cookies` exists.
    #[test]
    fn reads_a_legacy_cookies_only_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        write_cookies_db_for_tests(&profile, &one_row()).unwrap();
        assert_eq!(read_linux(&profile).unwrap().len(), 1);
    }

    fn touch(path: &std::path::Path, secs_ago: u64) {
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    /// Both exist: the newer one is read, in either direction.
    #[test]
    fn reads_the_newer_of_two_cookie_stores() {
        for network_newer in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let profile = dir.path().join("Default");
            write_cookies_db_for_tests(&profile, &[]).unwrap();
            write_network_cookies_db_for_tests(&profile, &one_row()).unwrap();
            if !network_newer {
                // Swap the contents: the root file holds the row and is newer.
                std::fs::remove_dir_all(profile.join("Network")).unwrap();
                std::fs::remove_file(profile.join("Cookies")).unwrap();
                write_network_cookies_db_for_tests(&profile, &[]).unwrap();
                write_cookies_db_for_tests(&profile, &one_row()).unwrap();
            }
            let (new, old) = if network_newer {
                (profile.join("Network/Cookies"), profile.join("Cookies"))
            } else {
                (profile.join("Cookies"), profile.join("Network/Cookies"))
            };
            touch(&old, 600);
            touch(&new, 5);
            let got = read_linux(&profile).unwrap();
            assert_eq!(got.len(), 1, "network_newer={network_newer}");
        }
    }

    /// Chrome is running: its newest cookies are only in the `-wal` file. The
    /// reader must see them (and never touch the live database).
    #[test]
    fn reads_cookies_that_only_exist_in_the_wal_of_a_running_chrome() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        write_network_cookies_db_for_tests(&profile, &[]).unwrap();
        let db = profile.join("Network/Cookies");
        let live = rusqlite::Connection::open(&db).unwrap();
        let mode: String = live
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        live.execute_batch("PRAGMA wal_autocheckpoint=0").unwrap();
        let row = &one_row()[0];
        live.execute(
            "INSERT INTO cookies (host_key, name, value, encrypted_value, path, expires_utc, \
             is_secure, is_httponly, samesite) VALUES (?1, ?2, '', ?3, ?4, 0, 1, 1, 1)",
            rusqlite::params![row.host_key, row.name, row.encrypted_value, row.path],
        )
        .unwrap();
        assert!(db.with_file_name("Cookies-wal").metadata().unwrap().len() > 0);
        let before = std::fs::read(&db).unwrap();
        let got = read_linux(&profile).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].value.as_str(), "gh-session-abc");
        // The live database was only copied, never written.
        assert_eq!(std::fs::read(&db).unwrap(), before);
        drop(live);
    }

    #[test]
    fn cookies_store_picks_by_mtime_wal_included() {
        use cua_teleport_bundle::layout::chrome::cookies_store;
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        // Neither: the root path, what current Chrome creates.
        assert_eq!(cookies_store(p), p.join("Cookies"));
        std::fs::write(p.join("Cookies"), b"x").unwrap();
        assert_eq!(cookies_store(p), p.join("Cookies"));
        std::fs::create_dir_all(p.join("Network")).unwrap();
        std::fs::write(p.join("Network/Cookies"), b"x").unwrap();
        touch(&p.join("Cookies"), 600);
        touch(&p.join("Network/Cookies"), 5);
        assert_eq!(cookies_store(p), p.join("Network/Cookies"));
        touch(&p.join("Cookies"), 1);
        touch(&p.join("Network/Cookies"), 600);
        assert_eq!(cookies_store(p), p.join("Cookies"));
        // A fresh -wal on the network store makes it the live one.
        std::fs::write(p.join("Network/Cookies-wal"), b"x").unwrap();
        touch(&p.join("Network/Cookies-wal"), 0);
        touch(&p.join("Cookies"), 100);
        assert_eq!(cookies_store(p), p.join("Network/Cookies"));
    }

    // Reads the macOS login Keychain through `security`: only meaningful there.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_cookies_use_the_named_browsers_safe_storage_item() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        let profile = home
            .join(user_data_dir_for(Platform::MacOS))
            .join("Default");
        write_cookies_db_for_tests(
            &profile,
            &[TestCookieRow {
                host_key: "example.test",
                name: "sid",
                encrypted_value: v10_row(b"brave-safe-storage-secret", 1003, "brave-cookie-value"),
                path: "/",
                expires_utc: 0,
                is_secure: true,
                is_httponly: false,
                samesite: 0,
            }],
        )
        .unwrap();
        let host = Arc::new(FakeHost::new().with_home(&home).with_responder(|c| {
            if c.args.iter().any(|a| a == "-w") {
                Ok(HostOutput::ok(b"brave-safe-storage-secret\n".to_vec()))
            } else {
                Ok(HostOutput::ok(
                    b"    \"acct\"<blob>=\"Brave\"\n    \"svce\"<blob>=\"Brave Safe Storage\"\n"
                        .to_vec(),
                ))
            }
        }));
        let cookies = ChromeCookies::new(host.clone())
            .with_platform(Platform::MacOS)
            .with_browser("brave")
            .read(&[])
            .unwrap();
        assert_eq!(cookies[0].value.as_str(), "brave-cookie-value");
        assert!(!host.calls_of(EffectKind::KeychainRead).is_empty());
    }

    #[test]
    fn plaintext_legacy_rows_pass_through_without_a_key() {
        // A row with an empty encrypted_value (legacy/unmigrated) is
        // returned as its plain `value`, with no Keychain/libsecret read.
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        let db = profile.join("Cookies");
        std::fs::create_dir_all(&profile).unwrap();
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, encrypted_value \
                 BLOB, path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, \
                 samesite INTEGER);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO cookies VALUES ('a.test','n','plain-value',X'','/',0,0,0,-1)",
                [],
            )
            .unwrap();
        }
        let reader = ChromeCookies::new(Arc::new(FakeHost::new()))
            .with_platform(Platform::Linux)
            .with_profile_dir(&profile);
        let rows = reader.read(&[]).unwrap();
        assert_eq!(rows[0].value.as_str(), "plain-value");
    }

    #[test]
    fn unknown_browser_id_on_macos_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        let profile = home
            .join(user_data_dir_for(Platform::MacOS))
            .join("Default");
        write_cookies_db_for_tests(
            &profile,
            &[TestCookieRow {
                host_key: "a.test",
                name: "n",
                encrypted_value: v10_row(b"x", 1, "v"),
                path: "/",
                expires_utc: 0,
                is_secure: false,
                is_httponly: false,
                samesite: -1,
            }],
        )
        .unwrap();
        let reader = ChromeCookies::new(Arc::new(FakeHost::new().with_home(&home)))
            .with_platform(Platform::MacOS)
            .with_browser("netscape-navigator")
            .with_profile_dir(&profile);
        let err = reader.read(&[]).unwrap_err().to_string();
        assert!(err.contains("netscape-navigator"), "{err}");
    }

    #[test]
    fn a_wrong_key_fails_the_whole_read() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        write_cookies_db_for_tests(
            &profile,
            &[TestCookieRow {
                host_key: "a.test",
                name: "n",
                encrypted_value: v10_row(b"not-peanuts", 1, "v"),
                path: "/",
                expires_utc: 0,
                is_secure: false,
                is_httponly: false,
                samesite: -1,
            }],
        )
        .unwrap();
        let err = ChromeCookies::new(Arc::new(FakeHost::new()))
            .with_platform(Platform::Linux)
            .with_profile_dir(&profile)
            .read(&[])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("did not decrypt") || err.contains("padding"),
            "{err}"
        );
    }
}
