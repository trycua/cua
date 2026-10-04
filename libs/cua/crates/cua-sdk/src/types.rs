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
}

/// How a process ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ExitInfo {
    /// Exit code for a normal exit.
    pub code: Option<i32>,
    /// Terminating signal (`kill`, `term`, ...).
    pub signal: Option<String>,
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
}

impl Default for UploadOptions {
    fn default() -> Self {
        UploadOptions {
            create_new: false,
            append: false,
            permissions: 0,
            create_parents: true,
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
