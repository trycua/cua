//! The daemon's share records: a 0600 JSON file, rewritten atomically, so
//! shares outlive the CLI process that made them and the daemon's restarts.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::model::{Access, Share};
use crate::{Error, Result};

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    shares: Vec<Share>,
}

/// All shares, in memory and on disk.
pub struct ShareStore {
    path: PathBuf,
    shares: Mutex<Vec<Share>>,
}

/// Constant-time equality of two byte strings of any length.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() ^ b.len()) as u8;
    for i in 0..a.len().max(b.len()) {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0
}

impl ShareStore {
    /// Opens (creating the directory if needed) the store at `path`. A
    /// missing file is an empty store; an unreadable one is an error rather
    /// than silently dropping every share.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let shares = match std::fs::read(&path) {
            Ok(raw) => {
                serde_json::from_slice::<File>(&raw)
                    .map_err(|e| Error::Store(format!("{}: {e}", path.display())))?
                    .shares
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(Error::Store(format!("{}: {e}", path.display()))),
        };
        if let Some(dir) = path.parent() {
            create_private_dir(dir)?;
        }
        Ok(Self {
            path,
            shares: Mutex::new(shares),
        })
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn save(&self, shares: &[Share]) -> Result<()> {
        let raw = serde_json::to_vec_pretty(&File {
            shares: shares.to_vec(),
        })
        .map_err(|e| Error::Store(e.to_string()))?;
        write_private(&self.path, &raw)
    }

    /// Adds a share.
    pub fn add(&self, share: Share) -> Result<()> {
        let mut g = self.shares.lock().expect("shares");
        g.push(share);
        if let Err(e) = self.save(&g) {
            g.pop();
            return Err(e);
        }
        Ok(())
    }

    /// Every share.
    pub fn list(&self) -> Vec<Share> {
        self.shares.lock().expect("shares").clone()
    }

    /// The private or team share whose key is `key`.
    pub fn by_key(&self, key: &str) -> Option<Share> {
        self.shares
            .lock()
            .expect("shares")
            .iter()
            .find(|s| !s.is_public() && ct_eq(s.key.as_bytes(), key.as_bytes()))
            .cloned()
    }

    /// The public share whose token is `token` (every share is compared, in
    /// constant time per share, so a near miss costs the same as a miss).
    pub fn by_token(&self, token: &str) -> Option<Share> {
        let g = self.shares.lock().expect("shares");
        let mut found = None;
        for s in g.iter().filter(|s| s.access == Access::Public) {
            if ct_eq(s.key.as_bytes(), token.as_bytes()) {
                found = Some(s.clone());
            }
        }
        found
    }

    /// The share with this handle.
    pub fn by_handle(&self, handle: &str) -> Option<Share> {
        self.shares
            .lock()
            .expect("shares")
            .iter()
            .find(|s| s.handle == handle)
            .cloned()
    }

    /// Removes the shares `pred` selects, returning them.
    pub fn remove_where(&self, pred: impl Fn(&Share) -> bool) -> Result<Vec<Share>> {
        let mut g = self.shares.lock().expect("shares");
        let (gone, keep): (Vec<_>, Vec<_>) = g.iter().cloned().partition(|s| pred(s));
        if gone.is_empty() {
            return Ok(gone);
        }
        self.save(&keep)?;
        *g = keep;
        Ok(gone)
    }

    /// Removes shares that ended at `now_ms`.
    pub fn purge_expired(&self, now_ms: u64) -> Result<Vec<Share>> {
        self.remove_where(|s| s.expired(now_ms))
    }
}

fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes `data` to `path` through a 0600 temp file and a rename.
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension("tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Owner, Target};

    fn share(handle: &str, key: &str, access: Access) -> Share {
        Share {
            handle: handle.into(),
            key: key.into(),
            name: None,
            target: Target::Port { port: 3000 },
            access,
            owner: Owner {
                account: "a".into(),
                email: None,
            },
            teams: vec![],
            created_ms: 1,
            expires_ms: None,
            url: "https://x".into(),
        }
    }

    #[test]
    fn persists_and_looks_up_by_kind() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("share").join("shares.json");
        let s = ShareStore::open(&p).unwrap();
        s.add(share("share-1", "aaaa", Access::Private)).unwrap();
        s.add(share("share-2", "bbbb", Access::Public)).unwrap();
        // A private key is never found as a public token and the reverse.
        assert!(s.by_key("aaaa").is_some() && s.by_token("aaaa").is_none());
        assert!(s.by_token("bbbb").is_some() && s.by_key("bbbb").is_none());
        let again = ShareStore::open(&p).unwrap();
        assert_eq!(again.list().len(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let gone = again.remove_where(|s| s.handle == "share-1").unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(ShareStore::open(&p).unwrap().list().len(), 1);
    }

    #[test]
    fn expiry_purges() {
        let dir = tempfile::tempdir().unwrap();
        let s = ShareStore::open(dir.path().join("s.json")).unwrap();
        let mut a = share("share-1", "aaaa", Access::Public);
        a.expires_ms = Some(100);
        s.add(a).unwrap();
        s.add(share("share-2", "bbbb", Access::Private)).unwrap();
        assert!(s.purge_expired(99).unwrap().is_empty());
        assert_eq!(s.purge_expired(100).unwrap().len(), 1);
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn corrupt_store_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        std::fs::write(&p, "{not json").unwrap();
        assert!(ShareStore::open(&p).is_err());
    }

    #[test]
    fn constant_time_eq_handles_lengths() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"", b"a"));
    }
}
