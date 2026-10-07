// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Firefox's profile data as Keyvault items, from the profile's own files
//! (formats in `~/projects/.cua-work/keyvault-v2/browser-formats.md`):
//!
//! * profile discovery: `profiles.ini` and `installs.ini`;
//! * cookies: `cookies.sqlite` (`moz_cookies`, schema 16), plaintext values;
//! * localStorage: `storage/default/<origin>/ls/data.sqlite` (`data`, values
//!   optionally UTF-16-to-UTF-8 converted and Snappy-compressed);
//! * saved logins: `logins.json` decrypted with the `key4.db` master key
//!   ([`cua_teleport_bundle::firefox_nss`]), only on request.
//!
//! Every read works from a private copy of the file(s), so a running Firefox
//! is never touched.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use cua_teleport_bundle::cookies::{CookieExtra, CookieItem};
use cua_teleport_bundle::local_storage::LocalStorageItem;
use cua_teleport_bundle::logins::{LoginItem, signon_realm};

use crate::TeleportError;

const CHROME_EPOCH_OFFSET_MICROS: i64 = 11_644_473_600_000_000;

fn perr(m: impl std::fmt::Display) -> TeleportError {
    TeleportError::Provider(m.to_string())
}

// ---- profile discovery ------------------------------------------------------

/// A profile named in `profiles.ini`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirefoxProfile {
    /// `Name=`.
    pub name: String,
    /// The profile directory (absolute).
    pub dir: PathBuf,
    /// `Default=1` in its section.
    pub is_default: bool,
}

fn parse_ini(text: &str) -> Vec<(String, BTreeMap<String, String>)> {
    let mut out: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push((name.to_string(), BTreeMap::new()));
        } else if let Some((k, v)) = line.split_once('=')
            && let Some((_, kv)) = out.last_mut()
        {
            kv.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

/// The profiles of the Firefox application-data directory `app_data`
/// (`~/Library/Application Support/Firefox`, `~/.mozilla/firefox`, `%APPDATA%\Mozilla\Firefox`).
pub fn profiles(app_data: &Path) -> Vec<FirefoxProfile> {
    let Ok(text) = std::fs::read_to_string(app_data.join("profiles.ini")) else {
        return Vec::new();
    };
    parse_ini(&text)
        .into_iter()
        .filter(|(s, kv)| s.starts_with("Profile") && kv.contains_key("Path"))
        .map(|(_, kv)| {
            let path = kv["Path"].clone();
            let relative = kv.get("IsRelative").map(|v| v == "1").unwrap_or(true);
            FirefoxProfile {
                name: kv.get("Name").cloned().unwrap_or_default(),
                dir: if relative {
                    app_data.join(&path)
                } else {
                    PathBuf::from(&path)
                },
                is_default: kv.get("Default").map(|v| v == "1").unwrap_or(false),
            }
        })
        .collect()
}

/// The profile Firefox opens: the install's `Default=` in `installs.ini`,
/// else the one marked `Default=1`, else the only one.
pub fn default_profile(app_data: &Path) -> Option<FirefoxProfile> {
    let all = profiles(app_data);
    if let Ok(text) = std::fs::read_to_string(app_data.join("installs.ini")) {
        for (section, kv) in parse_ini(&text) {
            // installs.ini sections are the install hash itself.
            let _ = &section;
            if let Some(d) = kv.get("Default") {
                let wanted = app_data.join(d);
                if let Some(p) = all
                    .iter()
                    .find(|p| p.dir == wanted || p.dir == Path::new(d))
                {
                    return Some(p.clone());
                }
            }
        }
    }
    all.iter()
        .find(|p| p.is_default)
        .cloned()
        .or_else(|| (all.len() == 1).then(|| all[0].clone()))
}

// ---- a private copy of a SQLite file ----------------------------------------

struct Copy {
    _dir: tempfile::TempDir,
    db: PathBuf,
}

fn private_copy(src: &Path) -> Result<Copy, TeleportError> {
    let dir = tempfile::Builder::new()
        .prefix("cua-ff-")
        .tempdir()
        .map_err(perr)?;
    let name = src.file_name().ok_or_else(|| perr("a database path"))?;
    let db = dir.path().join(name);
    crate::plain_copy::copy_named("a Firefox database", src, &db)?;
    for suffix in ["-wal", "-shm"] {
        let mut side = src.as_os_str().to_owned();
        side.push(suffix);
        let side = PathBuf::from(side);
        if side.is_file() {
            let mut to = db.as_os_str().to_owned();
            to.push(suffix);
            let _ = crate::plain_copy::copy_data_only(&side, Path::new(&to));
        }
    }
    Ok(Copy { _dir: dir, db })
}

fn open(c: &Copy) -> Result<rusqlite::Connection, TeleportError> {
    rusqlite::Connection::open(&c.db).map_err(|e| perr(format!("opening {}: {e}", c.db.display())))
}

// ---- cookies ----------------------------------------------------------------

/// The partition key of a Firefox `originAttributes`
/// (`^partitionKey=%28https%2Cexample.com%29`), as the top-level site Chromium
/// calls `top_frame_site_key` (`https://example.com`).
pub fn partition_of(origin_attributes: &str) -> Option<String> {
    let enc = origin_attributes
        .trim_start_matches('^')
        .split('&')
        .find_map(|p| p.strip_prefix("partitionKey="))?;
    let dec = percent_decode(enc);
    let inner = dec.strip_prefix('(')?.strip_suffix(')')?;
    let mut parts = inner.split(',');
    let scheme = parts.next()?;
    let host = parts.next()?;
    Some(format!("{scheme}://{host}"))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Every cookie of `profile` as the bundle's cookie rows: `expiry` (ms) and the
/// microsecond times become Chromium's microseconds since 1601; a partitioned
/// cookie keeps its partition.
pub fn read_cookies(profile: &Path) -> Result<Vec<CookieItem>, TeleportError> {
    let src = profile.join("cookies.sqlite");
    if !src.is_file() {
        return Ok(Vec::new());
    }
    let copy = private_copy(&src)?;
    let conn = open(&copy)?;
    let have: std::collections::HashSet<String> = {
        let mut st = conn
            .prepare("PRAGMA table_info(moz_cookies)")
            .map_err(perr)?;
        st.query_map([], |r| r.get::<_, String>(1))
            .map_err(perr)?
            .filter_map(Result::ok)
            .collect()
    };
    let col = |c: &str, fallback: &str| {
        if have.contains(c) {
            c.to_string()
        } else {
            fallback.to_string()
        }
    };
    let sql = format!(
        "SELECT name, value, host, path, expiry, {}, {}, isSecure, isHttpOnly, {}, {} FROM moz_cookies",
        col("lastAccessed", "0"),
        col("creationTime", "0"),
        col("sameSite", "0"),
        col("originAttributes", "''"),
    );
    let mut st = conn.prepare(&sql).map_err(perr)?;
    let rows = st
        .query_map([], |r| {
            let expiry_ms: i64 = r.get(4)?;
            let last: i64 = r.get(5)?;
            let created: i64 = r.get(6)?;
            let attrs: String = r.get::<_, Option<String>>(10)?.unwrap_or_default();
            let micros = |us: i64| (us > 0).then(|| us + CHROME_EPOCH_OFFSET_MICROS);
            Ok(CookieItem {
                host_key: r.get(2)?,
                name: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                value: r
                    .get::<_, Option<String>>(1)?
                    .unwrap_or_default()
                    .into_bytes(),
                path: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                // 0 is a session cookie; Firefox's expiry is milliseconds.
                expires_utc: if expiry_ms > 0 {
                    expiry_ms * 1000 + CHROME_EPOCH_OFFSET_MICROS
                } else {
                    0
                },
                is_secure: r.get::<_, i64>(7)? != 0,
                is_httponly: r.get::<_, i64>(8)? != 0,
                samesite: r.get::<_, i64>(9)?,
                extra: CookieExtra {
                    creation_utc: micros(created),
                    last_access_utc: micros(last),
                    partition_key: partition_of(&attrs),
                    ..Default::default()
                },
            })
        })
        .map_err(perr)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(perr)
}

// ---- localStorage -----------------------------------------------------------

/// The origin of a `storage/default/<dir>` name: Firefox escapes `:` and `/`
/// as `+` (`https+++example.com` is `https://example.com`) and keeps origin
/// attributes after `^`.
pub fn origin_of_dir(dir_name: &str) -> String {
    let base = dir_name.split('^').next().unwrap_or(dir_name);
    // `scheme+++host[+port]`: the first `+++` is `://`; a later `+` is `:`.
    match base.split_once("+++") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.replace('+', ":")),
        None => base.to_string(),
    }
}

fn decode_value(conversion: i64, compression: i64, blob: &[u8]) -> Result<String, TeleportError> {
    let bytes = if compression == 1 {
        snap::raw::Decoder::new()
            .decompress_vec(blob)
            .map_err(|e| perr(format!("a localStorage value is not valid Snappy: {e}")))?
    } else {
        blob.to_vec()
    };
    if conversion == 1 {
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    } else {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }
}

/// Every localStorage value under `storage/default/*/ls/data.sqlite`.
pub fn read_local_storage(profile: &Path) -> Result<Vec<LocalStorageItem>, TeleportError> {
    let root = profile.join("storage").join("default");
    let Ok(dirs) = std::fs::read_dir(&root) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for d in dirs.flatten() {
        let db = d.path().join("ls").join("data.sqlite");
        if !db.is_file() {
            continue;
        }
        let name = d.file_name().to_string_lossy().into_owned();
        let copy = private_copy(&db)?;
        let conn = open(&copy)?;
        // The exact origin is in the database; the directory name is the
        // escaped form of it.
        let origin: String = conn
            .query_row("SELECT origin FROM database LIMIT 1", [], |r| r.get(0))
            .unwrap_or_else(|_| origin_of_dir(&name));
        let mut st = conn
            .prepare("SELECT key, conversion_type, compression_type, value FROM data")
            .map_err(perr)?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                ))
            })
            .map_err(perr)?;
        for row in rows {
            let (key, conv, comp, blob) = row.map_err(perr)?;
            out.push(LocalStorageItem {
                origin: origin.clone(),
                key,
                value: decode_value(conv, comp, &blob)?,
                key_raw: None,
                value_raw: None,
            });
        }
    }
    Ok(out)
}

// ---- saved logins -----------------------------------------------------------

/// What a read of the saved logins produced.
#[derive(Debug, Default)]
pub struct FirefoxLogins {
    /// Decrypted logins.
    pub logins: Vec<LoginItem>,
    /// Logins that could not be decrypted, never with a value.
    pub unreadable: Vec<String>,
}

/// The origin of every saved login (the plaintext `hostname`), so a review can
/// count passwords per site without decrypting anything.
pub fn login_hosts(profile: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(profile.join("logins.json")) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    doc.get("logins")
        .and_then(|l| l.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    e.get("hostname")
                        .and_then(|h| h.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The master key from `key4.db` (empty master password), or why not.
pub fn login_master_key(profile: &Path) -> Result<zeroize::Zeroizing<Vec<u8>>, TeleportError> {
    let src = profile.join("key4.db");
    if !src.is_file() {
        return Err(perr(
            "this profile has no key4.db, so its logins cannot be decrypted",
        ));
    }
    let copy = private_copy(&src)?;
    let conn = open(&copy)?;
    let (salt, item2): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT item1, item2 FROM metaData WHERE id = 'password'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| perr(format!("Firefox key4.db has no password entry: {e}")))?;
    let a11: Vec<u8> = conn
        .query_row(
            "SELECT a11 FROM nssPrivate WHERE a11 IS NOT NULL LIMIT 1",
            [],
            |r| r.get(0),
        )
        .map_err(|e| perr(format!("Firefox key4.db has no login key: {e}")))?;
    cua_teleport_bundle::firefox_nss::master_key(&salt, &item2, &a11, b"")
}

/// The saved logins of `profile`, decrypted with its (empty-password) master
/// key. A profile with a master password is refused with a message, never
/// guessed at. Call this only when the user ticked the passwords.
pub fn read_logins(profile: &Path) -> Result<FirefoxLogins, TeleportError> {
    let src = profile.join("logins.json");
    let Ok(text) = std::fs::read_to_string(&src) else {
        return Ok(FirefoxLogins::default());
    };
    let doc: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| perr(format!("logins.json: {e}")))?;
    let Some(entries) = doc.get("logins").and_then(|l| l.as_array()) else {
        return Ok(FirefoxLogins::default());
    };
    let key = login_master_key(profile)?;
    let mut out = FirefoxLogins::default();
    for e in entries {
        let host = e.get("hostname").and_then(|v| v.as_str()).unwrap_or("");
        // A form login (`formSubmitURL` set) is a web login; an HTTP-auth one
        // has a realm. Both are origin + username + password.
        if host.is_empty() {
            continue;
        }
        let field = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let user = cua_teleport_bundle::firefox_nss::decrypt_sdr(&key, field("encryptedUsername"));
        let pass = cua_teleport_bundle::firefox_nss::decrypt_sdr(&key, field("encryptedPassword"));
        match (user, pass) {
            (Ok(u), Ok(p)) => out.logins.push(LoginItem {
                origin: host.to_string(),
                username: String::from_utf8_lossy(&u).into_owned(),
                password: p.to_vec(),
                signon_realm: signon_realm(host),
            }),
            _ => out.unreadable.push(host.to_string()),
        }
    }
    Ok(out)
}

// ---- fixtures ---------------------------------------------------------------

/// A Firefox-shaped profile for tests: `cookies.sqlite` (schema 16),
/// localStorage data files and a `key4.db` + `logins.json` pair, built from the
/// formats' sources (no real profile is read).
pub mod testing {
    use super::*;
    use cua_teleport_bundle::firefox_nss::testing as nss;

    /// The 3DES master key the fixture's `key4.db` wraps.
    pub const MASTER: [u8; 24] = *b"0123456789abcdef01234567";

    /// `cookies.sqlite` at schema 16 with `rows`
    /// `(host, name, value, path, expiry_ms, originAttributes)`.
    pub fn write_cookies(profile: &Path, rows: &[(&str, &str, &str, &str, i64, &str)]) {
        std::fs::create_dir_all(profile).unwrap();
        let conn = rusqlite::Connection::open(profile.join("cookies.sqlite")).unwrap();
        conn.execute_batch(
            "PRAGMA user_version = 16;
             CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, originAttributes TEXT NOT NULL DEFAULT '',
              name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER,
              creationTime INTEGER, isSecure INTEGER, isHttpOnly INTEGER, inBrowserElement INTEGER DEFAULT 0,
              sameSite INTEGER DEFAULT 0, schemeMap INTEGER DEFAULT 0, isPartitionedAttributeSet INTEGER DEFAULT 0,
              CONSTRAINT moz_uniqueid UNIQUE (name, host, path, originAttributes));",
        )
        .unwrap();
        for (host, name, value, path, expiry, attrs) in rows {
            conn.execute(
                "INSERT INTO moz_cookies (originAttributes, name, value, host, path, expiry, lastAccessed,
                 creationTime, isSecure, isHttpOnly, sameSite, schemeMap, isPartitionedAttributeSet)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1700000000000000, 1690000000000000, 1, 1, 1, 2, ?7)",
                rusqlite::params![attrs, name, value, host, path, expiry, (!attrs.is_empty()) as i64],
            )
            .unwrap();
        }
    }

    /// One origin's `ls/data.sqlite`: values as `(key, value, compress)`.
    pub fn write_local_storage(
        profile: &Path,
        dir_name: &str,
        origin: &str,
        rows: &[(&str, &str, bool)],
    ) {
        let dir = profile.join("storage/default").join(dir_name).join("ls");
        std::fs::create_dir_all(&dir).unwrap();
        let conn = rusqlite::Connection::open(dir.join("data.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE database (origin TEXT NOT NULL, usage INTEGER NOT NULL DEFAULT 0,
              last_vacuum_time INTEGER NOT NULL DEFAULT 0, last_analyze_time INTEGER NOT NULL DEFAULT 0,
              last_vacuum_size INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE data (key TEXT PRIMARY KEY, utf16_length INTEGER NOT NULL,
              conversion_type INTEGER NOT NULL, compression_type INTEGER NOT NULL,
              last_access_time INTEGER NOT NULL DEFAULT 0, value BLOB NOT NULL);",
        )
        .unwrap();
        conn.execute("INSERT INTO database (origin) VALUES (?1)", [origin])
            .unwrap();
        for (key, value, compress) in rows {
            let utf8 = value.as_bytes();
            let (comp, blob) = if *compress && utf8.len() > 16 {
                (1, snap::raw::Encoder::new().compress_vec(utf8).unwrap())
            } else {
                (0, utf8.to_vec())
            };
            conn.execute(
                "INSERT INTO data (key, utf16_length, conversion_type, compression_type, value)
                 VALUES (?1, ?2, 1, ?3, ?4)",
                rusqlite::params![key, value.encode_utf16().count() as i64, comp, blob],
            )
            .unwrap();
        }
    }

    /// `key4.db` (legacy or PBES2) and a `logins.json` of `(origin, user, password)`.
    pub fn write_logins(
        profile: &Path,
        logins: &[(&str, &str, &str)],
        pbes2: bool,
        master_password: &[u8],
    ) {
        std::fs::create_dir_all(profile).unwrap();
        let (salt, item2, a11) = nss::key4_blobs(&MASTER, master_password, pbes2);
        let key_id = vec![0xf8u8; 16];
        let conn = rusqlite::Connection::open(profile.join("key4.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE metaData (id PRIMARY KEY, item1, item2);
             CREATE TABLE nssPrivate (id INTEGER PRIMARY KEY, a11, a102);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO metaData VALUES ('password', ?1, ?2)",
            rusqlite::params![salt, item2],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nssPrivate (a11, a102) VALUES (?1, ?2)",
            rusqlite::params![a11, key_id],
        )
        .unwrap();
        let entries: Vec<serde_json::Value> = logins
            .iter()
            .enumerate()
            .map(|(i, (o, u, p))| {
                serde_json::json!({
                    "id": i + 1, "hostname": o, "httpRealm": null, "formSubmitURL": o,
                    "usernameField": "user", "passwordField": "pass",
                    "encryptedUsername": nss::sealed_field(&MASTER, &key_id, u.as_bytes()),
                    "encryptedPassword": nss::sealed_field(&MASTER, &key_id, p.as_bytes()),
                    "encType": 1, "timesUsed": 1
                })
            })
            .collect();
        std::fs::write(
            profile.join("logins.json"),
            serde_json::to_vec(
                &serde_json::json!({"nextId": logins.len() + 1, "logins": entries, "version": 3}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn profiles_follow_installs_ini_then_the_default_flag_then_the_only_one() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("profiles.ini"),
            "[Profile0]\nName=default\nIsRelative=1\nPath=Profiles/abc.default\n\n\
             [Profile1]\nName=default-release\nIsRelative=1\nPath=Profiles/xyz.default-release\nDefault=1\n\n\
             [General]\nStartWithLastProfile=1\n",
        )
        .unwrap();
        assert_eq!(profiles(d.path()).len(), 2);
        assert_eq!(default_profile(d.path()).unwrap().name, "default-release");
        std::fs::write(
            d.path().join("installs.ini"),
            "[4F96D1932A9F858E]\nDefault=Profiles/abc.default\nLocked=1\n",
        )
        .unwrap();
        assert_eq!(
            default_profile(d.path()).unwrap().name,
            "default",
            "the install's choice wins"
        );
    }

    #[test]
    fn cookies_convert_units_session_and_partition() {
        let p = tempfile::tempdir().unwrap();
        write_cookies(
            p.path(),
            &[
                (
                    ".github.com",
                    "user_session",
                    "abc",
                    "/",
                    1_900_000_000_000,
                    "",
                ),
                (
                    "example.com",
                    "sid",
                    "p",
                    "/",
                    0,
                    "^partitionKey=%28https%2Ctop.example%29",
                ),
            ],
        );
        let mut c = read_cookies(p.path()).unwrap();
        c.sort_by(|a, b| a.name.cmp(&b.name));
        let sid = &c[0];
        assert_eq!(sid.expires_utc, 0, "a session cookie stays one");
        assert_eq!(
            sid.extra.partition_key.as_deref(),
            Some("https://top.example")
        );
        let gh = &c[1];
        assert_eq!(gh.value, b"abc");
        assert_eq!(
            gh.expires_utc,
            1_900_000_000_000 * 1000 + CHROME_EPOCH_OFFSET_MICROS
        );
        assert_eq!(
            gh.extra.creation_utc,
            Some(1_690_000_000_000_000 + CHROME_EPOCH_OFFSET_MICROS)
        );
        assert!(gh.is_secure && gh.is_httponly && gh.samesite == 1);
        assert_eq!(partition_of("^userContextId=1"), None);
    }

    #[test]
    fn local_storage_decodes_snappy_and_names_the_origin() {
        let p = tempfile::tempdir().unwrap();
        let long = "x".repeat(200);
        write_local_storage(
            p.path(),
            "https+++app.example+8443",
            "https://app.example:8443",
            &[
                ("token", "short", false),
                ("blob", &long, true),
                ("emoji", "caf\u{e9} \u{1F642}", false),
            ],
        );
        let mut v = read_local_storage(p.path()).unwrap();
        v.sort_by(|a, b| a.key.cmp(&b.key));
        assert!(v.iter().all(|i| i.origin == "https://app.example:8443"));
        assert_eq!(v[0].value, long, "Snappy values come back whole");
        assert_eq!(v[1].value, "caf\u{e9} \u{1F642}");
        assert_eq!(
            origin_of_dir("https+++example.com^userContextId=1"),
            "https://example.com"
        );
        assert_eq!(
            origin_of_dir("https+++a.example+8443"),
            "https://a.example:8443"
        );
    }

    #[test]
    fn logins_decrypt_in_both_schemes_and_a_master_password_is_refused() {
        for pbes2 in [false, true] {
            let p = tempfile::tempdir().unwrap();
            write_logins(
                p.path(),
                &[
                    ("https://github.com", "octo", "gh-pw-1"),
                    ("https://example.org", "ada", "pw2"),
                ],
                pbes2,
                b"",
            );
            let r = read_logins(p.path()).unwrap();
            assert_eq!(r.logins.len(), 2, "pbes2={pbes2}");
            assert_eq!(r.logins[0].username, "octo");
            assert_eq!(r.logins[0].password, b"gh-pw-1");
            assert_eq!(r.logins[0].signon_realm, "https://github.com/");
            let locked = tempfile::tempdir().unwrap();
            write_logins(
                locked.path(),
                &[("https://a.test", "u", "p")],
                pbes2,
                b"master",
            );
            let err = read_logins(locked.path()).unwrap_err().to_string();
            assert!(err.contains("master password"), "{err}");
        }
        // No logins.json: nothing, not an error.
        let empty = tempfile::tempdir().unwrap();
        assert!(read_logins(empty.path()).unwrap().logins.is_empty());
    }
}
