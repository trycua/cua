//! Lifecycle management for optional, registry-known Driver extensions.
//!
//! Installation is deliberately local-only in this first slice. A caller
//! supplies a release-shaped `.tar.gz`; the manager validates the archive in
//! memory before placing an exact version under the Driver home and switching
//! the small active-version pointer.

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use fs2::FileExt;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MANIFEST_NAME: &str = "extension.json";
const INSTALL_RECORD_NAME: &str = ".install.json";
const ACTIVE_NAME: &str = "active.json";
const ACTIVE_BACKUP_NAME: &str = "active.backup.json";
const ACTIVE_NEW_NAME: &str = "active.new.json";
const MANIFEST_SCHEMA_VERSION: u32 = 1;
const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_FILE_COUNT: usize = 10_000;

#[derive(Clone, Copy)]
struct RegistryEntry {
    id: &'static str,
    display_name: &'static str,
    description: &'static str,
    protocol_version: u32,
}

const REGISTRY: &[RegistryEntry] = &[RegistryEntry {
    id: "perception",
    display_name: "Cua Perception",
    description: "Optional model-neutral visual region worker and model bundle.",
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
    bytes: Vec<u8>,
    mode: u32,
}

#[derive(Debug)]
struct InspectedArchive {
    manifest: ExtensionManifest,
    manifest_bytes: Vec<u8>,
    files: Vec<ArchiveFile>,
}

struct ExtensionStore {
    root: PathBuf,
}

struct InstallLock {
    _file: fs::File,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivationFailpoint {
    None,
    AfterBackup,
}

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn run(args: &[String]) {
    if let Err(error) = run_inner(args) {
        eprintln!("cua-driver extension: {error:#}");
        std::process::exit(1);
    }
}

fn run_inner(args: &[String]) -> Result<()> {
    let subcommand = args.first().map(String::as_str).unwrap_or("list");
    if !matches!(
        subcommand,
        "list" | "info" | "status" | "install" | "update" | "uninstall" | "path"
    ) {
        bail!("unknown subcommand {subcommand:?}; expected list, info, status, install, update, uninstall, or path");
    }
    let json = args.iter().any(|arg| arg == "--json");
    let positionals: Vec<&str> = args
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            if index == 0
                || value.starts_with('-')
                || args
                    .get(index.wrapping_sub(1))
                    .is_some_and(|previous| previous == "--archive")
            {
                None
            } else {
                Some(value.as_str())
            }
        })
        .collect();
    let id = positionals.first().copied();
    let store = ExtensionStore::new(extension_root()?);

    match subcommand {
        "list" => print_infos(&store, REGISTRY, json),
        "info" => print_info(&store, registry_entry(required_id(id, "info")?)?, json),
        "status" => match id {
            Some(id) => print_info(&store, registry_entry(id)?, json),
            None => print_infos(&store, REGISTRY, json),
        },
        "install" | "update" => {
            let id = required_id(id, subcommand)?;
            let entry = registry_entry(id)?;
            let archive = flag_value(args, "--archive").ok_or_else(|| {
                anyhow!("{subcommand} requires --archive <path>; verified network download is not implemented in this slice")
            })?;
            let installed = store.install_archive(entry, Path::new(&archive))?;
            println!(
                "{} {} {} at {}",
                if subcommand == "update" {
                    "Updated"
                } else {
                    "Installed"
                },
                id,
                installed.version,
                installed.path.display()
            );
            Ok(())
        }
        "uninstall" => {
            let id = required_id(id, "uninstall")?;
            registry_entry(id)?;
            let removed = store.uninstall(id)?;
            if removed == 0 {
                println!("{id} is not installed");
            } else {
                println!("Uninstalled {id} ({removed} verified version(s) removed)");
            }
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

fn required_id<'a>(id: Option<&'a str>, command: &str) -> Result<&'a str> {
    id.ok_or_else(|| anyhow!("usage: cua-driver extension {command} <name>"))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
        .or_else(|| {
            args.iter()
                .find_map(|arg| arg.strip_prefix(&format!("{flag}=")).map(str::to_owned))
        })
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

    fn recover_activation(&self, id: &str) -> Result<()> {
        let dir = self.extension_dir(id);
        if !verified_directory_or_missing(&dir)? {
            return Ok(());
        }
        let active = dir.join(ACTIVE_NAME);
        let backup = dir.join(ACTIVE_BACKUP_NAME);
        let new = dir.join(ACTIVE_NEW_NAME);
        let mut has_active = verified_regular_file_or_missing(&active)?;
        let mut has_backup = verified_regular_file_or_missing(&backup)?;
        let has_new = verified_regular_file_or_missing(&new)?;
        if !has_active && has_backup {
            fs::rename(&backup, &active)
                .with_context(|| format!("restore interrupted activation for {id}"))?;
            has_active = true;
            has_backup = false;
        }
        if has_active && has_backup {
            fs::remove_file(&backup)
                .with_context(|| format!("remove stale activation backup for {id}"))?;
        }
        if has_new {
            fs::remove_file(&new)
                .with_context(|| format!("remove stale activation candidate for {id}"))?;
        }
        Ok(())
    }

    fn lock(&self, id: &str) -> Result<InstallLock> {
        if let Some(parent) = self.root.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create Driver home {}", parent.display()))?;
        }
        ensure_directory(&self.root)?;
        let lock_dir = self.root.join(".locks");
        ensure_directory(&lock_dir)?;
        let path = lock_dir.join(format!("{id}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open extension lock {}", path.display()))?;
        file.try_lock_exclusive()
            .with_context(|| format!("extension {id} is already being modified"))?;
        Ok(InstallLock { _file: file })
    }

    fn install_archive(&self, entry: &RegistryEntry, archive: &Path) -> Result<InstalledVersion> {
        let _lock = self.lock(entry.id)?;
        self.recover_activation(entry.id)?;
        let inspected = inspect_archive(archive, entry)?;
        let version = Version::parse(&inspected.manifest.version)
            .context("manifest version is not valid semantic versioning")?;
        let version_text = version.to_string();
        ensure_directory(&self.extension_dir(entry.id))?;
        let versions = self.versions_dir(entry.id);
        ensure_directory(&versions)?;
        let destination = versions.join(&version_text);

        if destination.symlink_metadata().is_ok() {
            verify_installed_version(&destination, entry, Some(&inspected.manifest_bytes))?;
        } else {
            let staging_parent = self.root.join(".staging");
            ensure_directory(&staging_parent)?;
            let staging = staging_parent.join(unique_name(entry.id));
            fs::create_dir(&staging)
                .with_context(|| format!("create staging directory {}", staging.display()))?;
            let staged = (|| -> Result<()> {
                write_inspected_archive(&staging, &inspected)?;
                write_install_record(&staging, &inspected)?;
                verify_installed_version(&staging, entry, Some(&inspected.manifest_bytes))?;
                fs::rename(&staging, &destination).with_context(|| {
                    format!("place extension version at {}", destination.display())
                })?;
                Ok(())
            })();
            if staged.is_err() {
                let _ = fs::remove_dir_all(&staging);
            }
            staged?;
        }

        self.activate(entry.id, &version_text, ActivationFailpoint::None)?;
        Ok(InstalledVersion {
            version: version_text,
            path: destination,
        })
    }

    fn activate(&self, id: &str, version: &str, failpoint: ActivationFailpoint) -> Result<()> {
        let dir = self.extension_dir(id);
        ensure_directory(&dir)?;
        let active = dir.join(ACTIVE_NAME);
        let backup = dir.join(ACTIVE_BACKUP_NAME);
        let new = dir.join(ACTIVE_NEW_NAME);
        let pointer = serde_json::to_vec_pretty(&ActivePointer {
            id: id.to_owned(),
            version: version.to_owned(),
        })?;
        write_new_file(&new, &pointer)?;
        let had_active = active.is_file();
        if had_active {
            fs::rename(&active, &backup)
                .context("move current active pointer to rollback backup")?;
        }
        let switched = if failpoint == ActivationFailpoint::AfterBackup {
            Err(anyhow!("simulated interruption after activation backup"))
        } else {
            fs::rename(&new, &active).context("activate inspected extension version")
        };
        if let Err(error) = switched {
            let _ = fs::remove_file(&new);
            if had_active {
                fs::rename(&backup, &active).context("restore previous active extension")?;
            }
            return Err(error);
        }
        if backup.exists() {
            fs::remove_file(&backup).context("remove activation rollback backup")?;
        }
        Ok(())
    }

    fn active_pointer(&self, id: &str) -> Result<Option<ActivePointer>> {
        self.recover_activation(id)?;
        let path = self.extension_dir(id).join(ACTIVE_NAME);
        if !verified_regular_file_or_missing(&path)? {
            return Ok(None);
        }
        let pointer: ActivePointer = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        if pointer.id != id {
            bail!("active pointer for {id} names extension {}", pointer.id);
        }
        validate_version_segment(&pointer.version)?;
        Ok(Some(pointer))
    }

    fn active_path(&self, id: &str) -> Result<Option<PathBuf>> {
        let Some(pointer) = self.active_pointer(id)? else {
            return Ok(None);
        };
        let path = self.versions_dir(id).join(&pointer.version);
        verify_installed_version(&path, registry_entry(id)?, None)?;
        Ok(Some(path))
    }

    fn info<'a>(&self, entry: &'a RegistryEntry) -> Result<ExtensionInfo<'a>> {
        let pointer = match self.active_pointer(entry.id) {
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
                    detail: format!("unhealthy: {error:#}"),
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
                detail: "not installed (healthy)".to_owned(),
            });
        };
        let version = pointer.version.clone();
        let path = self.versions_dir(entry.id).join(&version);
        match verify_installed_version(&path, entry, None) {
            Ok(_) => Ok(ExtensionInfo {
                id: entry.id,
                display_name: entry.display_name,
                description: entry.description,
                protocol_version: entry.protocol_version,
                installed: true,
                active_version: Some(version),
                healthy: true,
                detail: "installed and verified".to_owned(),
            }),
            Err(error) => Ok(ExtensionInfo {
                id: entry.id,
                display_name: entry.display_name,
                description: entry.description,
                protocol_version: entry.protocol_version,
                installed: true,
                active_version: Some(version),
                healthy: false,
                detail: format!("unhealthy: {error:#}"),
            }),
        }
    }

    fn uninstall(&self, id: &str) -> Result<usize> {
        let _lock = self.lock(id)?;
        self.recover_activation(id)?;
        let entry = registry_entry(id)?;
        let extension_dir = self.extension_dir(id);
        let versions = self.versions_dir(id);
        let mut owned = Vec::new();
        if versions.is_dir() {
            for child in fs::read_dir(&versions)? {
                let child = child?;
                let path = child.path();
                if !child.file_type()?.is_dir() {
                    continue;
                }
                let version = child.file_name().to_string_lossy().into_owned();
                if validate_version_segment(&version).is_ok()
                    && verify_installed_version(&path, entry, None).is_ok()
                {
                    owned.push(path);
                }
            }
        }
        let active = extension_dir.join(ACTIVE_NAME);
        if active.exists() {
            let pointer = self.active_pointer(id)?;
            if let Some(pointer) = pointer {
                let expected = versions.join(pointer.version);
                if !owned.iter().any(|path| path == &expected) {
                    bail!("refusing to remove active pointer: its version is not a verified extension-owned directory");
                }
            }
            fs::remove_file(&active).context("remove active extension pointer")?;
        }
        for path in &owned {
            fs::remove_dir_all(path)
                .with_context(|| format!("remove verified extension version {}", path.display()))?;
        }
        let _ = fs::remove_dir(&versions);
        let _ = fs::remove_dir(&extension_dir);
        let staging = self.root.join(".staging");
        let _ = fs::remove_dir(&staging);
        Ok(owned.len())
    }
}

fn inspect_archive(path: &Path, entry: &RegistryEntry) -> Result<InspectedArchive> {
    let file = fs::File::open(path).with_context(|| format!("open archive {}", path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    let mut total = 0_u64;
    for item in archive.entries().context("read extension archive")? {
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
        if !seen.insert(normalized.clone()) {
            bail!("archive contains duplicate entry {normalized}");
        }
        if files.len() >= MAX_FILE_COUNT {
            bail!("archive contains more than {MAX_FILE_COUNT} files");
        }
        let size = item.header().size().context("read archive entry size")?;
        total = total
            .checked_add(size)
            .ok_or_else(|| anyhow!("archive size overflow"))?;
        if total > MAX_ARCHIVE_BYTES {
            bail!(
                "archive expands beyond the {} byte limit",
                MAX_ARCHIVE_BYTES
            );
        }
        let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        item.read_to_end(&mut bytes)
            .with_context(|| format!("read archive entry {normalized}"))?;
        let mode = item.header().mode().unwrap_or(0o644);
        files.push(ArchiveFile {
            path: normalized,
            bytes,
            mode,
        });
    }
    let manifest_bytes = files
        .iter()
        .find(|file| file.path == MANIFEST_NAME)
        .map(|file| file.bytes.clone())
        .ok_or_else(|| anyhow!("archive does not contain {MANIFEST_NAME} at its root"))?;
    let manifest: ExtensionManifest = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parse {MANIFEST_NAME}"))?;
    validate_manifest(&manifest, entry, &files)?;
    Ok(InspectedArchive {
        manifest,
        manifest_bytes,
        files,
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
    let target = current_target();
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
        let digest = hex_sha256(&file.bytes);
        if !digest.eq_ignore_ascii_case(&expected.sha256) {
            bail!(
                "SHA-256 mismatch for {path}: expected {}, got {digest}",
                expected.sha256
            );
        }
    }
    Ok(())
}

fn write_inspected_archive(root: &Path, inspected: &InspectedArchive) -> Result<()> {
    for file in &inspected.files {
        let destination = root.join(Path::new(&file.path));
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        write_new_file(&destination, &file.bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = inspected
                .manifest
                .files
                .iter()
                .find(|manifest_file| manifest_file.path == file.path)
                .is_some_and(|manifest_file| manifest_file.executable);
            let mode = if executable {
                file.mode | 0o111
            } else {
                file.mode & !0o111
            };
            fs::set_permissions(&destination, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

fn write_install_record(root: &Path, inspected: &InspectedArchive) -> Result<()> {
    let record = InstallRecord {
        schema_version: MANIFEST_SCHEMA_VERSION,
        id: inspected.manifest.id.clone(),
        version: inspected.manifest.version.clone(),
        manifest_sha256: hex_sha256(&inspected.manifest_bytes),
    };
    write_new_file(
        &root.join(INSTALL_RECORD_NAME),
        &serde_json::to_vec_pretty(&record)?,
    )
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn verify_installed_version(
    root: &Path,
    entry: &RegistryEntry,
    expected_manifest: Option<&[u8]>,
) -> Result<ExtensionManifest> {
    if !root.is_dir() || root.symlink_metadata()?.file_type().is_symlink() {
        bail!(
            "installed version path is not an owned directory: {}",
            root.display()
        );
    }
    let manifest_path = root.join(MANIFEST_NAME);
    let bytes = fs::read(&manifest_path)
        .with_context(|| format!("read installed manifest {}", manifest_path.display()))?;
    if expected_manifest.is_some_and(|expected| expected != bytes) {
        bail!("version directory already exists with a different manifest");
    }
    let manifest: ExtensionManifest = serde_json::from_slice(&bytes)?;
    let actual_paths = installed_regular_files(root)?;
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
            let path = root.join(&file.path);
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("inspect installed file {}", path.display()))?;
            if !metadata.file_type().is_file() {
                bail!("installed path is not a regular file: {}", path.display());
            }
            let bytes = fs::read(&path)?;
            Ok(ArchiveFile {
                path: file.path.clone(),
                bytes,
                mode: 0,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut all_files = synthetic_files;
    all_files.push(ArchiveFile {
        path: MANIFEST_NAME.to_owned(),
        bytes: bytes.clone(),
        mode: 0,
    });
    validate_manifest(&manifest, entry, &all_files)?;
    verify_install_record(root, entry.id, &manifest.version)?;
    Ok(manifest)
}

fn installed_regular_files(root: &Path) -> Result<BTreeSet<String>> {
    fn visit(base: &Path, directory: &Path, paths: &mut BTreeSet<String>) -> Result<()> {
        for child in fs::read_dir(directory)? {
            let child = child?;
            let path = child.path();
            let file_type = child.file_type()?;
            if file_type.is_symlink() {
                bail!(
                    "installed extension contains a symbolic link: {}",
                    path.display()
                );
            }
            if file_type.is_dir() {
                visit(base, &path, paths)?;
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(base)
                    .expect("visited path remains under extension root")
                    .to_string_lossy()
                    .replace('\\', "/");
                paths.insert(relative);
            } else {
                bail!(
                    "installed extension contains a special file: {}",
                    path.display()
                );
            }
        }
        Ok(())
    }

    let mut paths = BTreeSet::new();
    visit(root, root, &mut paths)?;
    Ok(paths)
}

fn verify_install_record(root: &Path, id: &str, version: &str) -> Result<()> {
    let manifest = fs::read(root.join(MANIFEST_NAME))?;
    let record: InstallRecord = serde_json::from_slice(&fs::read(root.join(INSTALL_RECORD_NAME))?)?;
    if record.schema_version != MANIFEST_SCHEMA_VERSION
        || record.id != id
        || record.version != version
        || record.manifest_sha256 != hex_sha256(&manifest)
    {
        bail!("install record does not verify extension ownership");
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

fn ensure_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => bail!(
            "extension path is not an owned directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(path)
            .with_context(|| format!("create extension directory {}", path.display())),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn verified_directory_or_missing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            Ok(true)
        }
        Ok(_) => bail!(
            "extension path is not an owned directory: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
}

fn verified_regular_file_or_missing(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Ok(true)
        }
        Ok(_) => bail!(
            "extension path is not an owned regular file: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspect {}", path.display())),
    }
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

fn current_target() -> String {
    let arch = std::env::consts::ARCH;
    let suffix = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        "linux" => "unknown-linux-gnu",
        other => other,
    };
    format!("{arch}-{suffix}")
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
        let path = directory.join(format!("perception-{version}.tar.gz"));
        let file = fs::File::create(&path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let manifest = ExtensionManifest {
            schema_version: 1,
            id: "perception".to_owned(),
            version: version.to_owned(),
            driver_version: driver_version.to_owned(),
            protocol_version: protocol,
            target: target.to_owned(),
            entrypoint: "bin/cua-perception".to_owned(),
            files: vec![ManifestFile {
                path: "bin/cua-perception".to_owned(),
                sha256: declared_hash.to_owned(),
                executable: true,
            }],
        };
        append(
            &mut builder,
            MANIFEST_NAME,
            &serde_json::to_vec_pretty(&manifest).unwrap(),
        );
        append(&mut builder, "bin/cua-perception", payload);
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
            _ => unreachable!(),
        }
        builder.finish().unwrap();
        path
    }

    #[test]
    fn fresh_and_idempotent_fixture_install() {
        let temp = TempDir::new().unwrap();
        let archive = fixture_archive(temp.path(), "1.2.3", &current_target(), 1, b"worker");
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("perception").unwrap();

        let first = store.install_archive(entry, &archive).unwrap();
        let second = store.install_archive(entry, &archive).unwrap();

        assert_eq!(first.path, second.path);
        assert_eq!(store.active_path("perception").unwrap(), Some(first.path));
        assert!(store.info(entry).unwrap().healthy);
    }

    #[test]
    fn rejects_corrupt_target_and_protocol_mismatches() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("perception").unwrap();

        let wrong_target = fixture_archive(temp.path(), "1.0.0", "wrong-target", 1, b"worker");
        assert!(store
            .install_archive(entry, &wrong_target)
            .unwrap_err()
            .to_string()
            .contains("target"));

        let wrong_protocol =
            fixture_archive(temp.path(), "1.0.1", &current_target(), 99, b"worker");
        assert!(store
            .install_archive(entry, &wrong_protocol)
            .unwrap_err()
            .to_string()
            .contains("protocol"));

        let wrong_driver = fixture_archive_with(
            temp.path(),
            "1.0.2",
            &current_target(),
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
            &current_target(),
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

        let corrupt = fixture_archive(temp.path(), "1.0.4", &current_target(), 1, b"worker");
        let bytes = fs::read(&corrupt).unwrap();
        fs::write(&corrupt, &bytes[..bytes.len() / 2]).unwrap();
        assert!(store.install_archive(entry, &corrupt).is_err());
        assert!(store.active_path("perception").unwrap().is_none());
    }

    #[test]
    fn rejects_traversal_and_link_entries_before_staging() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("perception").unwrap();

        for kind in ["traversal", "symlink"] {
            let archive = unsafe_archive(temp.path(), kind);
            assert!(store.install_archive(entry, &archive).is_err(), "{kind}");
        }
        assert!(!store.root.join(".staging").exists());
    }

    #[test]
    fn activation_failure_restores_previous_version() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("perception").unwrap();
        let archive = fixture_archive(temp.path(), "1.0.0", &current_target(), 1, b"worker");
        store.install_archive(entry, &archive).unwrap();

        let error = store
            .activate("perception", "2.0.0", ActivationFailpoint::AfterBackup)
            .unwrap_err();

        assert!(error.to_string().contains("simulated interruption"));
        assert_eq!(
            store.active_pointer("perception").unwrap().unwrap().version,
            "1.0.0"
        );
    }

    #[test]
    fn uninstall_preserves_neighboring_files() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("perception").unwrap();
        let archive = fixture_archive(temp.path(), "1.0.0", &current_target(), 1, b"worker");
        store.install_archive(entry, &archive).unwrap();
        let neighbor = store.extension_dir("perception").join("user-notes.txt");
        fs::write(&neighbor, b"keep").unwrap();
        let unowned_version = store.versions_dir("perception").join("9.9.9");
        fs::create_dir_all(&unowned_version).unwrap();
        fs::write(unowned_version.join("user-notes.txt"), b"keep version").unwrap();

        assert_eq!(store.uninstall("perception").unwrap(), 1);
        assert_eq!(fs::read(&neighbor).unwrap(), b"keep");
        assert_eq!(
            fs::read(unowned_version.join("user-notes.txt")).unwrap(),
            b"keep version"
        );
        assert!(store.active_path("perception").unwrap().is_none());
    }

    #[test]
    fn absence_is_healthy() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let info = store.info(registry_entry("perception").unwrap()).unwrap();
        assert!(!info.installed);
        assert!(info.healthy);
        assert_eq!(info.detail, "not installed (healthy)");
    }
}
