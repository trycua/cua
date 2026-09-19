//! Authenticated lifecycle for optional Driver extensions.
//!
//! Normal installs consume a target-specific catalog signed by a pinned
//! publisher and verify the catalog, archive, manifest, payload, and model
//! hashes before activation. An explicitly marked developer mode accepts a
//! local unsigned archive, but records and reports that weaker trust class.

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
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
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
#[cfg(any(windows, test))]
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use cua_driver_core::perception_client::containment::{
    recover_windows_acl_profile_journal, windows_acl_profile_journal_path,
};
use cua_driver_core::perception_client::containment::{
    run_contained_hook, ContainmentLimits, HookOutcome,
};
use cua_driver_core::perception_client::{PerceptionClient, PerceptionWorkerConfig};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};

const MANIFEST_NAME: &str = "extension.json";
const INSTALL_RECORD_NAME: &str = ".install.json";
const ACTIVE_NAME: &str = "active.json";
const ACTIVE_BACKUP_NAME: &str = "active.backup.json";
const ACTIVE_NEW_NAME: &str = "active.new.json";
const TRUST_NAME: &str = ".publisher-trust.json";
const TRUST_NEW_NAME: &str = ".publisher-trust.new.json";
const TRUST_BACKUP_NAME: &str = ".publisher-trust.backup.json";
const MANIFEST_SCHEMA_VERSION: u32 = 1;
const CATALOG_SCHEMA_VERSION: u32 = 1;
// The initial lifecycle targets one executable plus a moderate model bundle.
// These defaults keep disk/memory exposure bounded without claiming support
// for multi-tens-of-gigabytes production model distributions.
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_IN_MEMORY_BYTES: u64 = 1024 * 1024;
const MAX_FILE_COUNT: usize = 4_096;
const MAX_ARCHIVE_PATH_BYTES: usize = 512;
const DEVELOPER_TRUST_NOTICE: &str = "developer-only unsigned local install; it is not publisher-verified and cannot be represented as verified";
#[cfg(feature = "review-trust-root")]
const REVIEW_TRUST_NOTICE: &str = "REVIEW ONLY: locally overridden trust root; never production, release, or publishable evidence";
#[cfg_attr(feature = "review-trust-root", allow(dead_code))]
const VERIFIED_PUBLISHER_ID: &str = "cua";
#[cfg_attr(feature = "review-trust-root", allow(dead_code))]
const VERIFIED_PUBLISHER_NAME: &str = "Cua";
#[cfg(not(feature = "review-trust-root"))]
const VERIFIED_KEY_ID: &str = "cua-extension-ed25519-2026-01";
// The corresponding private key is held outside this repository.
#[cfg(not(feature = "review-trust-root"))]
const VERIFIED_KEY_VALID_FROM_UNIX: u64 = 1_735_689_600; // 2025-01-01
#[cfg(not(feature = "review-trust-root"))]
const VERIFIED_KEY_VALID_UNTIL_UNIX: u64 = 2_082_758_400; // 2036-01-01
#[cfg(any(not(feature = "review-trust-root"), test))]
const VERIFIED_PUBLIC_KEY: [u8; 32] = [
    0x74, 0x1e, 0xc4, 0xff, 0x7e, 0x9f, 0x4d, 0x72, 0xe2, 0x1c, 0xf7, 0xeb, 0xf3, 0x26, 0xb8, 0x8b,
    0x84, 0xc5, 0x6e, 0xcb, 0x14, 0xfe, 0x3a, 0x4e, 0xf7, 0x3a, 0xd2, 0xf2, 0x09, 0x78, 0x6e, 0xc4,
];
#[cfg(feature = "review-trust-root")]
const REVIEW_PUBLISHER_ID: &str = "cua-review-only";
#[cfg(feature = "review-trust-root")]
const REVIEW_PUBLISHER_NAME: &str = "Cua REVIEW ONLY";
#[cfg(feature = "review-trust-root")]
const REVIEW_KEY_ID: &str = "review-only-build-override";
#[cfg(feature = "review-trust-root")]
const REVIEW_PUBLIC_KEY_BASE64: &str = env!(
    "CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64",
    "review-trust-root requires CUA_DRIVER_REVIEW_EXTENSION_PUBLIC_KEY_BASE64 at build time"
);
const PERCEPTION_ID: &str = "cua-perception";
const PERCEPTION_RUNTIME_CONTRACT: &str = "metadata/runtime-contract.json";
const PERCEPTION_MODEL_MANIFEST_NAME: &str = "model-manifest.json";
#[cfg(any(windows, test))]
const WINDOWS_ACL_LEASE_RETRY_INTERVAL: Duration = Duration::from_millis(50);

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy)]
struct WindowsAclDrift;

#[cfg(any(windows, test))]
impl std::fmt::Display for WindowsAclDrift {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("extension Windows ACL was modified after hardening")
    }
}

#[cfg(any(windows, test))]
impl std::error::Error for WindowsAclDrift {}

#[cfg(windows)]
const WINDOWS_ACL_CONVERGENCE_ATTEMPTS: u32 = 3;
#[cfg(windows)]
const WINDOWS_ACL_CONVERGENCE_RETRY_WINDOW: Duration = Duration::from_secs(90);

#[derive(Clone, Copy)]
struct RegistryEntry {
    id: &'static str,
    display_name: &'static str,
    description: &'static str,
    protocol_version: u32,
}

const REGISTRY: &[RegistryEntry] = &[RegistryEntry {
    id: "cua-perception",
    display_name: "Cua Perception",
    description: "Optional signed perception worker and target-specific model bundle.",
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
    models: Vec<ManifestModel>,
    components: Vec<ComponentLicense>,
    #[serde(default)]
    corresponding_source_file: Option<ArtifactBinding>,
    license: String,
    source: String,
    corresponding_source_uri: String,
    corresponding_source_revision: String,
    provenance: String,
    #[serde(default)]
    health_args: Vec<String>,
    #[serde(default)]
    self_test_args: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PerceptionRuntimeContract {
    #[serde(rename = "$schema")]
    schema: Option<String>,
    schema_version: u32,
    target: String,
    protocol_version: u32,
    worker: RuntimeBinding,
    runtime: RuntimeBinding,
    models: Vec<RuntimeBinding>,
    dictionary: RuntimeBinding,
    reject_mismatch: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeBinding {
    name: String,
    #[serde(default)]
    role: Option<String>,
    sha256: String,
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
struct ManifestModel {
    path: String,
    revision: String,
    original_sha256: String,
    conversion_sha256: String,
    #[serde(default)]
    license_file: Option<ArtifactBinding>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComponentLicense {
    name: String,
    version: String,
    license: String,
    notice: String,
    source_uri: String,
    source_revision: String,
    #[serde(default)]
    notice_file: Option<ArtifactBinding>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ArtifactBinding {
    path: String,
    sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedCatalog {
    payload: CatalogPayload,
    signature_algorithm: String,
    signature: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PublisherKey {
    key_id: String,
    public_key_base64: String,
    valid_from_unix: u64,
    valid_until_unix: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PublisherTrust {
    schema_version: u32,
    publisher_id: String,
    generation: u64,
    highest_catalog_version: u64,
    current_key: PublisherKey,
    pending_key: Option<PublisherKey>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CatalogPayload {
    schema_version: u32,
    catalog_version: u64,
    expires_unix: u64,
    publisher_id: String,
    publisher_name: String,
    key_id: String,
    extension_id: String,
    version: String,
    target: String,
    archive: String,
    archive_size: u64,
    archive_sha256: String,
    manifest_sha256: String,
    license: String,
    source: String,
    corresponding_source_uri: String,
    corresponding_source_revision: String,
    provenance: String,
    #[serde(default)]
    next_key: Option<PublisherKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum TrustClass {
    PublisherVerified,
    #[cfg(feature = "review-trust-root")]
    ReviewOnlyPublisherVerified,
    DeveloperUnsignedLocal,
}

#[derive(Debug, Clone)]
struct InstallSource {
    archive: PathBuf,
    trust: TrustClass,
    catalog: Option<CatalogPayload>,
    signed_catalog: Option<SignedCatalog>,
    trust_update: Option<PublisherTrust>,
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
    trust: TrustClass,
    publisher_id: Option<String>,
    key_id: Option<String>,
    archive_sha256: String,
    catalog_version: Option<u64>,
    #[serde(default)]
    publisher_trust: Option<PublisherTrust>,
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
    trust: Option<TrustClass>,
    evidence_class: Option<&'static str>,
    publisher_id: Option<String>,
    publisher_key_id: Option<String>,
    catalog_version: Option<u64>,
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
    archive_sha256: String,
    archive_size: u64,
    expanded_size: u64,
}

#[derive(Debug, Serialize)]
struct InstallReview {
    extension: &'static str,
    id: String,
    name: String,
    artifact: String,
    backend: &'static str,
    acceleration: &'static str,
    version: String,
    target: String,
    publisher: Option<String>,
    publisher_id: Option<String>,
    publisher_name: Option<String>,
    publisher_key_id: Option<String>,
    signing_key_algorithm: Option<String>,
    signing_key_status: &'static str,
    publisher_signature_verified: bool,
    catalog_version: Option<u64>,
    catalog_expires_unix: Option<u64>,
    trust: TrustClass,
    evidence_class: &'static str,
    artifact_source: String,
    destination: String,
    download_size: u64,
    installed_size: u64,
    archive_sha256: String,
    manifest_sha256: String,
    license: String,
    license_notices: Vec<LicenseNoticeReview>,
    model_licenses: Vec<ModelLicenseReview>,
    source: String,
    corresponding_source: CorrespondingSourceReview,
    corresponding_source_uri: String,
    corresponding_source_revision: String,
    provenance: String,
    files: Vec<ManifestFile>,
    models: Vec<ManifestModel>,
    components: Vec<ComponentLicense>,
    authorization: InstallAuthorizationReview,
    mutation_performed: bool,
    installed: bool,
    ran: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    installed_version: Option<String>,
}

#[derive(Debug, Serialize)]
struct LicenseNoticeReview {
    component: String,
    license: String,
    file: Option<ArtifactBinding>,
}

#[derive(Debug, Serialize)]
struct ModelLicenseReview {
    model: String,
    file: Option<ArtifactBinding>,
}

#[derive(Debug, Serialize)]
struct CorrespondingSourceReview {
    file: Option<ArtifactBinding>,
    uri: String,
    revision: String,
}

#[derive(Debug, Serialize)]
struct InstallAuthorizationReview {
    request: &'static str,
    confirmation_required: bool,
    confirmation_received: bool,
    mutation_authorized: bool,
    mutation_performed: bool,
}

#[derive(Clone, Copy)]
enum InstallReviewRequest {
    CliInspect,
    CliInstall,
    CliUpdate,
    Mcp { confirmed: bool },
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
    #[cfg(windows)]
    _windows_acl_file: fs::File,
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
    catalog: Option<PathBuf>,
    allow_unsigned_local: bool,
    self_test: bool,
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
        #[cfg(windows)]
        windows_reject_reparse_directory(&directory)?;
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
        #[cfg(windows)]
        windows_reject_reparse_directory(&directory)?;
    }
    #[cfg(windows)]
    windows_secure_handle(&directory)?;
    verify_cap_directory_permissions_portable(&directory)?;
    Ok(directory)
}

fn sync_cap_dir(dir: &Dir) -> Result<()> {
    #[cfg(unix)]
    {
        let mut options = CapOpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        dir.open_with(".", &options)?
            .sync_all()
            .context("sync containing directory")?;
    }
    Ok(())
}

pub fn run(args: &[String]) {
    if let Err(error) = run_inner(args) {
        eprintln!("cua-driver extension: {error:#}");
        std::process::exit(1);
    }
}

pub(crate) fn register_host_tools(registry: &mut ToolRegistry) {
    registry.register(Box::new(InstallExtensionTool));
}

struct InstallExtensionTool;
static INSTALL_EXTENSION_DEF: OnceLock<ToolDef> = OnceLock::new();

#[async_trait::async_trait]
impl Tool for InstallExtensionTool {
    fn def(&self) -> &ToolDef {
        INSTALL_EXTENSION_DEF.get_or_init(|| ToolDef {
            name: "install_extension".to_owned(),
            description: "Preview or install one Driver-managed optional extension. The first call without confirm returns the exact signed artifact, destination, license, source, and trust plan without mutation. Re-call with confirm=true to perform that exact verified installation.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "enum": ["perception"]},
                    "confirm": {"type": "boolean", "description": "Install the previewed extension. Omit or false for a read-only plan."}
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        })
    }

    async fn invoke(&self, args: serde_json::Value) -> ToolResult {
        let confirmed = args.get("confirm").and_then(serde_json::Value::as_bool) == Some(true);
        if args.get("name").and_then(serde_json::Value::as_str) != Some("perception") {
            return ToolResult::error("install_extension requires name=\"perception\"");
        }
        match tokio::task::spawn_blocking(move || install_extension_from_mcp(confirmed)).await {
            Ok(Ok((message, structured))) => ToolResult::text(message).with_structured(structured),
            Ok(Err(error)) => ToolResult::error(format!("install_extension failed: {error:#}")),
            Err(error) => ToolResult::error(format!("install_extension task failed: {error}")),
        }
    }
}

fn install_extension_from_mcp(confirmed: bool) -> Result<(String, serde_json::Value)> {
    let catalog = std::env::var_os("CUA_DRIVER_PERCEPTION_CATALOG")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("CUA_DRIVER_PERCEPTION_CATALOG must name the reviewed signed catalog at daemon launch"))?;
    let entry = registry_entry("perception")?;
    let store = ExtensionStore::new(extension_root()?);
    let parsed = ParsedCommand {
        subcommand: "install".to_owned(),
        id: Some("perception".to_owned()),
        archive: None,
        catalog: Some(catalog),
        allow_unsigned_local: false,
        self_test: false,
        json: false,
    };
    let source = resolve_install_source(entry, &parsed, &store)?;
    let inspected = inspect_without_mutation(entry, &source)?;
    let mut review = install_review(
        entry,
        &store,
        &inspected,
        &source,
        InstallReviewRequest::Mcp { confirmed },
    );
    if !confirmed {
        return Ok((
            "Perception extension plan verified. Review the structured plan, then re-call install_extension with name=\"perception\" and confirm=true.".to_owned(),
            serde_json::to_value(review)?,
        ));
    }
    if store.active_path(entry.id)?.is_some() {
        bail!("perception is already installed; use the CLI extension update command with a reviewed catalog");
    }
    let installed = store.install_source(entry, &source, false)?;
    review.authorization.mutation_performed = true;
    review.mutation_performed = true;
    review.installed = true;
    review.ran = true;
    review.installed_version = Some(installed.version);
    Ok((
        "Perception extension installed from the verified plan.".to_owned(),
        serde_json::to_value(review)?,
    ))
}

fn install_review(
    entry: &RegistryEntry,
    store: &ExtensionStore,
    inspected: &InspectedArchive,
    source: &InstallSource,
    request: InstallReviewRequest,
) -> InstallReview {
    let manifest = &inspected.manifest;
    let catalog = source.catalog.as_ref();
    let license_notices = manifest
        .components
        .iter()
        .map(|component| LicenseNoticeReview {
            component: component.name.clone(),
            license: component.license.clone(),
            file: component.notice_file.clone(),
        })
        .collect::<Vec<_>>();
    let model_licenses = manifest
        .models
        .iter()
        .map(|model| ModelLicenseReview {
            model: model.path.clone(),
            file: model.license_file.clone(),
        })
        .collect::<Vec<_>>();
    let (request_name, confirmation_required, confirmation_received, mutation_authorized) =
        match request {
            InstallReviewRequest::CliInspect => ("cli-inspect", false, false, false),
            InstallReviewRequest::CliInstall => ("cli-install", false, true, true),
            InstallReviewRequest::CliUpdate => ("cli-update", false, true, true),
            InstallReviewRequest::Mcp { confirmed } => ("mcp-install", true, confirmed, confirmed),
        };
    InstallReview {
        extension: "perception",
        id: entry.id.to_owned(),
        name: entry.display_name.to_owned(),
        artifact: format!("{} {}", entry.id, manifest.version),
        backend: "icon detection and OCR",
        acceleration: "CPU",
        version: manifest.version.clone(),
        target: manifest.target.clone(),
        publisher: catalog.map(|catalog| catalog.publisher_name.clone()),
        publisher_id: catalog.map(|catalog| catalog.publisher_id.clone()),
        publisher_name: catalog.map(|catalog| catalog.publisher_name.clone()),
        publisher_key_id: catalog.map(|catalog| catalog.key_id.clone()),
        signing_key_algorithm: source
            .signed_catalog
            .as_ref()
            .map(|catalog| catalog.signature_algorithm.clone()),
        signing_key_status: if catalog.is_some() {
            "verified"
        } else {
            "unsigned-local"
        },
        publisher_signature_verified: catalog.is_some(),
        catalog_version: catalog.map(|catalog| catalog.catalog_version),
        catalog_expires_unix: catalog.map(|catalog| catalog.expires_unix),
        trust: source.trust.clone(),
        evidence_class: evidence_class(&source.trust),
        artifact_source: source.archive.display().to_string(),
        destination: store
            .versions_dir(entry.id)
            .join(&manifest.version)
            .display()
            .to_string(),
        download_size: inspected.archive_size,
        installed_size: inspected.expanded_size,
        archive_sha256: inspected.archive_sha256.clone(),
        manifest_sha256: hex_sha256(&inspected.manifest_bytes),
        license: manifest.license.clone(),
        license_notices,
        model_licenses,
        source: manifest.source.clone(),
        corresponding_source: CorrespondingSourceReview {
            file: manifest.corresponding_source_file.clone(),
            uri: manifest.corresponding_source_uri.clone(),
            revision: manifest.corresponding_source_revision.clone(),
        },
        corresponding_source_uri: manifest.corresponding_source_uri.clone(),
        corresponding_source_revision: manifest.corresponding_source_revision.clone(),
        provenance: manifest.provenance.clone(),
        files: manifest.files.clone(),
        models: manifest.models.clone(),
        components: manifest.components.clone(),
        authorization: InstallAuthorizationReview {
            request: request_name,
            confirmation_required,
            confirmation_received,
            mutation_authorized,
            mutation_performed: false,
        },
        mutation_performed: false,
        installed: false,
        ran: false,
        installed_version: None,
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
        "info" => print_info(
            &store,
            registry_entry(required_id(id, "info")?)?,
            false,
            json,
        ),
        "inspect" => {
            let entry = registry_entry(required_id(id, "inspect")?)?;
            let source = resolve_install_source(entry, &parsed, &store)?;
            let inspected = inspect_without_mutation(entry, &source)?;
            print_preview(
                &install_review(
                    entry,
                    &store,
                    &inspected,
                    &source,
                    InstallReviewRequest::CliInspect,
                ),
                json,
            )
        }
        "status" => match id {
            Some(id) => print_info(&store, registry_entry(id)?, parsed.self_test, json),
            None => print_infos(&store, REGISTRY, json),
        },
        "install" | "update" => {
            ensure_mutations_supported()?;
            let id = required_id(id, subcommand)?;
            let entry = registry_entry(id)?;
            let source = resolve_install_source(entry, &parsed, &store)?;
            let inspected = inspect_without_mutation(entry, &source)?;
            let request = if subcommand == "update" {
                InstallReviewRequest::CliUpdate
            } else {
                InstallReviewRequest::CliInstall
            };
            print_preview(
                &install_review(entry, &store, &inspected, &source, request),
                false,
            )?;
            let installed = store.install_source(entry, &source, subcommand == "update")?;
            println!(
                "{} {} {} at {} ({})",
                if subcommand == "update" {
                    "Updated"
                } else {
                    "Installed"
                },
                id,
                installed.version,
                installed.path.display(),
                trust_label(&source.trust),
            );
            Ok(())
        }
        "remove" => {
            ensure_mutations_supported()?;
            let id = required_id(id, "remove")?;
            let entry = registry_entry(id)?;
            store.remove(entry)?;
            println!("Removed {id}");
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
        "list" | "info" | "inspect" | "status" | "install" | "update" | "remove" | "path"
    ) {
        bail!("unknown subcommand {subcommand:?}; expected list, inspect, status, install, update, or remove");
    }
    let mut id = None;
    let mut archive = None;
    let mut catalog = None;
    let mut allow_unsigned_local = false;
    let mut self_test = false;
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
            "--catalog" => {
                if catalog.is_some() {
                    bail!("duplicate option --catalog");
                }
                let next = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow!("--catalog requires a value"))?;
                if next.starts_with('-') {
                    bail!("--catalog requires a value");
                }
                catalog = Some(PathBuf::from(next));
                index += 2;
            }
            "--allow-unsigned-local" => {
                if allow_unsigned_local {
                    bail!("duplicate flag --allow-unsigned-local");
                }
                allow_unsigned_local = true;
                index += 1;
            }
            "--self-test" => {
                if self_test {
                    bail!("duplicate flag --self-test");
                }
                self_test = true;
                index += 1;
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
            _ if value.starts_with("--catalog=") => {
                if catalog.is_some() {
                    bail!("duplicate option --catalog");
                }
                let path = value.trim_start_matches("--catalog=");
                if path.is_empty() {
                    bail!("--catalog requires a value");
                }
                catalog = Some(PathBuf::from(path));
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
        "info" | "inspect" | "install" | "update" | "remove" | "path" if id.is_none() => {
            bail!("{subcommand} requires an extension name")
        }
        _ => {}
    }
    if !matches!(subcommand, "inspect" | "install" | "update")
        && (archive.is_some() || catalog.is_some() || allow_unsigned_local)
    {
        bail!("source options are only valid with inspect, install, or update");
    }
    if !matches!(subcommand, "list" | "info" | "inspect" | "status") && json {
        bail!("--json is not valid with {subcommand}");
    }
    if self_test && subcommand != "status" {
        bail!("--self-test is only valid with status");
    }
    if self_test && id.is_none() {
        bail!("status --self-test requires an extension name");
    }
    if allow_unsigned_local && catalog.is_some() {
        bail!("--allow-unsigned-local cannot be combined with --catalog");
    }
    Ok(ParsedCommand {
        subcommand: subcommand.to_owned(),
        id,
        archive,
        catalog,
        allow_unsigned_local,
        self_test,
        json,
    })
}

fn ensure_mutations_supported() -> Result<()> {
    Ok(())
}

fn required_id<'a>(id: Option<&'a str>, command: &str) -> Result<&'a str> {
    id.ok_or_else(|| anyhow!("usage: cua-driver extension {command} <name>"))
}

fn registry_entry(id: &str) -> Result<&'static RegistryEntry> {
    let id = match id {
        "perception" => PERCEPTION_ID,
        other => other,
    };
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

/// Resolve the optional perception worker through the same active-pointer and
/// installed-payload verification used by extension lifecycle commands.
pub(crate) fn perception_client() -> PerceptionClient {
    match perception_worker_config() {
        Ok(Some(config)) => PerceptionClient::new(config).unwrap_or_else(|error| {
            tracing::warn!("installed cua-perception configuration is unusable: {error:?}");
            PerceptionClient::unavailable()
        }),
        Ok(None) => PerceptionClient::unavailable(),
        Err(error) => {
            tracing::warn!("installed cua-perception extension is unavailable: {error:#}");
            PerceptionClient::unavailable()
        }
    }
}

pub(crate) fn perception_client_for_cli() -> Result<PerceptionClient> {
    match perception_worker_config()? {
        Some(config) => PerceptionClient::new(config).map_err(|error| {
            anyhow!(serde_json::to_string(&error).unwrap_or_else(|_| error.message.clone()))
        }),
        None => Ok(PerceptionClient::unavailable()),
    }
}

fn perception_worker_config() -> Result<Option<PerceptionWorkerConfig>> {
    perception_worker_config_in(&ExtensionStore::new(extension_root()?))
}

fn perception_worker_config_in(store: &ExtensionStore) -> Result<Option<PerceptionWorkerConfig>> {
    if !store.root.exists() {
        return Ok(None);
    }
    let Some((active, manifest)) = store.active_path_for_startup(PERCEPTION_ID)? else {
        return Ok(None);
    };

    let contract_path = resolve_owned_file(&active, &manifest, PERCEPTION_RUNTIME_CONTRACT)?;
    let contract: PerceptionRuntimeContract =
        serde_json::from_slice(&read_small_local_file(&contract_path)?)
            .context("parse perception runtime contract")?;
    validate_runtime_contract(&manifest, &contract)?;

    let executable = resolve_owned_file(&active, &manifest, &manifest.entrypoint)?;
    let model_manifest_relative = unique_model_manifest_path(&manifest)?;
    let model_manifest = resolve_owned_file(&active, &manifest, model_manifest_relative)?;
    let runtime_relative = format!("runtime/{}", contract.runtime.name);
    let runtime_library = resolve_owned_file(&active, &manifest, &runtime_relative)?;
    let model_manifest = model_manifest
        .to_str()
        .ok_or_else(|| anyhow!("perception model manifest path is not valid UTF-8"))?;
    let runtime_library = runtime_library
        .to_str()
        .ok_or_else(|| anyhow!("perception runtime library path is not valid UTF-8"))?;

    let mut config = PerceptionWorkerConfig::installed_with_identity(
        executable,
        manifest.id.clone(),
        manifest.version.clone(),
    );
    #[cfg(windows)]
    {
        config = config.with_windows_acl_lease_path(store.windows_acl_lease_path(PERCEPTION_ID));
    }
    config.args = vec![
        "--manifest".to_owned(),
        model_manifest.to_owned(),
        "--onnx-runtime-library".to_owned(),
        runtime_library.to_owned(),
        "--extension-id".to_owned(),
        manifest.id.clone(),
        "--extension-version".to_owned(),
        manifest.version.clone(),
    ];
    Ok(Some(config))
}

fn validate_runtime_contract(
    manifest: &ExtensionManifest,
    contract: &PerceptionRuntimeContract,
) -> Result<()> {
    let _ = &contract.schema;
    if contract.schema_version != 1
        || contract.target != manifest.target
        || contract.protocol_version != manifest.protocol_version
        || !contract.reject_mismatch
    {
        bail!("perception runtime contract does not match the active extension");
    }
    let worker_name = Path::new(&manifest.entrypoint)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("perception entrypoint has no portable file name"))?;
    if contract.worker.name != worker_name {
        bail!("perception runtime contract names a different worker");
    }
    if contract.runtime.name.is_empty()
        || Path::new(&contract.runtime.name)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(contract.runtime.name.as_str())
    {
        bail!("perception runtime contract contains an unsafe runtime name");
    }
    validate_runtime_binding(manifest, &manifest.entrypoint, &contract.worker)?;
    validate_runtime_binding(
        manifest,
        &format!("runtime/{}", contract.runtime.name),
        &contract.runtime,
    )?;
    for model in &contract.models {
        validate_runtime_binding(manifest, &format!("models/{}", model.name), model)?;
    }
    validate_runtime_binding(
        manifest,
        &format!("models/{}", contract.dictionary.name),
        &contract.dictionary,
    )?;
    let entrypoint = manifest
        .files
        .iter()
        .find(|file| file.path == manifest.entrypoint)
        .ok_or_else(|| anyhow!("perception entrypoint is not extension-owned"))?;
    if !entrypoint.executable {
        bail!("perception entrypoint is not marked executable");
    }
    Ok(())
}

fn validate_runtime_binding(
    manifest: &ExtensionManifest,
    relative: &str,
    binding: &RuntimeBinding,
) -> Result<()> {
    safe_manifest_path(relative)?;
    validate_sha256("perception runtime binding", &binding.sha256)?;
    if binding.role.as_deref().is_some_and(str::is_empty) {
        bail!("perception runtime binding has an empty role");
    }
    let owned = manifest
        .files
        .iter()
        .find(|file| file.path == relative)
        .ok_or_else(|| anyhow!("perception runtime binding is not extension-owned: {relative}"))?;
    if owned.sha256 != binding.sha256 {
        bail!("perception runtime binding hash differs from the extension manifest");
    }
    Ok(())
}

fn unique_model_manifest_path(manifest: &ExtensionManifest) -> Result<&str> {
    let mut candidates = manifest.files.iter().filter(|file| {
        Path::new(&file.path)
            .file_name()
            .and_then(|name| name.to_str())
            == Some(PERCEPTION_MODEL_MANIFEST_NAME)
    });
    let candidate = candidates
        .next()
        .ok_or_else(|| anyhow!("installed perception extension has no model manifest"))?;
    if candidates.next().is_some() {
        bail!("installed perception extension has ambiguous model manifests");
    }
    Ok(&candidate.path)
}

fn resolve_owned_file(
    active: &Path,
    manifest: &ExtensionManifest,
    relative: &str,
) -> Result<PathBuf> {
    safe_manifest_path(relative)?;
    if !manifest.files.iter().any(|file| file.path == relative) {
        bail!("perception runtime path is not extension-owned: {relative}");
    }
    let path = active.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("inspect installed perception file {relative}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("installed perception path is not a regular non-symlink file: {relative}");
    }
    let canonical_root = fs::canonicalize(active).context("resolve active perception directory")?;
    let canonical_path = fs::canonicalize(&path)
        .with_context(|| format!("resolve installed perception file {relative}"))?;
    if !canonical_path.starts_with(&canonical_root) {
        bail!("installed perception path escapes the active extension: {relative}");
    }
    Ok(canonical_path)
}

fn print_infos(store: &ExtensionStore, entries: &[RegistryEntry], json: bool) -> Result<()> {
    let infos = entries
        .iter()
        .map(|entry| store.info(entry, false))
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

fn print_info(
    store: &ExtensionStore,
    entry: &RegistryEntry,
    self_test: bool,
    json: bool,
) -> Result<()> {
    let info = store.info(entry, self_test)?;
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

fn trust_label(trust: &TrustClass) -> &'static str {
    match trust {
        TrustClass::PublisherVerified => "publisher-verified",
        #[cfg(feature = "review-trust-root")]
        TrustClass::ReviewOnlyPublisherVerified => "review-only-publisher-verified",
        TrustClass::DeveloperUnsignedLocal => "developer-unsigned-local",
    }
}

fn evidence_class(trust: &TrustClass) -> &'static str {
    match trust {
        TrustClass::PublisherVerified => "production-publisher-verified",
        #[cfg(feature = "review-trust-root")]
        TrustClass::ReviewOnlyPublisherVerified => "review-only-not-release-evidence",
        TrustClass::DeveloperUnsignedLocal => "developer-local-not-publisher-evidence",
    }
}

fn expected_publisher_id() -> &'static str {
    #[cfg(feature = "review-trust-root")]
    {
        REVIEW_PUBLISHER_ID
    }
    #[cfg(not(feature = "review-trust-root"))]
    {
        VERIFIED_PUBLISHER_ID
    }
}

fn expected_publisher_name() -> &'static str {
    #[cfg(feature = "review-trust-root")]
    {
        REVIEW_PUBLISHER_NAME
    }
    #[cfg(not(feature = "review-trust-root"))]
    {
        VERIFIED_PUBLISHER_NAME
    }
}

fn verified_trust_class() -> TrustClass {
    #[cfg(feature = "review-trust-root")]
    {
        TrustClass::ReviewOnlyPublisherVerified
    }
    #[cfg(not(feature = "review-trust-root"))]
    {
        TrustClass::PublisherVerified
    }
}

fn resolve_install_source(
    entry: &RegistryEntry,
    parsed: &ParsedCommand,
    store: &ExtensionStore,
) -> Result<InstallSource> {
    if parsed.allow_unsigned_local {
        let archive = parsed.archive.clone().ok_or_else(|| {
            anyhow!("developer mode requires --archive <path> with --allow-unsigned-local")
        })?;
        return Ok(InstallSource {
            archive,
            trust: TrustClass::DeveloperUnsignedLocal,
            catalog: None,
            signed_catalog: None,
            trust_update: None,
        });
    }
    if parsed.archive.is_some() {
        bail!("unsigned archives require the explicit developer-only --allow-unsigned-local flag");
    }
    let catalog_path = parsed
        .catalog
        .as_deref()
        .ok_or_else(|| anyhow!("verified installs require --catalog <signed-catalog.json>"))?;
    let bytes = read_small_local_file(catalog_path)?;
    let signed: SignedCatalog =
        serde_json::from_slice(&bytes).context("parse signed extension catalog")?;
    let trust = store.load_publisher_trust()?;
    let trust_update = verify_catalog_at(entry, &signed, &trust, unix_now()?)?;
    let archive_rel = safe_relative_path(Path::new(&signed.payload.archive))?;
    if archive_rel.as_os_str().is_empty() || archive_rel != Path::new(&signed.payload.archive) {
        bail!("catalog archive path must be canonical and relative");
    }
    let archive = catalog_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(archive_rel);
    Ok(InstallSource {
        archive,
        trust: verified_trust_class(),
        catalog: Some(signed.payload.clone()),
        signed_catalog: Some(signed),
        trust_update: Some(trust_update),
    })
}

fn verify_catalog_at(
    entry: &RegistryEntry,
    signed: &SignedCatalog,
    trust: &PublisherTrust,
    now: u64,
) -> Result<PublisherTrust> {
    validate_publisher_trust(trust)?;
    let payload = &signed.payload;
    if signed.signature_algorithm != "ed25519" {
        bail!("unsupported catalog signature algorithm");
    }
    if payload.schema_version != CATALOG_SCHEMA_VERSION {
        bail!("unsupported catalog schema {}", payload.schema_version);
    }
    if payload.catalog_version == 0 {
        bail!("catalog version must be greater than zero");
    }
    validate_catalog_freshness_at(payload, now)?;
    if payload.publisher_id != expected_publisher_id()
        || payload.publisher_name != expected_publisher_name()
        || trust.publisher_id != expected_publisher_id()
    {
        bail!("catalog publisher identity is not trusted");
    }
    if payload.catalog_version <= trust.highest_catalog_version {
        bail!(
            "catalog anti-rollback version {} must be newer than trusted version {}",
            payload.catalog_version,
            trust.highest_catalog_version
        );
    }
    if payload.extension_id != entry.id {
        bail!("catalog extension id does not match requested extension");
    }
    if payload.target != current_target()? {
        bail!("catalog target does not match the current target");
    }
    validate_sha256("archive", &payload.archive_sha256)?;
    validate_sha256("manifest", &payload.manifest_sha256)?;
    if payload.archive_size > MAX_ARCHIVE_BYTES {
        bail!("catalog archive size exceeds the supported limit");
    }
    Version::parse(&payload.version).context("catalog version is not semantic versioning")?;
    for (name, value) in [
        ("license", payload.license.as_str()),
        ("source", payload.source.as_str()),
        (
            "corresponding_source_uri",
            payload.corresponding_source_uri.as_str(),
        ),
        (
            "corresponding_source_revision",
            payload.corresponding_source_revision.as_str(),
        ),
        ("provenance", payload.provenance.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("catalog {name} must not be empty");
        }
    }
    let (signing_key, promote_pending) = if payload.key_id == trust.current_key.key_id {
        (&trust.current_key, false)
    } else if trust
        .pending_key
        .as_ref()
        .is_some_and(|key| key.key_id == payload.key_id)
    {
        (
            trust.pending_key.as_ref().expect("checked pending key"),
            true,
        )
    } else {
        bail!("catalog signing key is unknown or has been retired");
    };
    validate_publisher_key(signing_key)?;
    if now < signing_key.valid_from_unix {
        bail!("catalog signing key is not valid yet");
    }
    if now >= signing_key.valid_until_unix {
        bail!("catalog signing key has expired");
    }
    let public_key = decode_publisher_key(signing_key)?;
    let message = serde_json::to_vec(payload)?;
    verify_ed25519_signature(&public_key, &message, &signed.signature)?;

    let mut updated = trust.clone();
    if promote_pending {
        updated.current_key = signing_key.clone();
        updated.pending_key = None;
        updated.generation = updated
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("publisher key generation overflow"))?;
    }
    if let Some(next) = &payload.next_key {
        validate_publisher_key(next)?;
        if next.key_id == updated.current_key.key_id {
            bail!("next publisher key must differ from the current key");
        }
        if next.valid_from_unix <= updated.current_key.valid_from_unix
            || next.valid_from_unix >= next.valid_until_unix
            || next.valid_from_unix >= updated.current_key.valid_until_unix
        {
            bail!("next publisher key has an invalid rotation window");
        }
        match &updated.pending_key {
            Some(existing) if existing != next => {
                bail!("catalog attempts to replace a pending publisher key")
            }
            _ => updated.pending_key = Some(next.clone()),
        }
    }
    updated.highest_catalog_version = payload.catalog_version;
    Ok(updated)
}

fn initial_publisher_trust() -> PublisherTrust {
    #[cfg(feature = "review-trust-root")]
    let (key_id, public_key_base64, valid_from_unix, valid_until_unix) = (
        REVIEW_KEY_ID,
        REVIEW_PUBLIC_KEY_BASE64.to_owned(),
        1,
        u64::MAX,
    );
    #[cfg(not(feature = "review-trust-root"))]
    let (key_id, public_key_base64, valid_from_unix, valid_until_unix) = (
        VERIFIED_KEY_ID,
        BASE64.encode(VERIFIED_PUBLIC_KEY),
        VERIFIED_KEY_VALID_FROM_UNIX,
        VERIFIED_KEY_VALID_UNTIL_UNIX,
    );
    PublisherTrust {
        schema_version: 1,
        publisher_id: expected_publisher_id().to_owned(),
        generation: 1,
        highest_catalog_version: 0,
        current_key: PublisherKey {
            key_id: key_id.to_owned(),
            public_key_base64,
            valid_from_unix,
            valid_until_unix,
        },
        pending_key: None,
    }
}

fn load_publisher_trust_at(root: &Dir) -> Result<PublisherTrust> {
    let active = private_regular_file_or_missing_at(root, TRUST_NAME)?;
    let backup = private_regular_file_or_missing_at(root, TRUST_BACKUP_NAME)?;
    if !active && backup {
        bail!("publisher trust update is interrupted; run an extension mutation to recover it");
    }
    if !active {
        return Ok(initial_publisher_trust());
    }
    let trust: PublisherTrust =
        serde_json::from_slice(&read_small_file_at(root, Path::new(TRUST_NAME))?)
            .context("parse publisher trust state")?;
    validate_publisher_trust(&trust)?;
    Ok(trust)
}

fn validate_publisher_trust(trust: &PublisherTrust) -> Result<()> {
    if trust.schema_version != 1
        || trust.publisher_id != expected_publisher_id()
        || trust.generation == 0
    {
        bail!("publisher trust state is invalid");
    }
    validate_publisher_key(&trust.current_key)?;
    if let Some(pending) = &trust.pending_key {
        validate_publisher_key(pending)?;
        if pending.key_id == trust.current_key.key_id
            || pending.valid_from_unix <= trust.current_key.valid_from_unix
            || pending.valid_from_unix >= trust.current_key.valid_until_unix
        {
            bail!("publisher trust rotation state is invalid");
        }
    }
    Ok(())
}

fn validate_publisher_key(key: &PublisherKey) -> Result<()> {
    if key.key_id.is_empty()
        || key.key_id.len() > 128
        || !key
            .key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || key.valid_from_unix >= key.valid_until_unix
    {
        bail!("publisher key metadata is invalid");
    }
    decode_publisher_key(key).map(|_| ())
}

fn decode_publisher_key(key: &PublisherKey) -> Result<[u8; 32]> {
    let bytes = BASE64
        .decode(&key.public_key_base64)
        .context("publisher key is not valid base64")?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("publisher Ed25519 key must be 32 bytes"))
}

fn verify_ed25519_signature(public_key: &[u8], message: &[u8], signature: &str) -> Result<()> {
    let signature = BASE64
        .decode(signature)
        .context("catalog signature is not valid base64")?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(message, &signature)
        .map_err(|_| anyhow!("catalog signature verification failed"))
}

fn validate_catalog_freshness(payload: &CatalogPayload) -> Result<()> {
    validate_catalog_freshness_at(payload, unix_now()?)
}

fn unix_now() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock precedes the Unix epoch")?
        .as_secs())
}

fn validate_catalog_freshness_at(payload: &CatalogPayload, now: u64) -> Result<()> {
    if payload.expires_unix <= now {
        bail!("signed extension catalog has expired");
    }
    Ok(())
}

fn inspect_without_mutation(
    entry: &RegistryEntry,
    source: &InstallSource,
) -> Result<InspectedArchive> {
    let temp_root = std::env::temp_dir().join(unique_name("cua-extension-inspect"));
    let parent = open_directory_path_nofollow(
        temp_root
            .parent()
            .ok_or_else(|| anyhow!("inspection path has no parent"))?,
    )?;
    let name = temp_root
        .file_name()
        .ok_or_else(|| anyhow!("inspection path has no name"))?;
    create_private_subdirectory(&parent, name)?;
    let staging = parent.open_dir_nofollow(name)?;
    let result = inspect_archive(&source.archive, entry, &staging);
    drop(staging);
    let cleanup = remove_cap_subdirectory(&parent, name);
    let inspected = match (result, cleanup) {
        (Ok(inspected), Ok(())) => inspected,
        (Err(error), _) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
    };
    validate_source_metadata(&inspected, source)?;
    Ok(inspected)
}

fn validate_source_metadata(inspected: &InspectedArchive, source: &InstallSource) -> Result<()> {
    if let Some(catalog) = &source.catalog {
        validate_catalog_freshness(catalog)?;
        let manifest = &inspected.manifest;
        if inspected.archive_size != catalog.archive_size
            || inspected.archive_sha256 != catalog.archive_sha256
            || hex_sha256(&inspected.manifest_bytes) != catalog.manifest_sha256
            || manifest.version != catalog.version
            || manifest.license != catalog.license
            || manifest.source != catalog.source
            || manifest.corresponding_source_uri != catalog.corresponding_source_uri
            || manifest.corresponding_source_revision != catalog.corresponding_source_revision
            || manifest.provenance != catalog.provenance
        {
            bail!("manifest identity or provenance does not exactly match signed catalog");
        }
        if manifest.components.is_empty()
            || manifest.models.is_empty()
            || manifest.corresponding_source_file.is_none()
            || manifest
                .components
                .iter()
                .any(|component| component.notice_file.is_none())
            || manifest
                .models
                .iter()
                .any(|model| model.license_file.is_none())
        {
            bail!("verified perception extensions require digest-bound notice, corresponding-source, and model-license files");
        }
    }
    Ok(())
}

fn print_preview(preview: &InstallReview, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(&preview)?);
    } else {
        println!("Extension: {} {}", preview.id, preview.version);
        println!("Name: {}", preview.name);
        println!("Trust: {}", trust_label(&preview.trust));
        println!("Evidence class: {}", preview.evidence_class);
        #[cfg(feature = "review-trust-root")]
        if preview.trust == TrustClass::ReviewOnlyPublisherVerified {
            println!("{REVIEW_TRUST_NOTICE}");
        }
        if let (Some(publisher), Some(key), Some(catalog), Some(expires)) = (
            &preview.publisher_name,
            &preview.publisher_key_id,
            preview.catalog_version,
            preview.catalog_expires_unix,
        ) {
            println!(
                "Publisher signature: verified {} with key {} (catalog {}, expires {})",
                publisher, key, catalog, expires
            );
        }
        println!("Signing key status: {}", preview.signing_key_status);
        if let Some(publisher_id) = &preview.publisher_id {
            println!("Publisher ID: {publisher_id}");
        }
        if let Some(algorithm) = &preview.signing_key_algorithm {
            println!("Signing key algorithm: {algorithm}");
        }
        println!("Artifact source: {}", preview.artifact_source);
        println!("Destination: {}", preview.destination);
        println!("Download size: {} bytes", preview.download_size);
        println!("Installed size: {} bytes", preview.installed_size);
        println!("License: {}", preview.license);
        println!("Source: {}", preview.source);
        println!(
            "Corresponding source: {} @ {}",
            preview.corresponding_source_uri, preview.corresponding_source_revision
        );
        if let Some(file) = &preview.corresponding_source.file {
            println!(
                "Corresponding source file: {} | SHA-256 {}",
                file.path, file.sha256
            );
        }
        println!("Provenance: {}", preview.provenance);
        for component in &preview.components {
            println!(
                "Component: {} {} | {} | {} @ {} | notice: {}",
                component.name,
                component.version,
                component.license,
                component.source_uri,
                component.source_revision,
                component.notice
            );
            if let Some(file) = &component.notice_file {
                println!(
                    "Component notice file: {} | SHA-256 {}",
                    file.path, file.sha256
                );
            }
        }
        for model in &preview.models {
            println!(
                "Model: {} @ {} | original {} | conversion {}",
                model.path, model.revision, model.original_sha256, model.conversion_sha256
            );
            if let Some(file) = &model.license_file {
                println!(
                    "Model license file: {} | SHA-256 {}",
                    file.path, file.sha256
                );
            }
        }
        println!("Target: {}", preview.target);
        println!("Archive SHA-256: {}", preview.archive_sha256);
        println!("Manifest SHA-256: {}", preview.manifest_sha256);
        println!(
            "Authorization: request={}, confirmation_required={}, confirmation_received={}, mutation_authorized={}",
            preview.authorization.request,
            preview.authorization.confirmation_required,
            preview.authorization.confirmation_received,
            preview.authorization.mutation_authorized,
        );
        println!("Preview complete; no extension state was changed.");
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

    fn load_publisher_trust(&self) -> Result<PublisherTrust> {
        if !self.root.exists() {
            return Ok(initial_publisher_trust());
        }
        let root = open_directory_path_nofollow(&self.root)?;
        verify_cap_directory_permissions_portable(&root)?;
        let active = private_regular_file_or_missing_at(&root, TRUST_NAME)?;
        let backup = private_regular_file_or_missing_at(&root, TRUST_BACKUP_NAME)?;
        if !active && backup {
            bail!("publisher trust update is interrupted; run an extension mutation to recover it");
        }
        if !active {
            return Ok(initial_publisher_trust());
        }
        load_publisher_trust_at(&root)
    }

    fn recover_publisher_trust_locked(&self, root: &Dir) -> Result<()> {
        let active = private_regular_file_or_missing_at(root, TRUST_NAME)?;
        let backup = private_regular_file_or_missing_at(root, TRUST_BACKUP_NAME)?;
        if !active && backup {
            root.rename(TRUST_BACKUP_NAME, root, TRUST_NAME)
                .context("restore interrupted publisher trust state")?;
        } else if active && backup {
            root.remove_file(TRUST_BACKUP_NAME)
                .context("remove stale publisher trust backup")?;
        }
        if private_regular_file_or_missing_at(root, TRUST_NEW_NAME)? {
            root.remove_file(TRUST_NEW_NAME)
                .context("remove stale publisher trust candidate")?;
        }
        sync_cap_dir(root)
    }

    fn commit_publisher_trust_locked(&self, root: &Dir, trust: &PublisherTrust) -> Result<()> {
        validate_publisher_trust(trust)?;
        self.recover_publisher_trust_locked(root)?;
        write_new_file_at(root, TRUST_NEW_NAME, &serde_json::to_vec_pretty(trust)?)?;
        let had_active = private_regular_file_or_missing_at(root, TRUST_NAME)?;
        if had_active {
            root.rename(TRUST_NAME, root, TRUST_BACKUP_NAME)
                .context("back up publisher trust state")?;
        }
        if let Err(error) = root.rename(TRUST_NEW_NAME, root, TRUST_NAME) {
            let _ = root.remove_file(TRUST_NEW_NAME);
            if had_active {
                root.rename(TRUST_BACKUP_NAME, root, TRUST_NAME)
                    .context("restore publisher trust state")?;
            }
            return Err(error).context("activate publisher trust state");
        }
        if had_active {
            root.remove_file(TRUST_BACKUP_NAME)?;
        }
        sync_cap_dir(root)
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

    fn recover_activation_for_startup_locked(&self, root: &Dir, id: &str) -> Result<()> {
        let Some(extension) = open_private_subdirectory_if_present(root, id)? else {
            return Ok(());
        };
        let mut has_active = private_regular_file_or_missing_at(&extension, ACTIVE_NAME)?;
        let mut has_backup = private_regular_file_or_missing_at(&extension, ACTIVE_BACKUP_NAME)?;
        let has_new = private_regular_file_or_missing_at(&extension, ACTIVE_NEW_NAME)?;
        if !has_backup && !has_new {
            return Ok(());
        }
        if has_active {
            self.validate_pointer_target_for_startup_locked(id, &extension, ACTIVE_NAME)?;
        }
        if has_backup {
            self.validate_pointer_target_for_startup_locked(id, &extension, ACTIVE_BACKUP_NAME)?;
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

    fn recover_publisher_trust_from_active_locked(&self, root: &Dir, id: &str) -> Result<()> {
        let Some(pointer) = self.active_pointer_locked(root, id)? else {
            return Ok(());
        };
        let extension = root.open_dir_nofollow(id)?;
        let version = extension
            .open_dir_nofollow("versions")?
            .open_dir_nofollow(&pointer.version)?;
        let manifest = verify_installed_version_at(&version, registry_entry(id)?, None)?;
        self.recover_publisher_trust_from_verified_active_locked(root, id, &version, &manifest)
    }

    fn recover_publisher_trust_from_verified_active_locked(
        &self,
        root: &Dir,
        id: &str,
        version: &Dir,
        manifest: &ExtensionManifest,
    ) -> Result<()> {
        let record = read_install_record_at(&version, id, &manifest.version)?;
        let Some(candidate) = record.publisher_trust else {
            return Ok(());
        };
        if Some(candidate.highest_catalog_version) != record.catalog_version {
            bail!("installed publisher trust does not match its catalog version");
        }
        let current = load_publisher_trust_at(root)?;
        if candidate.highest_catalog_version > current.highest_catalog_version {
            self.commit_publisher_trust_locked(root, &candidate)?;
        } else if candidate.highest_catalog_version == current.highest_catalog_version
            && candidate != current
        {
            bail!("installed publisher trust conflicts with durable trust state");
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

    fn validate_pointer_target_for_startup_locked(
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
        #[cfg(not(windows))]
        verify_cap_directory_permissions_portable(&version)?;
        verify_installed_version_for_startup(&version, registry_entry(id)?)?;
        Ok(())
    }

    fn lock(&self, id: &str) -> Result<InstallLock> {
        let root = ensure_private_directory_path(&self.root)?;
        let lock_dir = ensure_private_subdirectory(&root, ".locks")?;
        let lock_name = format!("{id}.lock");
        let file = open_lock_file_at(&lock_dir, &lock_name)?;
        file.try_lock_exclusive()
            .with_context(|| format!("extension {id} is already being modified"))?;
        #[cfg(windows)]
        let windows_acl_file = {
            let acl_name = windows_acl_lease_name(id);
            let acl_file = open_lock_file_at(&lock_dir, &acl_name)?;
            acquire_windows_acl_lease_and_recover_with(
                || {
                    acquire_windows_acl_lease(&acl_file)
                        .with_context(|| format!("wait for extension {id} Windows ACL lease"))
                },
                || {
                    recover_windows_acl_profile_journal(&windows_acl_profile_journal_path(
                        &self.windows_acl_lease_path(id),
                    ))
                    .map_err(|failure| anyhow!(failure.message))
                    .with_context(|| format!("recover extension {id} Windows ACL authority"))
                },
            )?;
            acl_file
        };
        Ok(InstallLock {
            _file: file,
            #[cfg(windows)]
            _windows_acl_file: windows_acl_file,
            root,
        })
    }

    #[cfg(windows)]
    fn windows_acl_lease_path(&self, id: &str) -> PathBuf {
        self.root.join(".locks").join(windows_acl_lease_name(id))
    }

    fn acl_profile_journal_path(&self, id: &str) -> Option<PathBuf> {
        #[cfg(windows)]
        {
            Some(windows_acl_profile_journal_path(
                &self.windows_acl_lease_path(id),
            ))
        }
        #[cfg(not(windows))]
        {
            let _ = id;
            None
        }
    }

    fn install_source(
        &self,
        entry: &RegistryEntry,
        source: &InstallSource,
        is_update: bool,
    ) -> Result<InstalledVersion> {
        let lock = self.lock(entry.id)?;
        self.recover_publisher_trust_locked(&lock.root)?;
        self.recover_activation_locked(&lock.root, entry.id)?;
        self.recover_publisher_trust_from_active_locked(&lock.root, entry.id)?;
        if let Some(signed) = &source.signed_catalog {
            let current_trust = load_publisher_trust_at(&lock.root)?;
            let verified = verify_catalog_at(entry, signed, &current_trust, unix_now()?)
                .context("signed catalog changed or became stale before mutation")?;
            if source.trust_update.as_ref() != Some(&verified) {
                bail!("signed catalog trust decision changed before mutation; inspect it again");
            }
        }
        let active = self.active_pointer_locked(&lock.root, entry.id)?;
        if is_update && active.is_none() {
            bail!("{} is not installed; use extension install", entry.id);
        }
        if !is_update && active.is_some() {
            bail!("{} is already installed; use extension update", entry.id);
        }
        if let Some(pointer) = &active {
            let extension = lock.root.open_dir_nofollow(entry.id)?;
            let versions = extension.open_dir_nofollow("versions")?;
            let installed = versions.open_dir_nofollow(&pointer.version)?;
            let manifest = verify_installed_version_at(&installed, entry, None)?;
            let record = read_install_record_at(&installed, entry.id, &manifest.version)?;
            if record.trust != TrustClass::DeveloperUnsignedLocal
                && source.trust == TrustClass::DeveloperUnsignedLocal
            {
                bail!("developer-only unsigned mode cannot replace a publisher-verified install");
            }
            if let (Some(previous), Some(next)) = (
                record.catalog_version,
                source
                    .catalog
                    .as_ref()
                    .map(|catalog| catalog.catalog_version),
            ) {
                if next <= previous {
                    bail!(
                        "catalog anti-rollback version {next} must be newer than installed version {previous}"
                    );
                }
            }
        }
        let extension_handle = ensure_private_subdirectory(&lock.root, entry.id)?;
        let versions = self.versions_dir(entry.id);
        let versions_handle = ensure_private_subdirectory(&extension_handle, "versions")?;
        let staging_parent = self.root.join(".staging");
        let staging_parent_handle = ensure_private_subdirectory(&lock.root, ".staging")?;
        let staging = staging_parent.join(unique_name(entry.id));
        let staging_name = staging.file_name().expect("generated staging name");
        create_private_subdirectory(&staging_parent_handle, staging_name)?;
        let staging_handle = staging_parent_handle.open_dir_nofollow(staging_name)?;
        let inspected = match inspect_archive(&source.archive, entry, &staging_handle) {
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
        if let Some(active) = &active {
            let current = Version::parse(&active.version)?;
            if version < current {
                remove_cap_subdirectory(&staging_parent_handle, staging_name)?;
                bail!("update version {version} must be newer than active version {current}");
            }
        }
        validate_source_metadata(&inspected, source)?;
        let destination = versions.join(&version_text);
        match versions_handle.open_dir_nofollow(&version_text) {
            Ok(existing) => {
                verify_cap_directory_permissions_portable(&existing)?;
                let verified =
                    verify_installed_version_at(&existing, entry, Some(&inspected.manifest_bytes));
                let cleanup = remove_cap_subdirectory(&staging_parent_handle, staging_name);
                let manifest = verified?;
                verify_install_record_at(&existing, entry.id, &manifest.version)?;
                run_verified_extension_hook(
                    &destination,
                    &existing,
                    &manifest,
                    entry,
                    Some(&inspected.manifest_bytes),
                    true,
                    self.acl_profile_journal_path(entry.id),
                )?;
                cleanup?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut placed = false;
                let staged = (|| -> Result<()> {
                    // Record and fully verify the staged version before any
                    // hook instruction runs, so a hook can never execute from
                    // an unrecorded or unverified install. A failure here
                    // removes the staging directory below, so the rollback
                    // stays durable whichever step failed.
                    write_install_record_at(&staging_handle, &inspected, source)?;
                    #[cfg(windows)]
                    converge_windows_installed_version(
                        &staging_handle,
                        entry,
                        Some(&inspected.manifest_bytes),
                    )?;
                    #[cfg(not(windows))]
                    verify_installed_version_at(
                        &staging_handle,
                        entry,
                        Some(&inspected.manifest_bytes),
                    )?;
                    run_verified_extension_hook(
                        &staging,
                        &staging_handle,
                        &inspected.manifest,
                        entry,
                        Some(&inspected.manifest_bytes),
                        true,
                        self.acl_profile_journal_path(entry.id),
                    )?;
                    drop(staging_handle);
                    staging_parent_handle
                        .rename(staging_name, &versions_handle, &version_text)
                        .with_context(|| {
                            format!("place extension version at {}", destination.display())
                        })?;
                    placed = true;
                    #[cfg(windows)]
                    {
                        let installed = versions_handle
                            .open_dir_nofollow(&version_text)
                            .context("reopen placed extension version")?;
                        converge_windows_installed_version(
                            &installed,
                            entry,
                            Some(&inspected.manifest_bytes),
                        )?;
                    }
                    sync_cap_dir(&staging_parent_handle)?;
                    sync_cap_dir(&versions_handle)?;
                    Ok(())
                })();
                if staged.is_err() {
                    if placed {
                        #[cfg(windows)]
                        if let Ok(installed) = versions_handle.open_dir_nofollow(&version_text) {
                            let _ = windows_harden_private_tree(&installed);
                        }
                        remove_cap_subdirectory(&versions_handle, version_text.as_ref())?;
                    } else {
                        remove_cap_subdirectory(&staging_parent_handle, staging_name)?;
                    }
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
        if let Some(trust) = &source.trust_update {
            self.commit_publisher_trust_locked(&lock.root, trust)?;
        }
        Ok(InstalledVersion {
            version: version_text,
            path: destination,
        })
    }

    #[cfg(test)]
    fn install_archive(&self, entry: &RegistryEntry, archive: &Path) -> Result<InstalledVersion> {
        // Test fixtures own their store exclusively. Inspect the pointer entry
        // directly so the helper does not acquire and immediately reacquire
        // the mutation lock before every install. Some sandbox filesystems can
        // transiently retain that just-released advisory lock and return
        // EAGAIN to the second acquisition.
        let active = self.extension_dir(entry.id).join(ACTIVE_NAME);
        let is_update = match fs::symlink_metadata(&active) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("inspect test activation pointer {}", active.display())
                })
            }
        };
        self.install_source(
            entry,
            &InstallSource {
                archive: archive.to_owned(),
                trust: TrustClass::DeveloperUnsignedLocal,
                catalog: None,
                signed_catalog: None,
                trust_update: None,
            },
            is_update,
        )
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
        self.recover_publisher_trust_from_active_locked(&lock.root, id)?;
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

    fn active_path_for_startup(&self, id: &str) -> Result<Option<(PathBuf, ExtensionManifest)>> {
        let lock = self.lock(id)?;
        self.recover_activation_for_startup_locked(&lock.root, id)?;
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
        #[cfg(not(windows))]
        verify_cap_directory_permissions_portable(&version)?;
        let manifest = verify_installed_version_for_startup(&version, registry_entry(id)?)?;
        self.recover_publisher_trust_from_verified_active_locked(
            &lock.root, id, &version, &manifest,
        )?;
        Ok(Some((
            self.versions_dir(id).join(&pointer.version),
            manifest,
        )))
    }

    fn info<'a>(&self, entry: &'a RegistryEntry, self_test: bool) -> Result<ExtensionInfo<'a>> {
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
                    trust: None,
                    evidence_class: None,
                    publisher_id: None,
                    publisher_key_id: None,
                    catalog_version: None,
                    detail: format!("unhealthy: {error:#}"),
                });
            }
        };
        let pointer = match self
            .recover_activation_locked(&lock.root, entry.id)
            .and_then(|()| self.recover_publisher_trust_from_active_locked(&lock.root, entry.id))
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
                    trust: None,
                    evidence_class: None,
                    publisher_id: None,
                    publisher_key_id: None,
                    catalog_version: None,
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
                trust: None,
                evidence_class: None,
                publisher_id: None,
                publisher_key_id: None,
                catalog_version: None,
                detail: "not installed (healthy)".to_owned(),
            });
        };
        let version = pointer.version.clone();
        let verified = (|| -> Result<InstallRecord> {
            let extension = lock.root.open_dir_nofollow(entry.id)?;
            let versions = extension
                .open_dir_nofollow("versions")
                .context("active extension has no owned versions directory")?;
            verify_cap_directory_permissions_portable(&versions)?;
            let installed = versions.open_dir_nofollow(&version).with_context(|| {
                format!("active extension version {version} is not an owned directory")
            })?;
            verify_cap_directory_permissions_portable(&installed)?;
            let manifest = verify_installed_version_at(&installed, entry, None)?;
            let record = run_verified_extension_hook(
                &self.versions_dir(entry.id).join(&version),
                &installed,
                &manifest,
                entry,
                None,
                self_test,
                self.acl_profile_journal_path(entry.id),
            )?;
            Ok(record)
        })();
        match verified {
            Ok(record) => {
                let trust = record.trust.clone();
                let detail = match trust {
                    TrustClass::DeveloperUnsignedLocal => {
                        format!("healthy; local integrity checks passed; {DEVELOPER_TRUST_NOTICE}")
                    }
                    #[cfg(feature = "review-trust-root")]
                    TrustClass::ReviewOnlyPublisherVerified => {
                        format!("healthy; integrity and review-key verification passed; {REVIEW_TRUST_NOTICE}")
                    }
                    TrustClass::PublisherVerified => {
                        "healthy; integrity and publisher verification checks passed".to_owned()
                    }
                };
                let record_evidence_class = evidence_class(&trust);
                Ok(ExtensionInfo {
                    id: entry.id,
                    display_name: entry.display_name,
                    description: entry.description,
                    protocol_version: entry.protocol_version,
                    installed: true,
                    active_version: Some(version),
                    healthy: true,
                    detail,
                    trust: Some(trust),
                    evidence_class: Some(record_evidence_class),
                    publisher_id: record.publisher_id,
                    publisher_key_id: record.key_id,
                    catalog_version: record.catalog_version,
                })
            }
            Err(error) => Ok(ExtensionInfo {
                id: entry.id,
                display_name: entry.display_name,
                description: entry.description,
                protocol_version: entry.protocol_version,
                installed: true,
                active_version: Some(version),
                healthy: false,
                trust: None,
                evidence_class: None,
                publisher_id: None,
                publisher_key_id: None,
                catalog_version: None,
                detail: format!("unhealthy: {error:#}"),
            }),
        }
    }

    fn remove(&self, entry: &RegistryEntry) -> Result<()> {
        let lock = self.lock(entry.id)?;
        self.recover_activation_locked(&lock.root, entry.id)?;
        self.recover_publisher_trust_from_active_locked(&lock.root, entry.id)?;
        let Some(extension) = open_private_subdirectory_if_present(&lock.root, entry.id)? else {
            bail!("{} is not installed", entry.id);
        };
        let names = extension
            .entries()?
            .map(|item| item.map(|entry| entry.file_name()))
            .collect::<std::io::Result<BTreeSet<_>>>()?;
        let allowed = [
            std::ffi::OsString::from("versions"),
            std::ffi::OsString::from(ACTIVE_NAME),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        if !names.is_subset(&allowed) {
            bail!("refusing removal because the extension directory contains unowned state");
        }
        if let Some(pointer) = self.active_pointer_locked(&lock.root, entry.id)? {
            self.validate_pointer_target_locked(entry.id, &extension, ACTIVE_NAME)?;
            validate_version_segment(&pointer.version)?;
        }
        match extension.open_dir_nofollow("versions") {
            Ok(versions) => {
                for child in versions.entries()? {
                    let child = child?;
                    if !child.file_type()?.is_dir() {
                        bail!("refusing removal because versions contains non-directory state");
                    }
                    let version_name = child.file_name();
                    let version_text = version_name
                        .to_str()
                        .ok_or_else(|| anyhow!("installed version name is not UTF-8"))?;
                    validate_version_segment(version_text)?;
                    let version = versions.open_dir_nofollow(&version_name)?;
                    verify_installed_version_at(&version, entry, None)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("open owned versions directory for removal"),
        }
        drop(extension);
        remove_cap_subdirectory(&lock.root, std::ffi::OsStr::new(entry.id))
    }
}

fn inspect_archive(
    path: &Path,
    entry: &RegistryEntry,
    staging_dir: &Dir,
) -> Result<InspectedArchive> {
    let mut file = open_existing_no_follow(path)?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect archive {}", path.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        if metadata.file_attributes() & 0x400 != 0 {
            bail!("archive is a Windows reparse point: {}", path.display());
        }
    }
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("archive is not a regular local file: {}", path.display());
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        bail!("archive exceeds the {MAX_ARCHIVE_BYTES} byte input limit");
    }
    let archive_sha256 = hash_open_file(&mut file, MAX_ARCHIVE_BYTES)?;
    file.seek(std::io::SeekFrom::Start(0))?;
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
        archive_sha256,
        archive_size: metadata.len(),
        expanded_size: expanded,
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
    for (name, value) in [
        ("license", manifest.license.as_str()),
        ("source", manifest.source.as_str()),
        (
            "corresponding_source_uri",
            manifest.corresponding_source_uri.as_str(),
        ),
        (
            "corresponding_source_revision",
            manifest.corresponding_source_revision.as_str(),
        ),
        ("provenance", manifest.provenance.as_str()),
    ] {
        if value.trim().is_empty() {
            bail!("manifest {name} must not be empty");
        }
    }
    let mut declared = BTreeMap::new();
    for file in &manifest.files {
        safe_manifest_path(&file.path)?;
        if file.path == MANIFEST_NAME || file.path == INSTALL_RECORD_NAME {
            bail!("manifest cannot claim reserved path {:?}", file.path);
        }
        validate_sha256(&file.path, &file.sha256)?;
        if declared.insert(file.path.as_str(), file).is_some() {
            bail!("manifest declares duplicate file {}", file.path);
        }
    }
    let mut model_paths = BTreeSet::new();
    for model in &manifest.models {
        safe_manifest_path(&model.path)?;
        if model.revision.trim().is_empty() {
            bail!("model {} has no immutable revision", model.path);
        }
        validate_sha256("model original", &model.original_sha256)?;
        validate_sha256("model conversion", &model.conversion_sha256)?;
        if !model_paths.insert(model.path.as_str()) {
            bail!("manifest declares duplicate model {}", model.path);
        }
        let file = declared
            .get(model.path.as_str())
            .ok_or_else(|| anyhow!("model {} is not declared in files", model.path))?;
        if !file.sha256.eq_ignore_ascii_case(&model.conversion_sha256) {
            bail!(
                "model hash does not exactly match file hash for {}",
                model.path
            );
        }
        if let Some(binding) = &model.license_file {
            validate_artifact_binding("model license", binding, &declared)?;
        }
    }
    let mut component_names = BTreeSet::new();
    for component in &manifest.components {
        for (field, value) in [
            ("name", component.name.as_str()),
            ("version", component.version.as_str()),
            ("license", component.license.as_str()),
            ("notice", component.notice.as_str()),
            ("source_uri", component.source_uri.as_str()),
            ("source_revision", component.source_revision.as_str()),
        ] {
            if value.trim().is_empty() {
                bail!("component {} has an empty {field}", component.name);
            }
        }
        if !component_names.insert(component.name.as_str()) {
            bail!("manifest declares duplicate component {}", component.name);
        }
        if let Some(binding) = &component.notice_file {
            validate_artifact_binding("component notice", binding, &declared)?;
        }
    }
    if let Some(binding) = &manifest.corresponding_source_file {
        validate_artifact_binding("corresponding source", binding, &declared)?;
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

fn validate_artifact_binding(
    label: &str,
    binding: &ArtifactBinding,
    declared: &BTreeMap<&str, &ManifestFile>,
) -> Result<()> {
    safe_manifest_path(&binding.path)?;
    validate_sha256(label, &binding.sha256)?;
    let file = declared
        .get(binding.path.as_str())
        .ok_or_else(|| anyhow!("{label} file {} is not declared in files", binding.path))?;
    if !file.sha256.eq_ignore_ascii_case(&binding.sha256) {
        bail!(
            "{label} digest does not match declared file {}",
            binding.path
        );
    }
    Ok(())
}

fn write_install_record_at(
    root: &Dir,
    inspected: &InspectedArchive,
    source: &InstallSource,
) -> Result<()> {
    let record = InstallRecord {
        schema_version: MANIFEST_SCHEMA_VERSION,
        id: inspected.manifest.id.clone(),
        version: inspected.manifest.version.clone(),
        manifest_sha256: hex_sha256(&inspected.manifest_bytes),
        trust: source.trust.clone(),
        publisher_id: source
            .catalog
            .as_ref()
            .map(|catalog| catalog.publisher_id.clone()),
        key_id: source
            .catalog
            .as_ref()
            .map(|catalog| catalog.key_id.clone()),
        archive_sha256: inspected.archive_sha256.clone(),
        catalog_version: source
            .catalog
            .as_ref()
            .map(|catalog| catalog.catalog_version),
        publisher_trust: source.trust_update.clone(),
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
    #[cfg(windows)]
    windows_secure_handle(&file)?;
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

/// Verify the installed payload needed to construct the perception worker
/// without rehashing large, non-executed compliance and corresponding-source
/// files on every daemon start. Lifecycle commands continue to use the full
/// verifier above.
fn verify_installed_version_for_startup_at(
    root: &Dir,
    entry: &RegistryEntry,
) -> Result<ExtensionManifest> {
    let bytes = read_small_file_at(root, Path::new(MANIFEST_NAME))?;
    let manifest: ExtensionManifest = serde_json::from_slice(&bytes)?;
    read_install_record_at(root, entry.id, &manifest.version)?;
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
    verify_manifest_permissions_at(root, &manifest)?;

    let contract_file = manifest
        .files
        .iter()
        .find(|file| file.path == PERCEPTION_RUNTIME_CONTRACT)
        .ok_or_else(|| anyhow!("perception runtime contract is not extension-owned"))?;
    let contract_hash = hash_file_at(root, Path::new(PERCEPTION_RUNTIME_CONTRACT), MAX_FILE_BYTES)?;
    if !contract_hash.eq_ignore_ascii_case(&contract_file.sha256) {
        bail!("SHA-256 mismatch for {PERCEPTION_RUNTIME_CONTRACT}");
    }
    let contract: PerceptionRuntimeContract = serde_json::from_slice(&read_small_file_at(
        root,
        Path::new(PERCEPTION_RUNTIME_CONTRACT),
    )?)
    .context("parse perception runtime contract")?;
    validate_runtime_contract(&manifest, &contract)?;

    let mut required = manifest
        .files
        .iter()
        .filter(|file| {
            file.executable
                || ["bin/", "runtime/", "models/"]
                    .iter()
                    .any(|prefix| file.path.starts_with(prefix))
        })
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    required.insert(manifest.entrypoint.clone());
    required.insert(PERCEPTION_RUNTIME_CONTRACT.to_owned());
    required.insert(unique_model_manifest_path(&manifest)?.to_owned());
    required.extend(manifest.models.iter().map(|model| model.path.clone()));
    required.insert(format!("runtime/{}", contract.runtime.name));
    required.extend(
        contract
            .models
            .iter()
            .map(|model| format!("models/{}", model.name)),
    );
    required.insert(format!("models/{}", contract.dictionary.name));
    // Reuse complete manifest validation while hashing only worker execution
    // inputs. Non-executed compliance payload remains covered by exact file-set,
    // ownership, permissions, and full lifecycle/status verification.
    let mut startup_files = manifest
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
                sha256: if required.contains(&file.path) {
                    hash_file_at(root, path, MAX_FILE_BYTES)?
                } else {
                    file.sha256.clone()
                },
                size: metadata.len(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    startup_files.push(ArchiveFile {
        path: MANIFEST_NAME.to_owned(),
        sha256: hex_sha256(&bytes),
        size: bytes.len() as u64,
    });
    validate_manifest(&manifest, entry, &startup_files)?;
    Ok(manifest)
}

fn verify_installed_version_for_startup(
    root: &Dir,
    entry: &RegistryEntry,
) -> Result<ExtensionManifest> {
    #[cfg(windows)]
    {
        let started = Instant::now();
        return converge_windows_startup_verification_with(
            WINDOWS_ACL_CONVERGENCE_ATTEMPTS,
            WINDOWS_ACL_CONVERGENCE_RETRY_WINDOW,
            || started.elapsed(),
            std::thread::sleep,
            || windows_harden_private_tree(root),
            || verify_installed_version_for_startup_at(root, entry),
        );
    }
    #[cfg(not(windows))]
    verify_installed_version_for_startup_at(root, entry)
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

fn read_install_record_at(root: &Dir, id: &str, version: &str) -> Result<InstallRecord> {
    let manifest = read_small_file_at(root, Path::new(MANIFEST_NAME))?;
    let record: InstallRecord =
        serde_json::from_slice(&read_small_file_at(root, Path::new(INSTALL_RECORD_NAME))?)?;
    if record.schema_version != MANIFEST_SCHEMA_VERSION
        || record.id != id
        || record.version != version
        || record.manifest_sha256 != hex_sha256(&manifest)
        || validate_sha256("record archive", &record.archive_sha256).is_err()
    {
        bail!("install record does not verify extension ownership");
    }
    match record.trust {
        TrustClass::PublisherVerified => {
            #[cfg(feature = "review-trust-root")]
            bail!("review-only builds cannot accept production-class install records");
            #[cfg(not(feature = "review-trust-root"))]
            validate_verified_install_record(&record)?;
        }
        #[cfg(feature = "review-trust-root")]
        TrustClass::ReviewOnlyPublisherVerified => validate_verified_install_record(&record)?,
        TrustClass::DeveloperUnsignedLocal => {
            if record.publisher_id.is_some()
                || record.key_id.is_some()
                || record.catalog_version.is_some()
                || record.publisher_trust.is_some()
            {
                bail!("developer install record cannot claim a verified publisher");
            }
        }
    }
    Ok(record)
}

fn validate_verified_install_record(record: &InstallRecord) -> Result<()> {
    if record.publisher_id.as_deref() != Some(expected_publisher_id())
        || record.key_id.as_deref().is_none_or(str::is_empty)
    {
        bail!("verified install record has an untrusted publisher identity");
    }
    if record.catalog_version.is_none() {
        bail!("verified install record has no catalog anti-rollback version");
    }
    let trust = record
        .publisher_trust
        .as_ref()
        .ok_or_else(|| anyhow!("verified install record has no publisher trust snapshot"))?;
    validate_publisher_trust(trust)?;
    if Some(trust.highest_catalog_version) != record.catalog_version
        || Some(trust.current_key.key_id.as_str()) != record.key_id.as_deref()
            && trust.pending_key.as_ref().map(|key| key.key_id.as_str()) != record.key_id.as_deref()
    {
        bail!("verified install record publisher trust does not match its catalog");
    }
    Ok(())
}

fn verify_install_record_at(root: &Dir, id: &str, version: &str) -> Result<()> {
    read_install_record_at(root, id, version).map(drop)
}

fn run_extension_hook(
    root: &Path,
    manifest: &ExtensionManifest,
    self_test: bool,
    journal_path: Option<PathBuf>,
) -> Result<()> {
    run_extension_hook_with_timeout(
        root,
        manifest,
        self_test,
        std::time::Duration::from_secs(10),
        journal_path,
    )
}

#[cfg(windows)]
fn converge_windows_installed_version(
    root: &Dir,
    entry: &RegistryEntry,
    expected_manifest: Option<&[u8]>,
) -> Result<ExtensionManifest> {
    converge_windows_acl(
        WINDOWS_ACL_CONVERGENCE_ATTEMPTS,
        WINDOWS_ACL_CONVERGENCE_RETRY_WINDOW,
        || windows_harden_private_tree(root),
        || verify_installed_version_at(root, entry, expected_manifest),
    )
}

fn run_verified_extension_hook(
    root_path: &Path,
    root: &Dir,
    manifest: &ExtensionManifest,
    entry: &RegistryEntry,
    expected_manifest: Option<&[u8]>,
    self_test: bool,
    journal_path: Option<PathBuf>,
) -> Result<InstallRecord> {
    run_verified_extension_hook_with(root, entry, expected_manifest, &manifest.version, || {
        run_extension_hook(root_path, manifest, self_test, journal_path)
    })
}

fn run_verified_extension_hook_with(
    root: &Dir,
    entry: &RegistryEntry,
    expected_manifest: Option<&[u8]>,
    version: &str,
    hook: impl FnOnce() -> Result<()>,
) -> Result<InstallRecord> {
    let hook = hook();
    #[cfg(windows)]
    {
        let restored = converge_windows_installed_version(root, entry, expected_manifest).map(drop);
        match (hook, restored) {
            (Ok(()), restored) => restored,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(restore_error)) => Err(anyhow!(
                "{error:#}; additionally failed to restore and verify private Windows ACLs after the extension hook: {restore_error:#}"
            )),
        }?;
    }
    #[cfg(not(windows))]
    {
        let _ = (root, entry, expected_manifest);
        hook?;
    }
    read_install_record_at(root, entry.id, version)
}

/// Test-only record of the installed state each hook launch could see.
///
/// A hook may run only after its extension has been recorded and fully
/// verified, and the only way to prove that ordering is to inspect the install
/// directory at the moment of launch — a fresh install's staging directory is
/// gone by the time a test could look.
#[cfg(test)]
mod hook_observer {
    use super::{
        open_directory_path_nofollow, read_install_record_at, registry_entry,
        verify_installed_version_at, INSTALL_RECORD_NAME, PERCEPTION_ID,
    };
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[derive(Clone, Debug)]
    pub(super) struct Launch {
        pub(super) install_record_present: bool,
        pub(super) fully_verified: bool,
        root: PathBuf,
    }

    static LAUNCHES: Mutex<Vec<Launch>> = Mutex::new(Vec::new());

    /// Called before the hook process is created, so nothing it observes can
    /// have been produced by the hook itself.
    pub(super) fn note_launch(root: &Path) {
        let launch = Launch {
            root: root.to_path_buf(),
            install_record_present: root.join(INSTALL_RECORD_NAME).exists(),
            fully_verified: fully_verified(root),
        };
        LAUNCHES.lock().expect("hook launch observer").push(launch);
    }

    /// Whether the manager itself would accept this directory as an installed,
    /// recorded, integrity-verified extension version right now.
    fn fully_verified(root: &Path) -> bool {
        let Ok(directory) = open_directory_path_nofollow(root) else {
            return false;
        };
        let Ok(entry) = registry_entry(PERCEPTION_ID) else {
            return false;
        };
        let Ok(manifest) = verify_installed_version_at(&directory, entry, None) else {
            return false;
        };
        read_install_record_at(&directory, entry.id, &manifest.version).is_ok()
    }

    /// Launches observed anywhere beneath one directory, which is how a test
    /// reaches a fresh install's staging directory without knowing its
    /// generated name. Tests use their own temporary roots, so this stays exact
    /// while the suite runs in parallel.
    pub(super) fn launches_under(root: &Path) -> Vec<Launch> {
        LAUNCHES
            .lock()
            .expect("hook launch observer")
            .iter()
            .filter(|launch| launch.root.starts_with(root))
            .cloned()
            .collect()
    }
}

/// Run one installed extension hook under the same fail-closed native
/// containment boundary a perception parse gets.
///
/// The hook is the extension's own worker binary, so the only authority it is
/// granted beyond the shared boundary is reading its own installed bundle. It
/// inherits no environment, no descriptor, and none of the Driver's capture,
/// input, or credential authority; a platform that cannot install every
/// boundary fails the launch instead of running the hook.
fn run_extension_hook_with_timeout(
    root: &Path,
    manifest: &ExtensionManifest,
    self_test: bool,
    timeout: std::time::Duration,
    journal_path: Option<PathBuf>,
) -> Result<()> {
    let args = if self_test {
        &manifest.self_test_args
    } else {
        &manifest.health_args
    };
    if args.is_empty() {
        return Ok(());
    }
    if args.len() > 16
        || args
            .iter()
            .any(|arg| arg.len() > 4096 || arg.contains('\0'))
    {
        bail!("extension hook arguments exceed safe limits");
    }
    let entrypoint = root.join(&manifest.entrypoint);
    let metadata =
        fs::symlink_metadata(&entrypoint).context("inspect extension hook entrypoint")?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("extension hook entrypoint is not a regular file");
    }
    #[cfg(test)]
    hook_observer::note_launch(root);
    let limits = extension_hook_containment_limits(root, journal_path);
    let kind = if self_test { "self-test" } else { "health" };
    match run_contained_hook_blocking(&entrypoint, args, &limits, timeout)? {
        HookOutcome::Succeeded => Ok(()),
        HookOutcome::Failed(detail) => bail!("extension {kind} hook failed with {detail}"),
        HookOutcome::ResourceLimited(detail) => {
            bail!("extension {kind} hook exceeded a containment limit: {detail}")
        }
        HookOutcome::TimedOut => bail!("extension hook exceeded its execution limit"),
    }
}

fn extension_hook_containment_limits(
    root: &Path,
    journal_path: Option<PathBuf>,
) -> ContainmentLimits {
    ContainmentLimits {
        additional_readable_paths: vec![root.to_path_buf()],
        // The lifecycle caller already owns this extension's runtime ACL
        // lease. Reacquiring it would deadlock, so hook teardown stays on the
        // dedicated hook thread and must finish before the caller releases it.
        windows_require_synchronous_cleanup: true,
        windows_acl_profile_journal_path: journal_path,
        ..ContainmentLimits::default()
    }
}

/// Bridge the asynchronous contained launch into the synchronous extension
/// lifecycle. The hook owns a dedicated thread and runtime, so this stays
/// correct whether or not the caller already sits inside one.
fn run_contained_hook_blocking(
    entrypoint: &Path,
    args: &[String],
    limits: &ContainmentLimits,
    timeout: std::time::Duration,
) -> Result<HookOutcome> {
    std::thread::scope(|scope| -> Result<HookOutcome> {
        scope
            .spawn(|| -> Result<HookOutcome> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .context("start the contained extension hook runtime")?;
                runtime
                    .block_on(run_contained_hook(entrypoint, args, limits, timeout))
                    .map_err(|failure| match failure.detail {
                        Some(detail) => {
                            anyhow!("contain the extension hook: {} ({detail})", failure.message)
                        }
                        None => anyhow!("contain the extension hook: {}", failure.message),
                    })
            })
            .join()
            .map_err(|_| anyhow!("the contained extension hook supervisor panicked"))?
    })
}

fn stream_archive_file_at<R: Read>(
    root: &Dir,
    reader: &mut R,
    path: &Path,
    expected: u64,
) -> Result<String> {
    let (parent, name) = open_cap_parent_nofollow(root, path)?;
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
    let mut file = parent.open_with(&name, &options)?;
    #[cfg(windows)]
    windows_secure_handle(&file)?;
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
    let (parent, name) = open_cap_parent_nofollow(directory, path)?;
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = parent.open_with(&name, &options)?;
    if !file.metadata()?.is_file() {
        bail!(
            "extension path is not an owned regular file: {}",
            path.display()
        );
    }
    verify_private_cap_file_permissions(&file, &path.to_string_lossy())?;
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
    let (parent, name) = open_cap_parent_nofollow(directory, path)?;
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = parent.open_with(&name, &options)?;
    if !file.metadata()?.is_file() {
        bail!(
            "extension path is not an owned regular file: {}",
            path.display()
        );
    }
    verify_private_cap_file_permissions(&file, &path.to_string_lossy())?;
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

fn open_cap_parent_nofollow(root: &Dir, relative: &Path) -> Result<(Dir, std::ffi::OsString)> {
    let name = relative
        .file_name()
        .ok_or_else(|| anyhow!("relative extension path has no final component"))?
        .to_owned();
    let mut current = root.try_clone()?;
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            let Component::Normal(segment) = component else {
                bail!("extension path contains an unsafe parent");
            };
            current = current.open_dir_nofollow(segment)?;
            verify_cap_directory_permissions_portable(&current)?;
        }
    }
    Ok((current, name))
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
    #[cfg(windows)]
    {
        let directory = parent.open_dir_nofollow(name)?;
        windows_secure_handle(&directory)?;
    }
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

#[cfg(windows)]
fn verify_private_cap_file_permissions(file: &cap_std::fs::File, path: &str) -> Result<()> {
    windows_verify_file_handle(file).with_context(|| format!("verify Windows ACL for {path}"))
}

#[cfg(all(not(unix), not(windows)))]
fn verify_private_cap_file_permissions(_file: &cap_std::fs::File, _path: &str) -> Result<()> {
    Ok(())
}

fn remove_cap_subdirectory(parent: &Dir, name: &std::ffi::OsStr) -> Result<()> {
    let opened = match parent.open_dir_nofollow(name) {
        Ok(opened) => opened,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    verify_cap_directory_permissions_portable(&opened)?;
    inspect_owned_tree(&opened)?;
    opened.remove_open_dir_all()?;
    sync_cap_dir(parent)
}

fn inspect_owned_tree(directory: &Dir) -> Result<()> {
    for child in directory.entries()? {
        let child = child?;
        let file_type = child.file_type()?;
        #[cfg(windows)]
        {
            use cap_std::fs::MetadataExt as _;
            if child.metadata()?.file_attributes() & 0x400 != 0 {
                bail!(
                    "refusing to delete extension directory containing reparse point {:?}",
                    child.file_name()
                );
            }
        }
        if file_type.is_symlink() {
            bail!(
                "refusing to delete extension directory containing symbolic link {:?}",
                child.file_name()
            );
        }
        if file_type.is_dir() {
            let opened = directory.open_dir_nofollow(child.file_name())?;
            verify_cap_directory_permissions_portable(&opened)?;
            inspect_owned_tree(&opened)?;
        } else if file_type.is_file() {
            let mut options = CapOpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            let file = child.open_with(&options)?;
            verify_private_cap_file_permissions(&file, &child.file_name().to_string_lossy())?;
        } else {
            bail!(
                "refusing to delete extension directory containing special file {:?}",
                child.file_name()
            );
        }
    }
    Ok(())
}

#[cfg(windows)]
fn windows_harden_private_tree(directory: &Dir) -> Result<()> {
    windows_harden_private_tree_with(directory, &mut |_| {})
}

#[cfg(windows)]
fn windows_harden_private_tree_with(
    directory: &Dir,
    after_initial_directory_harden: &mut impl FnMut(&Dir),
) -> Result<()> {
    windows_reject_reparse_directory(directory)?;
    windows_secure_handle(directory)?;
    after_initial_directory_harden(directory);
    for child in directory.entries()? {
        let child = child?;
        let file_type = child.file_type()?;
        use cap_std::fs::MetadataExt as _;
        if child.metadata()?.file_attributes() & 0x400 != 0 {
            bail!(
                "refusing to harden extension directory containing reparse point {:?}",
                child.file_name()
            );
        }
        if file_type.is_symlink() {
            bail!(
                "refusing to harden extension directory containing symbolic link {:?}",
                child.file_name()
            );
        }
        if file_type.is_dir() {
            let opened = directory.open_dir_nofollow(child.file_name())?;
            windows_harden_private_tree_with(&opened, after_initial_directory_harden)?;
        } else if file_type.is_file() {
            let mut options = CapOpenOptions::new();
            options.read(true).follow(FollowSymlinks::No);
            let file = child.open_with(&options)?;
            windows_secure_handle(&file)?;
        } else {
            bail!(
                "refusing to harden extension directory containing special file {:?}",
                child.file_name()
            );
        }
    }
    // Reassert the directory after its descendants so a privileged service
    // cannot use the recursive walk as a window to restore an explicit ACE.
    windows_secure_handle(directory)
}

#[cfg(any(windows, test))]
fn converge_windows_acl_with<T>(
    attempts: u32,
    retry_window: std::time::Duration,
    mut elapsed: impl FnMut() -> std::time::Duration,
    mut sleep: impl FnMut(std::time::Duration),
    mut harden: impl FnMut() -> Result<()>,
    mut verify: impl FnMut() -> Result<T>,
) -> Result<T> {
    // Privileged Windows services can add an explicit ACE after a large tree is
    // hardened. Re-run hardening and the complete content verification as one
    // unit, but only for the typed drift signal and within strict bounds. The
    // retry window controls whether another blocking attempt may begin; it
    // cannot interrupt a Windows filesystem call that is already in flight.
    debug_assert!(attempts > 0);
    for attempt in 1..=attempts {
        if attempt > 1 && elapsed() >= retry_window {
            bail!(
                "extension tree Windows ACLs did not converge before the {:?} retry window after {} hardening attempt(s)",
                retry_window,
                attempt - 1
            );
        }
        let result = match harden() {
            Ok(()) => verify(),
            Err(error) => Err(error),
        };
        match result {
            Ok(value) => return Ok(value),
            Err(error) => {
                let retryable = error.downcast_ref::<WindowsAclDrift>().is_some();
                if !retryable {
                    return Err(error);
                }
                if attempt == attempts {
                    return Err(error).context(format!(
                        "extension tree Windows ACLs did not converge after {attempt} hardening attempt(s)"
                    ));
                }
                let remaining = retry_window.saturating_sub(elapsed());
                let backoff = std::time::Duration::from_millis(250 * u64::from(attempt));
                if remaining <= backoff {
                    return Err(error).context(format!(
                        "extension tree Windows ACLs did not converge before the {:?} retry window after {attempt} hardening attempt(s)",
                        retry_window
                    ));
                }
                sleep(backoff);
            }
        }
    }
    unreachable!("the final convergence attempt always returns")
}

#[cfg(any(windows, test))]
fn converge_windows_startup_verification_with<T>(
    attempts: u32,
    retry_window: std::time::Duration,
    elapsed: impl FnMut() -> std::time::Duration,
    sleep: impl FnMut(std::time::Duration),
    harden: impl FnMut() -> Result<()>,
    mut verify: impl FnMut() -> Result<T>,
) -> Result<T> {
    match verify() {
        Ok(value) => Ok(value),
        Err(error) if error.downcast_ref::<WindowsAclDrift>().is_some() => {
            converge_windows_acl_with(attempts, retry_window, elapsed, sleep, harden, verify)
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn converge_windows_acl<T>(
    attempts: u32,
    retry_window: Duration,
    harden: impl FnMut() -> Result<()>,
    verify: impl FnMut() -> Result<T>,
) -> Result<T> {
    let started = Instant::now();
    converge_windows_acl_with(
        attempts,
        retry_window,
        || started.elapsed(),
        std::thread::sleep,
        harden,
        verify,
    )
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

#[cfg(windows)]
fn set_private_file_permissions_at(root: &Dir, path: &Path, _executable: bool) -> Result<()> {
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = root.open_with(path, &options)?;
    windows_secure_handle(&file)
}

#[cfg(all(not(unix), not(windows)))]
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

#[cfg(windows)]
fn verify_expected_file_permissions_at(root: &Dir, path: &Path, _executable: bool) -> Result<()> {
    let mut options = CapOpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = root.open_with(path, &options)?;
    windows_verify_file_handle(&file)
}

#[cfg(all(not(unix), not(windows)))]
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
            #[cfg(windows)]
            windows_secure_handle(&file)?;
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

#[cfg(any(windows, test))]
fn windows_acl_lease_name(id: &str) -> String {
    format!("{id}.runtime-acl.lock")
}

#[cfg(any(windows, test))]
fn acquire_windows_acl_lease_with(
    retry_window: Duration,
    mut elapsed: impl FnMut() -> Duration,
    mut sleep: impl FnMut(Duration),
    mut try_lock: impl FnMut() -> std::io::Result<()>,
) -> Result<()> {
    loop {
        match try_lock() {
            Ok(()) => return Ok(()),
            Err(cause)
                if cause.kind() == std::io::ErrorKind::WouldBlock
                    || cause.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                if elapsed() >= retry_window {
                    bail!("timed out waiting for the Windows ACL lease");
                }
                sleep(WINDOWS_ACL_LEASE_RETRY_INTERVAL);
            }
            Err(cause) => return Err(cause).context("acquire Windows ACL lease"),
        }
    }
}

#[cfg(any(windows, test))]
fn acquire_windows_acl_lease_and_recover_with(
    acquire: impl FnOnce() -> Result<()>,
    recover: impl FnOnce() -> Result<()>,
) -> Result<()> {
    acquire()?;
    recover()
}

#[cfg(windows)]
fn acquire_windows_acl_lease(file: &fs::File) -> Result<()> {
    let started = Instant::now();
    acquire_windows_acl_lease_with(
        WINDOWS_ACL_CONVERGENCE_RETRY_WINDOW,
        || started.elapsed(),
        std::thread::sleep,
        || file.try_lock_exclusive(),
    )
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

#[cfg(windows)]
fn windows_verify_file_handle(file: &cap_std::fs::File) -> Result<()> {
    use cap_std::fs::MetadataExt as _;
    if file.metadata()?.file_attributes() & 0x400 != 0 {
        bail!("extension file is a Windows reparse point");
    }
    use std::os::windows::io::AsRawHandle as _;
    windows_verify_security(file.as_raw_handle().cast())
}

#[cfg(windows)]
fn windows_verify_directory_handle(directory: &Dir) -> Result<()> {
    windows_reject_reparse_directory(directory)?;
    use std::os::windows::io::AsRawHandle as _;
    windows_verify_security(directory.as_raw_handle().cast())
}

#[cfg(windows)]
fn windows_reject_reparse_directory(directory: &Dir) -> Result<()> {
    use cap_std::fs::MetadataExt as _;
    if directory.dir_metadata()?.file_attributes() & 0x400 != 0 {
        bail!("extension directory is a Windows reparse point");
    }
    Ok(())
}

#[cfg(windows)]
fn windows_secure_handle<T: std::os::windows::io::AsRawHandle>(handle: &T) -> Result<()> {
    windows_set_private_security(handle.as_raw_handle().cast())?;
    windows_verify_security(handle.as_raw_handle().cast())
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WindowsFileIdentity {
    volume_serial_number: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
#[repr(C)]
struct WindowsFileTime {
    low_date_time: u32,
    high_date_time: u32,
}

#[cfg(windows)]
#[repr(C)]
struct WindowsByHandleFileInformation {
    file_attributes: u32,
    creation_time: WindowsFileTime,
    last_access_time: WindowsFileTime,
    last_write_time: WindowsFileTime,
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

#[cfg(windows)]
fn windows_file_identity(handle: *mut std::ffi::c_void) -> Result<WindowsFileIdentity> {
    let information = windows_file_information(handle)?;
    Ok(WindowsFileIdentity {
        volume_serial_number: information.volume_serial_number,
        file_index_high: information.file_index_high,
        file_index_low: information.file_index_low,
    })
}

#[cfg(windows)]
fn windows_file_information(
    handle: *mut std::ffi::c_void,
) -> Result<WindowsByHandleFileInformation> {
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFileInformationByHandle(
                file: *mut std::ffi::c_void,
                information: *mut WindowsByHandleFileInformation,
            ) -> i32;
        }
        let mut information = std::mem::MaybeUninit::uninit();
        if GetFileInformationByHandle(handle, information.as_mut_ptr()) == 0 {
            return Err(std::io::Error::last_os_error())
                .context("read Windows extension object identity");
        }
        let information = information.assume_init();
        if information.file_attributes & 0x400 != 0 {
            bail!("extension object is a Windows reparse point");
        }
        Ok(information)
    }
}

#[cfg(windows)]
fn windows_final_path(handle: *mut std::ffi::c_void) -> Result<Vec<u16>> {
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetFinalPathNameByHandleW(
                file: *mut std::ffi::c_void,
                path: *mut u16,
                path_size: u32,
                flags: u32,
            ) -> u32;
        }
        let required = GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, 0);
        if required == 0 {
            return Err(std::io::Error::last_os_error())
                .context("size Windows extension object path");
        }
        let mut path = vec![0_u16; required as usize];
        let written = GetFinalPathNameByHandleW(handle, path.as_mut_ptr(), path.len() as u32, 0);
        if written == 0 {
            return Err(std::io::Error::last_os_error())
                .context("read Windows extension object path");
        }
        if written as usize >= path.len() {
            bail!("Windows extension object path changed while it was read");
        }
        path.truncate(written as usize);
        path.push(0);
        Ok(path)
    }
}

#[cfg(windows)]
fn windows_verify_same_object(
    original: *mut std::ffi::c_void,
    reopened: *mut std::ffi::c_void,
) -> Result<()> {
    let original_identity = windows_file_identity(original)?;
    let reopened_identity = windows_file_identity(reopened)?;
    if original_identity != reopened_identity {
        bail!("extension object changed while reopening it for a Windows ACL update");
    }
    Ok(())
}

#[cfg(windows)]
fn windows_reopen_for_security(
    handle: *mut std::ffi::c_void,
) -> Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::FromRawHandle as _;

    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn CreateFileW(
                file_name: *const u16,
                desired_access: u32,
                share_mode: u32,
                security_attributes: *const std::ffi::c_void,
                creation_disposition: u32,
                flags_and_attributes: u32,
                template_file: *mut std::ffi::c_void,
            ) -> *mut std::ffi::c_void;
        }
        // Reopen the path reported by the capability handle itself. Opening
        // the reparse point prevents the final component from being followed.
        let path = windows_final_path(handle)?;
        let reopened = CreateFileW(
            path.as_ptr(),
            0x0008_0000 | 0x0004_0000 | 0x0002_0000,
            0x1 | 0x2 | 0x4,
            std::ptr::null(),
            3,
            0x0200_0000 | 0x0020_0000,
            std::ptr::null_mut(),
        );
        if reopened.is_null() || reopened as isize == -1 {
            return Err(std::io::Error::last_os_error())
                .context("reopen extension object for Windows ACL update");
        }
        let reopened = std::os::windows::io::OwnedHandle::from_raw_handle(reopened.cast());
        use std::os::windows::io::AsRawHandle as _;
        // A rename or replacement between path lookup and CreateFileW must not
        // redirect the ACL update to a different filesystem object.
        windows_verify_same_object(handle, reopened.as_raw_handle().cast())?;
        Ok(reopened)
    }
}

#[cfg(windows)]
#[repr(C)]
struct WindowsAcl {
    revision: u8,
    sbz1: u8,
    size: u16,
    ace_count: u16,
    sbz2: u16,
}

#[cfg(windows)]
#[repr(C)]
struct WindowsAceHeader {
    ace_type: u8,
    ace_flags: u8,
    ace_size: u16,
}

#[cfg(windows)]
#[repr(C)]
struct WindowsAccessAllowedAce {
    header: WindowsAceHeader,
    mask: u32,
    sid_start: u32,
}

#[cfg(windows)]
unsafe fn windows_current_user_sid() -> Result<Vec<u8>> {
    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut std::ffi::c_void,
        attributes: u32,
    }
    #[repr(C)]
    struct TokenUser {
        user: SidAndAttributes,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(
            process: *mut std::ffi::c_void,
            desired_access: u32,
            token: *mut *mut std::ffi::c_void,
        ) -> i32;
        fn GetTokenInformation(
            token: *mut std::ffi::c_void,
            class: u32,
            information: *mut std::ffi::c_void,
            length: u32,
            required: *mut u32,
        ) -> i32;
        fn GetLengthSid(sid: *mut std::ffi::c_void) -> u32;
    }
    let mut token = std::ptr::null_mut();
    if OpenProcessToken(GetCurrentProcess(), 0x0008, &mut token) == 0 {
        bail!("open current process token for extension ACL");
    }
    let mut required = 0;
    let _ = GetTokenInformation(token, 1, std::ptr::null_mut(), 0, &mut required);
    let mut buffer = vec![0_u8; required as usize];
    let ok = required != 0
        && GetTokenInformation(
            token,
            1,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        ) != 0;
    let _ = CloseHandle(token);
    if !ok {
        bail!("read current process user SID for extension ACL");
    }
    let token_user = std::ptr::read_unaligned(buffer.as_ptr().cast::<TokenUser>());
    let length = GetLengthSid(token_user.user.sid);
    if length == 0 {
        bail!("current process user SID is invalid");
    }
    Ok(std::slice::from_raw_parts(token_user.user.sid.cast::<u8>(), length as usize).to_vec())
}

#[cfg(windows)]
unsafe fn windows_sid_string(sid: *mut std::ffi::c_void) -> Result<String> {
    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertSidToStringSidW(sid: *mut std::ffi::c_void, value: *mut *mut u16) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }
    let mut value = std::ptr::null_mut();
    if ConvertSidToStringSidW(sid, &mut value) == 0 || value.is_null() {
        bail!("convert Windows SID for extension ACL");
    }
    let length = (0..).find(|&index| *value.add(index) == 0).unwrap_or(0);
    let result = String::from_utf16_lossy(std::slice::from_raw_parts(value, length));
    let _ = LocalFree(value.cast());
    Ok(result)
}

#[cfg(windows)]
fn windows_set_private_security(handle: *mut std::ffi::c_void) -> Result<()> {
    unsafe {
        #[link(name = "advapi32")]
        extern "system" {
            fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
                value: *const u16,
                revision: u32,
                descriptor: *mut *mut std::ffi::c_void,
                size: *mut u32,
            ) -> i32;
            fn GetSecurityDescriptorDacl(
                descriptor: *mut std::ffi::c_void,
                present: *mut i32,
                dacl: *mut *mut WindowsAcl,
                defaulted: *mut i32,
            ) -> i32;
            fn SetSecurityInfo(
                handle: *mut std::ffi::c_void,
                object_type: i32,
                information: u32,
                owner: *mut std::ffi::c_void,
                group: *mut std::ffi::c_void,
                dacl: *mut WindowsAcl,
                sacl: *mut WindowsAcl,
            ) -> u32;
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        }
        let user = windows_current_user_sid()?;
        let user_ptr = user.as_ptr().cast_mut().cast();
        let inheritance = if windows_file_information(handle)?.file_attributes & 0x10 != 0 {
            "OICI"
        } else {
            ""
        };
        let sddl = format!(
            "D:P(A;{inheritance};FA;;;{})(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;BA)",
            windows_sid_string(user_ptr)?
        );
        let wide = sddl
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut descriptor = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            bail!("build private extension ACL");
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = std::ptr::null_mut();
        let got_dacl =
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted);
        let result = if got_dacl == 0 || present == 0 || dacl.is_null() {
            Err(anyhow!("private extension ACL has no DACL"))
        } else {
            (|| -> Result<()> {
                let security_handle = windows_reopen_for_security(handle)?;
                use std::os::windows::io::AsRawHandle as _;
                let status = SetSecurityInfo(
                    security_handle.as_raw_handle().cast(),
                    1,
                    0x1 | 0x4 | 0x8000_0000,
                    user_ptr,
                    std::ptr::null_mut(),
                    dacl,
                    std::ptr::null_mut(),
                );
                if status == 0 {
                    Ok(())
                } else {
                    Err(anyhow!(
                        "set private extension ACL failed with Windows error {status}"
                    ))
                }
            })()
        };
        let _ = LocalFree(descriptor);
        result
    }
}

#[cfg(windows)]
fn windows_verify_security(handle: *mut std::ffi::c_void) -> Result<()> {
    unsafe {
        #[link(name = "advapi32")]
        extern "system" {
            fn GetSecurityInfo(
                handle: *mut std::ffi::c_void,
                object_type: i32,
                information: u32,
                owner: *mut *mut std::ffi::c_void,
                group: *mut *mut std::ffi::c_void,
                dacl: *mut *mut WindowsAcl,
                sacl: *mut *mut WindowsAcl,
                descriptor: *mut *mut std::ffi::c_void,
            ) -> u32;
            fn GetSecurityDescriptorControl(
                descriptor: *mut std::ffi::c_void,
                control: *mut u16,
                revision: *mut u32,
            ) -> i32;
            fn GetAce(acl: *const WindowsAcl, index: u32, ace: *mut *mut std::ffi::c_void) -> i32;
            fn EqualSid(left: *mut std::ffi::c_void, right: *mut std::ffi::c_void) -> i32;
            fn IsWellKnownSid(sid: *mut std::ffi::c_void, kind: i32) -> i32;
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        }
        let user = windows_current_user_sid()?;
        let user_ptr = user.as_ptr().cast_mut().cast();
        let object = windows_final_path(handle)
            .map(|path| {
                String::from_utf16_lossy(path.strip_suffix(&[0]).unwrap_or(path.as_slice()))
            })
            .unwrap_or_else(|_| "<unresolved extension object>".to_owned());
        let mut owner = std::ptr::null_mut();
        let mut dacl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        let status = GetSecurityInfo(
            handle,
            1,
            0x1 | 0x4,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        );
        if status != 0 {
            bail!("read extension ACL failed with Windows error {status}");
        }
        let result = (|| -> Result<()> {
            if owner.is_null() || EqualSid(owner, user_ptr) == 0 {
                bail!("extension object is not owned by the current Windows user");
            }
            if dacl.is_null() {
                bail!("extension object has an unprotected or absent Windows DACL");
            }
            let mut control = 0_u16;
            let mut revision = 0_u32;
            if GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) == 0
                || control & 0x1000 == 0
            {
                bail!("extension object Windows DACL is not protected");
            }
            let mut user_full_control = false;
            for index in 0..(*dacl).ace_count as u32 {
                let mut raw = std::ptr::null_mut();
                if GetAce(dacl, index, &mut raw) == 0 || raw.is_null() {
                    bail!("read extension Windows ACL entry {index} for {object}");
                }
                let ace = &*(raw.cast::<WindowsAccessAllowedAce>());
                if ace.header.ace_type == 1 {
                    continue;
                }
                if ace.header.ace_type != 0 {
                    bail!(
                        "extension Windows ACL for {object} contains unsupported ACE {index}: type={}, flags=0x{:02x}, mask=0x{:08x}",
                        ace.header.ace_type,
                        ace.header.ace_flags,
                        ace.mask
                    );
                }
                let sid = std::ptr::addr_of!(ace.sid_start).cast_mut().cast();
                let is_user = EqualSid(sid, user_ptr) != 0;
                let is_system = IsWellKnownSid(sid, 22) != 0;
                let is_admin = IsWellKnownSid(sid, 26) != 0;
                if !is_user && !is_system && !is_admin {
                    let sid = windows_sid_string(sid)
                        .unwrap_or_else(|_| "<invalid Windows SID>".to_owned());
                    return Err(anyhow!(WindowsAclDrift).context(format!(
                        "extension Windows ACL for {object} grants access to untrusted principal {sid} in ACE {index}: flags=0x{:02x}, mask=0x{:08x}, inherited={}",
                        ace.header.ace_flags,
                        ace.mask,
                        ace.header.ace_flags & 0x10 != 0
                    )));
                }
                if is_user && ace.mask & 0x001f_01ff == 0x001f_01ff {
                    user_full_control = true;
                }
            }
            if !user_full_control {
                bail!("extension Windows ACL does not grant the owner full control");
            }
            Ok(())
        })();
        let _ = LocalFree(descriptor);
        result
    }
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
fn verify_cap_directory_permissions_portable(directory: &Dir) -> Result<()> {
    #[cfg(windows)]
    return windows_verify_directory_handle(directory);
    #[cfg(not(windows))]
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

fn validate_sha256(label: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        bail!("invalid canonical SHA-256 for {label}");
    }
    Ok(())
}

fn read_small_local_file(path: &Path) -> Result<Vec<u8>> {
    let file = open_existing_no_follow(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_IN_MEMORY_BYTES {
        bail!("file is not a bounded regular file: {}", path.display());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_IN_MEMORY_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_IN_MEMORY_BYTES {
        bail!("file exceeds the in-memory limit: {}", path.display());
    }
    Ok(bytes)
}

fn hash_open_file(file: &mut fs::File, limit: u64) -> Result<String> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        bail!("archive exceeds the {limit} byte limit");
    }
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > limit {
            bail!("archive exceeds the {limit} byte limit");
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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
    use std::cell::Cell;
    use std::io::Cursor;
    use tempfile::TempDir;

    #[cfg(feature = "review-trust-root")]
    const REVIEW_SEED: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];

    fn acl_drift_error() -> anyhow::Error {
        anyhow!(WindowsAclDrift)
            .context("extension Windows ACL grants access to untrusted principal in test fixture")
    }

    #[test]
    fn windows_acl_convergence_retries_drift_then_succeeds() {
        let hardens = Cell::new(0);
        let verifies = Cell::new(0);
        let sleeps = Cell::new(0);
        let value = converge_windows_acl_with(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| sleeps.set(sleeps.get() + 1),
            || {
                hardens.set(hardens.get() + 1);
                Ok(())
            },
            || {
                verifies.set(verifies.get() + 1);
                if verifies.get() == 1 {
                    Err(acl_drift_error())
                } else {
                    Ok("verified")
                }
            },
        )
        .unwrap();

        assert_eq!(value, "verified");
        assert_eq!(hardens.get(), 2);
        assert_eq!(verifies.get(), 2);
        assert_eq!(sleeps.get(), 1);
    }

    #[test]
    fn windows_acl_convergence_retries_hardening_drift_then_succeeds() {
        let hardens = Cell::new(0);
        let verifies = Cell::new(0);
        let sleeps = Cell::new(0);
        let value = converge_windows_acl_with(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| sleeps.set(sleeps.get() + 1),
            || {
                hardens.set(hardens.get() + 1);
                if hardens.get() == 1 {
                    Err(acl_drift_error())
                } else {
                    Ok(())
                }
            },
            || {
                verifies.set(verifies.get() + 1);
                Ok("verified")
            },
        )
        .unwrap();

        assert_eq!(value, "verified");
        assert_eq!(hardens.get(), 2);
        assert_eq!(verifies.get(), 1);
        assert_eq!(sleeps.get(), 1);
    }

    #[test]
    fn windows_acl_convergence_stops_after_persistent_hardening_drift() {
        let hardens = Cell::new(0);
        let verifies = Cell::new(0);
        let error = converge_windows_acl_with::<()>(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| {},
            || {
                hardens.set(hardens.get() + 1);
                Err(acl_drift_error())
            },
            || {
                verifies.set(verifies.get() + 1);
                Ok(())
            },
        )
        .unwrap_err();

        assert!(error.downcast_ref::<WindowsAclDrift>().is_some());
        assert!(error
            .to_string()
            .contains("did not converge after 3 hardening attempt(s)"));
        assert_eq!(hardens.get(), 3);
        assert_eq!(verifies.get(), 0);
    }

    #[test]
    fn windows_acl_lease_retries_contention_until_acquired() {
        let attempts = Cell::new(0);
        let elapsed = Cell::new(Duration::ZERO);
        let sleeps = Cell::new(0);
        acquire_windows_acl_lease_with(
            Duration::from_secs(1),
            || elapsed.get(),
            |duration| {
                sleeps.set(sleeps.get() + 1);
                elapsed.set(elapsed.get() + duration);
            },
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() < 3 {
                    Err(fs2::lock_contended_error())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();

        assert_eq!(attempts.get(), 3);
        assert_eq!(sleeps.get(), 2);
    }

    #[test]
    fn windows_acl_lease_name_is_component_scoped() {
        assert_eq!(
            windows_acl_lease_name(PERCEPTION_ID),
            "cua-perception.runtime-acl.lock"
        );
    }

    #[test]
    fn extension_hooks_forbid_deferred_windows_acl_cleanup() {
        let root = Path::new("extension-root");
        let journal = Some(PathBuf::from("runtime-acl.lock.profile"));
        let limits = extension_hook_containment_limits(root, journal.clone());

        assert_eq!(limits.additional_readable_paths, vec![root.to_path_buf()]);
        assert!(limits.windows_require_synchronous_cleanup);
        assert!(limits.windows_acl_lease_path.is_none());
        assert_eq!(limits.windows_acl_profile_journal_path, journal);
    }

    #[test]
    fn lifecycle_acl_lock_recovers_before_entering_the_critical_section() {
        let acquired = Cell::new(false);
        let recovered = Cell::new(false);
        acquire_windows_acl_lease_and_recover_with(
            || {
                acquired.set(true);
                Ok(())
            },
            || {
                assert!(acquired.get());
                recovered.set(true);
                Ok(())
            },
        )
        .unwrap();

        assert!(recovered.get());
    }

    #[test]
    fn windows_acl_lease_timeout_never_enters_the_critical_section() {
        let attempts = Cell::new(0);
        let error = acquire_windows_acl_lease_with(
            Duration::ZERO,
            || Duration::ZERO,
            |_| panic!("an expired ACL lease wait must not sleep"),
            || {
                attempts.set(attempts.get() + 1);
                Err(fs2::lock_contended_error())
            },
        )
        .unwrap_err();

        assert_eq!(attempts.get(), 1);
        assert!(error
            .to_string()
            .contains("timed out waiting for the Windows ACL lease"));
    }

    #[test]
    fn windows_acl_convergence_stops_after_persistent_drift() {
        let hardens = Cell::new(0);
        let verifies = Cell::new(0);
        let error = converge_windows_acl_with::<()>(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| {},
            || {
                hardens.set(hardens.get() + 1);
                Ok(())
            },
            || {
                verifies.set(verifies.get() + 1);
                Err(acl_drift_error())
            },
        )
        .unwrap_err();

        assert!(error.downcast_ref::<WindowsAclDrift>().is_some());
        assert!(error
            .to_string()
            .contains("did not converge after 3 hardening attempt(s)"));
        assert_eq!(hardens.get(), 3);
        assert_eq!(verifies.get(), 3);
    }

    #[test]
    fn windows_acl_convergence_does_not_retry_matching_untyped_text() {
        let hardens = Cell::new(0);
        let error = converge_windows_acl_with::<()>(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| panic!("non-drift verification failure must not sleep"),
            || {
                hardens.set(hardens.get() + 1);
                Ok(())
            },
            || bail!("extension Windows ACL grants access to untrusted principal"),
        )
        .unwrap_err();

        assert!(error.downcast_ref::<WindowsAclDrift>().is_none());
        assert_eq!(hardens.get(), 1);
    }

    #[test]
    fn windows_acl_convergence_does_not_retry_hardening_failure() {
        let hardens = Cell::new(0);
        let verifies = Cell::new(0);
        let error = converge_windows_acl_with::<()>(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| panic!("hardening failure must not sleep"),
            || {
                hardens.set(hardens.get() + 1);
                bail!("hardening failed")
            },
            || {
                verifies.set(verifies.get() + 1);
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "hardening failed");
        assert_eq!(hardens.get(), 1);
        assert_eq!(verifies.get(), 0);
    }

    #[test]
    fn windows_acl_convergence_honors_retry_window_after_attempt() {
        let hardens = Cell::new(0);
        let sleeps = Cell::new(0);
        let elapsed = Cell::new(std::time::Duration::ZERO);
        let error = converge_windows_acl_with::<()>(
            3,
            std::time::Duration::from_millis(250),
            || elapsed.get(),
            |_| sleeps.set(sleeps.get() + 1),
            || {
                hardens.set(hardens.get() + 1);
                Ok(())
            },
            || {
                elapsed.set(std::time::Duration::from_millis(300));
                Err(acl_drift_error())
            },
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("did not converge before the 250ms retry window"));
        assert!(error.downcast_ref::<WindowsAclDrift>().is_some());
        assert_eq!(hardens.get(), 1);
        assert_eq!(sleeps.get(), 0);
    }

    #[test]
    fn windows_startup_verification_hardens_and_retries_typed_acl_drift() {
        let hardens = Cell::new(0);
        let verifies = Cell::new(0);
        let sleeps = Cell::new(0);
        let manifest = converge_windows_startup_verification_with(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| sleeps.set(sleeps.get() + 1),
            || {
                hardens.set(hardens.get() + 1);
                Ok(())
            },
            || {
                verifies.set(verifies.get() + 1);
                if verifies.get() == 1 {
                    Err(acl_drift_error())
                } else {
                    Ok("startup manifest")
                }
            },
        )
        .unwrap();

        assert_eq!(manifest, "startup manifest");
        assert_eq!(hardens.get(), 1);
        assert_eq!(verifies.get(), 2);
        assert_eq!(sleeps.get(), 0);
    }

    #[test]
    fn windows_startup_verification_does_not_rewrite_a_healthy_tree() {
        let hardens = Cell::new(0);
        let verifies = Cell::new(0);
        let manifest = converge_windows_startup_verification_with(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| panic!("healthy startup verification must not sleep"),
            || {
                hardens.set(hardens.get() + 1);
                Ok(())
            },
            || {
                verifies.set(verifies.get() + 1);
                Ok("startup manifest")
            },
        )
        .unwrap();

        assert_eq!(manifest, "startup manifest");
        assert_eq!(hardens.get(), 0);
        assert_eq!(verifies.get(), 1);
    }

    #[test]
    fn windows_startup_verification_preserves_non_acl_failure() {
        let hardens = Cell::new(0);
        let verifies = Cell::new(0);
        let error = converge_windows_startup_verification_with::<()>(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| panic!("non-ACL startup verification failure must not sleep"),
            || {
                hardens.set(hardens.get() + 1);
                Ok(())
            },
            || {
                verifies.set(verifies.get() + 1);
                bail!("SHA-256 mismatch for startup worker")
            },
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "SHA-256 mismatch for startup worker");
        assert!(error.downcast_ref::<WindowsAclDrift>().is_none());
        assert_eq!(hardens.get(), 0);
        assert_eq!(verifies.get(), 1);
    }

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

    fn perception_fixture_archive(directory: &Path, version: &str, runtime_name: &str) -> PathBuf {
        let worker = b"fixture-worker";
        let helper = b"fixture-helper";
        let model_manifest = b"{}";
        let runtime = b"fixture-runtime";
        let model = b"fixture-model";
        let dictionary = b"fixture-dictionary";
        let notice = b"fixture-notice";
        let worker_name = if cfg!(windows) {
            "cua-perception.exe"
        } else {
            "cua-perception"
        };
        let installed_runtime_name = if cfg!(target_os = "windows") {
            "onnxruntime.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        };
        let contract = serde_json::to_vec(&serde_json::json!({
            "$schema": "runtime-contract.schema.json",
            "schemaVersion": 1,
            "target": current_target().unwrap(),
            "protocolVersion": 1,
            "worker": {"name": worker_name, "sha256": hex_sha256(worker)},
            "runtime": {"name": runtime_name, "sha256": hex_sha256(runtime)},
            "models": [
                {"name": "icon.onnx", "role": "icon-detect", "sha256": hex_sha256(model)},
                {"name": "ocr-det.onnx", "role": "ocr-detect", "sha256": hex_sha256(model)},
                {"name": "ocr-rec.onnx", "role": "ocr-recognize", "sha256": hex_sha256(model)}
            ],
            "dictionary": {"name": "dictionary.txt", "role": "ocr-dictionary", "sha256": hex_sha256(dictionary)},
            "rejectMismatch": true
        }))
        .unwrap();
        let entrypoint = format!("bin/{worker_name}");
        let runtime_path = format!("runtime/{installed_runtime_name}");
        let files = vec![
            (entrypoint.clone(), worker.as_slice(), true),
            ("bin/helper.dat".to_owned(), helper.as_slice(), false),
            (
                "models/model-manifest.json".to_owned(),
                model_manifest.as_slice(),
                false,
            ),
            (runtime_path, runtime.as_slice(), false),
            ("models/icon.onnx".to_owned(), model.as_slice(), false),
            ("models/ocr-det.onnx".to_owned(), model.as_slice(), false),
            ("models/ocr-rec.onnx".to_owned(), model.as_slice(), false),
            (
                "models/dictionary.txt".to_owned(),
                dictionary.as_slice(),
                false,
            ),
            (
                PERCEPTION_RUNTIME_CONTRACT.to_owned(),
                contract.as_slice(),
                false,
            ),
            (
                "notices/THIRD_PARTY_NOTICES.md".to_owned(),
                notice.as_slice(),
                false,
            ),
        ];
        let manifest = ExtensionManifest {
            schema_version: 1,
            id: PERCEPTION_ID.to_owned(),
            version: version.to_owned(),
            driver_version: format!("={}", env!("CARGO_PKG_VERSION")),
            protocol_version: 1,
            target: current_target().unwrap(),
            entrypoint,
            files: files
                .iter()
                .map(|(path, bytes, executable)| ManifestFile {
                    path: path.clone(),
                    sha256: hex_sha256(bytes),
                    executable: *executable,
                })
                .collect(),
            models: Vec::new(),
            components: vec![ComponentLicense {
                name: PERCEPTION_ID.to_owned(),
                version: version.to_owned(),
                license: "Apache-2.0".to_owned(),
                notice: "fixture".to_owned(),
                source_uri: "https://github.com/trycua/cua".to_owned(),
                source_revision: "fixture".to_owned(),
                notice_file: None,
            }],
            corresponding_source_file: None,
            license: "Apache-2.0".to_owned(),
            source: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_uri: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_revision: "fixture".to_owned(),
            provenance: "fixture".to_owned(),
            health_args: Vec::new(),
            self_test_args: Vec::new(),
        };
        let path = directory.join(format!("perception-{version}.tar.gz"));
        let file = fs::File::create(&path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append(
            &mut builder,
            MANIFEST_NAME,
            &serde_json::to_vec_pretty(&manifest).unwrap(),
        );
        for (relative, bytes, _) in files {
            append(&mut builder, &relative, bytes);
        }
        builder.finish().unwrap();
        path
    }

    #[test]
    fn absent_perception_extension_has_no_worker_config() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        assert!(perception_worker_config_in(&store).unwrap().is_none());
        assert!(!store.root.exists());
    }

    #[test]
    fn active_perception_extension_resolves_owned_worker_arguments() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let runtime_name = if cfg!(target_os = "windows") {
            "onnxruntime.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        };
        let archive = perception_fixture_archive(temp.path(), "1.0.0", runtime_name);
        store
            .install_archive(registry_entry(PERCEPTION_ID).unwrap(), &archive)
            .unwrap();

        let config = perception_worker_config_in(&store).unwrap().unwrap();
        assert!(config.executable.ends_with(if cfg!(windows) {
            "bin/cua-perception.exe"
        } else {
            "bin/cua-perception"
        }));
        assert_eq!(config.args[0], "--manifest");
        assert!(config.args[1].ends_with("models/model-manifest.json"));
        assert_eq!(config.args[2], "--onnx-runtime-library");
        assert!(config.args[3].ends_with(&format!("runtime/{runtime_name}")));
        assert_eq!(
            &config.args[4..],
            [
                "--extension-id",
                "cua-perception",
                "--extension-version",
                "1.0.0"
            ]
        );
        assert!(config.warm_worker.is_none());
        assert_eq!(
            config
                .expected_extension_identity
                .as_ref()
                .map(|identity| (identity.id.as_str(), identity.version.as_str(),)),
            Some(("cua-perception", "1.0.0"))
        );
    }

    #[test]
    fn perception_startup_rejects_tampered_execution_inputs() {
        let runtime_name = if cfg!(target_os = "windows") {
            "onnxruntime.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        };
        let worker = if cfg!(windows) {
            "bin/cua-perception.exe"
        } else {
            "bin/cua-perception"
        };
        for relative in [
            worker.to_owned(),
            "bin/helper.dat".to_owned(),
            "models/model-manifest.json".to_owned(),
            "models/icon.onnx".to_owned(),
            format!("runtime/{runtime_name}"),
            "models/dictionary.txt".to_owned(),
            PERCEPTION_RUNTIME_CONTRACT.to_owned(),
        ] {
            let temp = TempDir::new().unwrap();
            let store = ExtensionStore::new(temp.path().join("extensions"));
            let archive = perception_fixture_archive(temp.path(), "1.0.0", runtime_name);
            store
                .install_archive(registry_entry(PERCEPTION_ID).unwrap(), &archive)
                .unwrap();
            let active = store.active_path(PERCEPTION_ID).unwrap().unwrap();
            fs::write(active.join(&relative), b"tampered execution input").unwrap();

            let error = perception_worker_config_in(&store).unwrap_err();
            assert!(
                error.to_string().contains("SHA-256 mismatch"),
                "relative={relative}, error={error:#}"
            );
        }
    }

    #[test]
    fn nonexecuted_tampering_is_deferred_to_full_verification() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let runtime_name = if cfg!(target_os = "windows") {
            "onnxruntime.dll"
        } else if cfg!(target_os = "macos") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        };
        let archive = perception_fixture_archive(temp.path(), "1.0.0", runtime_name);
        store
            .install_archive(registry_entry(PERCEPTION_ID).unwrap(), &archive)
            .unwrap();
        let active = store.active_path(PERCEPTION_ID).unwrap().unwrap();
        fs::write(
            active.join("notices/THIRD_PARTY_NOTICES.md"),
            b"tampered notice",
        )
        .unwrap();

        perception_worker_config_in(&store).unwrap().unwrap();
        let installed = open_directory_path_nofollow(&active).unwrap();
        let error =
            verify_installed_version_at(&installed, registry_entry(PERCEPTION_ID).unwrap(), None)
                .unwrap_err();
        assert!(error.to_string().contains("SHA-256 mismatch"));
    }

    #[test]
    fn perception_runtime_contract_cannot_escape_the_active_directory() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let archive = perception_fixture_archive(temp.path(), "1.0.0", "../outside-runtime");
        store
            .install_archive(registry_entry(PERCEPTION_ID).unwrap(), &archive)
            .unwrap();
        let error = perception_worker_config_in(&store).unwrap_err();
        assert!(error.to_string().contains("unsafe runtime name"));
    }

    #[cfg(unix)]
    #[test]
    fn perception_resolver_rejects_symlinked_installed_payload() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let runtime_name = if cfg!(target_os = "macos") {
            "libonnxruntime.dylib"
        } else {
            "libonnxruntime.so"
        };
        let archive = perception_fixture_archive(temp.path(), "1.0.0", runtime_name);
        store
            .install_archive(registry_entry(PERCEPTION_ID).unwrap(), &archive)
            .unwrap();
        let active = store.active_path(PERCEPTION_ID).unwrap().unwrap();
        let runtime = active.join("runtime").join(runtime_name);
        fs::remove_file(&runtime).unwrap();
        symlink(temp.path().join("outside-runtime"), &runtime).unwrap();

        let error = perception_worker_config_in(&store).unwrap_err();
        assert!(error.to_string().contains("symbolic link"));
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
        let manifest = fixture_manifest(
            version,
            target,
            protocol,
            declared_hash,
            driver_version,
            executable,
        );
        write_fixture_archive(directory, version, &manifest, payload)
    }

    /// The same fixture extension, but declaring a self-test hook so a test can
    /// observe when the installer runs it.
    fn fixture_archive_with_self_test(
        directory: &Path,
        version: &str,
        target: &str,
        payload: &[u8],
        self_test_args: &[&str],
    ) -> PathBuf {
        let mut manifest = fixture_manifest(
            version,
            target,
            1,
            &hex_sha256(payload),
            &format!("={}", env!("CARGO_PKG_VERSION")),
            true,
        );
        manifest.self_test_args = self_test_args.iter().map(|arg| (*arg).to_owned()).collect();
        write_fixture_archive(directory, version, &manifest, payload)
    }

    fn write_fixture_archive(
        directory: &Path,
        version: &str,
        manifest: &ExtensionManifest,
        payload: &[u8],
    ) -> PathBuf {
        let path = directory.join(format!("local-extension-{version}.tar.gz"));
        let file = fs::File::create(&path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append(
            &mut builder,
            MANIFEST_NAME,
            &serde_json::to_vec_pretty(manifest).unwrap(),
        );
        append(&mut builder, "bin/local-extension", payload);
        builder.finish().unwrap();
        path
    }

    fn fixture_manifest(
        version: &str,
        target: &str,
        protocol: u32,
        declared_hash: &str,
        driver_version: &str,
        executable: bool,
    ) -> ExtensionManifest {
        ExtensionManifest {
            schema_version: 1,
            id: "cua-perception".to_owned(),
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
            models: Vec::new(),
            components: vec![ComponentLicense {
                name: "cua-perception".to_owned(),
                version: version.to_owned(),
                license: "Apache-2.0".to_owned(),
                notice: "Copyright Cua contributors".to_owned(),
                source_uri: "https://github.com/trycua/cua".to_owned(),
                source_revision: "test-fixture".to_owned(),
                notice_file: None,
            }],
            corresponding_source_file: None,
            license: "Apache-2.0".to_owned(),
            source: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_uri: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_revision: "test-fixture".to_owned(),
            provenance: "developer test fixture".to_owned(),
            health_args: Vec::new(),
            self_test_args: Vec::new(),
        }
    }

    #[cfg(feature = "review-trust-root")]
    fn review_fixture(directory: &Path) -> (PathBuf, PathBuf) {
        use ring::signature::Ed25519KeyPair;

        let worker = b"#!/bin/sh\nexit 0\n";
        let model = b"model";
        let notice = b"notice";
        let source = b"source";
        let model_license = b"model license";
        let bindings = [
            ("bin/cua-perception", worker.as_slice(), true),
            ("models/parser.bin", model.as_slice(), false),
            ("LICENSES/NOTICE.txt", notice.as_slice(), false),
            ("LICENSES/model.txt", model_license.as_slice(), false),
            ("SOURCE/source.txt", source.as_slice(), false),
        ];
        let manifest = ExtensionManifest {
            schema_version: 1,
            id: PERCEPTION_ID.to_owned(),
            version: "1.0.0".to_owned(),
            driver_version: format!("={}", env!("CARGO_PKG_VERSION")),
            protocol_version: 1,
            target: current_target().unwrap(),
            entrypoint: "bin/cua-perception".to_owned(),
            files: bindings
                .iter()
                .map(|(path, bytes, executable)| ManifestFile {
                    path: (*path).to_owned(),
                    sha256: hex_sha256(bytes),
                    executable: *executable,
                })
                .collect(),
            models: vec![ManifestModel {
                path: "models/parser.bin".to_owned(),
                revision: "model-v1".to_owned(),
                original_sha256: "1".repeat(64),
                conversion_sha256: hex_sha256(model),
                license_file: Some(ArtifactBinding {
                    path: "LICENSES/model.txt".to_owned(),
                    sha256: hex_sha256(model_license),
                }),
            }],
            components: vec![ComponentLicense {
                name: PERCEPTION_ID.to_owned(),
                version: "1.0.0".to_owned(),
                license: "AGPL-3.0-or-later".to_owned(),
                notice: "fixture notice".to_owned(),
                source_uri: "https://github.com/trycua/cua".to_owned(),
                source_revision: "fixture".to_owned(),
                notice_file: Some(ArtifactBinding {
                    path: "LICENSES/NOTICE.txt".to_owned(),
                    sha256: hex_sha256(notice),
                }),
            }],
            corresponding_source_file: Some(ArtifactBinding {
                path: "SOURCE/source.txt".to_owned(),
                sha256: hex_sha256(source),
            }),
            license: "AGPL-3.0-or-later".to_owned(),
            source: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_uri: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_revision: "fixture".to_owned(),
            provenance: "review fixture".to_owned(),
            health_args: Vec::new(),
            self_test_args: Vec::new(),
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        let archive = directory.join("review-extension.tar.gz");
        let encoder = GzEncoder::new(fs::File::create(&archive).unwrap(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append(&mut builder, MANIFEST_NAME, &manifest_bytes);
        for (path, bytes, _) in bindings {
            append(&mut builder, path, bytes);
        }
        builder.finish().unwrap();
        drop(builder);
        let archive_bytes = fs::read(&archive).unwrap();
        let payload = CatalogPayload {
            schema_version: 1,
            catalog_version: 1,
            expires_unix: u64::MAX,
            publisher_id: REVIEW_PUBLISHER_ID.to_owned(),
            publisher_name: REVIEW_PUBLISHER_NAME.to_owned(),
            key_id: REVIEW_KEY_ID.to_owned(),
            extension_id: PERCEPTION_ID.to_owned(),
            version: manifest.version.clone(),
            target: manifest.target.clone(),
            archive: archive.file_name().unwrap().to_str().unwrap().to_owned(),
            archive_size: archive_bytes.len() as u64,
            archive_sha256: hex_sha256(&archive_bytes),
            manifest_sha256: hex_sha256(&manifest_bytes),
            license: manifest.license.clone(),
            source: manifest.source.clone(),
            corresponding_source_uri: manifest.corresponding_source_uri.clone(),
            corresponding_source_revision: manifest.corresponding_source_revision.clone(),
            provenance: manifest.provenance.clone(),
            next_key: None,
        };
        let pair = Ed25519KeyPair::from_seed_unchecked(&REVIEW_SEED).unwrap();
        let signed = sign_test_catalog(&pair, payload);
        let catalog = directory.join("review-catalog.json");
        fs::write(&catalog, serde_json::to_vec_pretty(&signed).unwrap()).unwrap();
        (archive, catalog)
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
        let entry = registry_entry("cua-perception").unwrap();

        let first = store.install_archive(entry, &archive).unwrap();
        let second = store.install_archive(entry, &archive).unwrap();

        assert_eq!(first.path, second.path);
        assert_eq!(
            store.active_path("cua-perception").unwrap(),
            Some(first.path)
        );
        assert!(store.info(entry, false).unwrap().healthy);
    }

    #[test]
    fn rejects_corrupt_target_and_protocol_mismatches() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("cua-perception").unwrap();

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
        assert!(store.active_path("cua-perception").unwrap().is_none());
    }

    #[test]
    fn malformed_manifest_version_cleans_staging() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("cua-perception").unwrap();
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
        let entry = registry_entry("cua-perception").unwrap();

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
            vec!["inspect", "cua-perception", "extra"],
            vec!["status", "--bogus"],
            vec!["status", "--json", "--json"],
            vec!["install", "cua-perception", "--archive"],
            vec!["install", "cua-perception", "--archive", "a", "--archive=b"],
            vec!["remove", "cua-perception", "--archive=a"],
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            assert!(parse_command(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn mutation_platform_contract_is_explicit() {
        assert!(ensure_mutations_supported().is_ok());
    }

    #[test]
    fn activation_failure_restores_previous_version() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("cua-perception").unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        store.install_archive(entry, &archive).unwrap();

        let lock = store.lock("cua-perception").unwrap();
        let error = store
            .activate_locked(
                &lock.root,
                "cua-perception",
                "2.0.0",
                ActivationFailpoint::AfterBackup,
            )
            .unwrap_err();
        drop(lock);

        assert!(error.to_string().contains("simulated interruption"));
        let lock = store.lock("cua-perception").unwrap();
        assert_eq!(
            store
                .active_pointer_locked(&lock.root, "cua-perception")
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
        let entry = registry_entry("cua-perception").unwrap();
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
            .info(registry_entry("cua-perception").unwrap(), false)
            .unwrap();
        assert!(!info.installed);
        assert!(info.healthy);
        assert_eq!(info.detail, "not installed (healthy)");
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
            let entry = registry_entry("cua-perception").unwrap();
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

    #[cfg(windows)]
    #[test]
    fn hardens_a_precreated_extension_root_with_a_broad_acl() {
        let temp = TempDir::new().unwrap();
        let root_path = temp.path().join("extensions");
        fs::create_dir(&root_path).unwrap();
        let root = open_directory_path_nofollow(&root_path).unwrap();
        windows_secure_handle(&root).unwrap();
        drop(root);
        let acl = std::process::Command::new("icacls")
            .arg(&root_path)
            .args(["/grant", "*S-1-1-0:(W)"])
            .output()
            .unwrap();
        assert!(
            acl.status.success(),
            "{}",
            String::from_utf8_lossy(&acl.stderr)
        );

        let broad = open_directory_path_nofollow(&root_path).unwrap();
        assert!(windows_verify_directory_handle(&broad)
            .unwrap_err()
            .to_string()
            .contains("untrusted principal"));
        drop(broad);

        let hardened = ensure_private_directory_path(&root_path).unwrap();
        windows_verify_directory_handle(&hardened).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn installed_directory_tree_inherits_only_private_acl_entries() {
        use std::os::windows::io::AsRawHandle as _;

        unsafe fn assert_private_inheritance(directory: &Dir, relative: &Path) {
            #[link(name = "advapi32")]
            extern "system" {
                fn GetSecurityInfo(
                    handle: *mut std::ffi::c_void,
                    object_type: i32,
                    information: u32,
                    owner: *mut *mut std::ffi::c_void,
                    group: *mut *mut std::ffi::c_void,
                    dacl: *mut *mut WindowsAcl,
                    sacl: *mut *mut WindowsAcl,
                    descriptor: *mut *mut std::ffi::c_void,
                ) -> u32;
                fn GetAce(
                    acl: *const WindowsAcl,
                    index: u32,
                    ace: *mut *mut std::ffi::c_void,
                ) -> i32;
            }
            #[link(name = "kernel32")]
            extern "system" {
                fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
            }

            let mut dacl = std::ptr::null_mut();
            let mut descriptor = std::ptr::null_mut();
            let status = GetSecurityInfo(
                directory.as_raw_handle().cast(),
                1,
                0x4,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            );
            assert_eq!(status, 0, "read ACL for {}", relative.display());
            assert!(!dacl.is_null(), "missing DACL for {}", relative.display());
            for index in 0..(*dacl).ace_count as u32 {
                let mut raw = std::ptr::null_mut();
                assert_ne!(
                    GetAce(dacl, index, &mut raw),
                    0,
                    "read ACE {index} for {}",
                    relative.display()
                );
                let ace = &*(raw.cast::<WindowsAccessAllowedAce>());
                if ace.header.ace_type == 0 {
                    assert_eq!(
                        ace.header.ace_flags & 0x3,
                        0x3,
                        "directory ACE {index} does not propagate privately at {}",
                        relative.display()
                    );
                }
            }
            let _ = LocalFree(descriptor);

            for child in directory.entries().unwrap() {
                let child = child.unwrap();
                if child.file_type().unwrap().is_dir() {
                    let child_relative = relative.join(child.file_name());
                    let child_directory = directory.open_dir_nofollow(child.file_name()).unwrap();
                    assert_private_inheritance(&child_directory, &child_relative);
                }
            }
        }

        let temp = TempDir::new().unwrap();
        let acl = std::process::Command::new("icacls")
            .arg(temp.path())
            .args(["/grant", "*S-1-1-0:(OI)(CI)(W)"])
            .output()
            .unwrap();
        assert!(
            acl.status.success(),
            "{}",
            String::from_utf8_lossy(&acl.stderr)
        );
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("cua-perception").unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );

        store.install_archive(entry, &archive).unwrap();
        let root = open_directory_path_nofollow(&store.root).unwrap();
        unsafe { assert_private_inheritance(&root, Path::new("extensions")) };
    }

    #[cfg(windows)]
    #[test]
    fn acl_hardening_reopens_directory_and_file_handles() {
        use std::os::windows::io::AsRawHandle as _;

        let temp = TempDir::new().unwrap();
        let parent = open_directory_path_nofollow(temp.path()).unwrap();

        create_private_subdirectory(&parent, std::ffi::OsStr::new("private")).unwrap();
        let private = parent.open_dir_nofollow("private").unwrap();
        let reopened_directory =
            windows_reopen_for_security(private.as_raw_handle().cast()).unwrap();
        windows_verify_same_object(
            private.as_raw_handle().cast(),
            reopened_directory.as_raw_handle().cast(),
        )
        .unwrap();
        windows_verify_directory_handle(&private).unwrap();

        write_new_file_at(&private, "owned", b"content").unwrap();
        let mut options = CapOpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = private.open_with("owned", &options).unwrap();
        let reopened_file = windows_reopen_for_security(file.as_raw_handle().cast()).unwrap();
        windows_verify_same_object(
            file.as_raw_handle().cast(),
            reopened_file.as_raw_handle().cast(),
        )
        .unwrap();
        assert!(private_regular_file_or_missing_at(&private, "owned").unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn recursive_acl_hardening_removes_package_access_from_staged_tree() {
        let temp = TempDir::new().unwrap();
        let parent = open_directory_path_nofollow(temp.path()).unwrap();
        create_private_subdirectory(&parent, std::ffi::OsStr::new("staged")).unwrap();
        let staged = parent.open_dir_nofollow("staged").unwrap();
        create_private_subdirectory(&staged, std::ffi::OsStr::new("nested")).unwrap();
        let nested = staged.open_dir_nofollow("nested").unwrap();
        write_new_file_at(&nested, "payload", b"content").unwrap();

        for path in [
            temp.path().join("staged"),
            temp.path().join("staged/nested"),
            temp.path().join("staged/nested/payload"),
        ] {
            let output = std::process::Command::new("icacls")
                .arg(&path)
                .arg("/grant")
                .arg("*S-1-15-2-1:(RX)")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "icacls failed for {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        assert!(windows_verify_directory_handle(&staged).is_err());
        assert!(inspect_owned_tree(&staged).is_err());

        windows_harden_private_tree(&staged).unwrap();
        windows_verify_directory_handle(&staged).unwrap();
        inspect_owned_tree(&staged).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn recursive_acl_hardening_reasserts_root_after_descendant_walk() {
        let temp = TempDir::new().unwrap();
        let parent = open_directory_path_nofollow(temp.path()).unwrap();
        create_private_subdirectory(&parent, std::ffi::OsStr::new("staged")).unwrap();
        let staged = parent.open_dir_nofollow("staged").unwrap();
        create_private_subdirectory(&staged, std::ffi::OsStr::new("nested")).unwrap();
        let nested = staged.open_dir_nofollow("nested").unwrap();
        write_new_file_at(&nested, "payload", b"content").unwrap();

        let injected = Cell::new(false);
        windows_harden_private_tree_with(&staged, &mut |directory| {
            if injected.replace(true) {
                return;
            }
            let output = std::process::Command::new("icacls")
                .arg(temp.path().join("staged"))
                .arg("/grant")
                .arg("*S-1-15-2-1:(RX)")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "icacls failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(windows_verify_directory_handle(directory).is_err());
        })
        .unwrap();

        assert!(injected.get());
        windows_verify_directory_handle(&staged).unwrap();
        inspect_owned_tree(&staged).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn staged_install_verification_survives_acl_drift_during_recursive_hardening() {
        let temp = TempDir::new().unwrap();
        let parent = open_directory_path_nofollow(temp.path()).unwrap();
        create_private_subdirectory(&parent, std::ffi::OsStr::new("staged")).unwrap();
        let staged = parent.open_dir_nofollow("staged").unwrap();
        let entry = registry_entry(PERCEPTION_ID).unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        let source = InstallSource {
            archive: archive.clone(),
            trust: TrustClass::DeveloperUnsignedLocal,
            catalog: None,
            signed_catalog: None,
            trust_update: None,
        };
        let inspected = inspect_archive(&archive, entry, &staged).unwrap();
        write_install_record_at(&staged, &inspected, &source).unwrap();

        let hardens = Cell::new(0);
        let injected = Cell::new(false);
        let manifest = converge_windows_acl_with(
            3,
            std::time::Duration::from_secs(90),
            || std::time::Duration::ZERO,
            |_| {},
            || {
                hardens.set(hardens.get() + 1);
                windows_harden_private_tree_with(&staged, &mut |_| {
                    if !injected.replace(true) {
                        let output = std::process::Command::new("icacls")
                            .arg(temp.path().join("staged"))
                            .arg("/grant")
                            .arg("*S-1-15-2-1:(RX)")
                            .output()
                            .unwrap();
                        assert!(
                            output.status.success(),
                            "icacls failed: {}",
                            String::from_utf8_lossy(&output.stderr)
                        );
                    }
                })
            },
            || verify_installed_version_at(&staged, entry, Some(&inspected.manifest_bytes)),
        )
        .unwrap();

        assert_eq!(manifest.version, "1.0.0");
        assert!(injected.get());
        assert_eq!(hardens.get(), 1);
        windows_verify_directory_handle(&staged).unwrap();
        inspect_owned_tree(&staged).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn post_hook_acl_grant_is_removed_before_staged_version_is_placed() {
        let temp = TempDir::new().unwrap();
        let root = open_directory_path_nofollow(temp.path()).unwrap();
        create_private_subdirectory(&root, std::ffi::OsStr::new("staging")).unwrap();
        create_private_subdirectory(&root, std::ffi::OsStr::new("versions")).unwrap();
        let staging = root.open_dir_nofollow("staging").unwrap();
        let versions = root.open_dir_nofollow("versions").unwrap();
        let entry = registry_entry(PERCEPTION_ID).unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        let source = InstallSource {
            archive: archive.clone(),
            trust: TrustClass::DeveloperUnsignedLocal,
            catalog: None,
            signed_catalog: None,
            trust_update: None,
        };
        let inspected = inspect_archive(&archive, entry, &staging).unwrap();
        write_install_record_at(&staging, &inspected, &source).unwrap();

        run_verified_extension_hook_with(
            &staging,
            entry,
            Some(&inspected.manifest_bytes),
            &inspected.manifest.version,
            || {
                let grant = std::process::Command::new("icacls")
                    .arg(temp.path().join("staging"))
                    .arg("/grant")
                    .arg("*S-1-15-2-1:(OI)(CI)(RX)")
                    .output()?;
                if !grant.status.success() {
                    bail!("icacls failed: {}", String::from_utf8_lossy(&grant.stderr));
                }
                assert!(verify_installed_version_at(
                    &staging,
                    entry,
                    Some(&inspected.manifest_bytes)
                )
                .is_err());
                Ok(())
            },
        )
        .unwrap();
        drop(staging);
        root.rename("staging", &versions, "1.0.0").unwrap();
        let installed = versions.open_dir_nofollow("1.0.0").unwrap();

        let manifest =
            converge_windows_installed_version(&installed, entry, Some(&inspected.manifest_bytes))
                .unwrap();
        assert_eq!(manifest.version, "1.0.0");
        windows_verify_directory_handle(&installed).unwrap();
        inspect_owned_tree(&installed).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn health_hook_acl_grant_is_removed_before_status_returns_healthy() {
        let temp = TempDir::new().unwrap();
        let root = open_directory_path_nofollow(temp.path()).unwrap();
        create_private_subdirectory(&root, std::ffi::OsStr::new("installed")).unwrap();
        let installed = root.open_dir_nofollow("installed").unwrap();
        let entry = registry_entry(PERCEPTION_ID).unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        let source = InstallSource {
            archive: archive.clone(),
            trust: TrustClass::DeveloperUnsignedLocal,
            catalog: None,
            signed_catalog: None,
            trust_update: None,
        };
        let inspected = inspect_archive(&archive, entry, &installed).unwrap();
        write_install_record_at(&installed, &inspected, &source).unwrap();
        converge_windows_installed_version(&installed, entry, None).unwrap();

        let post_hook_archive_sha256 = "ab".repeat(32);
        let record = run_verified_extension_hook_with(
            &installed,
            entry,
            None,
            &inspected.manifest.version,
            || {
                let grant = std::process::Command::new("icacls")
                    .arg(temp.path().join("installed"))
                    .arg("/grant")
                    .arg("*S-1-15-2-1:(OI)(CI)(RX)")
                    .output()?;
                if !grant.status.success() {
                    bail!("icacls failed: {}", String::from_utf8_lossy(&grant.stderr));
                }
                let mut record =
                    read_install_record_at(&installed, entry.id, &inspected.manifest.version)?;
                record.archive_sha256 = post_hook_archive_sha256.clone();
                fs::write(
                    temp.path().join("installed").join(INSTALL_RECORD_NAME),
                    serde_json::to_vec_pretty(&record)?,
                )?;
                assert!(verify_installed_version_at(&installed, entry, None).is_err());
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(record.archive_sha256, post_hook_archive_sha256);
        let manifest = verify_installed_version_at(&installed, entry, None).unwrap();
        assert_eq!(manifest.version, "1.0.0");
        windows_verify_directory_handle(&installed).unwrap();
        inspect_owned_tree(&installed).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn acl_identity_check_rejects_a_different_object() {
        use std::os::windows::io::AsRawHandle as _;

        let temp = TempDir::new().unwrap();
        let root = open_directory_path_nofollow(temp.path()).unwrap();
        write_new_file_at(&root, "first", b"first").unwrap();
        write_new_file_at(&root, "second", b"second").unwrap();

        let mut options = CapOpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let first = root.open_with("first", &options).unwrap();
        let second = root.open_with("second", &options).unwrap();
        assert!(windows_verify_same_object(
            first.as_raw_handle().cast(),
            second.as_raw_handle().cast(),
        )
        .is_err());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_junctions_and_broad_acl_entries() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let root = ensure_private_directory_path(&store.root).unwrap();
        let external = temp.path().join("external");
        fs::create_dir(&external).unwrap();
        let junction = store.root.join("junction");
        let linked = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&external)
            .output()
            .unwrap();
        assert!(
            linked.status.success(),
            "{}",
            String::from_utf8_lossy(&linked.stderr)
        );
        assert!(open_directory_path_nofollow(&junction).is_err());

        write_new_file_at(&root, "owned", b"content").unwrap();
        let file = store.root.join("owned");
        let acl = std::process::Command::new("icacls")
            .arg(&file)
            .args(["/grant", "*S-1-1-0:(W)"])
            .output()
            .unwrap();
        assert!(
            acl.status.success(),
            "{}",
            String::from_utf8_lossy(&acl.stderr)
        );
        assert!(private_regular_file_or_missing_at(&root, "owned").is_err());
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
        let entry = registry_entry("cua-perception").unwrap();
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

    /// An interpreted entrypoint cannot execute inside the worker boundary:
    /// the sandbox grants execution to the entrypoint file alone, never to an
    /// interpreter. The hook therefore fails before its first instruction, so
    /// nothing it would have done — here, forking a descendant that outlives
    /// the deadline — can happen at all.
    #[cfg(unix)]
    #[test]
    fn a_hook_cannot_run_a_single_instruction_outside_its_containment() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("hook");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let script = root.join("worker");
        fs::write(
            &script,
            b"#!/bin/sh\n(/bin/sleep 1; printf escaped > \"$1\") &\n/bin/sleep 30\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let sentinel = temp.path().join("escaped");
        let manifest = hook_fixture_manifest(vec![sentinel.to_string_lossy().into_owned()]);

        assert!(run_extension_hook_with_timeout(
            &root,
            &manifest,
            false,
            std::time::Duration::from_secs(5),
            None,
        )
        .is_err());
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        assert!(!sentinel.exists());
        // The launch happened through the contained path, so the manager saw
        // the hook root before any process existed.
        assert_eq!(hook_observer::launches_under(&root).len(), 1);
    }

    /// A hook whose entrypoint is not an executable image never reaches the
    /// worker boundary either, and the failure is reported rather than ignored.
    #[test]
    fn a_hook_with_an_unlaunchable_entrypoint_fails_closed() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("hook");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("worker"), b"not an executable image").unwrap();
        let manifest = hook_fixture_manifest(vec!["--health".to_owned()]);

        assert!(run_extension_hook_with_timeout(
            &root,
            &manifest,
            false,
            std::time::Duration::from_secs(5),
            None,
        )
        .is_err());
    }

    fn hook_fixture_manifest(health_args: Vec<String>) -> ExtensionManifest {
        ExtensionManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            id: "cua-perception".to_owned(),
            version: "1.0.0".to_owned(),
            driver_version: format!("={}", env!("CARGO_PKG_VERSION")),
            protocol_version: 1,
            target: current_target().unwrap(),
            entrypoint: "worker".to_owned(),
            files: Vec::new(),
            models: Vec::new(),
            components: Vec::new(),
            corresponding_source_file: None,
            license: "Apache-2.0".to_owned(),
            source: "test".to_owned(),
            corresponding_source_uri: "test".to_owned(),
            corresponding_source_revision: "test".to_owned(),
            provenance: "test".to_owned(),
            health_args,
            self_test_args: Vec::new(),
        }
    }

    /// A fresh install must write its record and pass full verification before
    /// its self-test hook is allowed to run, so a hook can never execute from
    /// an unrecorded or unverified directory.
    #[test]
    fn a_fresh_install_records_and_verifies_before_running_its_hook() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("cua-perception").unwrap();
        let archive = fixture_archive_with_self_test(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            b"worker",
            &["--self-test"],
        );

        // The fixture payload is not an executable image, so the contained
        // hook cannot succeed; the install fails and rolls back.
        assert!(store.install_archive(entry, &archive).is_err());
        assert!(store.active_path(entry.id).unwrap().is_none());
        assert!(staging_is_empty(&store), "staging was not rolled back");

        let launches = hook_observer::launches_under(&store.root);
        assert_eq!(launches.len(), 1, "expected exactly one hook launch");
        assert!(
            launches[0].install_record_present,
            "the hook ran before the install record was written"
        );
        assert!(
            launches[0].fully_verified,
            "the hook ran before the staged version was fully verified"
        );
    }

    fn staging_is_empty(store: &ExtensionStore) -> bool {
        let staging = store.root.join(".staging");
        !staging.exists() || fs::read_dir(&staging).unwrap().next().is_none()
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

    #[test]
    fn ed25519_verification_rejects_tampering() {
        use ring::signature::{Ed25519KeyPair, KeyPair};

        let seed = [
            0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec,
            0x2c, 0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03,
            0x1c, 0xae, 0x7f, 0x60,
        ];
        let pair = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let message = b"signed target-specific extension catalog";
        let signature = BASE64.encode(pair.sign(message).as_ref());
        verify_ed25519_signature(pair.public_key().as_ref(), message, &signature).unwrap();
        assert!(verify_ed25519_signature(
            pair.public_key().as_ref(),
            b"tampered catalog",
            &signature
        )
        .is_err());
    }

    #[test]
    fn python_node_catalog_golden_is_canonical_and_verified_by_rust() {
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/extension_catalog/signed-catalog.golden.json"
        ))
        .unwrap();
        let payload: CatalogPayload = serde_json::from_value(golden["payload"].clone()).unwrap();
        assert_eq!(
            serde_json::to_string(&payload).unwrap(),
            golden["canonical_payload"]
        );
        let public_key = BASE64
            .decode(golden["public_key_base64"].as_str().unwrap())
            .unwrap();
        verify_ed25519_signature(
            &public_key,
            golden["canonical_payload"].as_str().unwrap().as_bytes(),
            golden["signature"].as_str().unwrap(),
        )
        .unwrap();
        let mut tampered = golden["canonical_payload"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec();
        tampered[0] ^= 1;
        assert!(verify_ed25519_signature(
            &public_key,
            &tampered,
            golden["signature"].as_str().unwrap(),
        )
        .is_err());
    }

    #[cfg(feature = "review-trust-root")]
    #[test]
    fn review_build_uses_only_the_overridden_review_key_and_evidence_class() {
        let trust = initial_publisher_trust();
        assert_eq!(trust.publisher_id, REVIEW_PUBLISHER_ID);
        assert_eq!(trust.current_key.key_id, REVIEW_KEY_ID);
        assert_eq!(
            trust.current_key.public_key_base64,
            REVIEW_PUBLIC_KEY_BASE64
        );
        assert_eq!(
            evidence_class(&TrustClass::ReviewOnlyPublisherVerified),
            "review-only-not-release-evidence"
        );
        assert_ne!(
            trust.current_key.public_key_base64,
            BASE64.encode(VERIFIED_PUBLIC_KEY)
        );
    }

    #[cfg(feature = "review-trust-root")]
    #[test]
    fn mcp_install_requires_confirm_after_returning_the_exact_verified_plan() {
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = TempDir::new().unwrap();
        let (_archive, catalog) = review_fixture(temp.path());
        let home = temp.path().join("driver-home");
        std::env::set_var("CUA_DRIVER_RS_HOME", &home);
        std::env::set_var("CUA_DRIVER_PERCEPTION_CATALOG", &catalog);

        let (_, plan) = install_extension_from_mcp(false).unwrap();
        assert_eq!(plan["extension"], "perception");
        assert_eq!(plan["artifact"], "cua-perception 1.0.0");
        assert_eq!(plan["target"], current_target().unwrap());
        assert_eq!(plan["publisher_id"], REVIEW_PUBLISHER_ID);
        assert_eq!(plan["publisher_name"], REVIEW_PUBLISHER_NAME);
        assert_eq!(plan["publisher_key_id"], REVIEW_KEY_ID);
        assert_eq!(plan["signing_key_algorithm"], "ed25519");
        assert_eq!(plan["signing_key_status"], "verified");
        assert_eq!(plan["publisher_signature_verified"], true);
        assert_eq!(plan["trust"], "review-only-publisher-verified");
        assert_eq!(plan["evidence_class"], "review-only-not-release-evidence");
        assert_eq!(
            plan["artifact_source"],
            temp.path()
                .join("review-extension.tar.gz")
                .to_string_lossy()
                .as_ref()
        );
        assert_eq!(
            plan["destination"],
            home.join("extensions/cua-perception/versions/1.0.0")
                .to_string_lossy()
                .as_ref()
        );
        assert!(plan["download_size"].as_u64().unwrap() > 0);
        assert!(plan["installed_size"].as_u64().unwrap() > 0);
        assert_eq!(plan["models"][0]["revision"], "model-v1");
        assert_eq!(plan["models"][0]["original_sha256"], "1".repeat(64));
        assert_eq!(plan["models"][0]["conversion_sha256"], hex_sha256(b"model"));
        assert_eq!(plan["components"][0]["license"], "AGPL-3.0-or-later");
        assert_eq!(plan["license_notices"][0]["license"], "AGPL-3.0-or-later");
        assert_eq!(plan["authorization"]["request"], "mcp-install");
        assert_eq!(plan["authorization"]["confirmation_required"], true);
        assert_eq!(plan["authorization"]["confirmation_received"], false);
        assert_eq!(plan["authorization"]["mutation_authorized"], false);
        assert_eq!(plan["authorization"]["mutation_performed"], false);
        assert_eq!(plan["mutation_performed"], false);
        assert_eq!(plan["ran"], false);
        assert!(!home.join("extensions/cua-perception").exists());

        let (_, installed) = install_extension_from_mcp(true).unwrap();
        assert_eq!(installed["ran"], true);
        assert_eq!(installed["installed"], true);
        assert_eq!(installed["installed_version"], "1.0.0");
        assert_eq!(installed["authorization"]["confirmation_received"], true);
        assert_eq!(installed["authorization"]["mutation_authorized"], true);
        assert_eq!(installed["authorization"]["mutation_performed"], true);
        assert_eq!(installed["mutation_performed"], true);
        assert!(home.join("extensions/cua-perception/active.json").is_file());

        std::env::remove_var("CUA_DRIVER_PERCEPTION_CATALOG");
        std::env::remove_var("CUA_DRIVER_RS_HOME");
    }

    fn test_catalog_payload(key_id: &str, version: u64) -> CatalogPayload {
        CatalogPayload {
            schema_version: CATALOG_SCHEMA_VERSION,
            catalog_version: version,
            expires_unix: 900,
            publisher_id: expected_publisher_id().to_owned(),
            publisher_name: expected_publisher_name().to_owned(),
            key_id: key_id.to_owned(),
            extension_id: "cua-perception".to_owned(),
            version: format!("1.0.{version}"),
            target: current_target().unwrap(),
            archive: "extension.tar.gz".to_owned(),
            archive_size: 1,
            archive_sha256: "0".repeat(64),
            manifest_sha256: "1".repeat(64),
            license: "Apache-2.0".to_owned(),
            source: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_uri: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_revision: "test".to_owned(),
            provenance: "test catalog".to_owned(),
            next_key: None,
        }
    }

    fn test_publisher_key(
        key_id: &str,
        pair: &ring::signature::Ed25519KeyPair,
        valid_from_unix: u64,
        valid_until_unix: u64,
    ) -> PublisherKey {
        use ring::signature::KeyPair as _;
        PublisherKey {
            key_id: key_id.to_owned(),
            public_key_base64: BASE64.encode(pair.public_key().as_ref()),
            valid_from_unix,
            valid_until_unix,
        }
    }

    fn sign_test_catalog(
        pair: &ring::signature::Ed25519KeyPair,
        payload: CatalogPayload,
    ) -> SignedCatalog {
        SignedCatalog {
            signature_algorithm: "ed25519".to_owned(),
            signature: BASE64.encode(pair.sign(&serde_json::to_vec(&payload).unwrap()).as_ref()),
            payload,
        }
    }

    #[test]
    fn publisher_key_rotation_enforces_windows_and_rollback() {
        let current = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
        let next = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[9; 32]).unwrap();
        let current_key = test_publisher_key("current", &current, 100, 800);
        let next_key = test_publisher_key("next", &next, 200, 900);
        let trust = PublisherTrust {
            schema_version: 1,
            publisher_id: expected_publisher_id().to_owned(),
            generation: 1,
            highest_catalog_version: 0,
            current_key: current_key.clone(),
            pending_key: None,
        };
        let entry = registry_entry("cua-perception").unwrap();

        let valid = sign_test_catalog(&current, test_catalog_payload("current", 1));
        let accepted = verify_catalog_at(entry, &valid, &trust, 150).unwrap();
        assert_eq!(accepted.highest_catalog_version, 1);

        let mut tampered = valid.clone();
        tampered.payload.archive_size = 2;
        assert!(verify_catalog_at(entry, &tampered, &trust, 150)
            .unwrap_err()
            .to_string()
            .contains("signature verification"));

        let mut rotation_payload = test_catalog_payload("current", 2);
        rotation_payload.next_key = Some(next_key.clone());
        let rotation = sign_test_catalog(&current, rotation_payload);
        let pending = verify_catalog_at(entry, &rotation, &trust, 150).unwrap();
        assert_eq!(pending.pending_key.as_ref().unwrap().key_id, "next");

        let mut retrograde_payload = test_catalog_payload("current", 2);
        retrograde_payload.next_key = Some(test_publisher_key("retrograde", &next, 100, 900));
        assert!(verify_catalog_at(
            entry,
            &sign_test_catalog(&current, retrograde_payload),
            &trust,
            150,
        )
        .unwrap_err()
        .to_string()
        .contains("invalid rotation window"));

        let next_catalog = sign_test_catalog(&next, test_catalog_payload("next", 3));
        assert!(verify_catalog_at(entry, &next_catalog, &pending, 199)
            .unwrap_err()
            .to_string()
            .contains("not valid yet"));
        let promoted = verify_catalog_at(entry, &next_catalog, &pending, 250).unwrap();
        assert_eq!(promoted.current_key.key_id, "next");
        assert_eq!(promoted.generation, 2);
        assert!(promoted.pending_key.is_none());

        let retired = sign_test_catalog(&current, test_catalog_payload("current", 4));
        assert!(verify_catalog_at(entry, &retired, &promoted, 300)
            .unwrap_err()
            .to_string()
            .contains("retired"));

        let rollback = sign_test_catalog(&next, test_catalog_payload("next", 3));
        assert!(verify_catalog_at(entry, &rollback, &promoted, 300)
            .unwrap_err()
            .to_string()
            .contains("anti-rollback"));

        let expired_trust = PublisherTrust {
            current_key: test_publisher_key("expired", &current, 100, 200),
            ..trust
        };
        let expired = sign_test_catalog(&current, test_catalog_payload("expired", 5));
        assert!(verify_catalog_at(entry, &expired, &expired_trust, 200)
            .unwrap_err()
            .to_string()
            .contains("expired"));
    }

    #[test]
    fn catalog_verification_rejects_invalid_trust_and_stale_two_phase_decisions() {
        let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[11; 32]).unwrap();
        let key = test_publisher_key("current", &pair, 1, u64::MAX);
        let entry = registry_entry("cua-perception").unwrap();
        let mut payload = test_catalog_payload("current", 1);
        payload.expires_unix = u64::MAX;
        let signed = sign_test_catalog(&pair, payload.clone());
        let trust = PublisherTrust {
            schema_version: 1,
            publisher_id: expected_publisher_id().to_owned(),
            generation: 1,
            highest_catalog_version: 0,
            current_key: key,
            pending_key: None,
        };
        let mut invalid_trust = trust.clone();
        invalid_trust.generation = 0;
        assert!(verify_catalog_at(entry, &signed, &invalid_trust, 100)
            .unwrap_err()
            .to_string()
            .contains("trust state is invalid"));

        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let root = ensure_private_directory_path(&store.root).unwrap();
        store.commit_publisher_trust_locked(&root, &trust).unwrap();
        let decision = verify_catalog_at(entry, &signed, &trust, 100).unwrap();
        let mut advanced = trust.clone();
        advanced.highest_catalog_version = 2;
        store
            .commit_publisher_trust_locked(&root, &advanced)
            .unwrap();
        let source = InstallSource {
            archive: temp.path().join("must-not-be-opened.tar.gz"),
            trust: verified_trust_class(),
            catalog: Some(payload),
            signed_catalog: Some(signed),
            trust_update: Some(decision),
        };
        let error = store.install_source(entry, &source, false).unwrap_err();
        assert!(format!("{error:#}").contains("anti-rollback"));
        assert!(!store.extension_dir(entry.id).exists());
    }

    #[test]
    fn publisher_trust_state_is_durable_and_recovers_backup() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let root = ensure_private_directory_path(&store.root).unwrap();
        let mut trust = initial_publisher_trust();
        trust.highest_catalog_version = 7;
        store.commit_publisher_trust_locked(&root, &trust).unwrap();
        assert_eq!(store.load_publisher_trust().unwrap(), trust);

        root.rename(TRUST_NAME, &root, TRUST_BACKUP_NAME).unwrap();
        write_new_file_at(&root, TRUST_NEW_NAME, b"interrupted").unwrap();
        store.recover_publisher_trust_locked(&root).unwrap();
        assert_eq!(store.load_publisher_trust().unwrap(), trust);
        assert!(!store.root.join(TRUST_BACKUP_NAME).exists());
        assert!(!store.root.join(TRUST_NEW_NAME).exists());
    }

    #[test]
    fn model_hash_must_exactly_match_the_declared_file_hash() {
        let file_hash = hex_sha256(b"model");
        let manifest = ExtensionManifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            id: "cua-perception".to_owned(),
            version: "1.0.0".to_owned(),
            driver_version: format!("={}", env!("CARGO_PKG_VERSION")),
            protocol_version: 1,
            target: current_target().unwrap(),
            entrypoint: "bin/cua-perception".to_owned(),
            files: vec![
                ManifestFile {
                    path: "bin/cua-perception".to_owned(),
                    sha256: hex_sha256(b"worker"),
                    executable: true,
                },
                ManifestFile {
                    path: "models/parser.bin".to_owned(),
                    sha256: file_hash.clone(),
                    executable: false,
                },
            ],
            models: vec![ManifestModel {
                path: "models/parser.bin".to_owned(),
                revision: "model-v1".to_owned(),
                original_sha256: "1".repeat(64),
                conversion_sha256: "0".repeat(64),
                license_file: None,
            }],
            components: vec![ComponentLicense {
                name: "model-runtime".to_owned(),
                version: "1.0.0".to_owned(),
                license: "Apache-2.0".to_owned(),
                notice: "test notice".to_owned(),
                source_uri: "https://example.invalid/source".to_owned(),
                source_revision: "abc123".to_owned(),
                notice_file: None,
            }],
            corresponding_source_file: None,
            license: "Apache-2.0".to_owned(),
            source: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_uri: "https://github.com/trycua/cua".to_owned(),
            corresponding_source_revision: "test".to_owned(),
            provenance: "test".to_owned(),
            health_args: Vec::new(),
            self_test_args: Vec::new(),
        };
        let files = vec![
            ArchiveFile {
                path: MANIFEST_NAME.to_owned(),
                sha256: "0".repeat(64),
                size: 1,
            },
            ArchiveFile {
                path: "bin/cua-perception".to_owned(),
                sha256: hex_sha256(b"worker"),
                size: 6,
            },
            ArchiveFile {
                path: "models/parser.bin".to_owned(),
                sha256: file_hash,
                size: 5,
            },
        ];
        assert!(
            validate_manifest(&manifest, registry_entry("cua-perception").unwrap(), &files)
                .unwrap_err()
                .to_string()
                .contains("model hash")
        );
    }

    #[test]
    fn removal_refuses_unowned_files() {
        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("cua-perception").unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        store.install_archive(entry, &archive).unwrap();
        fs::write(store.extension_dir(entry.id).join("foreign"), b"keep").unwrap();
        let error = store.remove(entry).unwrap_err().to_string();
        assert!(error.contains("unowned state"));
        assert_eq!(
            fs::read(store.extension_dir(entry.id).join("foreign")).unwrap(),
            b"keep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn removal_rejects_a_swapped_versions_symlink_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let store = ExtensionStore::new(temp.path().join("extensions"));
        let entry = registry_entry("cua-perception").unwrap();
        let archive = fixture_archive(
            temp.path(),
            "1.0.0",
            &current_target().unwrap(),
            1,
            b"worker",
        );
        store.install_archive(entry, &archive).unwrap();
        let external = temp.path().join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("sentinel"), b"keep").unwrap();
        let versions = store.versions_dir(entry.id);
        fs::rename(&versions, temp.path().join("owned-versions")).unwrap();
        fs::remove_file(store.extension_dir(entry.id).join(ACTIVE_NAME)).unwrap();
        symlink(&external, &versions).unwrap();

        assert!(store.remove(entry).is_err());
        assert_eq!(fs::read(external.join("sentinel")).unwrap(), b"keep");
    }
}
