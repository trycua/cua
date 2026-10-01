// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What cua-spacesd's `TeleportService` drives:
//!
//! - [`Receiver::import_bundle`] imports an app-session [`SessionBundle`]
//!   with the importer named in its header and launches the app through the
//!   injected [`HostEffects`] (tests pass a [`crate::FakeHost`], so no real
//!   app is ever started).
//! - [`IgnoreRules`] evaluates gitignore-syntax patterns (request patterns
//!   and `.gitignore` files that are part of a transfer) for
//!   `ReceiveFiles`.
//! - [`validate_relative_path`] and [`conflict_free_path`] implement the
//!   transfer path rules.
//!
//! [`SessionBundle`]: cua_teleport_bundle::bundle::SessionBundle

use std::io::{Cursor, Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cua_teleport_bundle::bundle::{BundleReader, DEFAULT_MAX_TOTAL_BYTES};

use crate::host::{EffectKind, HostCommand, HostEffects};
use crate::ledger::{ImportRecord, Ledger, WipeReport};
use crate::{ImportRegistry, Platform};

/// Shared configuration and importer registry.
pub struct Receiver {
    /// App-session importers.
    pub registry: ImportRegistry,
    /// Home directory imports land in.
    pub dest_home: PathBuf,
    /// Total-size guard for bundles.
    pub max_total_bytes: u64,
    /// Where imported apps are launched.
    pub host: Arc<dyn HostEffects>,
}

/// Outcome of one import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    /// Provider that imported the bundle.
    pub provider_id: String,
    /// Items imported (bundle entry groups).
    pub imported: Vec<String>,
    /// True if the app was launched.
    pub launched: bool,
    /// Launched pid, if any.
    pub pid: Option<u32>,
    /// Everything the importer wrote (files, created directories, Keychain
    /// items), for the import ledger.
    pub record: ImportRecord,
}

/// Why an import failed.
#[derive(Debug)]
pub enum ImportError {
    /// Not a valid bundle.
    InvalidBundle(String),
    /// No importer for the bundle's provider id.
    UnknownProvider(String),
    /// The bundle was for a different app than requested.
    AppMismatch {
        /// Requested app id.
        requested: String,
        /// Provider id in the bundle.
        bundle: String,
    },
    /// The provider failed.
    Failed(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBundle(e) => write!(f, "invalid bundle: {e}"),
            Self::UnknownProvider(id) => write!(f, "no provider for id {id:?}"),
            Self::AppMismatch { requested, bundle } => write!(
                f,
                "bundle is for provider {bundle:?}, but app {requested:?} was requested"
            ),
            Self::Failed(e) => write!(f, "import failed: {e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl Receiver {
    /// A receiver acting on `host`, importing into `dest_home`.
    pub fn with_host(dest_home: PathBuf, host: Arc<dyn HostEffects>) -> Self {
        Self {
            registry: ImportRegistry::with_builtin_host(host.clone()),
            dest_home,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host,
        }
    }

    /// Provider ids this guest can import.
    pub fn provider_ids(&self) -> Vec<String> {
        self.registry
            .importers()
            .iter()
            .map(|p| p.id().to_owned())
            .collect()
    }

    /// Imports a bundle. When `app` is non-empty it must name the bundle's
    /// provider. With `launch`, the returned launch spec is spawned detached
    /// through the host (a launch failure is not an import failure). The
    /// bundle is fully verified (header, size limits) before any importer
    /// runs; entries are checksum-verified before each is written.
    pub fn import_bundle<R: Read + Seek>(
        &self,
        mut bundle: R,
        app: &str,
        launch: bool,
    ) -> Result<ImportOutcome, ImportError> {
        let header = BundleReader::open_with_limit(&mut bundle, self.max_total_bytes)
            .map_err(|e| ImportError::InvalidBundle(e.to_string()))?
            .header()
            .clone();
        bundle
            .rewind()
            .map_err(|e| ImportError::InvalidBundle(e.to_string()))?;
        let provider_id = header.provider_id.clone();
        if !app.is_empty() && !app.eq_ignore_ascii_case(&provider_id) {
            return Err(ImportError::AppMismatch {
                requested: app.to_owned(),
                bundle: provider_id,
            });
        }
        let provider = self
            .registry
            .find_by_id(&provider_id)
            .ok_or_else(|| ImportError::UnknownProvider(provider_id.clone()))?;
        let mut record = ImportRecord::default();
        let spec = match provider.import_recorded(
            &mut bundle,
            &self.dest_home,
            Platform::current(),
            &mut record,
        ) {
            Ok(spec) => spec,
            Err(e) => {
                // Undo the partial import: nothing it wrote is ledgered.
                let partial = Ledger::new("", &provider_id, &record, 0, 0);
                let _ = self.wipe(&partial);
                return Err(ImportError::Failed(e.to_string()));
            }
        };
        let mut imported: Vec<String> = header
            .entries
            .iter()
            .map(|e| e.rel_path.split('/').next().unwrap_or_default().to_owned())
            .collect();
        imported.sort();
        imported.dedup();
        let (launched, pid) = if launch {
            let mut command = HostCommand::new(EffectKind::AppLaunch, spec.program.clone())
                .args(spec.args.clone());
            command.env = spec.env.clone();
            command.cwd = spec.cwd.clone().map(PathBuf::from);
            match self.host.spawn(&command) {
                Ok(pid) => (true, Some(pid)),
                Err(_) => (false, None),
            }
        } else {
            (false, None)
        };
        Ok(ImportOutcome {
            provider_id,
            imported,
            launched,
            pid,
            record,
        })
    }

    /// Undoes a ledgered import under [`Self::dest_home`] through
    /// [`Self::host`] (see [`crate::ledger::wipe`]).
    pub fn wipe(&self, ledger: &Ledger) -> WipeReport {
        crate::ledger::wipe(ledger, &self.dest_home, &*self.host)
    }

    /// Convenience for in-memory bundles.
    pub fn import_bytes(
        &self,
        bytes: &[u8],
        app: &str,
        launch: bool,
    ) -> Result<ImportOutcome, ImportError> {
        self.import_bundle(Cursor::new(bytes), app, launch)
    }
}

/// Checks a transfer-relative path: non-empty, `/`-separated, no `..`, not
/// absolute. Returns the normalized path.
pub fn validate_relative_path(relative: &str) -> Result<PathBuf, String> {
    if relative.is_empty() {
        return Err("empty relative path".into());
    }
    if relative.contains('\\') || relative.contains('\0') {
        return Err(format!("{relative:?}: invalid character"));
    }
    let path = Path::new(relative);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "{relative:?}: absolute paths and '..' are not allowed"
                ))
            }
        }
    }
    if out.as_os_str().is_empty() {
        return Err(format!("{relative:?}: empty path"));
    }
    Ok(out)
}

/// `name (1).ext`, `name (2).ext`, ... — the first path that does not exist.
pub fn conflict_free_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..10_000 {
        let candidate = parent.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem} ({}){ext}", uuid_like()))
}

fn uuid_like() -> String {
    format!(
        "{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    )
}

/// gitignore-syntax rules: request-level patterns relative to the transfer
/// root, plus `.gitignore` files at any directory of the transfer.
#[derive(Default)]
pub struct IgnoreRules {
    root: Option<ignore::gitignore::Gitignore>,
    nested: Vec<(PathBuf, ignore::gitignore::Gitignore)>,
}

impl IgnoreRules {
    /// Rules from request patterns (relative to the virtual root).
    pub fn from_patterns(patterns: &[String]) -> Result<Self, String> {
        let mut rules = Self::default();
        if !patterns.is_empty() {
            let mut builder = ignore::gitignore::GitignoreBuilder::new("/");
            for pattern in patterns {
                builder
                    .add_line(None, pattern)
                    .map_err(|e| format!("ignore pattern {pattern:?}: {e}"))?;
            }
            rules.root = Some(builder.build().map_err(|e| e.to_string())?);
        }
        Ok(rules)
    }

    /// Adds the contents of a `.gitignore` located at directory `dir`
    /// (transfer-relative, empty for the root).
    pub fn add_gitignore(&mut self, dir: &Path, contents: &str) -> Result<(), String> {
        let base = Path::new("/").join(dir);
        let mut builder = ignore::gitignore::GitignoreBuilder::new(&base);
        for line in contents.lines() {
            builder
                .add_line(None, line)
                .map_err(|e| format!("{}: {e}", dir.join(".gitignore").display()))?;
        }
        let built = builder.build().map_err(|e| e.to_string())?;
        self.nested.push((dir.to_path_buf(), built));
        // Deeper files override shallower ones.
        self.nested.sort_by_key(|(d, _)| d.components().count());
        Ok(())
    }

    /// True if `relative` (or any of its parent directories) is ignored.
    pub fn is_ignored(&self, relative: &Path, is_dir: bool) -> bool {
        let absolute = Path::new("/").join(relative);
        let mut ignored = false;
        if let Some(root) = &self.root {
            let m = root.matched_path_or_any_parents(&absolute, is_dir);
            if m.is_ignore() {
                ignored = true;
            } else if m.is_whitelist() {
                ignored = false;
            }
        }
        for (dir, rules) in &self.nested {
            if !relative.starts_with(dir) {
                continue;
            }
            let m = rules.matched_path_or_any_parents(&absolute, is_dir);
            if m.is_ignore() {
                ignored = true;
            } else if m.is_whitelist() {
                ignored = false;
            }
        }
        ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_confined() {
        assert_eq!(
            validate_relative_path("a/./b").unwrap(),
            PathBuf::from("a/b")
        );
        assert!(validate_relative_path("../x").is_err());
        assert!(validate_relative_path("/etc/passwd").is_err());
        assert!(validate_relative_path("a/../../x").is_err());
        assert!(validate_relative_path("").is_err());
    }

    #[test]
    fn conflicts_get_numbered_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        assert_eq!(conflict_free_path(&path), path);
        std::fs::write(&path, b"x").unwrap();
        assert_eq!(conflict_free_path(&path), dir.path().join("report (1).pdf"));
        std::fs::write(dir.path().join("report (1).pdf"), b"x").unwrap();
        assert_eq!(conflict_free_path(&path), dir.path().join("report (2).pdf"));
    }

    #[test]
    fn request_patterns_and_nested_gitignores() {
        let mut rules = IgnoreRules::from_patterns(&["*.log".into(), "build/".into()]).unwrap();
        rules
            .add_gitignore(Path::new("pkg"), "secret.txt\n!keep.log\n")
            .unwrap();
        assert!(rules.is_ignored(Path::new("a.log"), false));
        assert!(rules.is_ignored(Path::new("build/out.o"), false));
        assert!(rules.is_ignored(Path::new("pkg/secret.txt"), false));
        assert!(
            !rules.is_ignored(Path::new("secret.txt"), false),
            "nested rule is scoped"
        );
        assert!(
            !rules.is_ignored(Path::new("pkg/keep.log"), false),
            "negation wins when deeper"
        );
        assert!(!rules.is_ignored(Path::new("src/main.rs"), false));
    }

    fn chrome_bundle() -> Vec<u8> {
        crate::importers::fixtures::bundle(
            "chrome",
            &[
                ("tabs.json", 0o644, b"[]"),
                (
                    ".config/google-chrome/Default/Cookies",
                    0o600,
                    b"COOKIE-BYTES",
                ),
                (
                    ".config/google-chrome/Default/Local Storage/x.log",
                    0o600,
                    b"LOCAL-STORAGE-BYTES",
                ),
            ],
        )
    }

    /// The importer reports exactly what it wrote, and a wipe of that record
    /// removes exactly that: pre-existing neighbours stay.
    #[test]
    fn import_reports_what_it_wrote_and_wipe_removes_exactly_that() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let parent = h
            .join(crate::layout::chrome::user_data_dir_for(Platform::current()))
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::write(parent.join("unrelated"), b"keep").unwrap();
        let receiver = Receiver::with_host(h.to_path_buf(), Arc::new(crate::FakeHost::new()));
        let outcome = receiver
            .import_bytes(&chrome_bundle(), "chrome", false)
            .unwrap();
        let record = &outcome.record;
        assert_eq!(record.files.len(), 2, "{record:?}");
        assert!(record.files.iter().all(|f| f.is_file() && f.starts_with(h)));
        assert!(!record.created_dirs.contains(&parent));
        assert!(record
            .created_dirs
            .iter()
            .all(|d| d.starts_with(&parent) && d.is_dir()));
        let ledger = Ledger::new("i", &outcome.provider_id, record, 0, 1);
        let report = receiver.wipe(&ledger);
        assert!(report.complete() && report.refused.is_empty(), "{report:?}");
        assert!(record.files.iter().all(|f| !f.exists()));
        assert!(record.created_dirs.iter().all(|d| !d.exists()));
        assert_eq!(std::fs::read(parent.join("unrelated")).unwrap(), b"keep");
    }

    /// A bundle that fails part way is undone: nothing it wrote remains.
    #[test]
    fn failed_import_leaves_nothing_behind() {
        let mut bytes = chrome_bundle();
        // Corrupt the last entry's content, so the first entry lands before
        // the checksum failure.
        let needle = b"LOCAL-STORAGE-BYTES";
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap();
        bytes[pos] ^= 0xff;
        let home = tempfile::tempdir().unwrap();
        let receiver =
            Receiver::with_host(home.path().to_path_buf(), Arc::new(crate::FakeHost::new()));
        let error = receiver.import_bytes(&bytes, "chrome", false).unwrap_err();
        assert!(matches!(error, ImportError::Failed(_)), "{error}");
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    }
}
