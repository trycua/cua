//! The persisted Spaces registry.
//!
//! Two files under the cua home (`$CUA_HOME`, else `~/.cua`):
//!
//! - `spaces.json`: a JSON array of `cua.daemon.v1.Space` in canonical
//!   proto3 JSON, the exact shape `cua-daemon`'s `SpaceService` serves. No
//!   secrets ever go here, so the file can be read, synced or pasted into a
//!   bug report.
//! - `spaces-credentials.json`: `{ "<space id>": { "url", "token" } }`,
//!   created with mode 0600 (and re-chmodded on every write). The spacesd
//!   token lives only here.
//! - `space-meta.json`: what a Space was created with that no handshake
//!   reports ([`SpaceMeta`]: its labels, when it was created and its egress policy), kept for relay-listed Spaces too
//!   and dropped with the Space ([`Registry::remove`]).
//! - `direct-hosts.json`: the machines added with `cua spaces add <addr>
//!   --host` ([`DirectHost`]), which provide Spaces over their direct
//!   listener without the relay. No secrets: each host's token is its
//!   Space's credential.
//!
//! Writes are atomic (temp file + rename) so a crash never leaves a torn
//! registry, and serialized within the process by a mutex.

use crate::error::{Error, Result};
use cua_proto::daemon::v1 as dpb;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// File name of the registry.
pub const SPACES_FILE: &str = "spaces.json";
/// File name of the 0600 credential store.
pub const CREDENTIALS_FILE: &str = "spaces-credentials.json";
/// Advisory lock file serializing writers across processes.
pub const LOCK_FILE: &str = "spaces.lock";
/// File name of the direct-host list.
pub const DIRECT_HOSTS_FILE: &str = "direct-hosts.json";
/// File name of the per-Space metadata (labels, creation time, egress policy).
pub const META_FILE: &str = "space-meta.json";

/// Most labels on one Space.
pub const MAX_LABELS: usize = 64;
/// Longest label key, in bytes.
pub const MAX_LABEL_KEY: usize = 63;
/// Longest label value, in bytes.
pub const MAX_LABEL_VALUE: usize = 256;

/// What cua keeps about a Space that no handshake reports.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceMeta {
    /// Labels set at create ([`validate_labels`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// When the Space was created (Unix seconds; 0 when unknown).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub created_at_unix: u64,
    /// The egress policy last set on the Space ([`crate::egress`]).
    #[serde(default, skip_serializing_if = "crate::egress::EgressPolicy::is_empty")]
    pub egress: crate::egress::EgressPolicy,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Checks a Space's labels: at most [`MAX_LABELS`], each key and value
/// non-empty, keys up to [`MAX_LABEL_KEY`] bytes and values up to
/// [`MAX_LABEL_VALUE`] bytes (UTF-8).
pub fn validate_labels<'a>(
    labels: impl IntoIterator<Item = (&'a String, &'a String)>,
) -> Result<()> {
    let mut count = 0;
    for (k, v) in labels {
        count += 1;
        if k.is_empty() || v.is_empty() {
            return Err(Error::invalid(format!(
                "label {k:?}={v:?}: keys and values are not empty"
            )));
        }
        if k.len() > MAX_LABEL_KEY {
            return Err(Error::invalid(format!(
                "label key {k:?} is {} bytes: at most {MAX_LABEL_KEY}",
                k.len()
            )));
        }
        if v.len() > MAX_LABEL_VALUE {
            return Err(Error::invalid(format!(
                "label {k:?} has a value of {} bytes: at most {MAX_LABEL_VALUE}",
                v.len()
            )));
        }
    }
    if count > MAX_LABELS {
        return Err(Error::invalid(format!(
            "{count} labels: at most {MAX_LABELS} per Space"
        )));
    }
    Ok(())
}

/// Parses `key=value` (split at the first `=`) as the CLI and MCP write a
/// label.
pub fn parse_label(text: &str) -> Result<(String, String)> {
    match text.split_once('=') {
        Some((k, v)) if !k.is_empty() && !v.is_empty() => Ok((k.into(), v.into())),
        _ => Err(Error::invalid(format!(
            "label {text:?}: write it as key=value (neither empty)"
        ))),
    }
}

/// A machine that provides Spaces over its direct listener (Tailscale or
/// LAN), without the relay: added with `cua spaces add <addr> --host`.
/// `on="host:<name>"` finds it when the relay does not know the name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectHost {
    /// The name it is found by (its display name).
    pub name: String,
    /// Its own Space (`direct:<addr>:<port>`), whose credential holds the
    /// host's env token.
    pub space: String,
    /// The Spaces it created for this device: their id here
    /// (`direct:<addr>:<forwarded port>`) to their id on the host
    /// (`local:<name>`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spaces: BTreeMap<String, String>,
}
/// The `spaces.json` version key written before the cua-spacesd rename.
const LEGACY_VERSION_KEY: &str = "envDriverVersion";

/// The cua home: `$CUA_HOME`, else `~/.cua` (same rule as `cua-daemon`).
pub fn cua_home() -> PathBuf {
    cua_home::cua_home()
}

/// How to reach a Space again: its URL and spacesd token.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// The URL it was added with (direct Spaces), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The spacesd token (or, for a Space added by an MCP URL, the
    /// bearer that URL needs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Services reached by URL (a Space added by a plain MCP endpoint):
    /// name → endpoint URL.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub service_urls: BTreeMap<String, String>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("url", &self.url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("service_urls", &self.service_urls)
            .finish()
    }
}

/// The registry on disk.
#[derive(Debug)]
pub struct Registry {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl Registry {
    /// A registry in `dir` (created on first write, mode 0700 when new).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            lock: Mutex::new(()),
        }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of `spaces.json`.
    pub fn spaces_path(&self) -> PathBuf {
        self.dir.join(SPACES_FILE)
    }

    /// Path of the credential store.
    pub fn credentials_path(&self) -> PathBuf {
        self.dir.join(CREDENTIALS_FILE)
    }

    /// Every registered Space, in insertion order.
    pub fn list(&self) -> Result<Vec<dpb::Space>> {
        let _g = self.lock.lock().expect("registry lock");
        self.read_spaces()
    }

    /// One Space by id (any accepted spelling).
    pub fn get(&self, id: &str) -> Result<Option<dpb::Space>> {
        let id = crate::id::canonical_id(id);
        Ok(self.list()?.into_iter().find(|s| s.id == id))
    }

    /// The stored credential for `id` (any accepted spelling).
    pub fn credential(&self, id: &str) -> Result<Option<Credential>> {
        let _g = self.lock.lock().expect("registry lock");
        Ok(self
            .read_credentials()?
            .remove(&crate::id::canonical_id(id)))
    }

    /// Path of `space-meta.json`.
    pub fn meta_path(&self) -> PathBuf {
        self.dir.join(META_FILE)
    }

    /// What is kept about every Space ([`SpaceMeta`]), by canonical id.
    pub fn all_meta(&self) -> BTreeMap<String, SpaceMeta> {
        let _g = self.lock.lock().expect("registry lock");
        self.read_meta()
    }

    /// What is kept about one Space (any accepted spelling of its id).
    pub fn meta(&self, id: &str) -> Option<SpaceMeta> {
        self.all_meta().remove(&crate::id::canonical_id(id))
    }

    /// Changes what is kept about a Space (atomic; an empty record is
    /// dropped). Works for relay-listed Spaces, which have no registry
    /// entry.
    pub fn update_meta(&self, id: &str, change: impl FnOnce(&mut SpaceMeta)) -> Result<()> {
        let id = crate::id::canonical_id(id);
        let _g = self.lock.lock().expect("registry lock");
        let _file = self.exclusive()?;
        let mut all = self.read_meta();
        let mut m = all.remove(&id).unwrap_or_default();
        change(&mut m);
        if m != SpaceMeta::default() {
            all.insert(id, m);
        }
        self.write_meta(&all)
    }

    /// Gives Space `to` the labels and egress policy of Space `from` (a
    /// fork starts as its source was; its creation time and lifetime are
    /// its own). Returns whether `from` had any.
    pub fn inherit_meta(&self, from: &str, to: &str) -> Result<bool> {
        let (from, to) = (crate::id::canonical_id(from), crate::id::canonical_id(to));
        let _g = self.lock.lock().expect("registry lock");
        let _file = self.exclusive()?;
        let mut all = self.read_meta();
        let Some(source) = all.get(&from).cloned() else {
            return Ok(false);
        };
        let kept = all.entry(to).or_default();
        kept.labels = source.labels;
        kept.egress = source.egress;
        self.write_meta(&all)?;
        Ok(true)
    }

    fn read_meta(&self) -> BTreeMap<String, SpaceMeta> {
        std::fs::read(self.meta_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write_meta(&self, all: &BTreeMap<String, SpaceMeta>) -> Result<()> {
        if all.is_empty() && !self.meta_path().exists() {
            return Ok(());
        }
        let mut bytes = serde_json::to_vec_pretty(all)?;
        bytes.push(b'\n');
        self.atomic_write(&self.meta_path(), &bytes, 0o644)
    }

    /// Path of the cross-process lock file.
    pub fn lock_path(&self) -> PathBuf {
        self.dir.join(LOCK_FILE)
    }

    /// Path of the direct-host list.
    pub fn direct_hosts_path(&self) -> PathBuf {
        self.dir.join(DIRECT_HOSTS_FILE)
    }

    /// The machines added as direct hosts, in insertion order.
    pub fn direct_hosts(&self) -> Result<Vec<DirectHost>> {
        let _g = self.lock.lock().expect("registry lock");
        self.read_direct_hosts()
    }

    /// The direct host that created Space `id` here, with its id on the
    /// host.
    pub fn direct_host_of(&self, id: &str) -> Result<Option<(DirectHost, String)>> {
        let id = crate::id::canonical_id(id);
        Ok(self.direct_hosts()?.into_iter().find_map(|h| {
            let on_host = h.spaces.get(&id).cloned()?;
            Some((h, on_host))
        }))
    }

    /// Changes the direct-host list under the registry lock (atomic).
    pub fn update_direct_hosts(&self, change: impl FnOnce(&mut Vec<DirectHost>)) -> Result<()> {
        let _g = self.lock.lock().expect("registry lock");
        let _file = self.exclusive()?;
        let mut hosts = self.read_direct_hosts()?;
        change(&mut hosts);
        let mut bytes = serde_json::to_vec_pretty(&hosts)?;
        bytes.push(b'\n');
        self.atomic_write(&self.direct_hosts_path(), &bytes, 0o644)
    }

    /// Adds or renames a direct host (matched by its Space), keeping the
    /// Spaces it created.
    pub fn upsert_direct_host(&self, name: &str, space: &str) -> Result<()> {
        let space = crate::id::canonical_id(space);
        let name = name.trim().to_string();
        self.update_direct_hosts(|hosts| match hosts.iter_mut().find(|h| h.space == space) {
            Some(h) => h.name = name,
            None => hosts.push(DirectHost {
                name,
                space,
                spaces: BTreeMap::new(),
            }),
        })
    }

    /// Forgets Space `id` in the direct-host list: the host it is, or the
    /// entry of a Space a host created. Returns whether anything changed.
    pub fn forget_direct(&self, id: &str) -> Result<bool> {
        let id = crate::id::canonical_id(id);
        if !self.direct_hosts_path().exists() {
            return Ok(false);
        }
        let mut changed = false;
        self.update_direct_hosts(|hosts| {
            let before = hosts.len();
            hosts.retain(|h| h.space != id);
            changed |= hosts.len() != before;
            for h in hosts.iter_mut() {
                changed |= h.spaces.remove(&id).is_some();
            }
        })?;
        Ok(changed)
    }

    /// Forgets direct host `space` with every Space it created here (their
    /// rows and credentials, and rows that name it as their host): the
    /// host was removed or is gone, so nothing reaches them through it any
    /// more. Returns the Spaces forgotten.
    pub(crate) fn forget_direct_host(&self, space: &str) -> Result<Vec<String>> {
        let space = crate::id::canonical_id(space);
        let mut ids = std::collections::BTreeSet::new();
        if self.direct_hosts_path().exists() {
            self.update_direct_hosts(|hosts| {
                hosts.retain(|h| {
                    if h.space == space {
                        ids.extend(h.spaces.keys().cloned());
                        false
                    } else {
                        true
                    }
                });
            })?;
        }
        ids.extend(
            self.list()?
                .into_iter()
                .filter(|s| s.host == space)
                .map(|s| s.id),
        );
        ids.remove(&space);
        let mut gone = Vec::new();
        for id in ids {
            if self.remove(&id)? {
                gone.push(id);
            }
        }
        Ok(gone)
    }

    fn read_direct_hosts(&self) -> Result<Vec<DirectHost>> {
        match std::fs::read(self.direct_hosts_path()) {
            Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(vec![]),
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                Error::invalid(format!(
                    "{} is not a direct-host list ({e}); fix or remove it",
                    self.direct_hosts_path().display()
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
            Err(e) => Err(e.into()),
        }
    }

    /// An exclusive advisory lock on the registry directory, held across a
    /// read-modify-write so concurrent writers in *other processes* (the
    /// Spaces app, the CLI, `cua daemon`) never lose each other's entries.
    /// Readers need no lock: files are replaced atomically.
    fn exclusive(&self) -> Result<std::fs::File> {
        self.ensure_dir()?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.lock_path())?;
        file.lock()?;
        Ok(file)
    }

    /// Adds or replaces a Space (matched by id) and its credential.
    pub fn upsert(&self, mut space: dpb::Space, credential: Credential) -> Result<()> {
        space.id = crate::id::canonical_id(&space.id);
        let _g = self.lock.lock().expect("registry lock");
        let _file = self.exclusive()?;
        let mut spaces = self.read_spaces()?;
        match spaces.iter_mut().find(|s| s.id == space.id) {
            Some(slot) => *slot = space.clone(),
            None => spaces.push(space.clone()),
        }
        let mut creds = self.read_credentials()?;
        if credential == Credential::default() {
            creds.remove(&space.id);
        } else {
            creds.insert(space.id.clone(), credential);
        }
        self.write_credentials(&creds)?;
        self.write_spaces(&spaces)
    }

    /// Path of `space-volumes.json`: the named volumes of Spaces that have
    /// no record in `spaces.json` (a machine of your relay provides them;
    /// this machine serves the volumes over the Space's own connection).
    pub fn volume_overlay_path(&self) -> PathBuf {
        self.dir.join("space-volumes.json")
    }

    /// The overlay: Space id to its mounts.
    pub fn volume_overlay(
        &self,
    ) -> Result<BTreeMap<String, Vec<crate::named_volumes::VolumeMount>>> {
        match std::fs::read(self.volume_overlay_path()) {
            Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(BTreeMap::new()),
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Replaces the overlay mounts of `id` (empty removes the entry).
    pub fn set_overlay_volumes(
        &self,
        id: &str,
        mounts: Vec<crate::named_volumes::VolumeMount>,
    ) -> Result<()> {
        let _g = self.lock.lock().expect("registry lock");
        let _file = self.exclusive()?;
        let mut overlay = self.volume_overlay()?;
        if mounts.is_empty() {
            if overlay.remove(id).is_none() {
                return Ok(());
            }
        } else {
            overlay.insert(id.to_owned(), mounts);
        }
        let mut bytes = serde_json::to_vec_pretty(&overlay)?;
        bytes.push(b'\n');
        self.atomic_write(&self.volume_overlay_path(), &bytes, 0o644)
    }

    /// Removes a Space and its credential. Returns whether it was present.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let id = crate::id::canonical_id(id);
        let id = id.as_str();
        let _g = self.lock.lock().expect("registry lock");
        let _file = self.exclusive()?;
        let mut spaces = self.read_spaces()?;
        let before = spaces.len();
        spaces.retain(|s| s.id != id);
        let mut creds = self.read_credentials()?;
        let had_cred = creds.remove(id).is_some();
        if had_cred {
            self.write_credentials(&creds)?;
        }
        // Its labels go with it (a Space later made under the same name
        // starts clean).
        let mut meta = self.read_meta();
        if meta.remove(id).is_some() {
            self.write_meta(&meta)?;
        }
        if spaces.len() != before {
            self.write_spaces(&spaces)?;
            return Ok(true);
        }
        Ok(had_cred)
    }

    /// Reads `spaces.json`. Ids an older client stored
    /// (`space://direct/<host:port>`, ...) come back in the new form
    /// (`direct:<host:port>`), and the version key it wrote before the
    /// cua-spacesd rename (`envDriverVersion`) as `spacesdVersion`; the file
    /// is rewritten on the next change.
    fn read_spaces(&self) -> Result<Vec<dpb::Space>> {
        let invalid = |e: serde_json::Error| {
            Error::invalid(format!(
                "{} is not a Spaces registry ({e}); fix or remove it",
                self.spaces_path().display()
            ))
        };
        let spaces: Vec<dpb::Space> = match std::fs::read(self.spaces_path()) {
            Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => vec![],
            Ok(bytes) => {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(invalid)?;
                for space in value.as_array_mut().into_iter().flatten() {
                    if let Some(space) = space.as_object_mut()
                        && let Some(version) = space.remove(LEGACY_VERSION_KEY)
                    {
                        space.entry("spacesdVersion").or_insert(version);
                    }
                }
                serde_json::from_value(value).map_err(invalid)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => return Err(e.into()),
        };
        let mut out: Vec<dpb::Space> = Vec::with_capacity(spaces.len());
        for mut space in spaces {
            space.id = crate::id::canonical_id(&space.id);
            if !out.iter().any(|s| s.id == space.id) {
                out.push(space);
            }
        }
        Ok(out)
    }

    /// Reads the credential store, keyed by canonical id.
    fn read_credentials(&self) -> Result<BTreeMap<String, Credential>> {
        let stored: BTreeMap<String, Credential> = match std::fs::read(self.credentials_path()) {
            Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => BTreeMap::new(),
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        let mut out = BTreeMap::new();
        for (id, cred) in stored {
            out.entry(crate::id::canonical_id(&id)).or_insert(cred);
        }
        Ok(out)
    }

    fn ensure_dir(&self) -> Result<()> {
        // A test must never touch the user's real registry.
        cua_home::guard_write(&self.dir)?;
        if !self.dir.exists() {
            std::fs::create_dir_all(&self.dir)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }

    fn write_spaces(&self, spaces: &[dpb::Space]) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(spaces)?;
        bytes.push(b'\n');
        self.atomic_write(&self.spaces_path(), &bytes, 0o644)
    }

    fn write_credentials(&self, creds: &BTreeMap<String, Credential>) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(creds)?;
        bytes.push(b'\n');
        self.atomic_write(&self.credentials_path(), &bytes, 0o600)
    }

    fn atomic_write(&self, path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
        self.ensure_dir()?;
        let tmp = path.with_extension(format!("tmp-{:08x}", rand::random::<u32>()));
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }
            #[cfg(not(unix))]
            let _ = mode;
            let mut file = options.open(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_survives_upserts_and_leaves_with_the_space() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path().join("cua"));
        reg.upsert(space("direct:h:1"), Credential::default())
            .unwrap();
        reg.update_meta("direct:h:1", |m| {
            m.labels.insert("team".into(), "research".into());
        })
        .unwrap();
        // An upsert (a reconnect refreshing the record) keeps it.
        reg.upsert(space("direct:h:1"), Credential::default())
            .unwrap();
        // A fresh handle (another process) reads it back.
        let again = Registry::new(dir.path().join("cua"));
        assert_eq!(again.meta("direct:h:1").unwrap().labels["team"], "research");
        // A Space with no registry entry (a relay machine) can have meta.
        again
            .update_meta("relay:abc", |m| {
                m.labels.insert("a".into(), "b".into());
            })
            .unwrap();
        assert_eq!(again.all_meta().len(), 2);
        // Removing the Space drops its meta, entry or not.
        assert!(again.remove("direct:h:1").unwrap());
        assert!(again.meta("direct:h:1").is_none());
        again.remove("relay:abc").unwrap();
        assert!(again.all_meta().is_empty());
        // An emptied record is dropped, not kept as `{}`.
        again
            .update_meta("direct:h:2", |m| {
                m.labels.insert("k".into(), "v".into());
            })
            .unwrap();
        again
            .update_meta("direct:h:2", |m| m.labels.clear())
            .unwrap();
        assert!(again.meta("direct:h:2").is_none());
    }

    #[test]
    fn a_fork_inherits_labels_and_egress_but_not_its_clock() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path().join("cua"));
        reg.update_meta("local:src", |m| {
            m.labels.insert("team".into(), "research".into());
            m.created_at_unix = 100;
            m.egress.deny.push("all".into());
        })
        .unwrap();
        reg.update_meta("local:fork", |m| m.created_at_unix = 200)
            .unwrap();
        assert!(reg.inherit_meta("local:src", "local:fork").unwrap());
        let fork = reg.meta("local:fork").unwrap();
        assert_eq!(fork.labels["team"], "research");
        assert_eq!(fork.egress.deny, ["all"]);
        assert_eq!(fork.created_at_unix, 200, "its own creation time");
        // The source is untouched, and a source with nothing gives nothing.
        assert_eq!(reg.meta("local:src").unwrap().created_at_unix, 100);
        assert!(!reg.inherit_meta("local:none", "local:other").unwrap());
        assert!(reg.meta("local:other").is_none());
    }

    #[test]
    fn label_limits() {
        let ok = |pairs: &[(&str, &str)]| {
            let m: BTreeMap<String, String> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            validate_labels(&m)
        };
        assert!(ok(&[]).is_ok());
        assert!(ok(&[("team", "research"), ("naïve", "ünï")]).is_ok());
        assert!(ok(&[(&"k".repeat(63), &"v".repeat(256))]).is_ok());
        assert!(ok(&[(&"k".repeat(64), "v")]).is_err());
        assert!(ok(&[("k", &"v".repeat(257))]).is_err());
        // Bytes, not characters: 32 two-byte characters are 64 bytes.
        assert!(ok(&[(&"é".repeat(32), "v")]).is_err());
        assert!(ok(&[(&"é".repeat(31), "v")]).is_ok());
        assert!(ok(&[("", "v")]).is_err());
        assert!(ok(&[("k", "")]).is_err());
        let many: BTreeMap<String, String> =
            (0..64).map(|i| (format!("k{i}"), "v".into())).collect();
        assert!(validate_labels(&many).is_ok());
        let mut over = many;
        over.insert("one-more".into(), "v".into());
        assert!(validate_labels(&over).is_err());
        assert_eq!(parse_label("a=b").unwrap(), ("a".into(), "b".into()));
        assert_eq!(parse_label("a=b=c").unwrap().1, "b=c");
        assert!(
            parse_label("a").is_err() && parse_label("a=").is_err() && parse_label("=b").is_err()
        );
    }

    #[test]
    fn concurrent_writers_in_separate_registries_lose_nothing() {
        // Separate `Registry` values share no in-process mutex, like two
        // processes: only the file lock serializes them.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cua");
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let reg = Registry::new(path);
                    for i in 0..15 {
                        let id = format!("direct:h{t}-{i}:1");
                        reg.upsert(
                            space(&id),
                            Credential {
                                url: None,
                                token: Some(format!("t{t}{i}")),
                                ..Default::default()
                            },
                        )
                        .unwrap();
                        if i % 5 == 4 {
                            assert!(reg.remove(&id).unwrap());
                        }
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let reg = Registry::new(&path);
        let ids: std::collections::BTreeSet<String> =
            reg.list().unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids.len(), 4 * 12, "{ids:?}");
        for t in 0..4 {
            assert!(ids.contains(&format!("direct:h{t}-3:1")));
            assert!(!ids.contains(&format!("direct:h{t}-4:1")));
            assert_eq!(
                reg.credential(&format!("direct:h{t}-3:1"))
                    .unwrap()
                    .unwrap()
                    .token
                    .as_deref(),
                Some(format!("t{t}3").as_str())
            );
        }
    }

    fn space(id: &str) -> dpb::Space {
        dpb::Space {
            id: id.into(),
            name: "n".into(),
            spacesd_version: "1".into(),
            features: vec!["presence".into()],
            os: "linux".into(),
            os_name: String::new(),
            services: vec![],
            added_at: None,
            ..Default::default()
        }
    }

    #[test]
    fn reads_the_version_key_written_before_the_rename() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path().join("cua"));
        std::fs::create_dir_all(dir.path().join("cua")).unwrap();
        std::fs::write(
            reg.spaces_path(),
            r#"[{"id":"direct:h:1","name":"n","envDriverVersion":"0.9.0","features":[]}]"#,
        )
        .unwrap();
        let spaces = reg.list().unwrap();
        assert_eq!(spaces.len(), 1);
        assert_eq!(spaces[0].spacesd_version, "0.9.0");
    }

    #[test]
    fn round_trips_and_keeps_tokens_out_of_spaces_json() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path().join("cua"));
        reg.upsert(
            space("direct:h:1"),
            Credential {
                url: Some("http://h:1".into()),
                token: Some("s3cret".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(reg.list().unwrap().len(), 1);
        let spaces = std::fs::read_to_string(reg.spaces_path()).unwrap();
        assert!(!spaces.contains("s3cret"));
        // The on-disk key follows the proto field.
        assert!(spaces.contains("spacesdVersion"), "{spaces}");
        assert_eq!(
            reg.credential("direct:h:1")
                .unwrap()
                .unwrap()
                .token
                .as_deref(),
            Some("s3cret")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(reg.credentials_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
            let dmode = std::fs::metadata(reg.dir()).unwrap().permissions().mode();
            assert_eq!(dmode & 0o777, 0o700);
        }
        // Re-adding replaces rather than duplicates.
        reg.upsert(space("direct:h:1"), Credential::default())
            .unwrap();
        assert_eq!(reg.list().unwrap().len(), 1);
        assert!(reg.credential("direct:h:1").unwrap().is_none());
        assert!(reg.remove("direct:h:1").unwrap());
        assert!(!reg.remove("direct:h:1").unwrap());
        assert!(reg.list().unwrap().is_empty());
    }

    /// `cua-daemon` reads `spaces.json` as `Vec<cua.daemon.v1.Space>`; the
    /// registry must stay readable by that exact loader.
    #[test]
    fn the_file_is_the_daemon_shape() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path());
        reg.upsert(space("local:a"), Credential::default()).unwrap();
        let bytes = std::fs::read(reg.spaces_path()).unwrap();
        let parsed: Vec<dpb::Space> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed[0].id, "local:a");
    }

    /// Registries written before the unified refs keep working: legacy ids
    /// read back in the new form, and lookups accept either spelling.
    #[test]
    fn legacy_ids_migrate_on_read() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path());
        std::fs::write(
            reg.spaces_path(),
            serde_json::to_vec(&[
                space("space://relay/0123abcd4567ef89"),
                space("space://direct/h:1"),
            ])
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            reg.credentials_path(),
            br#"{"space://relay/0123abcd4567ef89": {"token": "t"}}"#,
        )
        .unwrap();
        let ids: Vec<String> = reg.list().unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["relay:0123abcd4567ef89", "direct:h:1"]);
        let cred = reg.credential("relay:0123abcd4567ef89").unwrap().unwrap();
        assert_eq!(cred.token.as_deref(), Some("t"));
        assert!(reg.get("space://relay/0123abcd4567ef89").unwrap().is_some());
        // The next write stores the new form.
        assert!(reg.remove("space://direct/h:1").unwrap());
        let bytes = std::fs::read_to_string(reg.spaces_path()).unwrap();
        assert!(
            bytes.contains("\"relay:0123abcd4567ef89\"") && !bytes.contains("space://"),
            "{bytes}"
        );
    }

    #[test]
    fn a_corrupt_registry_is_an_error_not_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path());
        std::fs::write(reg.spaces_path(), b"{not json").unwrap();
        assert!(reg.list().is_err());
    }

    /// Direct hosts: added, renamed with their Spaces kept, found by a
    /// Space they created, and forgotten.
    #[test]
    fn direct_hosts_round_trip_and_forget() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path().join("cua"));
        assert!(reg.direct_hosts().unwrap().is_empty());
        assert!(!reg.forget_direct("direct:100.64.0.9:3211").unwrap());
        reg.upsert_direct_host("Mac mini (spare)", "direct:100.64.0.9:3211")
            .unwrap();
        reg.update_direct_hosts(|hosts| {
            hosts[0]
                .spaces
                .insert("direct:100.64.0.9:41001".into(), "local:space-1".into());
        })
        .unwrap();
        // Renaming keeps the Spaces it created.
        reg.upsert_direct_host("spare", "direct:100.64.0.9:3211")
            .unwrap();
        let hosts = reg.direct_hosts().unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "spare");
        let (host, on_host) = reg
            .direct_host_of("direct:100.64.0.9:41001")
            .unwrap()
            .unwrap();
        assert_eq!(
            (host.name.as_str(), on_host.as_str()),
            ("spare", "local:space-1")
        );
        assert!(reg.forget_direct("direct:100.64.0.9:41001").unwrap());
        assert!(
            reg.direct_host_of("direct:100.64.0.9:41001")
                .unwrap()
                .is_none()
        );
        assert!(reg.forget_direct("direct:100.64.0.9:3211").unwrap());
        assert!(reg.direct_hosts().unwrap().is_empty());
    }

    /// A removed or lost direct host takes the Spaces it created with it:
    /// their rows, credentials and its list entry; other rows stay.
    #[test]
    fn forgetting_a_direct_host_forgets_its_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path().join("cua"));
        let row = |id: &str, host: &str| dpb::Space {
            id: id.into(),
            host: host.into(),
            ..Default::default()
        };
        let cred = Credential {
            token: Some("t".into()),
            ..Default::default()
        };
        reg.upsert(row("direct:10.0.0.5:3211", ""), cred.clone())
            .unwrap();
        reg.upsert_direct_host("chaoshost", "direct:10.0.0.5:3211")
            .unwrap();
        for (port, name) in [(41001, "a"), (41002, "b")] {
            let id = format!("direct:10.0.0.5:{port}");
            reg.upsert(row(&id, "direct:10.0.0.5:3211"), cred.clone())
                .unwrap();
            reg.update_direct_hosts(|h| {
                h[0].spaces.insert(id.clone(), format!("local:{name}"));
            })
            .unwrap();
        }
        // A row whose host entry was lost already still goes (by its host).
        reg.upsert(
            row("direct:10.0.0.5:41003", "direct:10.0.0.5:3211"),
            cred.clone(),
        )
        .unwrap();
        reg.upsert(row("local:mine", ""), Credential::default())
            .unwrap();
        let mut gone = reg.forget_direct_host("direct:10.0.0.5:3211").unwrap();
        gone.sort();
        assert_eq!(
            gone,
            [
                "direct:10.0.0.5:41001",
                "direct:10.0.0.5:41002",
                "direct:10.0.0.5:41003"
            ]
        );
        assert!(reg.direct_hosts().unwrap().is_empty());
        let left: Vec<String> = reg.list().unwrap().into_iter().map(|s| s.id).collect();
        assert_eq!(left, ["direct:10.0.0.5:3211", "local:mine"]);
        assert!(reg.credential("direct:10.0.0.5:41001").unwrap().is_none());
    }

    /// The host-safety guard: a test that forgets a temp `CUA_HOME` gets an
    /// error instead of a Space in the user's real registry. The registry
    /// sits in a directory under the real `~/.cua` that does not exist, so
    /// the guard is exercised through `upsert`/`remove` without opening (or
    /// even statting) the user's real registry or credentials.
    #[test]
    fn a_test_cannot_write_the_real_registry() {
        let Some(real) = cua_home::real_cua_home() else {
            return;
        };
        let dir = real.join(format!(
            "registry-guard-test-{:016x}",
            rand::random::<u64>()
        ));
        assert!(!dir.exists());
        let reg = Registry::new(&dir);
        let err = reg
            .upsert(space("direct:127.0.0.1:1"), Credential::default())
            .unwrap_err();
        assert!(err.to_string().contains("CUA_HOME"), "{err}");
        assert!(reg.remove("direct:127.0.0.1:1").is_err());
        assert!(!dir.exists(), "nothing was created under the real ~/.cua");
    }
}
