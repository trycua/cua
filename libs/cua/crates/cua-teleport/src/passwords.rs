// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Saved browser passwords, decrypted for the Cua Keyvault.
//!
//! The Keyvault imports a browser's saved logins (first party, behind user
//! presence) and keeps them sealed; they are never delivered as files. This
//! module reads Chrome's `Login Data` SQLite database and decrypts each
//! `password_value` with Chrome's own scheme on the platform that wrote it:
//!
//! | platform | prefix | key |
//! |----------|--------|-----|
//! | macOS    | `v10`  | PBKDF2-HMAC-SHA1(`Chrome Safe Storage` Keychain secret, `saltysalt`, 1003 rounds, 16 bytes) |
//! | Linux    | `v10`  | PBKDF2-HMAC-SHA1(`peanuts`, `saltysalt`, 1 round, 16 bytes) |
//! | Linux    | `v11`  | PBKDF2-HMAC-SHA1(the libsecret `chrome` secret, `saltysalt`, 1 round, 16 bytes) |
//!
//! then AES-128-CBC with an IV of 16 spaces and PKCS#7 padding. Windows
//! (DPAPI-wrapped AES-GCM) is refused with a typed error rather than guessed.
//!
//! Every host effect (the Keychain read on macOS, `secret-tool` on Linux, the
//! home directory) goes through [`HostEffects`], so tests never touch the
//! machine.

use std::path::{Path, PathBuf};

use cua_teleport_bundle::chromium_crypto;
use cua_teleport_bundle::layout::chrome::user_data_dir_for;
use zeroize::Zeroizing;

use crate::host::HostEffects;
use crate::safe_storage::SafeStorageKeys;
use crate::{Platform, TeleportError};

/// Chrome's macOS Keychain item.
pub const MAC_SAFE_STORAGE: &str = "Chrome Safe Storage";
/// Chrome's fixed Linux v10 password (no key store).
pub const LINUX_V10_PASSWORD: &str = "peanuts";

/// One saved login, decrypted. `Debug` never prints the password, and the
/// password is wiped on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct SavedLogin {
    /// Origin the login was saved for (`https://github.com/`).
    pub origin: String,
    /// Registrable site the origin belongs to (`github.com`).
    pub site: String,
    /// Username (may be empty for password-only forms).
    pub username: String,
    /// Password.
    pub password: Zeroizing<String>,
}

impl std::fmt::Debug for SavedLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedLogin")
            .field("origin", &self.origin)
            .field("site", &self.site)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// An IPv4 or IPv6 literal (no network types here: host effects live in
/// `host.rs` only).
fn is_ip_literal(h: &str) -> bool {
    h.contains(':')
        || (h.split('.').count() == 4
            && h.split('.')
                .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit())))
}

/// The registrable site of `host`: an IP address or a single-label host is
/// its own site; otherwise [`crate::cookies::registrable_domain`].
pub fn site_for_host(host: &str) -> String {
    let h = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if is_ip_literal(&h) || !h.contains('.') {
        return h;
    }
    crate::cookies::registrable_domain(&h)
}

/// `scheme://host[:port]` of a URL, lowercased, without a path; `None` when
/// the string is not an `http(s)` URL.
pub fn origin_of(url: &str) -> Option<String> {
    let url = url.trim();
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    // Drop userinfo (never part of an origin).
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

/// The host part of an origin (`https://a.b:8443` -> `a.b`).
pub fn host_of_origin(origin: &str) -> String {
    let authority = origin.split_once("://").map(|x| x.1).unwrap_or(origin);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default().to_string();
    }
    authority.split(':').next().unwrap_or_default().to_string()
}

/// Encrypts `plain` the way Chrome does (tests and fixtures build synthetic
/// `Login Data` with it). Returns `prefix || AES-128-CBC(plain)`.
pub fn encrypt_for_tests(prefix: &[u8; 3], password: &[u8], rounds: u32, plain: &str) -> Vec<u8> {
    let key = chromium_crypto::derive_key(password, rounds);
    let ct = chromium_crypto::encrypt(&key, plain.as_bytes());
    let mut out = prefix.to_vec();
    out.extend(ct);
    out
}

/// Decrypts one `v10`/`v11` Safe-Storage value into a saved password,
/// requiring valid UTF-8 (a password is always text).
fn decrypt_password(
    keys: &mut SafeStorageKeys<'_>,
    value: &[u8],
) -> Result<Zeroizing<String>, TeleportError> {
    let plain = keys.decrypt(value)?;
    let text = String::from_utf8(plain.to_vec())
        .map_err(|_| TeleportError::Provider("a saved password is not UTF-8".into()))?;
    Ok(Zeroizing::new(text))
}

/// Reads the saved logins of one Chrome profile.
pub struct ChromePasswords {
    host: std::sync::Arc<dyn HostEffects>,
    platform: Platform,
    profile_dir: Option<PathBuf>,
    profile: Option<String>,
}

impl ChromePasswords {
    /// A reader for the current platform's Chrome, `Default` profile.
    pub fn new(host: std::sync::Arc<dyn HostEffects>) -> Self {
        Self {
            host,
            platform: Platform::current(),
            profile_dir: None,
            profile: None,
        }
    }

    /// Decrypt with `platform`'s scheme (the platform that wrote the profile).
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
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
            TeleportError::Provider("no home directory to read Chrome's profile from".into())
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

    /// Every saved login (blocklisted "never save" rows and rows without a
    /// password are skipped), optionally only for `sites`. A row that fails
    /// to decrypt fails the read: a partial import would silently lose a
    /// credential the user asked for.
    pub fn read(&self, sites: &[String]) -> Result<Vec<SavedLogin>, TeleportError> {
        self.read_report(sites).map(|r| r.logins)
    }

    /// [`Self::read`], also reporting the logins that cannot be read (a
    /// Chromium app-bound `v20` value) instead of failing the whole read.
    pub fn read_report(&self, sites: &[String]) -> Result<PasswordRead, TeleportError> {
        let dir = self.profile_dir()?;
        let db = dir.join("Login Data");
        if !db.is_file() {
            return Err(TeleportError::Provider(format!(
                "no saved passwords: {} has no Login Data",
                dir.display()
            )));
        }
        let rows = read_rows(&db)?;
        let wanted: Vec<String> = sites
            .iter()
            .map(|s| s.trim().to_ascii_lowercase())
            .collect();
        let mut keys = SafeStorageKeys::new(self.host.as_ref(), self.platform, MAC_SAFE_STORAGE);
        let mut out = PasswordRead::default();
        for r in rows {
            let Some(origin) = origin_of(&r.origin_url) else {
                continue;
            };
            let site = site_for_host(&host_of_origin(&origin));
            if !wanted.is_empty() && !wanted.contains(&site) {
                continue;
            }
            if r.password_value.is_empty() {
                continue;
            }
            if crate::browser_cookies::is_app_bound(&r.password_value) {
                out.unavailable.push(UnavailableLogin {
                    origin,
                    site,
                    username: r.username_value,
                    reason: crate::browser_cookies::APP_BOUND_REASON.into(),
                });
                continue;
            }
            let password = decrypt_password(&mut keys, &r.password_value)?;
            out.logins.push(SavedLogin {
                origin,
                site,
                username: r.username_value,
                password,
            });
        }
        Ok(out)
    }
}

/// What one read produced: the logins that decrypted and the ones that cannot
/// (and why).
#[derive(Debug, Default)]
pub struct PasswordRead {
    /// Decrypted logins.
    pub logins: Vec<SavedLogin>,
    /// Logins this build cannot read.
    pub unavailable: Vec<UnavailableLogin>,
}

/// A login that cannot be read, never with its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnavailableLogin {
    /// The page origin.
    pub origin: String,
    /// The registrable-domain site.
    pub site: String,
    /// The username.
    pub username: String,
    /// Why, in a sentence.
    pub reason: String,
}

impl ChromePasswords {
    /// How many saved logins each site holds and how many of them are
    /// app-bound (unreadable), from the plaintext columns: nothing is
    /// decrypted, so the Keychain is never asked.
    pub fn count_by_site(
        &self,
    ) -> Result<std::collections::BTreeMap<String, (u32, u32)>, TeleportError> {
        let dir = self.profile_dir()?;
        let db = dir.join("Login Data");
        let mut out: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
        if !db.is_file() {
            return Ok(out);
        }
        for r in read_rows(&db)? {
            let Some(origin) = origin_of(&r.origin_url) else {
                continue;
            };
            if r.password_value.is_empty() {
                continue;
            }
            let site = site_for_host(&host_of_origin(&origin));
            let e = out.entry(site).or_default();
            if crate::browser_cookies::is_app_bound(&r.password_value) {
                e.1 += 1;
            } else {
                e.0 += 1;
            }
        }
        Ok(out)
    }
}

struct Row {
    origin_url: String,
    username_value: String,
    password_value: Vec<u8>,
}

fn read_rows(db: &Path) -> Result<Vec<Row>, TeleportError> {
    // Chrome holds `Login Data` open; read a private copy so a running
    // browser's lock never blocks the import and the original is untouched.
    let tmp = tempfile_copy(db)?;
    let result = (|| -> rusqlite::Result<Vec<Row>> {
        let conn = rusqlite::Connection::open_with_flags(
            tmp.path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut stmt = conn.prepare(
            "SELECT origin_url, username_value, password_value FROM logins \
             WHERE blacklisted_by_user = 0 ORDER BY date_last_used DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Row {
                origin_url: r.get(0)?,
                username_value: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                password_value: r.get::<_, Option<Vec<u8>>>(2)?.unwrap_or_default(),
            })
        })?;
        rows.collect()
    })();
    result.map_err(|e| TeleportError::Provider(format!("reading Login Data failed: {e}")))
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
        let _ = std::fs::remove_file(&self.0);
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

fn tempfile_copy(src: &Path) -> Result<TempCopy, TeleportError> {
    let dir = std::env::temp_dir().join(format!("cua-logins-{:016x}", rand::random::<u64>()));
    std::fs::create_dir(&dir)
        .map_err(|e| TeleportError::Provider(format!("temp dir for Login Data: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let dst = dir.join("Login Data");
    std::fs::copy(src, &dst).map_err(|e| {
        let _ = std::fs::remove_dir(&dir);
        TeleportError::Provider(format!("copying Login Data failed: {e}"))
    })?;
    Ok(TempCopy(dst))
}

/// Writes a Chrome-shaped `Login Data` with `rows` of (origin, username,
/// encrypted password) into `profile_dir` (tests and e2e fixtures).
pub fn write_login_data_for_tests(
    profile_dir: &Path,
    rows: &[(&str, &str, Vec<u8>)],
) -> Result<(), TeleportError> {
    std::fs::create_dir_all(profile_dir)
        .map_err(|e| TeleportError::Provider(format!("profile dir: {e}")))?;
    let db = profile_dir.join("Login Data");
    let run = || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(&db)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS logins (origin_url VARCHAR NOT NULL, action_url VARCHAR, \
             username_element VARCHAR, username_value VARCHAR, password_element VARCHAR, \
             password_value BLOB, signon_realm VARCHAR NOT NULL, date_created INTEGER NOT NULL \
             DEFAULT 0, blacklisted_by_user INTEGER NOT NULL DEFAULT 0, scheme INTEGER NOT NULL \
             DEFAULT 0, date_last_used INTEGER NOT NULL DEFAULT 0);",
        )?;
        for (i, (origin, user, pw)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO logins (origin_url, username_value, password_value, signon_realm, \
                 date_last_used) VALUES (?1, ?2, ?3, ?1, ?4)",
                rusqlite::params![origin, user, pw, i as i64],
            )?;
        }
        Ok(())
    };
    run().map_err(|e| TeleportError::Provider(format!("writing Login Data: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{EffectKind, FakeHost, HostOutput};
    use std::sync::Arc;

    #[test]
    fn origins_and_sites() {
        assert_eq!(
            origin_of("https://GitHub.com/login?x=1").as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            origin_of("http://login.example.test:8000/").as_deref(),
            Some("http://login.example.test:8000")
        );
        assert_eq!(
            origin_of("https://a.b:443/").as_deref(),
            Some("https://a.b")
        );
        assert_eq!(
            origin_of("https://u:p@a.b/").as_deref(),
            Some("https://a.b")
        );
        assert_eq!(origin_of("android://x@com.app/"), None);
        assert_eq!(origin_of("not a url"), None);
        assert_eq!(site_for_host("login.example.test"), "example.test");
        assert_eq!(site_for_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(site_for_host("localhost"), "localhost");
        assert_eq!(
            host_of_origin("http://login.example.test:8000"),
            "login.example.test"
        );
    }

    #[test]
    fn linux_v10_and_v11_decrypt() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Default");
        write_login_data_for_tests(
            &profile,
            &[
                (
                    "http://login.example.test:8000/",
                    "ada@example.test",
                    encrypt_for_tests(b"v10", b"peanuts", 1, "s3cret-Pa55"),
                ),
                (
                    "https://other.test/",
                    "bob",
                    encrypt_for_tests(b"v11", b"libsecret-key", 1, "other-pw"),
                ),
            ],
        )
        .unwrap();
        let host = Arc::new(FakeHost::new().with_responder(|c| {
            assert_eq!(c.kind, EffectKind::KeychainRead);
            assert_eq!(c.program, "secret-tool");
            Ok(HostOutput::ok(b"libsecret-key\n".to_vec()))
        }));
        let reader = ChromePasswords::new(host.clone())
            .with_platform(Platform::Linux)
            .with_profile_dir(&profile);
        let only = reader.read(&["example.test".into()]).unwrap();
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].origin, "http://login.example.test:8000");
        assert_eq!(only[0].username, "ada@example.test");
        assert_eq!(only[0].password.as_str(), "s3cret-Pa55");
        assert!(
            host.calls().is_empty(),
            "v10 on Linux needs no key store read"
        );
        assert!(!format!("{only:?}").contains("s3cret"), "Debug redacts");
        let all = reader.read(&[]).unwrap();
        assert_eq!(all.len(), 2);
        let other = all.iter().find(|l| l.site == "other.test").unwrap();
        assert_eq!(other.password.as_str(), "other-pw");
        assert_eq!(host.calls().len(), 1, "libsecret is read once");
    }

    #[test]
    fn macos_v10_uses_the_safe_storage_secret() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        let profile = home
            .join(user_data_dir_for(Platform::MacOS))
            .join("Default");
        write_login_data_for_tests(
            &profile,
            &[(
                "https://github.com/",
                "octo",
                encrypt_for_tests(b"v10", b"mac-safe-storage", 1003, "gh-pw"),
            )],
        )
        .unwrap();
        let host = Arc::new(FakeHost::new().with_home(&home).with_responder(|c| {
            // `security find-generic-password -s "Chrome Safe Storage" -w`
            if c.args.iter().any(|a| a == "-w") {
                Ok(HostOutput::ok(b"mac-safe-storage\n".to_vec()))
            } else {
                Ok(HostOutput::ok(
                    b"    \"acct\"<blob>=\"Chrome\"\n    \"svce\"<blob>=\"Chrome Safe Storage\"\n"
                        .to_vec(),
                ))
            }
        }));
        let got = ChromePasswords::new(host.clone())
            .with_platform(Platform::MacOS)
            .read(&[]);
        if cfg!(target_os = "macos") {
            let got = got.unwrap();
            assert_eq!(got[0].password.as_str(), "gh-pw");
            assert!(!host.calls_of(EffectKind::KeychainRead).is_empty());
        } else {
            // Off macOS the Keychain seam reads nothing, so it fails closed.
            assert!(got.unwrap_err().to_string().contains("Safe Storage"));
        }
    }

    #[test]
    fn a_wrong_key_fails_the_whole_read() {
        let dir = tempfile::tempdir().unwrap();
        write_login_data_for_tests(
            dir.path(),
            &[(
                "https://a.test/",
                "u",
                encrypt_for_tests(b"v10", b"not-peanuts", 1, "pw"),
            )],
        )
        .unwrap();
        let err = ChromePasswords::new(Arc::new(FakeHost::new()))
            .with_platform(Platform::Linux)
            .with_profile_dir(dir.path())
            .read(&[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not decrypt"), "{err}");
        assert!(!err.contains("pw\""), "{err}");
    }

    #[test]
    fn windows_is_refused_by_name() {
        let dir = tempfile::tempdir().unwrap();
        write_login_data_for_tests(
            dir.path(),
            &[("https://a.test/", "u", b"\x01\x02".to_vec())],
        )
        .unwrap();
        let err = ChromePasswords::new(Arc::new(FakeHost::new()))
            .with_platform(Platform::Windows)
            .with_profile_dir(dir.path())
            .read(&[])
            .unwrap_err()
            .to_string();
        assert!(err.contains("Windows"), "{err}");
    }
}
