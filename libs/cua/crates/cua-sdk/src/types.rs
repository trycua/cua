//! Records shared by the native and browser (wasm32) builds.

use cua_proto::env::v1 as pb;
use std::collections::HashMap;

/// One capability flag.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesdFeature {
    /// Feature name (for example `a11y`, `stream.h264`, `audio.desktop`).
    pub name: String,
    /// Supported in this guest.
    pub supported: bool,
    /// Limitation when degraded or unsupported.
    pub limitation: Option<String>,
    /// Extra attributes.
    pub attributes: HashMap<String, String>,
}

/// `SystemService.GetCapabilities`, summarized. `json` has everything.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesdCapabilities {
    /// spacesd version.
    pub version: String,
    /// `cua.env.v1` protocol version.
    pub protocol_version: u32,
    /// Additive contract revision.
    pub protocol_revision: u32,
    /// OS family (`linux`, `macos`, `windows`).
    pub os_family: String,
    /// OS name.
    pub os_name: String,
    /// OS version.
    pub os_version: String,
    /// CPU architecture.
    pub arch: String,
    /// Guest hostname.
    pub hostname: String,
    /// `SystemService.Init` has run.
    pub initialized: bool,
    /// Feature flags.
    pub features: Vec<SpacesdFeature>,
    /// The full response as proto3 JSON.
    pub json: String,
}

pub(crate) fn enum_suffix(debug: String) -> String {
    debug.to_ascii_lowercase()
}

impl From<pb::GetCapabilitiesResponse> for SpacesdCapabilities {
    fn from(c: pb::GetCapabilitiesResponse) -> Self {
        let json = serde_json::to_string(&c).unwrap_or_default();
        let os = c.os.clone().unwrap_or_default();
        SpacesdCapabilities {
            version: c.version.clone(),
            protocol_version: c.protocol_version,
            protocol_revision: c.protocol_revision,
            os_family: enum_suffix(format!(
                "{:?}",
                pb::OsFamily::try_from(os.family).unwrap_or_default()
            )),
            os_name: os.name,
            os_version: os.version,
            arch: enum_suffix(format!(
                "{:?}",
                pb::Architecture::try_from(c.arch).unwrap_or_default()
            )),
            hostname: c.hostname.clone(),
            initialized: c.initialized,
            features: c
                .features
                .into_iter()
                .map(|f| SpacesdFeature {
                    name: f.name,
                    supported: f.supported,
                    limitation: (!f.limitation.is_empty()).then_some(f.limitation),
                    attributes: f.attributes.into_iter().collect(),
                })
                .collect(),
            json,
        }
    }
}

/// A PTY size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct PtySize {
    /// Columns.
    pub cols: u32,
    /// Rows.
    pub rows: u32,
}

/// A command to run in the guest.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesdCommand {
    /// Program (absolute path or `PATH` lookup).
    pub program: String,
    /// Arguments.
    #[uniffi(default = [])]
    pub args: Vec<String>,
    /// Extra environment.
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Working directory.
    #[uniffi(default = None)]
    pub cwd: Option<String>,
    /// Run as this user.
    #[uniffi(default = None)]
    pub user: Option<String>,
    /// Kill after this long.
    #[uniffi(default = None)]
    pub timeout_ms: Option<u32>,
    /// Tag for reattaching (`attach(tag=…)`).
    #[uniffi(default = None)]
    pub tag: Option<String>,
    /// Keep stdin open for `write_stdin`.
    #[uniffi(default = false)]
    pub stdin: bool,
    /// Run in a PTY of this size.
    #[uniffi(default = None)]
    pub pty: Option<PtySize>,
    /// Idempotency key: a later start with the same key reattaches to the
    /// running process or replays its recorded outcome instead of running
    /// the command again (the guest's exec ledger; see `GetExecRecord`).
    #[uniffi(default = None)]
    pub request_id: Option<String>,
    /// Fencing epoch: a start with an epoch lower than the Space's current
    /// one is refused; a higher one advances it. `None` is unfenced.
    #[uniffi(default = None)]
    pub epoch: Option<u64>,
    /// Size of the guest's output scrollback ring for this process, in
    /// bytes. The ring never blocks the process: a reader that falls more
    /// than this far behind loses the evicted bytes (visible as an offset
    /// gap). `None` uses the server default (8 MiB); the server clamps
    /// larger requests to its maximum (512 MiB) rather than failing. The
    /// ring is allocated up to this size as output arrives, so the cost is
    /// bounded by the output actually produced.
    #[uniffi(default = None)]
    pub scrollback_bytes: Option<u64>,
    /// Kill the process (`term`, then `kill`) when this client's connection
    /// to cua-spacesd ends before the process does, including when the
    /// client process dies. Off by default: processes are detached and can be
    /// reattached by tag.
    #[uniffi(default = false)]
    pub kill_on_disconnect: bool,
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ExitInfo {
    /// Exit code for a normal exit.
    pub code: Option<i32>,
    /// Terminating signal (`kill`, `term`, ...).
    pub signal: Option<String>,
    /// Raw number of the terminating signal on the guest's OS, set for every
    /// signal death, also for those `signal` has no name for (SIGSEGV,
    /// SIGABRT, SIGPIPE, ...).
    #[uniffi(default = None)]
    pub signal_number: Option<i32>,
    /// Conventional name of the terminating signal without `SIG`, in upper
    /// case ("SEGV", "ABRT"); independent of the guest's OS.
    #[uniffi(default = None)]
    pub signal_name: Option<String>,
    /// Stopped by its timeout.
    pub timed_out: bool,
    /// Supervisor error (for example "executable not found").
    pub error: Option<String>,
    /// `code == 0`.
    pub success: bool,
}

/// Collected output of a finished process.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProcessOutput {
    /// Exit.
    pub exit: ExitInfo,
    /// stdout bytes.
    pub stdout: Vec<u8>,
    /// stderr bytes.
    pub stderr: Vec<u8>,
    /// PTY bytes.
    pub pty: Vec<u8>,
}

/// Kind of a process event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ProcessEventKind {
    /// stdout bytes.
    Stdout,
    /// stderr bytes.
    Stderr,
    /// PTY bytes.
    Pty,
    /// The process ended (always last).
    Exit,
}

/// One output chunk or the exit.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProcessEvent {
    /// Kind.
    pub kind: ProcessEventKind,
    /// Combined-output offset of the first byte.
    pub offset: u64,
    /// Bytes (empty for `Exit`).
    pub data: Vec<u8>,
    /// Exit (only for `Exit`).
    pub exit: Option<ExitInfo>,
}

/// Screenshot encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ImageFormat {
    /// PNG.
    Png,
    /// JPEG.
    Jpeg,
    /// WebP.
    Webp,
}

impl ImageFormat {
    pub(crate) fn to_pb(self) -> pb::ImageFormat {
        match self {
            ImageFormat::Png => pb::ImageFormat::Png,
            ImageFormat::Jpeg => pb::ImageFormat::Jpeg,
            ImageFormat::Webp => pb::ImageFormat::Webp,
        }
    }

    pub(crate) fn from_pb(f: pb::ImageFormat) -> Self {
        match f {
            pb::ImageFormat::Jpeg => ImageFormat::Jpeg,
            pb::ImageFormat::Webp => ImageFormat::Webp,
            _ => ImageFormat::Png,
        }
    }
}

/// Screenshot options.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ScreenshotOptions {
    /// Display id (primary when unset).
    #[uniffi(default = None)]
    pub display: Option<String>,
    /// Encoding (PNG when unset).
    #[uniffi(default = None)]
    pub format: Option<ImageFormat>,
    /// Lossy quality 1–100.
    #[uniffi(default = None)]
    pub quality: Option<u32>,
    /// Long-edge cap in pixels.
    #[uniffi(default = None)]
    pub max_dimension: Option<u32>,
    /// Draw the cursor.
    #[uniffi(default = false)]
    pub include_cursor: bool,
}

/// A captured screenshot.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Screenshot {
    /// Encoded image.
    pub image: Vec<u8>,
    /// Encoding.
    pub format: ImageFormat,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Image pixels per logical point.
    pub scale: f64,
    /// Id for screenshot-space input.
    pub screenshot_id: String,
}

/// A Space's latest thumbnail, from the cache every client on this machine
/// shares ([`crate::native::spaces::Space::thumbnail`]).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SpaceThumbnail {
    /// Encoded image (a small JPEG of the primary display).
    pub image: Vec<u8>,
    /// Encoding.
    pub format: ImageFormat,
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// When it was captured, Unix milliseconds.
    pub captured_at_ms: u64,
}

/// A point.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct Point {
    /// X.
    pub x: f64,
    /// Y.
    pub y: f64,
}

/// A file or directory in the guest.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FileEntry {
    /// Base name.
    pub name: String,
    /// Absolute path.
    pub path: String,
    /// `file`, `directory`, `symlink` or `other`.
    pub kind: String,
    /// Size in bytes.
    pub size: u64,
    /// Unix mode bits.
    pub mode: u32,
    /// Modification time (Unix ms), when known.
    pub modified_ms: Option<i64>,
    /// Symlink target.
    pub symlink_target: Option<String>,
    /// Owning user name (a numeric id when the guest has no such user);
    /// `None` where the guest has no owners (Windows).
    pub owner: Option<String>,
    /// Owning group name; `None` where the guest has none (Windows).
    pub group: Option<String>,
    /// Extended attributes under the call's `xattr_namespace` (default
    /// `user.cua.meta.`), without the prefix. Empty when there are none or
    /// the guest file system has no extended attributes.
    pub metadata: std::collections::HashMap<String, String>,
}

impl From<pb::EntryInfo> for FileEntry {
    fn from(e: pb::EntryInfo) -> Self {
        let kind = match pb::FileType::try_from(e.r#type).unwrap_or_default() {
            pb::FileType::File => "file",
            pb::FileType::Directory => "directory",
            pb::FileType::Symlink => "symlink",
            _ => "other",
        };
        FileEntry {
            name: e.name,
            path: e.path,
            kind: kind.into(),
            size: e.size,
            mode: e.mode,
            modified_ms: e
                .modified_at
                .map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000),
            symlink_target: (!e.symlink_target.is_empty()).then_some(e.symlink_target),
            owner: (!e.owner.is_empty()).then_some(e.owner),
            group: (!e.group.is_empty()).then_some(e.group),
            metadata: e.metadata,
        }
    }
}

/// Upload options.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UploadOptions {
    /// Fail when the file exists (instead of overwriting).
    #[uniffi(default = false)]
    pub create_new: bool,
    /// Append instead of overwriting.
    #[uniffi(default = false)]
    pub append: bool,
    /// Unix permissions (0 = default).
    #[uniffi(default = 0)]
    pub permissions: u32,
    /// Create missing parent directories.
    #[uniffi(default = true)]
    pub create_parents: bool,
    /// Guest user to write as: the file (and parents it creates) is owned
    /// by that user and the user's permissions apply. Needs a guest agent
    /// that runs as root or has passwordless `sudo` (the Linux images do);
    /// otherwise the call fails with a permission error that says why.
    #[uniffi(default = None)]
    pub user: Option<String>,
    /// Extended attributes to store with the file, published atomically
    /// with its content. At most 32 keys; a key is 1-128 bytes of
    /// `A-Za-z0-9._:/@+-`, a value at most 1024 bytes, 3072 bytes in all.
    #[uniffi(default = None)]
    pub metadata: Option<std::collections::HashMap<String, String>>,
    /// Extended-attribute key prefix for `metadata` (and for the metadata
    /// reported back): must start with `user.` and end with `.`. Default
    /// `user.cua.meta.`.
    #[uniffi(default = None)]
    pub xattr_namespace: Option<String>,
}

impl Default for UploadOptions {
    fn default() -> Self {
        UploadOptions {
            create_new: false,
            append: false,
            permissions: 0,
            create_parents: true,
            user: None,
            metadata: None,
            xattr_namespace: None,
        }
    }
}

/// Result of an upload or download.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TransferResult {
    /// Bytes transferred.
    pub size: u64,
    /// SHA-256 (hex) of the content.
    pub sha256: String,
    /// Resumes after dropped connections.
    pub resumes: u32,
}

/// Media session options.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaOpenOptions {
    /// Display id (primary when unset). Ignored when `window_handle` is set.
    #[uniffi(default = None)]
    pub display: Option<String>,
    /// Window handle (from `ListTargets` / `ListWindows`).
    #[uniffi(default = None)]
    pub window_handle: Option<String>,
    /// FPS cap (0 = driver default).
    #[uniffi(default = 0)]
    pub max_fps: u32,
    /// Long-edge cap (0 = native).
    #[uniffi(default = 0)]
    pub max_dimension: u32,
    /// Also negotiate desktop/app audio (Opus).
    #[uniffi(default = false)]
    pub audio: bool,
    /// Audio only.
    #[uniffi(default = false)]
    pub disable_video: bool,
    /// Extra `OpenMediaRequest` fields as proto3 JSON, merged over the
    /// above (escape hatch).
    #[uniffi(default = None)]
    pub request_json: Option<String>,
}
