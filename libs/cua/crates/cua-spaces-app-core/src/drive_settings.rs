// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Storage: where the Cua Volume keeps its files (this machine or
//! an S3-compatible bucket), the drive as a Finder volume (or a mount on
//! Linux), and its block cache.
//!
//! Plain data in: the daemon's `volume_storage`, `volume_mount_status` and
//! `volume_cache_stats` answers, as they come (snake_case, the tools'
//! shape). Plain data out: one [`SettingsSection`] the shells draw like the
//! other Settings sections, and the command to run. S3 keys live only in
//! the form until they are sent once (`volume_storage_set` saves them in the
//! credential store, never in the config file); the daemon never returns
//! them. The Cua cloud backend is never offered.

use serde::{Deserialize, Serialize};

use crate::model::SpaceOs;
use crate::paths::display_path;
use crate::settings::{SettingsOption, SettingsRow, SettingsRowKind, SettingsSection, row};

/// `volume_mount_status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveMountInput {
    /// The user's opt-in.
    pub enabled: bool,
    /// `off`, `mounting`, `mounted`, `needs_approval`, `unsupported`, `error`.
    pub state: String,
    /// `fskit`, `nfs`, `fuse`, `none`.
    pub method: String,
    /// The mount point when mounted.
    pub path: Option<String>,
    /// "Cua Volume".
    pub volume_name: String,
    /// One sentence for `error`, `needs_approval`, `unsupported`.
    pub detail: Option<String>,
    /// `needs_approval`: System Settings' file system extensions.
    pub settings_url: Option<String>,
}

impl DriveMountInput {
    /// Mounted, with a path to open.
    pub fn mounted_path(&self) -> Option<&str> {
        (self.state == "mounted")
            .then_some(self.path.as_deref())
            .flatten()
            .filter(|p| !p.is_empty())
    }

    /// The mount can be switched on here.
    pub fn supported(&self) -> bool {
        self.state != "unsupported" && self.method != "none"
    }

    /// A Finder volume (macOS), else a mount point (Linux).
    pub fn in_finder(&self) -> bool {
        matches!(self.method.as_str(), "fskit" | "nfs")
    }
}

/// Where an S3-compatible bucket is (`DriveS3Settings`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveS3Input {
    /// None: AWS.
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    /// A key prefix inside the bucket.
    pub root: String,
    pub path_style: bool,
}

impl Default for DriveS3Input {
    fn default() -> Self {
        Self {
            endpoint: None,
            region: DEFAULT_REGION.into(),
            bucket: String::new(),
            root: String::new(),
            path_style: false,
        }
    }
}

/// `volume_storage`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveStorageInput {
    /// `fs` or `s3` (`cloud` is never offered).
    pub backend: String,
    /// Where `fs` keeps the bytes.
    pub fs_path: String,
    pub s3: Option<DriveS3Input>,
    /// S3 keys are saved in the credential store.
    pub has_keys: bool,
}

/// `volume_storage_set`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveCheckInput {
    pub ok: bool,
    pub reachable: bool,
    pub authorized: bool,
    pub versioning: bool,
    pub detail: Option<String>,
    /// Saved, and the daemon switched to it.
    pub applied: bool,
}

/// `volume_cache_stats` (the parts Settings shows).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveCacheInput {
    pub size_bytes: u64,
    pub capacity_bytes: u64,
}

/// Everything the Storage section reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StorageInput {
    /// This machine's system (the words: "This Mac", "Show in Finder").
    pub os: SpaceOs,
    /// The home folder, to show paths as `~/...`.
    pub home: Option<String>,
    /// None until read (or when the daemon cannot answer).
    pub storage: Option<DriveStorageInput>,
    pub mount: Option<DriveMountInput>,
    pub cache: Option<DriveCacheInput>,
}

impl Default for StorageInput {
    fn default() -> Self {
        Self {
            os: SpaceOs::Macos,
            home: None,
            storage: None,
            mount: None,
            cache: None,
        }
    }
}

/// `volume_storage_set`'s argument (snake_case: the tool's shape).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriveStorageUpdate {
    pub backend: String,
    pub s3: Option<DriveS3Input>,
    /// Both keys or neither.
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    /// Only check ("Test connection").
    pub dry_run: bool,
}

/// The S3 form (and the backend choice) as edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StorageForm {
    pub backend: String,
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    pub root: String,
    pub path_style: bool,
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl Default for StorageForm {
    fn default() -> Self {
        Self {
            backend: "fs".into(),
            endpoint: String::new(),
            region: DEFAULT_REGION.into(),
            bucket: String::new(),
            root: String::new(),
            path_style: false,
            access_key_id: String::new(),
            secret_access_key: String::new(),
        }
    }
}

impl StorageForm {
    fn from_storage(s: &DriveStorageInput) -> StorageForm {
        let s3 = s.s3.clone().unwrap_or_default();
        StorageForm {
            backend: if s.backend == "s3" { "s3" } else { "fs" }.into(),
            endpoint: s3.endpoint.unwrap_or_default(),
            region: s3.region,
            bucket: s3.bucket,
            root: s3.root,
            path_style: s3.path_style,
            access_key_id: String::new(),
            secret_access_key: String::new(),
        }
    }

    pub(crate) fn update(&self, dry_run: bool) -> DriveStorageUpdate {
        let keys = !self.access_key_id.trim().is_empty() && !self.secret_access_key.is_empty();
        let s3 = (self.backend == "s3").then(|| DriveS3Input {
            endpoint: Some(self.endpoint.trim().to_string()).filter(|e| !e.is_empty()),
            region: match self.region.trim() {
                "" => DEFAULT_REGION.into(),
                r => r.into(),
            },
            bucket: self.bucket.trim().into(),
            root: self.root.trim().into(),
            path_style: self.path_style,
        });
        DriveStorageUpdate {
            backend: self.backend.clone(),
            access_key_id: (s3.is_some() && keys).then(|| self.access_key_id.trim().to_string()),
            secret_access_key: (s3.is_some() && keys).then(|| self.secret_access_key.clone()),
            s3,
            dry_run,
        }
    }
}

/// A text field of the S3 form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorageField {
    Endpoint,
    Region,
    Bucket,
    AccessKeyId,
    SecretAccessKey,
}

impl StorageField {
    /// The row id the shells bind the field to.
    pub fn row_id(self) -> &'static str {
        match self {
            StorageField::Endpoint => "s3-endpoint",
            StorageField::Region => "s3-region",
            StorageField::Bucket => "s3-bucket",
            StorageField::AccessKeyId => "s3-access-key",
            StorageField::SecretAccessKey => "s3-secret",
        }
    }

    /// The field a row id binds to.
    pub fn from_row_id(id: &str) -> Option<StorageField> {
        [
            StorageField::Endpoint,
            StorageField::Region,
            StorageField::Bucket,
            StorageField::AccessKeyId,
            StorageField::SecretAccessKey,
        ]
        .into_iter()
        .find(|f| f.row_id() == id)
    }
}

/// The command the shell runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum StorageRequest {
    /// `volume_storage_set` with `dry_run`; answer with `checked`.
    Test { update: DriveStorageUpdate },
    /// `volume_storage_set`; answer with `saved`.
    Save { update: DriveStorageUpdate },
    /// `volume_storage_set` (keys from the credential store) for a bucket the
    /// user's agent configured; answer with `adopted`.
    Adopt { update: DriveStorageUpdate },
    /// `volume_mount`.
    Mount,
    /// `volume_unmount`.
    Unmount,
    /// Show the mount in the file manager (the shell's own call).
    Reveal { path: String },
    /// Open a URL (System Settings' file system extensions).
    OpenUrl { url: String },
    /// `volume_cache_set`.
    SetCache { capacity_bytes: u64 },
    /// `volume_cache_clear`.
    ClearCache,
}

impl StorageRequest {
    /// One line naming the command.
    pub fn text(&self) -> String {
        match self {
            StorageRequest::Test { update } => format!("test {}", update.backend),
            StorageRequest::Save { update } => format!(
                "save {}{}",
                update.backend,
                if update.access_key_id.is_some() {
                    " with keys"
                } else {
                    ""
                }
            ),
            StorageRequest::Adopt { update } => format!(
                "adopt {}",
                update.s3.as_ref().map(|s| s.bucket.as_str()).unwrap_or("")
            ),
            StorageRequest::Mount => "mount".into(),
            StorageRequest::Unmount => "unmount".into(),
            StorageRequest::Reveal { path } => format!("reveal {path}"),
            StorageRequest::OpenUrl { url } => format!("open {url}"),
            StorageRequest::SetCache { capacity_bytes } => {
                format!("cache limit {}", bytes_text(*capacity_bytes))
            }
            StorageRequest::ClearCache => "clear cache".into(),
        }
    }
}

/// The section's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StorageState {
    pub form: StorageForm,
    /// The form differs from what is saved.
    pub dirty: bool,
    pub busy: bool,
    pub request: Option<StorageRequest>,
    /// The last test or save.
    pub check: Option<DriveCheckInput>,
    pub error: Option<String>,
    /// The bucket's fields show (else the agent prompt).
    pub manual: bool,
    /// The last `volume_storage` seen (bucket, endpoint, region, keys), to
    /// notice the agent connecting a bucket while the prompt shows.
    pub seen: Option<String>,
}

/// An input to the section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StorageAction {
    /// `volume_storage` was read: the form follows it unless edited.
    Loaded {
        storage: DriveStorageInput,
    },
    /// The backend choice (`fs`, `s3`).
    SetBackend {
        backend: String,
    },
    SetField {
        field: StorageField,
        value: String,
    },
    SetPathStyle {
        on: bool,
    },
    /// "Test connection".
    Test,
    /// The section's Save.
    Save,
    /// A test's answer.
    Checked {
        check: DriveCheckInput,
    },
    /// A save's answer.
    Saved {
        check: DriveCheckInput,
    },
    /// An adopted bucket's answer.
    Adopted {
        check: DriveCheckInput,
    },
    /// "Enter details manually" (on) or "Use a prompt instead" (off).
    ShowManual {
        on: bool,
    },
    /// "Show Cua Volume in Finder" on or off.
    SetMount {
        on: bool,
    },
    /// "Show in Finder" / "Open".
    Reveal {
        path: String,
    },
    /// "Open System Settings".
    OpenUrl {
        url: String,
    },
    SetCache {
        capacity_bytes: u64,
    },
    ClearCache,
    Done,
    Failed {
        error: String,
    },
}

const DEFAULT_REGION: &str = "us-east-1";

/// What "Your S3 bucket" offers first: a prompt the user gives their coding
/// agent, which sets the bucket up and connects it with the CLI. The page
/// watches `volume_storage` and connects once the bucket shows up.
pub const S3_AGENT_PROMPT: &str = "Using the AWS CLI, create a private S3 bucket for Cua Volume with versioning on, and an IAM user whose access is limited to that bucket. Then connect it with `cua volume config set --backend s3 --bucket <bucket> --region <region>`, pipe the new keys into `cua volume config set-keys`, and run `cua volume status`. Don't print the secret key.";
const GIB: u64 = 1024 * 1024 * 1024;
/// The cache limits offered (the daemon's minimum is 256 MiB).
pub const CACHE_LIMITS: [u64; 4] = [GIB, 5 * GIB, 10 * GIB, 50 * GIB];

/// `512 MB`, `1.5 GB`, `10 GB`.
pub fn bytes_text(bytes: u64) -> String {
    let mb = 1024.0 * 1024.0;
    let b = bytes as f64;
    if bytes < 1024 * 1024 {
        format!("{} KB", (b / 1024.0).ceil() as u64)
    } else if bytes < GIB {
        format!("{} MB", (b / mb).round() as u64)
    } else {
        let g = b / GIB as f64;
        if (g - g.round()).abs() < 0.05 {
            format!("{} GB", g.round() as u64)
        } else {
            format!("{g:.1} GB")
        }
    }
}

/// What identifies a connected bucket in `volume_storage` (none unless it is
/// an S3 bucket with saved keys).
fn storage_signature(s: &DriveStorageInput) -> Option<String> {
    let s3 =
        s.s3.as_ref()
            .filter(|b| s.backend == "s3" && s.has_keys && !b.bucket.is_empty())?;
    Some(format!(
        "{}|{}|{}|{}",
        s3.bucket,
        s3.endpoint.as_deref().unwrap_or(""),
        s3.region,
        s3.path_style
    ))
}

/// Switches the running volume to the bucket `volume_storage` names, with
/// the keys already in the credential store.
fn adopt_update(s: &DriveStorageInput) -> Option<DriveStorageUpdate> {
    storage_signature(s)?;
    Some(DriveStorageUpdate {
        backend: "s3".into(),
        s3: s.s3.clone(),
        access_key_id: None,
        secret_access_key: None,
        dry_run: false,
    })
}

/// The section before anything is read.
pub fn storage_initial() -> StorageState {
    StorageState::default()
}

/// Advances the section.
pub fn storage_reduce(state: &StorageState, action: &StorageAction) -> StorageState {
    use StorageAction as A;
    let mut s = state.clone();
    let start = |s: &mut StorageState, r: StorageRequest| {
        s.busy = true;
        s.error = None;
        s.request = Some(r);
    };
    let edit = |s: &mut StorageState| {
        s.dirty = true;
        s.check = None;
        s.error = None;
    };
    match action {
        A::Loaded { storage } => {
            let sig = storage_signature(storage);
            let prompting = s.form.backend == "s3" && !s.manual;
            let changed = s.seen.is_some() && s.seen != sig;
            if prompting
                && changed
                && !s.busy
                && let Some(update) = adopt_update(storage)
            {
                s.form = StorageForm::from_storage(storage);
                s.dirty = false;
                start(&mut s, StorageRequest::Adopt { update });
            } else if !s.dirty && !prompting {
                s.form = StorageForm::from_storage(storage);
                // First read: a saved bucket shows its fields, otherwise
                // the prompt; later reads (polls) keep the user's choice.
                if s.seen.is_none() {
                    s.manual = storage.backend == "s3";
                }
            }
            s.seen = sig.or_else(|| Some(String::new()));
        }
        A::ShowManual { on } if !s.busy => {
            s.manual = *on;
            s.check = None;
        }
        A::Adopted { check } => {
            s.busy = false;
            s.request = None;
            s.check = Some(check.clone());
            if check.ok && check.applied {
                s.dirty = false;
                s.manual = true;
            }
        }
        A::SetBackend { backend } if !s.busy && matches!(backend.as_str(), "fs" | "s3") => {
            if s.form.backend != *backend {
                s.form.backend = backend.clone();
                edit(&mut s);
            }
        }
        A::SetField { field, value } if !s.busy => {
            let f = &mut s.form;
            match field {
                StorageField::Endpoint => f.endpoint = value.clone(),
                StorageField::Region => f.region = value.clone(),
                StorageField::Bucket => f.bucket = value.clone(),
                StorageField::AccessKeyId => f.access_key_id = value.clone(),
                StorageField::SecretAccessKey => f.secret_access_key = value.clone(),
            }
            edit(&mut s);
        }
        A::SetPathStyle { on } if !s.busy && s.form.path_style != *on => {
            s.form.path_style = *on;
            edit(&mut s);
        }
        A::Test if !s.busy && s.form.backend == "s3" => {
            s.check = None;
            let update = s.form.update(true);
            start(&mut s, StorageRequest::Test { update });
        }
        A::Save if !s.busy && s.dirty => {
            let update = s.form.update(false);
            start(&mut s, StorageRequest::Save { update });
        }
        A::Checked { check } => {
            s.busy = false;
            s.request = None;
            s.check = Some(check.clone());
        }
        A::Saved { check } => {
            s.busy = false;
            s.request = None;
            s.check = Some(check.clone());
            if check.ok && check.applied {
                s.dirty = false;
                s.form.access_key_id.clear();
                s.form.secret_access_key.clear();
            }
        }
        A::SetMount { on } if !s.busy => start(
            &mut s,
            if *on {
                StorageRequest::Mount
            } else {
                StorageRequest::Unmount
            },
        ),
        A::Reveal { path } if !s.busy => {
            start(&mut s, StorageRequest::Reveal { path: path.clone() })
        }
        A::OpenUrl { url } if !s.busy => {
            start(&mut s, StorageRequest::OpenUrl { url: url.clone() })
        }
        A::SetCache { capacity_bytes } if !s.busy => start(
            &mut s,
            StorageRequest::SetCache {
                capacity_bytes: *capacity_bytes,
            },
        ),
        A::ClearCache if !s.busy => start(&mut s, StorageRequest::ClearCache),
        A::Done => {
            s.busy = false;
            s.request = None;
        }
        A::Failed { error } => {
            s.busy = false;
            s.request = None;
            s.error = Some(error.clone());
        }
        _ => {}
    }
    s
}

fn opt(id: &str, label: &str, active: bool) -> SettingsOption {
    SettingsOption {
        id: id.into(),
        label: label.into(),
        active,
    }
}

pub(crate) fn this_machine(os: SpaceOs) -> &'static str {
    if os == SpaceOs::Macos {
        "This Mac"
    } else {
        "This computer"
    }
}

/// The mount switch's words: a Finder volume on macOS, a mount on Linux.
pub fn mount_label(os: SpaceOs) -> &'static str {
    if os == SpaceOs::Linux {
        "Mount Cua Volume"
    } else {
        "Add Cua Volume to Finder"
    }
}

/// When the mount cannot be used here and the daemon gave no reason.
pub fn not_available(os: SpaceOs) -> String {
    format!(
        "Not available on {} yet",
        if os == SpaceOs::Macos {
            "this Mac"
        } else {
            "this computer"
        }
    )
}

/// A test or save's answer in one line.
pub fn check_text(c: &DriveCheckInput) -> String {
    if c.ok {
        return "Connected, versioning on".into();
    }
    if let Some(d) = c.detail.as_deref().filter(|d| !d.is_empty()) {
        return d.into();
    }
    if !c.reachable {
        "The endpoint did not answer".into()
    } else if !c.authorized {
        "The keys cannot list and write the bucket".into()
    } else if !c.versioning {
        "Bucket versioning is off".into()
    } else {
        "The bucket cannot be used".into()
    }
}

pub(crate) fn form_ready(form: &StorageForm, has_keys: bool) -> bool {
    if form.backend != "s3" {
        return true;
    }
    let id = !form.access_key_id.trim().is_empty();
    let secret = !form.secret_access_key.is_empty();
    !form.bucket.trim().is_empty() && id == secret && (id || has_keys)
}

/// The Storage section as drawn (a Settings section).
pub fn storage_section(input: &StorageInput, state: &StorageState) -> SettingsSection {
    use SettingsRowKind::*;
    let os = input.os;
    let busy = state.busy;
    let mut rows: Vec<SettingsRow> = Vec::new();
    let has_keys = input.storage.as_ref().is_some_and(|s| s.has_keys);
    let saved_s3 = input.storage.as_ref().is_some_and(|s| s.backend == "s3");

    if let Some(storage) = &input.storage {
        let f = &state.form;
        let mut backend = row("backend", Choice, "Store files on");
        backend.options = vec![
            opt("fs", this_machine(os), f.backend == "fs"),
            opt("s3", "S3-compatible", f.backend == "s3"),
        ];
        backend.enabled = !busy;
        rows.push(backend);
        if f.backend == "fs" {
            let mut r = row("fs-path", Text, "Stored in");
            r.value = Some(display_path(&storage.fs_path, input.home.as_deref()));
            r.help = Some(storage.fs_path.clone());
            rows.push(r);
        } else {
            let field = |id: &str, kind: SettingsRowKind, label: &str, value: &str, ph: &str| {
                let mut r = row(id, kind, label);
                r.value = Some(value.into());
                r.placeholder = Some(ph.into()).filter(|p: &String| !p.is_empty());
                r.enabled = !busy;
                r
            };
            let saved = if has_keys && saved_s3 { "Saved" } else { "" };
            if !state.manual {
                let mut p = row("s3-prompt", Prompt, "Ask your agent to set it up:");
                p.value = Some(S3_AGENT_PROMPT.into());
                p.button = Some("Copy".into());
                rows.push(p);
                let mut m = row("s3-manual", Link, "Enter details manually");
                m.enabled = !busy;
                rows.push(m);
                if let Some(c) = &state.check {
                    if c.ok {
                        let mut t = row("s3-test", Text, "Connection");
                        t.value = Some(check_text(c));
                        rows.push(t);
                    } else {
                        rows.push(row("s3-check", Error, &check_text(c)));
                    }
                } else if matches!(state.request, Some(StorageRequest::Adopt { .. })) {
                    let mut t = row("s3-test", Text, "Connection");
                    t.value = Some("Connecting\u{2026}".into());
                    rows.push(t);
                }
            } else {
                rows.push(field("s3-endpoint", Field, "Endpoint", &f.endpoint, "AWS"));
                rows.push(field("s3-bucket", Field, "Bucket", &f.bucket, ""));
                rows.push(field(
                    "s3-region",
                    Field,
                    "Region",
                    &f.region,
                    DEFAULT_REGION,
                ));
                let mut ps = row("s3-path-style", Choice, "Path-style URLs");
                ps.options = vec![
                    opt("on", "On", f.path_style),
                    opt("off", "Off", !f.path_style),
                ];
                ps.enabled = !busy;
                rows.push(ps);
                rows.push(field(
                    "s3-access-key",
                    Field,
                    "Access key ID",
                    &f.access_key_id,
                    saved,
                ));
                rows.push(field(
                    "s3-secret",
                    Secret,
                    "Secret access key",
                    &f.secret_access_key,
                    saved,
                ));
                let testing = matches!(state.request, Some(StorageRequest::Test { .. }));
                let mut t = row("s3-test", Text, "Connection");
                t.value = state.check.as_ref().filter(|c| c.ok).map(check_text);
                t.button = Some(
                    if testing {
                        "Testing\u{2026}"
                    } else {
                        "Test connection"
                    }
                    .into(),
                );
                t.enabled = !busy && form_ready(f, has_keys);
                rows.push(t);
                if let Some(c) = state.check.as_ref().filter(|c| !c.ok) {
                    rows.push(row("s3-check", Error, &check_text(c)));
                }
                if !saved_s3 {
                    let mut l = row("s3-use-prompt", Link, "Use a prompt instead");
                    l.enabled = !busy;
                    rows.push(l);
                }
            }
        }
    }

    match &input.mount {
        Some(m) if m.supported() => {
            let mut r = row("mount", Choice, mount_label(os));
            r.options = vec![opt("on", "On", m.enabled), opt("off", "Off", !m.enabled)];
            r.enabled = !busy;
            rows.push(r);
            match m.state.as_str() {
                "mounted" => {
                    if let Some(path) = m.mounted_path() {
                        let mut r = row(
                            "mount-path",
                            Text,
                            if m.in_finder() {
                                "In Finder at"
                            } else {
                                "Mounted at"
                            },
                        );
                        r.value = Some(display_path(path, input.home.as_deref()));
                        r.help = Some(path.into());
                        r.button = Some(
                            if m.in_finder() {
                                "Show in Finder"
                            } else {
                                "Open"
                            }
                            .into(),
                        );
                        r.link_url = Some(path.into());
                        r.enabled = !busy;
                        rows.push(r);
                    }
                }
                "mounting" => rows.push(row("mount-path", Text, "Mounting\u{2026}")),
                "needs_approval" => {
                    let mut r = row(
                        "mount-approval",
                        Text,
                        m.detail
                            .as_deref()
                            .unwrap_or("Allow the Cua Volume extension in System Settings"),
                    );
                    if let Some(url) = &m.settings_url {
                        r.button = Some("Open System Settings".into());
                        r.link_url = Some(url.clone());
                    }
                    r.enabled = !busy;
                    rows.push(r);
                }
                "error" => rows.push(row(
                    "mount-error",
                    Error,
                    m.detail
                        .as_deref()
                        .unwrap_or("The volume could not be mounted"),
                )),
                _ => {}
            }
        }
        // Windows has no mount: nothing to show.
        _ if os == SpaceOs::Windows => {}
        m => {
            let mut r = row("mount", Text, mount_label(os));
            r.value = Some(
                m.as_ref()
                    .and_then(|m| m.detail.clone())
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| not_available(os)),
            );
            r.enabled = false;
            rows.push(r);
        }
    }

    if let Some(c) = &input.cache {
        let mut r = row("cache", Text, "Cache");
        r.value = Some(format!(
            "{} of {}",
            bytes_text(c.size_bytes),
            bytes_text(c.capacity_bytes)
        ));
        r.button = Some("Clear cache".into());
        r.enabled = !busy && c.size_bytes > 0;
        rows.push(r);
        let mut limits: Vec<u64> = CACHE_LIMITS.to_vec();
        if c.capacity_bytes > 0 && !limits.contains(&c.capacity_bytes) {
            limits.push(c.capacity_bytes);
            limits.sort_unstable();
        }
        let mut l = row("cache-limit", Choice, "Cache size limit");
        l.options = limits
            .into_iter()
            .map(|b| opt(&b.to_string(), &bytes_text(b), b == c.capacity_bytes))
            .collect();
        l.enabled = !busy;
        rows.push(l);
    }

    if input.storage.is_none()
        && input.mount.is_none()
        && input.cache.is_none()
        && os != SpaceOs::Windows
    {
        // The daemon did not answer: nothing to change here yet.
        rows.clear();
        let mut r = row("storage", Text, "Cua Volume");
        r.value = Some(not_available(os));
        r.enabled = false;
        rows.push(r);
    }

    if let Some(e) = &state.error {
        rows.push(row("storage-error", Error, e));
    }

    let saving = matches!(state.request, Some(StorageRequest::Save { .. }));
    SettingsSection {
        id: "storage".into(),
        title: "Storage".into(),
        // Save shows once something changed.
        button: (input.storage.is_some() && (state.dirty || saving))
            .then(|| if saving { "Saving\u{2026}" } else { "Save" }.into()),
        button_enabled: !busy && state.dirty && form_ready(&state.form, has_keys),
        button_help: Some("Keys go to the credential store, never the config file".into()),
        rows,
    }
}

/// [`StorageRequest::text`] (for the shells' logs and parity).
pub fn storage_request_text(r: &StorageRequest) -> String {
    r.text()
}

/// Which request a row's button asks for.
pub fn storage_press(input: &StorageInput, id: &str) -> Option<StorageAction> {
    match id {
        "s3-test" => Some(StorageAction::Test),
        "s3-manual" => Some(StorageAction::ShowManual { on: true }),
        "s3-use-prompt" => Some(StorageAction::ShowManual { on: false }),
        "cache" => Some(StorageAction::ClearCache),
        "mount-path" => input
            .mount
            .as_ref()
            .and_then(|m| m.mounted_path())
            .map(|p| StorageAction::Reveal { path: p.into() }),
        "mount-approval" => input
            .mount
            .as_ref()
            .and_then(|m| m.settings_url.clone())
            .map(|url| StorageAction::OpenUrl { url }),
        _ => None,
    }
}

/// Which action a row's choice asks for.
pub fn storage_choose(id: &str, option: &str) -> Option<StorageAction> {
    match id {
        "backend" => Some(StorageAction::SetBackend {
            backend: option.into(),
        }),
        "s3-path-style" => Some(StorageAction::SetPathStyle { on: option == "on" }),
        "mount" => Some(StorageAction::SetMount { on: option == "on" }),
        "cache-limit" => option
            .parse()
            .ok()
            .map(|capacity_bytes| StorageAction::SetCache { capacity_bytes }),
        _ => None,
    }
}

/// Which action a field's edit asks for.
pub fn storage_edit(id: &str, value: &str) -> Option<StorageAction> {
    StorageField::from_row_id(id).map(|field| StorageAction::SetField {
        field,
        value: value.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s3_storage() -> DriveStorageInput {
        DriveStorageInput {
            backend: "s3".into(),
            fs_path: "/Users/maya/.cua/volume/data".into(),
            s3: Some(DriveS3Input {
                endpoint: Some("http://127.0.0.1:9000".into()),
                region: "us-east-1".into(),
                bucket: "cua-volume".into(),
                root: String::new(),
                path_style: true,
            }),
            has_keys: true,
        }
    }

    fn ids(s: &SettingsSection) -> Vec<&str> {
        s.rows.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn this_mac_by_default_and_s3_keys_only_in_the_request() {
        let input = StorageInput {
            home: Some("/Users/maya".into()),
            storage: Some(DriveStorageInput {
                backend: "fs".into(),
                fs_path: "/Users/maya/.cua/volume/data".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let s = storage_reduce(
            &storage_initial(),
            &StorageAction::Loaded {
                storage: input.storage.clone().unwrap(),
            },
        );
        let v = storage_section(&input, &s);
        assert_eq!(v.rows[0].options[0].label, "This Mac");
        assert!(v.rows[0].options[0].active);
        assert_eq!(v.rows[1].value.as_deref(), Some("~/.cua/volume/data"));
        assert!(!v.button_enabled, "nothing to save");
        // No Cua cloud option.
        assert!(v.rows[0].options.iter().all(|o| o.id != "cloud"));

        let mut s = storage_reduce(&s, &storage_choose("backend", "s3").unwrap());
        // The agent prompt first; the fields on request.
        let v = storage_section(&input, &s);
        let prompt = v.rows.iter().find(|r| r.id == "s3-prompt").unwrap();
        assert_eq!(prompt.kind, SettingsRowKind::Prompt);
        assert_eq!(prompt.value.as_deref(), Some(S3_AGENT_PROMPT));
        assert!(v.rows.iter().all(|r| r.id != "s3-secret"));
        s = storage_reduce(&s, &storage_press(&input, "s3-manual").unwrap());
        let v = storage_section(&input, &s);
        assert!(v.rows.iter().all(|r| r.id != "s3-prompt"));
        assert!(v.rows.iter().any(|r| r.id == "s3-use-prompt"));
        for (id, value) in [
            ("s3-endpoint", "http://127.0.0.1:9000"),
            ("s3-bucket", "cua-volume"),
            ("s3-access-key", "AKIA"),
        ] {
            s = storage_reduce(&s, &storage_edit(id, value).unwrap());
        }
        let v = storage_section(&input, &s);
        assert!(
            !v.button_enabled,
            "a secret without the id or the other way"
        );
        s = storage_reduce(&s, &storage_edit("s3-secret", "shh").unwrap());
        let v = storage_section(&input, &s);
        assert!(v.button_enabled);
        assert_eq!(
            v.rows.iter().find(|r| r.id == "s3-secret").unwrap().kind,
            SettingsRowKind::Secret
        );
        let s = storage_reduce(&s, &StorageAction::Save);
        let Some(StorageRequest::Save { update }) = &s.request else {
            panic!("{:?}", s.request)
        };
        assert_eq!(update.access_key_id.as_deref(), Some("AKIA"));
        assert_eq!(update.secret_access_key.as_deref(), Some("shh"));
        assert!(!update.dry_run);
        let json = serde_json::to_value(update).unwrap();
        assert_eq!(json["s3"]["path_style"], false, "the tool's snake_case");
        let s = storage_reduce(
            &s,
            &StorageAction::Saved {
                check: DriveCheckInput {
                    ok: true,
                    reachable: true,
                    authorized: true,
                    versioning: true,
                    applied: true,
                    detail: None,
                },
            },
        );
        assert!(!s.dirty && s.form.secret_access_key.is_empty() && s.form.access_key_id.is_empty());
    }

    #[test]
    fn saved_keys_need_not_be_typed_again_and_a_failed_test_says_why() {
        let input = StorageInput {
            storage: Some(s3_storage()),
            ..Default::default()
        };
        let s = storage_reduce(
            &storage_initial(),
            &StorageAction::Loaded {
                storage: s3_storage(),
            },
        );
        let v = storage_section(&input, &s);
        let key = v.rows.iter().find(|r| r.id == "s3-access-key").unwrap();
        assert_eq!(key.placeholder.as_deref(), Some("Saved"));
        let s = storage_reduce(&s, &StorageAction::Test);
        let Some(StorageRequest::Test { update }) = &s.request else {
            panic!()
        };
        assert!(update.dry_run && update.access_key_id.is_none());
        let s = storage_reduce(
            &s,
            &StorageAction::Checked {
                check: DriveCheckInput {
                    reachable: true,
                    authorized: true,
                    ..Default::default()
                },
            },
        );
        let v = storage_section(&input, &s);
        assert!(ids(&v).contains(&"s3-check"));
        assert_eq!(
            v.rows.iter().find(|r| r.id == "s3-check").unwrap().label,
            "Bucket versioning is off"
        );
    }

    #[test]
    fn the_mount_row_follows_the_daemon() {
        let mut input = StorageInput::default();
        // Nothing answered: one honest disabled row.
        let v = storage_section(&input, &storage_initial());
        assert_eq!(ids(&v), ["storage"]);
        assert_eq!(
            v.rows[0].value.as_deref(),
            Some("Not available on this Mac yet")
        );
        assert!(!v.rows[0].enabled);
        input.mount = Some(DriveMountInput {
            enabled: true,
            state: "mounted".into(),
            method: "fskit".into(),
            path: Some("/Volumes/Cua Volume".into()),
            volume_name: "Cua Volume".into(),
            ..Default::default()
        });
        let v = storage_section(&input, &storage_initial());
        assert_eq!(ids(&v), ["mount", "mount-path"]);
        assert_eq!(v.rows[1].button.as_deref(), Some("Show in Finder"));
        assert_eq!(
            storage_press(&input, "mount-path"),
            Some(StorageAction::Reveal {
                path: "/Volumes/Cua Volume".into()
            })
        );
        input.os = SpaceOs::Linux;
        input.mount.as_mut().unwrap().method = "fuse".into();
        let v = storage_section(&input, &storage_initial());
        assert_eq!(v.rows[0].label, "Mount Cua Volume");
        assert_eq!(v.rows[1].button.as_deref(), Some("Open"));
        input.os = SpaceOs::Windows;
        input.mount = Some(DriveMountInput {
            state: "unsupported".into(),
            method: "none".into(),
            ..Default::default()
        });
        assert!(storage_section(&input, &storage_initial()).rows.is_empty());
    }

    #[test]
    fn cache_limits_and_sizes() {
        assert_eq!(bytes_text(10 * GIB), "10 GB");
        assert_eq!(bytes_text(GIB + GIB / 2), "1.5 GB");
        assert_eq!(bytes_text(300 * 1024 * 1024), "300 MB");
        let input = StorageInput {
            cache: Some(DriveCacheInput {
                size_bytes: 0,
                capacity_bytes: 2 * GIB,
            }),
            ..Default::default()
        };
        let v = storage_section(&input, &storage_initial());
        let row = |id: &str| v.rows.iter().find(|r| r.id == id).unwrap().clone();
        assert!(!row("cache").enabled, "nothing to clear");
        assert_eq!(row("cache").value.as_deref(), Some("0 KB of 2 GB"));
        let limit = row("cache-limit");
        let labels: Vec<_> = limit.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["1 GB", "2 GB", "5 GB", "10 GB", "50 GB"]);
        assert_eq!(
            storage_choose("cache-limit", &(5 * GIB).to_string()),
            Some(StorageAction::SetCache {
                capacity_bytes: 5 * GIB
            })
        );
    }

    #[test]
    fn a_bucket_the_agent_connects_is_noticed_and_adopted() {
        let fs = DriveStorageInput {
            backend: "fs".into(),
            fs_path: "/Users/maya/.cua/volume/data".into(),
            ..Default::default()
        };
        let mut input = StorageInput {
            storage: Some(fs.clone()),
            ..Default::default()
        };
        let s = storage_reduce(
            &storage_initial(),
            &StorageAction::Loaded {
                storage: fs.clone(),
            },
        );
        let s = storage_reduce(&s, &storage_choose("backend", "s3").unwrap());
        // Polling: nothing yet, and the choice stays.
        let s = storage_reduce(&s, &StorageAction::Loaded { storage: fs });
        assert_eq!(s.form.backend, "s3");
        assert!(s.request.is_none());
        // The agent ran `cua volume config set` and `set-keys`.
        let mut s3 = s3_storage();
        s3.s3.as_mut().unwrap().endpoint = None;
        let s = storage_reduce(
            &s,
            &StorageAction::Loaded {
                storage: s3.clone(),
            },
        );
        let Some(StorageRequest::Adopt { update }) = &s.request else {
            panic!("{:?}", s.request)
        };
        assert!(update.access_key_id.is_none() && !update.dry_run);
        assert_eq!(update.s3.as_ref().unwrap().bucket, "cua-volume");
        input.storage = Some(s3);
        let v = storage_section(&input, &s);
        assert_eq!(
            v.rows
                .iter()
                .find(|r| r.id == "s3-test")
                .unwrap()
                .value
                .as_deref(),
            Some("Connecting\u{2026}")
        );
        let s = storage_reduce(
            &s,
            &StorageAction::Adopted {
                check: DriveCheckInput {
                    ok: true,
                    reachable: true,
                    authorized: true,
                    versioning: true,
                    applied: true,
                    detail: None,
                },
            },
        );
        let v = storage_section(&input, &s);
        assert_eq!(
            v.rows
                .iter()
                .find(|r| r.id == "s3-test")
                .unwrap()
                .value
                .as_deref(),
            Some("Connected, versioning on")
        );
        // An already saved bucket is never adopted again on open.
        let again = storage_reduce(
            &storage_initial(),
            &StorageAction::Loaded {
                storage: s3_storage(),
            },
        );
        assert!(again.request.is_none() && again.manual);
    }

    #[test]
    fn a_poll_keeps_the_fields_open() {
        let fs = DriveStorageInput {
            backend: "fs".into(),
            ..Default::default()
        };
        let s = storage_reduce(
            &storage_initial(),
            &StorageAction::Loaded {
                storage: fs.clone(),
            },
        );
        let s = storage_reduce(&s, &storage_choose("backend", "s3").unwrap());
        let s = storage_reduce(&s, &StorageAction::ShowManual { on: true });
        let s = storage_reduce(&s, &StorageAction::Loaded { storage: fs });
        assert!(s.manual);
    }
}
