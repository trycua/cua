// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A site's icon, read from the source browser's own local `Favicons`
//! database. Never from the network: a favicon service would learn which
//! sites the user has saved.
//!
//! Chromium keeps them in `<profile>/Favicons` (SQLite): `icon_mapping`
//! links a page URL to an icon, `favicon_bitmaps` holds the PNGs. The
//! browser holds the file open (and may have recent writes in `-wal`), so a
//! private copy of the file and its sidecars is read, never the live file.
//! Icons are not secret, but they are small and bounded.

use std::path::{Path, PathBuf};

use crate::TeleportError;

/// Largest icon kept (bytes).
pub const MAX_ICON_BYTES: usize = 16 * 1024;

/// One site's icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favicon {
    /// The registrable site (`github.com`).
    pub site: String,
    /// PNG bytes.
    pub png: Vec<u8>,
}

/// The `Favicons` database of a Chromium profile directory.
pub fn favicons_db(profile_dir: &Path) -> PathBuf {
    profile_dir.join("Favicons")
}

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn private_copy(src: &Path) -> Result<(TempDir, PathBuf), TeleportError> {
    let dir = std::env::temp_dir().join(format!("cua-favicons-{:016x}", rand::random::<u64>()));
    std::fs::create_dir(&dir)
        .map_err(|e| TeleportError::Provider(format!("temp dir for Favicons: {e}")))?;
    let guard = TempDir(dir.clone());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let dst = dir.join("Favicons");
    std::fs::copy(src, &dst)
        .map_err(|e| TeleportError::Provider(format!("copying Favicons failed: {e}")))?;
    // Recent commits live in the sidecars of a WAL or journal database.
    for ext in ["-wal", "-shm", "-journal"] {
        let mut from = src.as_os_str().to_owned();
        from.push(ext);
        let mut to = dst.as_os_str().to_owned();
        to.push(ext);
        let _ = std::fs::copy(PathBuf::from(from), PathBuf::from(to));
    }
    Ok((guard, dst))
}

fn is_png(b: &[u8]) -> bool {
    b.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}

/// The icon of each of `sites` the database holds: the PNG nearest 32 px
/// (newest on a tie), no larger than [`MAX_ICON_BYTES`]. A site without an
/// icon is left out; a database that is missing or unreadable is an error.
pub fn read_favicons(db: &Path, sites: &[String]) -> Result<Vec<Favicon>, TeleportError> {
    if !db.is_file() {
        return Err(TeleportError::Provider(format!(
            "no Favicons database at {}",
            db.display()
        )));
    }
    let (_guard, copy) = private_copy(db)?;
    let run = || -> rusqlite::Result<Vec<Favicon>> {
        let conn = rusqlite::Connection::open_with_flags(
            &copy,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut stmt = conn.prepare(
            "SELECT b.image_data FROM icon_mapping m \
             JOIN favicon_bitmaps b ON b.icon_id = m.icon_id \
             WHERE m.page_url LIKE ?1 ESCAPE '\\' OR m.page_url LIKE ?2 ESCAPE '\\' \
             ORDER BY ABS(b.width - 32) ASC, b.last_updated DESC",
        )?;
        let mut out = Vec::new();
        for site in sites {
            let esc = site
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let exact = format!("%://{esc}/%");
            let sub = format!("%.{esc}/%");
            let rows = stmt.query_map([exact, sub], |r| r.get::<_, Option<Vec<u8>>>(0))?;
            for row in rows {
                if let Some(png) = row?
                    && is_png(&png)
                    && png.len() <= MAX_ICON_BYTES
                {
                    out.push(Favicon {
                        site: site.clone(),
                        png,
                    });
                    break;
                }
            }
        }
        Ok(out)
    };
    run().map_err(|e| TeleportError::Provider(format!("reading Favicons failed: {e}")))
}

/// A Chromium-shaped `Favicons` database (tests and fixtures): one row per
/// `(page_url, width, bytes)`.
pub fn write_favicons_db_for_tests(
    db: &Path,
    rows: &[(&str, u32, &[u8])],
) -> Result<(), TeleportError> {
    if let Some(p) = db.parent() {
        std::fs::create_dir_all(p).map_err(|e| TeleportError::Provider(e.to_string()))?;
    }
    let run = || -> rusqlite::Result<()> {
        let conn = rusqlite::Connection::open(db)?;
        conn.execute_batch(
            "CREATE TABLE icon_mapping (id INTEGER PRIMARY KEY, page_url LONGVARCHAR NOT NULL, \
             icon_id INTEGER); \
             CREATE TABLE favicon_bitmaps (id INTEGER PRIMARY KEY, icon_id INTEGER NOT NULL, \
             last_updated INTEGER DEFAULT 0, image_data BLOB, width INTEGER DEFAULT 0, \
             height INTEGER DEFAULT 0);",
        )?;
        for (n, (url, width, bytes)) in rows.iter().enumerate() {
            let id = n as i64 + 1;
            conn.execute(
                "INSERT INTO icon_mapping (page_url, icon_id) VALUES (?1, ?2)",
                rusqlite::params![url, id],
            )?;
            conn.execute(
                "INSERT INTO favicon_bitmaps (icon_id, last_updated, image_data, width, height) \
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                rusqlite::params![id, id, bytes, width],
            )?;
        }
        Ok(())
    };
    run().map_err(|e| TeleportError::Provider(format!("writing Favicons: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(tag: u8) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        v.extend([tag; 8]);
        v
    }

    #[test]
    fn reads_the_nearest_size_per_site_from_a_private_copy() {
        let d = tempfile::tempdir().unwrap();
        let db = favicons_db(d.path());
        write_favicons_db_for_tests(
            &db,
            &[
                ("https://github.com/login", 16, &png(1)),
                ("https://github.com/login", 32, &png(2)),
                ("https://gist.github.com/x", 64, &png(3)),
                ("https://notion.so/", 32, &png(4)),
                ("https://evilgithub.com/", 32, &png(9)),
                ("https://nopng.test/", 32, b"not a png"),
            ],
        )
        .unwrap();
        let before = std::fs::metadata(&db).unwrap().modified().unwrap();
        let out = read_favicons(
            &db,
            &[
                "github.com".into(),
                "notion.so".into(),
                "nopng.test".into(),
                "absent.test".into(),
            ],
        )
        .unwrap();
        assert_eq!(out.len(), 2, "no icon for a site without a PNG or a row");
        assert_eq!(out[0].site, "github.com");
        assert_eq!(out[0].png, png(2), "32 px beats 16 and 64");
        assert_eq!(out[1].png, png(4));
        // A look-alike never matches, and the live file is untouched.
        assert!(!out.iter().any(|f| f.png == png(9)));
        assert_eq!(std::fs::metadata(&db).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn oversized_icons_and_missing_databases_are_handled() {
        let d = tempfile::tempdir().unwrap();
        let db = favicons_db(d.path());
        let mut big = png(1);
        big.extend(vec![0; MAX_ICON_BYTES]);
        write_favicons_db_for_tests(&db, &[("https://big.test/", 32, &big)]).unwrap();
        assert!(read_favicons(&db, &["big.test".into()]).unwrap().is_empty());
        assert!(read_favicons(&d.path().join("none"), &["a.test".into()]).is_err());
    }
}
