//! Local storage-format prototype for optional Driver extensions.
//!
//! A caller supplies an unsigned local `.tar.gz`. The manager checks its
//! structure and hashes, but does not establish publisher or artifact
//! provenance. Archive payloads are streamed into a private staging directory
//! before an exact version is activated.

use anyhow::{anyhow, bail, Context, Result};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions as CapOpenOptions};
use flate2::read::GzDecoder;
use fs2::FileExt;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MANIFEST_NAME: &str = "extension.json";
const INSTALL_RECORD_NAME: &str = ".install.json";
const ACTIVE_NAME: &str = "active.json";
const ACTIVE_BACKUP_NAME: &str = "active.backup.json";
const ACTIVE_NEW_NAME: &str = "active.new.json";
const MANIFEST_SCHEMA_VERSION: u32 = 1;
// The local prototype targets one executable plus a moderate model bundle.
// These defaults keep disk/memory exposure bounded without claiming support
// for multi-tens-of-gigabytes production model distributions.
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_IN_MEMORY_BYTES: u64 = 1024 * 1024;
const MAX_FILE_COUNT: usize = 4_096;
const MAX_ARCHIVE_PATH_BYTES: usize = 512;
const TRUST_NOTICE: &str = "unsigned, untrusted local code; archive-provided hashes do not authenticate publisher or artifact provenance";

#[derive(Clone, Copy)]
struct RegistryEntry {
    id: &'static str,
    display_name: &'static str,
    description: &'static str,
    protocol_version: u32,
}

const REGISTRY: &[RegistryEntry] = &[RegistryEntry {
    id: "local-prototype",
    display_name: "Local Extension Prototype",
    description: "Unsigned local archive prototype; publisher provenance is not verified.",
    protocol_version: 1,
}];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExtensionManifest {
    schema_version: u32,
    id: String,
    version: String,
    driver_version: String,
    protocol_version: u32,
    target: String,
    entrypoint: String,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    path: String,
    sha256: String,
    #[serde(default)]
    executable: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActivePointer {
    id: String,
    version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InstallRecord {
    schema_version: u32,
    id: String,
    version: String,
    manifest_sha256: String,
}

#[derive(Debug, Serialize)]
struct ExtensionInfo<'a> {
    id: &'a str,
    display_name: &'a str,
    description: &'a str,
    protocol_version: u32,
    installed: bool,
    active_version: Option<String>,
    healthy: bool,
    detail: String,
}

#[derive(Debug)]
struct ArchiveFile {
    path: String,
    sha256: String,
    size: u64,
}

#[derive(Debug)]
struct InspectedArchive {
    manifest: ExtensionManifest,
    manifest_bytes: Vec<u8>,
}

struct BoundedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R: Read> Read for BoundedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "compressed archive exceeds its byte limit",
            ));
        }
        let limit = buffer.len().min(self.remaining as usize);
        let read = self.inner.read(&mut buffer[..limit])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

struct ExtensionStore {
    root: PathBuf,
}

struct InstallLock {
    _file: fs::File,
    root: Dir,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivationFailpoint {
    None,
    AfterBackup,
    LeaveInterruptedAfterBackup,
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedCommand {
    subcommand: String,
    id: Option<String>,
    archive: Option<PathBuf>,
    json: bool,
}

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn cap_parent(path: &Path) -> Result<(Dir, &std::ffi::OsStr)> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("path has no final component: {}", path.display()))?;
    let dir = open_directory_path_nofollow(parent)?;
    Ok((dir, name))
}

fn directory_path_anchor(path: &Path) -> Result<(Dir, Vec<std::ffi::OsString>)> {
    #[cfg(target_os = "macos")]
    let normalized;
    #[cfg(target_os = "macos")]
    let path = if let Ok(suffix) = path.strip_prefix("/var") {
        normalized = Path::new("/private/var").join(suffix);
        normalized.as_path()
    } else if let Ok(suffix) = path.strip_prefix("/tmp") {
        normalized = Path::new("/private/tmp").join(suffix);
        normalized.as_path()
    } else if let Ok(suffix) = path.strip_prefix("/etc") {
        normalized = Path::new("/private/etc").join(suffix);
        normalized.as_path()
    } else {
        path
    };
    let mut components = path.components().peekable();
    let mut anchor = PathBuf::new();
    if let Some(Component::Prefix(prefix)) = components.peek().copied() {
        anchor.push(prefix.as_os_str());
        components.next();
    }
    if matches!(components.peek(), Some(Component::RootDir)) {
        anchor.push(std::path::MAIN_SEPARATOR_STR);
        components.next();
    } else if anchor.as_os_str().is_empty() {
        anchor.push(".");
    }
    let mut names = Vec::new();
    for component in components {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => names.push(name.to_owned()),
            Component::ParentDir => bail!(
                "directory path must not contain parent traversal: {}",
                path.display()
            ),
            Component::Prefix(_) | Component::RootDir => {
                bail!("directory path has an invalid root: {}", path.display())
            }
        }
    }
    let directory = Dir::open_ambient_dir(&anchor, ambient_authority())
        .with_context(|| format!("open directory anchor {}", anchor.display()))?;
    Ok((directory, names))
}

fn open_directory_path_nofollow(path: &Path) -> Result<Dir> {
    let (mut directory, names) = directory_path_anchor(path)?;
    for name in names {
        directory = directory.open_dir_nofollow(&name).with_context(|| {
            format!("open directory component {:?} in {}", name, path.display())
        })?;
    }
    Ok(directory)
}

fn ensure_private_directory_path(path: &Path) -> Result<Dir> {
    let (mut directory, names) = directory_path_anchor(path)?;
    for name in names {
        directory = match directory.open_dir_nofollow(&name) {
            Ok(next) => next,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_private_subdirectory(&directory, &name)?;
                directory.open_dir_nofollow(&name)?
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("open directory component {:?} in {}", name, path.display())
                })
            }
        };
    }
    verify_cap_directory_permissions_portable(&directory)?;
    Ok(directory)
}

fn sync_cap_dir(dir: &Dir) -> Result<()> {
    #[cfg(unix)]
    dir.try_clone()?
        .into_std_file()
        .sync_all()
        .context("sync containing directory")?;
    Ok(())
}

pub fn run(args: &[String]) {
    if let Err(error) = run_inner(args) {
        eprintln!("cua-driver extension: {error:#}");
        std::process::exit(1);
    }
}

fn run_inner(args: &[String]) -> Result<()> {
    let parsed = parse_command(args)?;
    let subcommand = parsed.subcommand.as_str();
    let id = parsed.id.as_deref();
    let json = parsed.json;
    let store = ExtensionStore::new(extension_root()?);

    match subcommand {
        "list" => print_infos(&store, REGISTRY, json),
        "info" => print_info(&store, registry_entry(required_id(id, "info")?)?, json),
        "status" => match id {
            Some(id) => print_info(&store, registry_entry(id)?, json),
            None => print_infos(&store, REGISTRY, json),
        },
        "install" | "update" => {
            ensure_mutations_supported()?;
            let id = required_id(id, subcommand)?;
            let entry = registry_entry(id)?;
            let archive = parsed.archive.as_deref().ok_or_else(|| {
                anyhow!("{subcommand} requires --archive <path>; network download and publisher verification are not implemented")
            })?;
            let installed = store.install_archive(entry, archive)?;
            println!(
                "{} unsigned, untrusted local code prototype {} {} at {}; hashes are self-asserted by the same archive and do not establish provenance",
                if subcommand == "update" {
                    "Updated"
                } else {
                    "Stored"
                },
                id,
                installed.version,
                installed.path.display()
            );
            Ok(())
        }
        "path" => {
            let id = required_id(id, "path")?;
            registry_entry(id)?;
            let path = store
                .active_path(id)?
                .ok_or_else(|| anyhow!("{id} is not installed"))?;
            println!("{}", path.display());
            Ok(())
        }
        _ => unreachable!(),
    }
}

fn parse_command(args: &[String]) -> Result<ParsedCommand> {
    let subcommand = args.first().map(String::as_str).unwrap_or("list");
    if !matches!(
        subcommand,
        "list" | "info" | "status" | "install" | "update" | "path"
    ) {
        bail!("unknown subcommand {subcommand:?}; expected list, info, status, install, update, or path");
    }
    let mut id = None;
    let mut archive = None;
    let mut json = false;
    let mut index = usize::from(!args.is_empty());
    while index < args.len() {
        let value = &args[index];
        match value.as_str() {
            "--json" => {
                if json {
                    bail!("duplicate flag --json");
                }
                json = true;
                index += 1;
            }
            "--archive" => {
                if archive.is_some() {
                    bail!("duplicate option --archive");
                }
                let next = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow!("--archive requires a value"))?;
                if next.starts_with('-') {
                    bail!("--archive requires a value");
                }
                archive = Some(PathBuf::from(next));
                index += 2;
            }
            _ if value.starts_with("--archive=") => {
                if archive.is_some() {
                    bail!("duplicate option --archive");
                }
                let path = value.trim_start_matches("--archive=");
                if path.is_empty() {
                    bail!("--archive requires a value");
                }
                archive = Some(PathBuf::from(path));
                index += 1;
            }
            _ if value.starts_with('-') => bail!("unknown extension option {value:?}"),
            _ => {
                if id.replace(value.clone()).is_some() {
                    bail!("unexpected extra positional argument {value:?}");
                }
                index += 1;
            }
        }
    }
    match subcommand {
        "list" if id.is_some() => bail!("list does not accept an extension name"),
        "info" | "install" | "update" | "path" if id.is_none() => {
            bail!("{subcommand} requires an extension name")
        }
        _ => {}
    }
    if !matches!(subcommand, "install" | "update") && archive.is_some() {
        bail!("--archive is only valid with install or update");
    }
    if !matches!(subcommand, "list" | "info" | "status") && json {
        bail!("--json is not valid with {subcommand}");
    }
    Ok(ParsedCommand {
        subcommand: subcommand.to_owned(),
        id,
        archive,
        json,
    })
}

fn ensure_mutations_supported() -> Result<()> {
    #[cfg(windows)]
    bail!("extension install/update is unsupported on Windows until opened-handle reparse-point and ACL enforcement is implemented");
    #[cfg(not(windows))]
    Ok(())
}

fn required_id<'a>(id: Option<&'a str>, command: &str) -> Result<&'a str> {
    id.ok_or_else(|| anyhow!("usage: cua-driver extension {command} <name>"))
}

fn registry_entry(id: &str) -> Result<&'static RegistryEntry> {
    REGISTRY
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| anyhow!("unknown extension {id:?}; run `cua-driver extension list`"))
}

fn extension_root() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("CUA_DRIVER_RS_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home).join("extensions"));
    }
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE").ok_or_else(|| anyhow!("USERPROFILE not set"))?;
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home)
        .join(crate::bundle::user_home_subdirectory())
        .join("extensions"))
}

fn print_infos(store: &ExtensionStore, entries: &[RegistryEntry], json: bool) -> Result<()> {
    let infos = entries
        .iter()
        .map(|entry| store.info(entry))
        .collect::<Result<Vec<_>>>()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&infos)?);
    } else {
        for info in infos {
            let version = info.active_version.as_deref().unwrap_or("not installed");
            println!("{}\t{}\t{}", info.id, version, info.detail);
        }
    }
    Ok(())
}

fn print_info(store: &ExtensionStore, entry: &RegistryEntry, json: bool) -> Result<()> {
    let info = store.info(entry)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        println!("{} ({})", info.display_name, info.id);
        println!("{}", info.description);
        println!("Protocol: {}", info.protocol_version);
        println!("Status: {}", info.detail);
        if let Some(version) = info.active_version {
            println!("Active version: {version}");
        }
    }
    Ok(())
}

#[derive(Debug)]
struct InstalledVersion {
    version: String,
    path: PathBuf,
}

impl ExtensionStore {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn extension_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn versions_dir(&self, id: &str) -> PathBuf {
        self.extension_dir(id).join("versions")
    }

    fn recover_activation_locked(&self, root: &Dir, id: &str) -> Result<()> {
        let Some(extension) = open_private_subdirectory_if_present(root, id)? else {
            return Ok(());
        };
        let mut has_active = private_regular_file_or_missing_at(&extension, ACTIVE_NAME)?;
        let mut has_backup = private_regular_file_or_missing_at(&extension, ACTIVE_BACKUP_NAME)?;
        let has_new = private_regular_file_or_missing_at(&extension, ACTIVE_NEW_NAME)?;
        if has_active {
            self.validate_pointer_target_locked(id, &extension, ACTIVE_NAME)?;
        }
        if has_backup {
            self.validate_pointer_target_locked(id, &extension, ACTIVE_BACKUP_NAME)?;
        }
        if !has_active && has_backup {
            extension
                .rename(ACTIVE_BACKUP_NAME, &extension, ACTIVE_NAME)
                .with_context(|| format!("restore interrupted activation for {id}"))?;
            sync_cap_dir(&extension)?;
            has_active = true;
            has_backup = false;
        }
        if has_active && has_backup {
            extension
                .remove_file(ACTIVE_BACKUP_NAME)
                .with_context(|| format!("remove stale activation backup for {id}"))?;
            sync_cap_dir(&extension)?;
        }
        if has_new {
            extension
                .remove_file(ACTIVE_NEW_NAME)
                .with_context(|| format!("remove stale activation candidate for {id}"))?;
            sync_cap_dir(&extension)?;
        }
        Ok(())
    }

    fn validate_pointer_target_locked(
        &self,
        id: &str,
        extension: &Dir,
        pointer_name: &str,
    ) -> Result<()> {
        let bytes = read_small_file_at(extension, Path::new(pointer_name))?;
        let pointer: ActivePointer = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse activation pointer {pointer_name}"))?;
        if pointer.id != id {
            bail!("activation pointer for {id} names extension {}", pointer.id);
        }
        validate_version_segment(&pointer.version)?;
        let versions = extension
            .open_dir_nofollow("versions")
            .context("active extension has no owned versions directory")?;
        verify_cap_directory_permissions_portable(&versions)?;
        let version = versions
            .open_dir_nofollow(&pointer.version)
            .with_context(|| {
                format!(
                    "active extension version {} is not an owned directory",
                    pointer.version
                )
            })?;
        verify_cap_directory_permissions_portable(&version)?;
        verify_installed_version_at(&version, registry_entry(id)?, None)?;
        Ok(())
    }

    fn lock(&self, id: &str) -> Result<InstallLock> {
        let root = ensure_private_directory_path(&self.root)?;
        let lock_dir = ensure_private_subdirectory(&root, ".locks")?;
        let lock_name = format!("{id}.lock");
        let file = open_lock_file_at(&lock_dir, &lock_name)?;
        file.try_lock_exclusive()
            .with_context(|| format!("extension {id} is already being modified"))?;
        Ok(InstallLock { _file: file, root })
    }

    fn install_archive(&self, entry: &RegistryEntry, archive: &Path) -> Result<InstalledVersion> {
        let lock = self.lock(entry.id)?;
        self.recover_activation_locked(&lock.root, entry.id)?;
        let extension_handle = ensure_private_subdirectory(&lock.root, entry.id)?;
        let versions = self.versions_dir(entry.id);
        let versions_handle = ensure_private_subdirectory(&extension_handle, "versions")?;
        let staging_parent = self.root.join(".staging");
        let staging_parent_handle = ensure_private_subdirectory(&lock.root, ".staging")?;
        let staging = staging_parent.join(unique_name(entry.id));
        let staging_name = staging.file_name().expect("generated staging name");
        create_private_subdirectory(&staging_parent_handle, staging_name)?;
        let staging_handle = staging_parent_handle.open_dir_nofollow(staging_name)?;
        let inspected = match inspect_archive(archive, entry, &staging_handle) {
            Ok(inspected) => inspected,
            Err(error) => {
                remove_cap_subdirectory(&staging_parent_handle, staging_name)?;
                return Err(error);
            }
        };
        let version = match Version::parse(&inspected.manifest.version)
            .context("manifest version is not valid semantic versioning")
        {
            Ok(version) => version,
            Err(error) => {
                remove_cap_subdirectory(&staging_parent_handle, staging_name)?;
                return Err(error);
            }
        };
        let version_text = version.to_string();
        let destination = versions.join(&version_text);
        match versions_handle.open_dir_nofollow(&version_text) {
            Ok(existing) => {
                verify_cap_directory_permissions_portable(&existing)?;
                let verified =
                    verify_installed_version_at(&existing, entry, Some(&inspected.manifest_bytes));
                let cleanup = remove_cap_subdirectory(&staging_parent_handle, staging_name);
                verified?;
                cleanup?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let staged = (|| -> Result<()> {
                    write_install_record_at(&staging_handle, &inspected)?;
                    verify_installed_version_at(
                        &staging_handle,
                        entry,
                        Some(&inspected.manifest_bytes),
                    )?;
                    staging_parent_handle
                        .rename(staging_name, &versions_handle, &version_text)
                        .with_context(|| {
                            format!("place extension version at {}", destination.display())
                        })?;
                    sync_cap_dir(&staging_parent_handle)?;
                    sync_cap_dir(&versions_handle)?;
                    Ok(())
                })();
                if staged.is_err() {
                    remove_cap_subdirectory(&staging_parent_handle, staging_name)?;
                }
                staged?;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "inspect existing extension version {}",
                        destination.display()
                    )
                })
            }
        }

        self.activate_locked(
            &lock.root,
            entry.id,
            &version_text,
            ActivationFailpoint::None,
        )?;
        Ok(InstalledVersion {
            version: version_text,
            path: destination,
        })
    }

    fn activate_locked(
        &self,
        root: &Dir,
        id: &str,
        version: &str,
        failpoint: ActivationFailpoint,
    ) -> Result<()> {
        let extension = ensure_private_subdirectory(root, id)?;
        let had_active = private_regular_file_or_missing_at(&extension, ACTIVE_NAME)?;
        if private_regular_file_or_missing_at(&extension, ACTIVE_BACKUP_NAME)?
            || private_regular_file_or_missing_at(&extension, ACTIVE_NEW_NAME)?
        {
            bail!("activation pointer state was not recovered before activation");
        }
        let pointer = serde_json::to_vec_pretty(&ActivePointer {
            id: id.to_owned(),
            version: version.to_owned(),
        })?;
        write_new_file_at(&extension, ACTIVE_NEW_NAME, &pointer)?;
        sync_cap_dir(&extension)?;
        if had_active {
            extension
                .rename(ACTIVE_NAME, &extension, ACTIVE_BACKUP_NAME)
                .context("move current active pointer to rollback backup")?;
            sync_cap_dir(&extension)?;
        }
        if failpoint == ActivationFailpoint::LeaveInterruptedAfterBackup {
            return Err(anyhow!(
                "simulated process interruption after activation backup"
            ));
        }
        let switched = if failpoint == ActivationFailpoint::AfterBackup {
            Err(anyhow!("simulated interruption after activation backup"))
        } else {
            extension
                .rename(ACTIVE_NEW_NAME, &extension, ACTIVE_NAME)
                .context("activate inspected extension version")
        };
        if let Err(error) = switched {
            let _ = extension.remove_file(ACTIVE_NEW_NAME);
            if had_active {
                extension
                    .rename(ACTIVE_BACKUP_NAME, &extension, ACTIVE_NAME)
                    .context("restore previous active extension")?;
                sync_cap_dir(&extension)?;
            }
            return Err(error);
        }
        if private_regular_file_or_missing_at(&extension, ACTIVE_BACKUP_NAME)? {
            extension
                .remove_file(ACTIVE_BACKUP_NAME)
                .context("remove activation rollback backup")?;
        }
        sync_cap_dir(&extension)?;
        Ok(())
    }

    fn active_pointer_locked(&self, root: &Dir, id: &str) -> Result<Option<ActivePointer>> {
        let Some(extension) = open_private_subdirectory_if_present(root, id)? else {
            return Ok(None);
        };
        if !private_regular_file_or_missing_at(&extension, ACTIVE_NAME)? {
            return Ok(None);
        }
        let bytes = read_small_file_at(&extension, Path::new(ACTIVE_NAME))?;
        let pointer: ActivePointer =
            serde_json::from_slice(&bytes).context("parse active extension pointer")?;
        if pointer.id != id {
            bail!("active pointer for {id} names extension {}", pointer.id);
        }
        validate_version_segment(&pointer.version)?;
        Ok(Some(pointer))
    }

    fn active_path(&self, id: &str) -> Result<Option<PathBuf>> {
        let lock = self.lock(id)?;
        self.recover_activation_locked(&lock.root, id)?;
        let Some(pointer) = self.active_pointer_locked(&lock.root, id)? else {
            return Ok(None);
        };
        let extension = lock.root.open_dir_nofollow(id)?;
        let versions = extension
            .open_dir_nofollow("versions")
            .context("active extension has no owned versions directory")?;
        verify_cap_directory_permissions_portable(&versions)?;
        let version = versions
            .open_dir_nofollow(&pointer.version)
            .with_context(|| {
                format!(
                    "active extension version {} is not an owned directory",
                    pointer.version
                )
            })?;
        verify_cap_directory_permissions_portable(&version)?;
        let path = self.versions_dir(id).join(&pointer.version);
        verify_installed_version_at(&version, registry_entry(id)?, None)?;
        Ok(Some(path))
    }

    fn info<'a>(&self, entry: &'a RegistryEntry) -> Result<ExtensionInfo<'a>> {
        let lock = match self.lock(entry.id) {
            Ok(lock) => lock,
            Err(error) => {
                return Ok(ExtensionInfo {
                    id: entry.id,
                    display_name: entry.display_name,
                    description: entry.description,
                    protocol_version: entry.protocol_version,
                    installed: true,
                    active_version: None,
                    healthy: false,
                    detail: format!("unhealthy: {error:#}; {TRUST_NOTICE}"),
                });
            }
        };
        let pointer = match self
            .recover_activation_locked(&lock.root, entry.id)
            .and_then(|()| self.active_pointer_locked(&lock.root, entry.id))
        {
            Ok(pointer) => pointer,
            Err(error) => {
                return Ok(ExtensionInfo {
                    id: entry.id,
                    display_name: entry.display_name,
                    description: entry.description,
                    protocol_version: entry.protocol_version,
                    installed: true,
                    active_version: None,
                    healthy: false,
                    detail: format!("unhealthy: {error:#}; {TRUST_NOTICE}"),
                });
            }
        };
        let Some(pointer) = pointer else {
            return Ok(ExtensionInfo {
                id: entry.id,
                display_name: entry.display_name,
                description: entry.description,
                protocol_version: entry.protocol_version,
                installed: false,
                active_version: None,
                healthy: true,
                detail: format!("not installed (healthy); {TRUST_NOTICE}"),
            });
        };
        let version = pointer.version.clone();
        let verified = (|| -> Result<()> {
            let extension = lock.root.open_dir_nofollow(entry.id)?;
            let versions = extension
                .open_dir_nofollow("versions")
                .context("active extension has no owned versions directory")?;
            verify_cap_directory_permissions_portable(&versions)?;
            let installed = versions.open_dir_nofollow(&version).with_context(|| {
                format!("active extension version {version} is not an owned directory")
            })?;
            verify_cap_directory_permissions_portable(&installed)?;
            verify_installed_version_at(&installed, entry, None)?;
            Ok(())
        })();
        match verified {
            Ok(_) => Ok(ExtensionInfo {
                id: entry.id,
                display_name: entry.display_name,
                description: entry.description,
                protocol_version: entry.protocol_version,
                installed: true,
                active_version: Some(version),
                healthy: true,
                detail: format!("stored; local integrity checks passed; {TRUST_NOTICE}"),
            }),
            Err(error) => Ok(ExtensionInfo {
                id: entry.id,
                display_name: entry.display_name,
                description: entry.description,
                protocol_version: entry.protocol_version,
                installed: true,
                active_version: Some(version),
                healthy: false,
                detail: format!("unhealthy: {error:#}; {TRUST_NOTICE}"),
            }),
        }
    }
}

fn inspect_archive(
    path: &Path,
    entry: &RegistryEntry,
    staging_dir: &Dir,
) -> Result<InspectedArchive> {
    let file = open_existing_no_follow(path)?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect archive {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("archive is not a regular local file: {}", path.display());
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        bail!("archive exceeds the {MAX_ARCHIVE_BYTES} byte input limit");
    }
    let decoder = GzDecoder::new(BoundedReader {
        inner: file,
        remaining: MAX_ARCHIVE_BYTES + 1,
    });
    let mut archive = tar::Archive::new(decoder);
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut expanded = 0_u64;
    for item in archive
        .entries()
        .context("read extension archive")?
        .raw(true)
    {
        let mut item = item.context("read extension archive entry")?;
        let entry_type = item.header().entry_type();
        let path = item.path().context("read archive entry path")?;
        let normalized = safe_relative_path(&path)?;
        if normalized.as_os_str().is_empty() || entry_type.is_dir() {
            continue;
        }
        if !entry_type.is_file() {
            bail!(
                "archive entry {} is not a regular file",
                normalized.display()
            );
        }
        let normalized = normalized.to_string_lossy().replace('\\', "/");
        if normalized.len() > MAX_ARCHIVE_PATH_BYTES {
            bail!("archive path exceeds the {MAX_ARCHIVE_PATH_BYTES} byte limit");
        }
        if !seen.insert(normalized.clone()) {
            bail!("archive contains duplicate entry {normalized}");
        }
        if files.len() >= MAX_FILE_COUNT {
            bail!("archive contains more than {MAX_FILE_COUNT} files");
        }
        let size = item.header().size().context("read archive entry size")?;
        if size > MAX_FILE_BYTES {
            bail!("archive entry {normalized} exceeds the {MAX_FILE_BYTES} byte file limit");
        }
        if normalized == MANIFEST_NAME && size > MAX_IN_MEMORY_BYTES {
            bail!("{MANIFEST_NAME} exceeds the {MAX_IN_MEMORY_BYTES} byte in-memory limit");
        }
        expanded = expanded
            .checked_add(size)
            .ok_or_else(|| anyhow!("archive size overflow"))?;
        if expanded > MAX_EXPANDED_BYTES {
            bail!(
                "archive expands beyond the {} byte limit",
                MAX_EXPANDED_BYTES
            );
        }
        let relative = Path::new(&normalized);
        create_private_cap_parent_directories(staging_dir, relative)?;
        let sha256 = stream_archive_file_at(staging_dir, &mut item, relative, size)
            .with_context(|| format!("stage archive entry {normalized}"))?;
        files.push(ArchiveFile {
            path: normalized,
            sha256,
            size,
        });
    }
    if !files.iter().any(|file| file.path == MANIFEST_NAME) {
        bail!("archive does not contain {MANIFEST_NAME} at its root");
    }
    let manifest_bytes = read_small_file_at(staging_dir, Path::new(MANIFEST_NAME))?;
    let manifest: ExtensionManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parse {MANIFEST_NAME}"))?;
    validate_manifest(&manifest, entry, &files)?;
    apply_manifest_permissions_at(staging_dir, &manifest)?;
    Ok(InspectedArchive {
        manifest,
        manifest_bytes,
    })
}

fn validate_manifest(
    manifest: &ExtensionManifest,
    entry: &RegistryEntry,
    archive_files: &[ArchiveFile],
) -> Result<()> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        bail!(
            "unsupported extension manifest schema {}; expected {MANIFEST_SCHEMA_VERSION}",
            manifest.schema_version
        );
    }
    if manifest.id != entry.id {
        bail!(
            "manifest extension id {:?} does not match requested {:?}",
            manifest.id,
            entry.id
        );
    }
    Version::parse(&manifest.version)
        .context("manifest version is not valid semantic versioning")?;
    let requirement = VersionReq::parse(&manifest.driver_version)
        .context("manifest driver_version is not a valid semantic version requirement")?;
    let driver = Version::parse(env!("CARGO_PKG_VERSION"))?;
    if !requirement.matches(&driver) {
        bail!(
            "extension {} requires Driver {}; running Driver is {}",
            manifest.version,
            manifest.driver_version,
            driver
        );
    }
    if manifest.protocol_version != entry.protocol_version {
        bail!(
            "extension protocol {} is incompatible with Driver protocol {}",
            manifest.protocol_version,
            entry.protocol_version
        );
    }
    let target = current_target()?;
    if manifest.target != target {
        bail!(
            "extension target {:?} does not match current target {:?}",
            manifest.target,
            target
        );
    }
    safe_manifest_path(&manifest.entrypoint)?;
    let mut declared = BTreeMap::new();
    for file in &manifest.files {
        safe_manifest_path(&file.path)?;
        if file.path == MANIFEST_NAME || file.path == INSTALL_RECORD_NAME {
            bail!("manifest cannot claim reserved path {:?}", file.path);
        }
        if file.sha256.len() != 64 || !file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("invalid SHA-256 for {}", file.path);
        }
        if declared.insert(file.path.as_str(), file).is_some() {
            bail!("manifest declares duplicate file {}", file.path);
        }
    }
    if !declared.contains_key(manifest.entrypoint.as_str()) {
        bail!(
            "manifest entrypoint {} is not in files",
            manifest.entrypoint
        );
    }
    if !declared
        .get(manifest.entrypoint.as_str())
        .is_some_and(|file| file.executable)
    {
        bail!(
            "manifest entrypoint {} must be executable",
            manifest.entrypoint
        );
    }
    let actual: BTreeMap<&str, &ArchiveFile> = archive_files
        .iter()
        .filter(|file| file.path != MANIFEST_NAME)
        .map(|file| (file.path.as_str(), file))
        .collect();
    if actual.len() != declared.len() {
        bail!("archive file set does not exactly match the manifest");
    }
    for (path, expected) in declared {
        let file = actual
            .get(path)
            .ok_or_else(|| anyhow!("archive is missing declared file {path}"))?;
        if !file.sha256.eq_ignore_ascii_case(&expected.sha256) {
            bail!(
                "SHA-256 mismatch for {path}: expected {}, got {}",
                expected.sha256,
                file.sha256
            );
        }
        if file.size > MAX_FILE_BYTES {
            bail!("file {path} exceeds the supported file limit");
        }
    }
    Ok(())
}

fn write_install_record_at(root: &Dir, inspected: &InspectedArchive) -> Result<()> {
    let record = InstallRecord {
        schema_version: MANIFEST_SCHEMA_VERSION,
        id: inspected.manifest.id.clone(),
        version: inspected.manifest.version.clone(),
        manifest_sha256: hex_sha256(&inspected.manifest_bytes),
    };
    write_new_file_at(
        root,
        INSTALL_RECORD_NAME,
        &serde_json::to_vec_pretty(&record)?,
    )
}

fn write_new_file_at(directory: &Dir, path: &str, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 > MAX_IN_MEMORY_BYTES {
        bail!("refusing to write oversized in-memory file {path}");
    }
    let mut options = CapOpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = directory.open_with(path, &options)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn verify_installed_version_at(
    root: &Dir,
    entry: &RegistryEntry,
    expected_manifest: Option<&[u8]>,
) -> Result<ExtensionManifest> {
    let bytes = read_small_file_at(root, Path::new(MANIFEST_NAME))?;
    if expected_manifest.is_some_and(|expected| expected != bytes) {
        bail!("version directory already exists with a different manifest");
    }
    let manifest: ExtensionManifest = serde_json::from_slice(&bytes)?;
    safe_manifest_path(&manifest.entrypoint)?;
    for file in &manifest.files {
        safe_manifest_path(&file.path)?;
    }
    let actual_paths = installed_regular_files_at(root)?;
    let expected_paths: BTreeSet<String> = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .chain([MANIFEST_NAME.to_owned(), INSTALL_RECORD_NAME.to_owned()])
        .collect();
    if actual_paths != expected_paths {
        bail!("installed file set does not exactly match the ownership record");
    }
    let synthetic_files = manifest
        .files
        .iter()
        .map(|file| {
            let path = Path::new(&file.path);
            let metadata = root
                .symlink_metadata(path)
                .with_context(|| format!("inspect installed file {}", path.display()))?;
            if !metadata.file_type().is_file() {
                bail!("installed path is not a regular file: {}", path.display());
            }
            Ok(ArchiveFile {
                path: file.path.clone(),
                sha256: hash_file_at(root, path, MAX_FILE_BYTES)?,
                size: metadata.len(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut all_files = synthetic_files;
    all_files.push(ArchiveFile {
        path: MANIFEST_NAME.to_owned(),
        sha256: hex_sha256(&bytes),
        size: bytes.len() as u64,
    });
    validate_manifest(&manifest, entry, &all_files)?;
    verify_manifest_permissions_at(root, &manifest)?;
    verify_install_record_at(root, entry.id, &manifest.version)?;
    Ok(manifest)
}

fn installed_regular_files_at(root: &Dir) -> Result<BTreeSet<String>> {
    fn visit(
        directory: &Dir,
        prefix: &Path,
        paths: &mut BTreeSet<String>,
        expanded: &mut u64,
    ) -> Result<()> {
        verify_cap_directory_permissions_portable(directory)?;
        for child in directory.entries()? {
            let child = child?;
            let file_type = child.file_type()?;
            let relative = prefix.join(child.file_name());
            if file_type.is_symlink() {
                bail!(
                    "installed extension contains a symbolic link: {}",
                    relative.display()
                );
            }
            if file_type.is_dir() {
                visit(
                    &directory.open_dir_nofollow(child.file_name())?,
                    &relative,
                    paths,
                    expanded,
                )?;
            } else if file_type.is_file() {
                let size = child.metadata()?.len();
                if size > MAX_FILE_BYTES {
                    bail!(
                        "installed file exceeds the supported file limit: {}",
                        relative.display()
                    );
                }
                *expanded = expanded
                    .checked_add(size)
                    .ok_or_else(|| anyhow!("installed extension size overflow"))?;
                if *expanded > MAX_EXPANDED_BYTES {
                    bail!("installed extension exceeds the supported expanded size");
                }
                paths.insert(relative.to_string_lossy().replace('\\', "/"));
                if paths.len() > MAX_FILE_COUNT + 2 {
                    bail!("installed extension contains too many files");
                }
            } else {
                bail!(
                    "installed extension contains a special file: {}",
                    relative.display()
                );
            }
        }
        Ok(())
    }
    let mut paths = BTreeSet::new();
    let mut expanded = 0;
    visit(root, Path::new(""), &mut paths, &mut expanded)?;
    Ok(paths)
}

fn verify_install_record_at(root: &Dir, id: &str, version: &str) -> Result<()> {
    let manifest = read_small_file_at(root, Path::new(MANIFEST_NAME))?;
    let record: InstallRecord =
        serde_json::from_slice(&read_small_file_at(root, Path::new(INSTALL_RECORD_NAME))?)?;
    if record.schema_version != MANIFEST_SCHEMA_VERSION
        || record.id != id
        || record.version != version
        || record.manifest_sha256 != hex_sha256(&manifest)
    {
        bail!("install record does not verify extension ownership");
    }
    Ok(())
}

fn stream_archive_file_at<R: Read>(
    root: &Dir,
    reader: &mut R,
    path: &Path,
    expected: u64,
) -> Result<String> {
    let mut options = CapOpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = root.open_with(path, &options)?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| anyhow!("archive entry size overflow"))?;
        if copied > expected || copied > MAX_FILE_BYTES {
            bail!("archive entry exceeded its declared or supported size");
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])?;
    }
    if copied != expected {
        bail!("archive entry declared {expected} bytes but contained {copied}");
    }
    file.sync_all()?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_file_at(directory: &Dir, path: &Path, limit: u64) -> Result<String> {
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = directory.open_with(path, &options)?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| anyhow!("file size overflow"))?;
        if total > limit {
            bail!("file {} exceeds the {limit} byte limit", path.display());
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn read_small_file_at(directory: &Dir, path: &Path) -> Result<Vec<u8>> {
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = directory.open_with(path, &options)?;
    let mut bytes = Vec::new();
    file.take(MAX_IN_MEMORY_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_IN_MEMORY_BYTES {
        bail!("file {} exceeds the in-memory limit", path.display());
    }
    Ok(bytes)
}

fn create_private_cap_parent_directories(root: &Dir, relative: &Path) -> Result<()> {
    let mut current = root.try_clone()?;
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            let Component::Normal(segment) = component else {
                bail!("staging destination contains an unsafe parent");
            };
            match current.open_dir_nofollow(segment) {
                Ok(next) => current = next,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    create_private_subdirectory(&current, segment)?;
                    current = current.open_dir_nofollow(segment)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

fn create_private_subdirectory(parent: &Dir, name: &std::ffi::OsStr) -> Result<()> {
    #[cfg(unix)]
    {
        use cap_std::fs::{DirBuilder, DirBuilderExt};
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        parent.create_dir_with(name, &builder)?;
    }
    #[cfg(not(unix))]
    parent.create_dir(name)?;
    sync_cap_dir(parent)
}

fn ensure_private_subdirectory(parent: &Dir, name: &str) -> Result<Dir> {
    match parent.open_dir_nofollow(name) {
        Ok(directory) => {
            verify_cap_directory_permissions_portable(&directory)?;
            Ok(directory)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_subdirectory(parent, std::ffi::OsStr::new(name))?;
            let directory = parent.open_dir_nofollow(name)?;
            verify_cap_directory_permissions_portable(&directory)?;
            Ok(directory)
        }
        Err(error) => Err(error).with_context(|| format!("open private subdirectory {name}")),
    }
}

fn open_private_subdirectory_if_present(parent: &Dir, name: &str) -> Result<Option<Dir>> {
    match parent.open_dir_nofollow(name) {
        Ok(directory) => {
            verify_cap_directory_permissions_portable(&directory)?;
            Ok(Some(directory))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("open private subdirectory {name}")),
    }
}

fn private_regular_file_or_missing_at(directory: &Dir, path: &str) -> Result<bool> {
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    match directory.open_with(path, &options) {
        Ok(file) => {
            let metadata = file.metadata()?;
            if !metadata.is_file() {
                bail!("extension path is not an owned regular file: {path}");
            }
            verify_private_cap_file_permissions(&file, path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("open {path} without following links")),
    }
}

#[cfg(unix)]
fn verify_private_cap_file_permissions(file: &cap_std::fs::File, path: &str) -> Result<()> {
    use cap_std::fs::MetadataExt;
    let metadata = file.metadata()?;
    let mode = metadata.mode() & 0o777;
    if metadata.uid() != rustix::process::geteuid().as_raw()
        || mode & 0o077 != 0
        || mode & 0o600 != 0o600
    {
        bail!("extension file {path} has unsafe ownership or permissions");
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_private_cap_file_permissions(_file: &cap_std::fs::File, _path: &str) -> Result<()> {
    Ok(())
}

fn remove_cap_subdirectory(parent: &Dir, name: &std::ffi::OsStr) -> Result<()> {
    let opened = match parent.open_dir_nofollow(name) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    inspect_owned_tree(&opened)?;
    opened.remove_open_dir_all()?;
    sync_cap_dir(parent)
}

fn inspect_owned_tree(directory: &Dir) -> Result<()> {
    for child in directory.entries()? {
        let child = child?;
        let file_type = child.file_type()?;
        if file_type.is_symlink() {
            bail!(
                "refusing to delete extension directory containing symbolic link {:?}",
                child.file_name()
            );
        }
        if file_type.is_dir() {
            inspect_owned_tree(&directory.open_dir_nofollow(child.file_name())?)?;
        } else if file_type.is_file() {
            let mut options = CapOpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            drop(child.open_with(&options)?);
        } else {
            bail!(
                "refusing to delete extension directory containing special file {:?}",
                child.file_name()
            );
        }
    }
    Ok(())
}

fn apply_manifest_permissions_at(root: &Dir, manifest: &ExtensionManifest) -> Result<()> {
    set_private_file_permissions_at(root, Path::new(MANIFEST_NAME), false)?;
    for file in &manifest.files {
        set_private_file_permissions_at(root, Path::new(&file.path), file.executable)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions_at(root: &Dir, path: &Path, executable: bool) -> Result<()> {
    use cap_std::fs::PermissionsExt;
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = root.open_with(path, &options)?;
    file.set_permissions(cap_std::fs::Permissions::from_mode(if executable {
        0o700
    } else {
        0o600
    }))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions_at(_root: &Dir, _path: &Path, _executable: bool) -> Result<()> {
    Ok(())
}

fn verify_manifest_permissions_at(root: &Dir, manifest: &ExtensionManifest) -> Result<()> {
    verify_expected_file_permissions_at(root, Path::new(MANIFEST_NAME), false)?;
    verify_expected_file_permissions_at(root, Path::new(INSTALL_RECORD_NAME), false)?;
    for file in &manifest.files {
        verify_expected_file_permissions_at(root, Path::new(&file.path), file.executable)?;
    }
    Ok(())
}

#[cfg(unix)]
fn verify_expected_file_permissions_at(root: &Dir, path: &Path, executable: bool) -> Result<()> {
    use cap_std::fs::MetadataExt;
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = root.open_with(path, &options)?;
    let metadata = file.metadata()?;
    let expected = if executable { 0o700 } else { 0o600 };
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o777 != expected
    {
        bail!(
            "extension file {} has unsafe ownership or permissions",
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_expected_file_permissions_at(_root: &Dir, _path: &Path, _executable: bool) -> Result<()> {
    Ok(())
}

fn open_existing_no_follow(path: &Path) -> Result<fs::File> {
    let (dir, name) = cap_parent(path)?;
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    dir.open_with(name, &options)
        .map(cap_std::fs::File::into_std)
        .with_context(|| format!("open {} without following links", path.display()))
}

fn open_lock_file_at(dir: &Dir, name: &str) -> Result<fs::File> {
    let mut create = CapOpenOptions::new();
    create
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        create.mode(0o600);
    }
    match dir.open_with(name, &create) {
        Ok(file) => {
            verify_private_cap_file_permissions(&file, name)?;
            sync_cap_dir(dir)?;
            Ok(file.into_std())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut existing = CapOpenOptions::new();
            existing.read(true).write(true).follow(FollowSymlinks::No);
            let file = dir
                .open_with(name, &existing)
                .with_context(|| format!("open extension lock {name}"))?;
            verify_private_cap_file_permissions(&file, name)?;
            Ok(file.into_std())
        }
        Err(error) => Err(error).with_context(|| format!("create extension lock {name}")),
    }
}

#[cfg(unix)]
fn verify_cap_directory_permissions(directory: &Dir) -> Result<()> {
    use cap_std::fs::MetadataExt;
    let metadata = directory.dir_metadata()?;
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        bail!("extension directory is not owned by the current user");
    }
    let mode = metadata.mode() & 0o777;
    if mode != 0o700 {
        bail!("extension directory has unsafe permissions {mode:o}; expected 700");
    }
    Ok(())
}

fn safe_relative_path(path: &Path) -> Result<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => {
                let segment = segment.to_str().ok_or_else(|| {
                    anyhow!("archive path is not valid UTF-8: {}", path.display())
                })?;
                if segment.contains('\\') {
                    bail!("archive contains non-portable path {}", path.display());
                }
                result.push(segment);
            }
            _ => bail!("archive contains unsafe path {}", path.display()),
        }
    }
    Ok(result)
}

#[cfg(unix)]
fn verify_cap_directory_permissions_portable(directory: &Dir) -> Result<()> {
    verify_cap_directory_permissions(directory)
}

#[cfg(not(unix))]
fn verify_cap_directory_permissions_portable(_directory: &Dir) -> Result<()> {
    Ok(())
}

fn safe_manifest_path(path: &str) -> Result<()> {
    if path.is_empty()
        || safe_relative_path(Path::new(path))?
            .to_string_lossy()
            .replace('\\', "/")
            != path
    {
        bail!("manifest contains unsafe or non-canonical path {path:?}");
    }
    Ok(())
}

fn validate_version_segment(version: &str) -> Result<()> {
    let parsed =
        Version::parse(version).context("active version is not valid semantic versioning")?;
    if parsed.to_string() != version {
        bail!("active version is not canonical semantic versioning");
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unique_name(id: &str) -> String {
    let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{id}-{}-{counter}", std::process::id())
}

fn current_target() -> Result<String> {
    target_triple(
        std::env::consts::OS,
        std::env::consts::ARCH,
        option_env!("CARGO_CFG_TARGET_ENV").unwrap_or(if cfg!(target_env = "musl") {
            "musl"
        } else if cfg!(target_env = "msvc") {
            "msvc"
        } else if cfg!(target_env = "gnu") {
            "gnu"
        } else {
            ""
        }),
    )
}

fn target_triple(os: &str, arch: &str, abi: &str) -> Result<String> {
    let triple = match (os, arch, abi) {
        ("macos", "x86_64" | "aarch64", "") => format!("{arch}-apple-darwin"),
        ("windows", "x86_64" | "aarch64", "msvc") => format!("{arch}-pc-windows-msvc"),
        ("linux", "x86_64" | "aarch64", "gnu") => format!("{arch}-unknown-linux-gnu"),
        ("linux", "x86_64" | "aarch64", "musl") => format!("{arch}-unknown-linux-musl"),
        _ => bail!("extension archives are unsupported on target OS={os} arch={arch} ABI={abi}"),
    };
    Ok(triple)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Cursor;
    use tempfile::TempDir;

    fn fixture_archive(
        directory: &Path,
        version: &str,
        target: &str,
        protocol: u32,
        payload: &[u8],
    ) -> PathBuf {
        fixture_archive_with(
            directory,
            version,
            target,
            protocol,
            payload,
            &hex_sha256(payload),
            &format!("={}", env!("CARGO_PKG_VERSION")),
        )
    }

    fn fixture_archive_with(
        directory: &Path,
        version: &str,
        target: &str,
        protocol: u32,
        payload: &[u8],
        declared_hash: &str,
        driver_version: &str,
    ) -> PathBuf {
        fixture_archive_with_executable(
            directory,
            version,
            target,
            protocol,
            payload,
            declared_hash,
            driver_version,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn fixture_archive_with_executable(
        directory: &Path,
        version: &str,
        target: &str,
        protocol: u32,
        payload: &[u8],
        declared_hash: &str,
        driver_version: &str,
        executable: bool,
    ) -> PathBuf {
        let path = directory.join(format!("local-extension-{version}.tar.gz"));
        let file = fs::File::create(&path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let manifest = ExtensionManifest {
            schema_version: 1,
            id: "local-prototype".to_owned(),
            version: version.to_owned(),
            driver_version: driver_version.to_owned(),
            protocol_version: protocol,
            target: target.to_owned(),
            entrypoint: "bin/local-extension".to_owned(),
            files: vec![ManifestFile {
                path: "bin/local-extension".to_owned(),
                sha256: declared_hash.to_owned(),
                executable,
            }],
        };
        append(
            &mut builder,
            MANIFEST_NAME,
            &serde_json::to_vec_pretty(&manifest).unwrap(),
        );
        append(&mut builder, "bin/local-extension", payload);
        builder.finish().unwrap();
        path
    }

    fn append(builder: &mut tar::Builder<GzEncoder<fs::File>>, path: &str, bytes: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, path, Cursor::new(bytes))
            .unwrap();
    }

    fn unsafe_archive(directory: &Path, kind: &str) -> PathBuf {
        let path = directory.join(format!("unsafe-{kind}.tar.gz"));
        let file = fs::File::create(&path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o644);
        match kind {
            "traversal" => {
                let name = b"../outside";
                header.as_mut_bytes()[..name.len()].copy_from_slice(name);
                header.set_size(1);
                header.set_cksum();
                builder.append(&header, Cursor::new(b"x")).unwrap();
            }
            "symlink" => {
                header.set_path("link").unwrap();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("outside").unwrap();
                header.set_size(0);
                header.set_cksum();
                builder.append(&header, Cursor::new([])).unwrap();
            }
            "pax" => {
                builder
                    .append_pax_extensions([("comment", b"untrusted metadata".as_slice())])
                    .unwrap();
            }
            "long-path" => {
                let path = format!("{}/payload", "a".repeat(MAX_ARCHIVE_PATH_BYTES + 1));
                append(&mut builder, &path, b"x");
            }
            _ => unreachable!(),
        }
        builder.finish().unwrap();
        path
    }

    #[test]
    fn fresh_and_idempotent_fixture_install() {
        let temp = TempDir::new().unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.2.3",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("local-prototype").unwrap();

        let first = store.install_archive(entry, &archive).unwrap();
        let second = store.install_archive(entry, &archive).unwrap();

        assert_eq!(first.path, second.path);
        assert_eq!(
            store.active_path("local-prototype").unwrap(),
            Some(first.path)
        );
        assert!(store.info(entry).unwrap().healthy);
    }

    #[test]
    fn rejects_corrupt_target_and_protocol_mismatches() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("local-prototype").unwrap();

        let wrong_target = fixture_archive(temp.path(), "1.0.0", "wrong-target", 1, b"worker");
        assert!(store
            .install_archive(entry, &wrong_target)
            .unwrap_err()
            .to_string()
            .contains("target"));

        let wrong_protocol = fixture_archive(
            temp.path(),
            "1.0.1",
            &current_target().unwrap(),
            99,
            b"worker",
        );
        assert!(store
            .install_archive(entry, &wrong_protocol)
            .unwrap_err()
            .to_string()
            .contains("protocol"));

        let wrong_driver = fixture_archive_with(
            temp.path(),
            "1.0.2",
            &current_target().unwrap(),
            1,
            b"worker",
            &hex_sha256(b"worker"),
            ">999.0.0",
        );
        assert!(store
            .install_archive(entry, &wrong_driver)
            .unwrap_err()
            .to_string()
            .contains("requires Driver"));

        let wrong_hash = fixture_archive_with(
            temp.path(),
            "1.0.3",
            &current_target().unwrap(),
            1,
            b"worker",
            &"0".repeat(64),
            &format!("={}", env!("CARGO_PKG_VERSION")),
        );
        assert!(store
            .install_archive(entry, &wrong_hash)
            .unwrap_err()
            .to_string()
            .contains("SHA-256 mismatch"));

        let corrupt = fixture_archive(
            temp.path(),
            "1.0.4",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        let bytes = fs::read(&corrupt).unwrap();
        fs::write(&corrupt, &bytes[..bytes.len() / 2]).unwrap();
        assert!(store.install_archive(entry, &corrupt).is_err());
        assert!(store.active_path("local-prototype").unwrap().is_none());
    }

    #[test]
    fn malformed_manifest_version_cleans_staging() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("local-prototype").unwrap();
        let archive = fixture_archive(
            temp.path(),
            "not-a-version",
            &current_target().unwrap(),
            1,
            b"worker",
        );

        assert!(store
            .install_archive(entry, &archive)
            .unwrap_err()
            .to_string()
            .contains("semantic versioning"));
        assert_eq!(
            fs::read_dir(store.root.join(".staging")).unwrap().count(),
            0
        );
    }

    #[test]
    fn rejects_traversal_and_link_entries_and_cleans_staging() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("local-prototype").unwrap();

        for kind in ["traversal", "symlink", "pax", "long-path"] {
            let archive = unsafe_archive(temp.path(), kind);
            assert!(store.install_archive(entry, &archive).is_err(), "{kind}");
        }
        assert_eq!(
            fs::read_dir(store.root.join(".staging")).unwrap().count(),
            0
        );
    }

    #[test]
    fn compressed_reader_stops_at_its_limit() {
        let mut reader = BoundedReader {
            inner: Cursor::new(vec![b'x'; 16]),
            remaining: 4,
        };
        let mut output = Vec::new();
        let error = reader.read_to_end(&mut output).unwrap_err();
        assert_eq!(output.len(), 4);
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn strict_command_grammar_rejects_ambiguous_arguments() {
        for args in [
            vec!["list", "extra"],
            vec!["info", "local-prototype", "extra"],
            vec!["status", "--bogus"],
            vec!["status", "--json", "--json"],
            vec!["install", "local-prototype", "--archive"],
            vec![
                "install",
                "local-prototype",
                "--archive",
                "a",
                "--archive=b",
            ],
            vec!["path", "local-prototype", "--archive=a"],
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(parse_command(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn mutation_platform_contract_is_explicit() {
        #[cfg(windows)]
        assert!(ensure_mutations_supported()
            .unwrap_err()
            .to_string()
            .contains("unsupported on Windows"));
        #[cfg(not(windows))]
        assert!(ensure_mutations_supported().is_ok());
    }

    #[test]
    fn activation_failure_restores_previous_version() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("local-prototype").unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        store.install_archive(entry, &archive).unwrap();

        let lock = store.lock("local-prototype").unwrap();
        let error = store
            .activate_locked(
                &lock.root,
                "local-prototype",
                "2.0.0",
                ActivationFailpoint::AfterBackup,
            )
            .unwrap_err();
        drop(lock);

        assert!(error.to_string().contains("simulated interruption"));
        let lock = store.lock("local-prototype").unwrap();
        assert_eq!(
            store
                .active_pointer_locked(&lock.root, "local-prototype")
                .unwrap()
                .unwrap()
                .version,
            "1.0.0"
        );
    }

    #[test]
    fn recovery_validates_and_restores_interrupted_backup() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("local-prototype").unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        store.install_archive(entry, &archive).unwrap();

        let lock = store.lock(entry.id).unwrap();
        store
            .activate_locked(
                &lock.root,
                entry.id,
                "2.0.0",
                ActivationFailpoint::LeaveInterruptedAfterBackup,
            )
            .unwrap_err();
        drop(lock);

        assert_eq!(
            store.active_path(entry.id).unwrap().unwrap(),
            store.versions_dir(entry.id).join("1.0.0")
        );
        assert!(!store
            .extension_dir(entry.id)
            .join(ACTIVE_BACKUP_NAME)
            .exists());
        assert!(!store.extension_dir(entry.id).join(ACTIVE_NEW_NAME).exists());
    }

    #[test]
    fn absence_is_healthy() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let info = store
            .info(registry_entry("local-prototype").unwrap())
            .unwrap();
        assert!(!info.installed);
        assert!(info.healthy);
        assert_eq!(
            info.detail,
            format!("not installed (healthy); {TRUST_NOTICE}")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_store_components_without_deleting_external_targets() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        for component in [
            "ancestor",
            "root",
            "extension",
            "versions",
            "staging",
            "active",
            "active-backup",
            "active-new",
        ] {
            let temp = TempDir::new().unwrap();
            let external = temp.path().join("external");
            fs::create_dir(&external).unwrap();
            fs::set_permissions(&external, fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(external.join("sentinel"), b"keep").unwrap();
            fs::set_permissions(external.join("sentinel"), fs::Permissions::from_mode(0o600))
                .unwrap();
            let store = if component == "ancestor" {
                let linked_home = temp.path().join("linked-home");
                symlink(&external, &linked_home).unwrap();
                ExtensionStore::new(linked_home.join("extensions"))
            } else {
                ExtensionStore::new(temp.path().join("extensions"))
            };
            let entry = registry_entry("local-prototype").unwrap();
            let archive = fixture_archive(
                temp.path(),
                "1.0.0",
                &current_target().unwrap(),
                1,
                b"worker",
            );

            if !matches!(component, "ancestor" | "root") {
                ensure_private_directory_path(&store.root).unwrap();
            }
            match component {
                "ancestor" => {}
                "root" => symlink(&external, &store.root).unwrap(),
                "extension" => symlink(&external, store.extension_dir(entry.id)).unwrap(),
                "versions" => {
                    ensure_private_directory_path(&store.extension_dir(entry.id)).unwrap();
                    symlink(&external, store.versions_dir(entry.id)).unwrap();
                }
                "staging" => symlink(&external, store.root.join(".staging")).unwrap(),
                "active" | "active-backup" | "active-new" => {
                    store.install_archive(entry, &archive).unwrap();
                    let pointer_name = match component {
                        "active" => ACTIVE_NAME,
                        "active-backup" => ACTIVE_BACKUP_NAME,
                        "active-new" => ACTIVE_NEW_NAME,
                        _ => unreachable!(),
                    };
                    let pointer_path = store.extension_dir(entry.id).join(pointer_name);
                    if pointer_path.exists() {
                        fs::remove_file(&pointer_path).unwrap();
                    }
                    symlink(external.join("sentinel"), pointer_path).unwrap();
                }
                _ => unreachable!(),
            }

            let result = store.install_archive(entry, &archive).map(|_| ());
            assert!(result.is_err(), "{component} symlink was accepted");
            assert_eq!(fs::read(external.join("sentinel")).unwrap(), b"keep");
        }
    }

    #[cfg(unix)]
    #[test]
    fn retained_directory_handle_survives_name_swap_without_touching_replacement() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let root = ensure_private_directory_path(&store.root).unwrap();
        let retained = ensure_private_subdirectory(&root, "slot").unwrap();
        let external = temp.path().join("external");
        fs::create_dir(&external).unwrap();
        fs::set_permissions(&external, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(external.join("sentinel"), b"keep").unwrap();

        root.rename("slot", &root, "slot-original").unwrap();
        sync_cap_dir(&root).unwrap();
        symlink(&external, store.root.join("slot")).unwrap();

        write_new_file_at(&retained, "marker", b"retained").unwrap();
        assert_eq!(
            fs::read(store.root.join("slot-original/marker")).unwrap(),
            b"retained"
        );
        assert!(!external.join("marker").exists());
        assert!(remove_cap_subdirectory(&root, std::ffi::OsStr::new("slot")).is_err());
        assert_eq!(fs::read(external.join("sentinel")).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn installs_owner_only_files_and_requires_executable_entrypoint() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("local-prototype").unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        let installed = store.install_archive(entry, &archive).unwrap();
        assert_eq!(
            fs::symlink_metadata(installed.path.join("bin/local-extension"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::symlink_metadata(installed.path.join(MANIFEST_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let invalid = fixture_archive_with_executable(
            temp.path(),
            "2.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
            &hex_sha256(b"worker"),
            &format!("={}", env!("CARGO_PKG_VERSION")),
            false,
        );
        assert!(store
            .install_archive(entry, &invalid)
            .unwrap_err()
            .to_string()
            .contains("must be executable"));
    }

    #[test]
    fn rejects_unknown_or_unsupported_target_abis() {
        assert!(target_triple("linux", "x86_64", "unknown").is_err());
        assert!(target_triple("macos", "aarch64", "unknown").is_err());
        assert!(target_triple("freebsd", "x86_64", "gnu").is_err());
        assert_eq!(
            target_triple("linux", "aarch64", "musl").unwrap(),
            "aarch64-unknown-linux-musl"
        );
    }
}
