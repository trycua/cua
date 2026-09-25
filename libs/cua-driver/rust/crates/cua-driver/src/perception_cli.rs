use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::Result as AnyhowResult;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use image::{ImageDecoder, Limits};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use cua_driver_contract::{
    ParseVisualRegionsInput, ParseVisualRegionsOptions, VisualActionCoordinateSpace,
    VisualCaptureProvenance, VisualCaptureSource, VisualParseError, VisualParseErrorCode,
    VisualScreenshotReference,
};
use cua_driver_core::perception_client::PerceptionCancellation;
use cua_driver_core::perception_tools::build_local_output;

const MAX_CAPTURE_BYTES: u64 = 1024 * 1024;
const MAX_PNG_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PNG_DIMENSION: u32 = 16_384;
const MAX_DECODED_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalCaptureMetadata {
    source: LocalCaptureSource,
    #[serde(default)]
    snapshot_id: Option<String>,
    #[serde(default)]
    captured_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum LocalCaptureSource {
    Window { pid: u32, window_id: u64 },
    PrimaryDesktop { display_id: String },
}

impl From<LocalCaptureSource> for VisualCaptureSource {
    fn from(source: LocalCaptureSource) -> Self {
        match source {
            LocalCaptureSource::Window { pid, window_id } => Self::Window { pid, window_id },
            LocalCaptureSource::PrimaryDesktop { display_id } => {
                Self::PrimaryDesktop { display_id }
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    ok: bool,
    error: CliError,
}

#[derive(Debug, Serialize)]
struct CliError {
    code: String,
    message: String,
    retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl CliError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            retryable: false,
            detail: None,
        }
    }

    fn with_detail(code: &str, message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
            retryable: false,
            detail: Some(detail.into()),
        }
    }
}

impl From<VisualParseError> for CliError {
    fn from(error: VisualParseError) -> Self {
        Self {
            code: visual_error_code(error.code).to_owned(),
            message: error.message,
            retryable: error.retryable,
            detail: error.detail,
        }
    }
}

pub fn run(args: &[String]) {
    if let Err(error) = run_inner(args) {
        let envelope = ErrorEnvelope { ok: false, error };
        println!(
            "{}",
            serde_json::to_string(&envelope).unwrap_or_else(|_| {
                r#"{"ok":false,"error":{"code":"internal_error","message":"serialize perception error","retryable":false}}"#.to_owned()
            })
        );
        std::process::exit(1);
    }
}

fn run_inner(args: &[String]) -> Result<(), CliError> {
    let (image_path, capture_path) = parse_args(args)?;
    let capture_bytes = read_bounded_file(&capture_path, MAX_CAPTURE_BYTES, "capture metadata")?;
    let metadata: LocalCaptureMetadata =
        serde_json::from_slice(&capture_bytes).map_err(|error| {
            CliError::with_detail(
                "invalid_capture_metadata",
                "capture metadata is invalid",
                error.to_string(),
            )
        })?;
    if metadata
        .snapshot_id
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(CliError::new(
            "invalid_capture_metadata",
            "snapshot_id must not be empty",
        ));
    }
    if metadata
        .captured_at
        .as_ref()
        .is_some_and(|value| !is_rfc3339(value))
    {
        return Err(CliError::new(
            "invalid_capture_metadata",
            "captured_at must be an RFC3339 timestamp",
        ));
    }
    match &metadata.source {
        LocalCaptureSource::Window { pid, window_id } if *pid == 0 || *window_id == 0 => {
            return Err(CliError::new(
                "invalid_capture_metadata",
                "window capture metadata requires positive pid and window_id",
            ));
        }
        LocalCaptureSource::PrimaryDesktop { display_id } if display_id != "primary" => {
            return Err(CliError::new(
                "invalid_capture_metadata",
                "desktop capture metadata supports only display_id \"primary\"",
            ));
        }
        _ => {}
    }

    let png = read_bounded_file(&image_path, MAX_PNG_BYTES, "PNG image")?;
    let (width, height) = validate_png(&png).map_err(|error| {
        CliError::with_detail("invalid_png", "PNG image is invalid", format!("{error:#}"))
    })?;
    let digest = format!("{:x}", Sha256::digest(&png));
    let capture_id = local_capture_id(&digest);
    let input = ParseVisualRegionsInput {
        capture_id: capture_id.clone(),
        options: ParseVisualRegionsOptions::default(),
    };
    let provenance = VisualCaptureProvenance {
        capture_id: capture_id.clone(),
        source: metadata.source.into(),
        screenshot: VisualScreenshotReference {
            reference: format!("local-png-sha256:{digest}"),
            width,
            height,
            mime_type: "image/png".to_owned(),
            sha256: Some(digest.clone()),
        },
        // This describes region coordinates only. The local_input block below
        // explicitly denies any Driver action authority.
        action_coordinate_space: VisualActionCoordinateSpace::ScreenshotPixels,
        captured_at: metadata.captured_at,
    };

    let client = crate::extension_manager::perception_client_for_cli().map_err(|cause| {
        CliError::from(local_visual_error(
            VisualParseErrorCode::ArtifactInvalid,
            cause.to_string(),
        ))
    })?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            CliError::with_detail(
                "internal_error",
                "create perception runtime",
                error.to_string(),
            )
        })?;
    let started = Instant::now();
    let worker = runtime
        .block_on(client.parse(
            &capture_id,
            width,
            height,
            &png,
            &PerceptionCancellation::default(),
        ))
        .map_err(CliError::from)?;
    let output = build_local_output(&input, provenance, worker, started.elapsed().as_millis())
        .map_err(CliError::from)?;
    let mut value = serde_json::to_value(output).map_err(|error| {
        CliError::with_detail(
            "internal_error",
            "serialize perception result",
            error.to_string(),
        )
    })?;
    insert_local_input(&mut value, metadata.snapshot_id, digest)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&value).map_err(|error| {
            CliError::with_detail(
                "internal_error",
                "serialize perception result",
                error.to_string(),
            )
        })?
    );
    Ok(())
}

fn parse_args(args: &[String]) -> Result<(PathBuf, PathBuf), CliError> {
    if args.first().map(String::as_str) != Some("parse") {
        return Err(CliError::new(
            "invalid_arguments",
            "usage: cua-driver perception parse --image <png> --capture <capture.json> --json",
        ));
    }
    let mut image = None;
    let mut capture = None;
    let mut json = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--image" | "--capture" => {
                let flag = args[index].as_str();
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| {
                        CliError::new("invalid_arguments", format!("{flag} requires a path"))
                    })?;
                let slot = if flag == "--image" {
                    &mut image
                } else {
                    &mut capture
                };
                if slot.replace(PathBuf::from(value)).is_some() {
                    return Err(CliError::new(
                        "invalid_arguments",
                        format!("duplicate option {flag}"),
                    ));
                }
                index += 2;
            }
            "--json" => {
                if json {
                    return Err(CliError::new("invalid_arguments", "duplicate flag --json"));
                }
                json = true;
                index += 1;
            }
            value => {
                return Err(CliError::new(
                    "invalid_arguments",
                    format!("unknown perception parse argument {value:?}"),
                ));
            }
        }
    }
    if !json {
        return Err(CliError::new(
            "invalid_arguments",
            "perception parse requires --json",
        ));
    }
    Ok((
        image.ok_or_else(|| {
            CliError::new(
                "invalid_arguments",
                "perception parse requires --image <png>",
            )
        })?,
        capture.ok_or_else(|| {
            CliError::new(
                "invalid_arguments",
                "perception parse requires --capture <capture.json>",
            )
        })?,
    ))
}

fn read_bounded_file(path: &Path, maximum: u64, label: &str) -> Result<Vec<u8>, CliError> {
    let (directory, name) = open_parent_nofollow(path).map_err(|error| {
        CliError::with_detail(
            "input_open_failed",
            format!("cannot open {label}"),
            format!("{}: {error:#}", path.display()),
        )
    })?;
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let cap_file = directory.open_with(name, &options).map_err(|error| {
        CliError::with_detail(
            "input_open_failed",
            format!("cannot open {label}"),
            format!("{}: {error}", path.display()),
        )
    })?;
    let mut file = cap_file.into_std();
    let metadata = file.metadata().map_err(|error| {
        CliError::with_detail(
            "input_open_failed",
            format!("cannot inspect {label}"),
            format!("{}: {error}", path.display()),
        )
    })?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(CliError::new(
                "input_not_regular_file",
                format!("{label} must not be a symlink or reparse point"),
            ));
        }
    }
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::new(
            "input_not_regular_file",
            format!("{label} is not a regular file"),
        ));
    }
    if metadata.len() > maximum {
        return Err(CliError::new(
            "input_too_large",
            format!("{label} exceeds the {maximum}-byte limit"),
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CliError::with_detail(
                "input_read_failed",
                format!("cannot read {label}"),
                format!("{}: {error}", path.display()),
            )
        })?;
    if bytes.len() as u64 > maximum {
        return Err(CliError::new(
            "input_too_large",
            format!("{label} exceeds the {maximum}-byte limit"),
        ));
    }
    Ok(bytes)
}

fn open_parent_nofollow(path: &Path) -> AnyhowResult<(Dir, std::ffi::OsString)> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent"))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no final component"))?
        .to_owned();
    let (mut directory, names) = directory_path_anchor(parent)?;
    for component in names {
        directory = directory.open_dir_nofollow(component)?;
    }
    Ok((directory, name))
}

fn directory_path_anchor(path: &Path) -> AnyhowResult<(Dir, Vec<std::ffi::OsString>)> {
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
            Component::ParentDir => anyhow::bail!("path must not contain parent traversal"),
            Component::Prefix(_) | Component::RootDir => anyhow::bail!("path has an invalid root"),
        }
    }
    Ok((Dir::open_ambient_dir(anchor, ambient_authority())?, names))
}

fn validate_png(bytes: &[u8]) -> AnyhowResult<(u32, u32)> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_PNG_DIMENSION);
    limits.max_image_height = Some(MAX_PNG_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    let decoder = image::codecs::png::PngDecoder::with_limits(Cursor::new(bytes), limits)?;
    let dimensions = decoder.dimensions();
    if dimensions.0 == 0 || dimensions.1 == 0 || decoder.total_bytes() > MAX_DECODED_BYTES {
        anyhow::bail!("PNG dimensions exceed local perception limits");
    }
    Ok(dimensions)
}

fn local_capture_id(digest: &str) -> String {
    format!("local_png_{}", &digest[..24])
}

fn insert_local_input(
    value: &mut serde_json::Value,
    snapshot_id: Option<String>,
    digest: String,
) -> Result<(), CliError> {
    value
        .as_object_mut()
        .ok_or_else(|| CliError::new("internal_error", "perception result is not an object"))?
        .insert(
            "local_input".to_owned(),
            json!({
                "kind": "local_file",
                "snapshot_id": snapshot_id,
                "sha256": digest,
                "action_eligible": false,
                "action_authority": "none",
                "reason": "Local image input is not a Driver-owned capture and cannot authorize input."
            }),
        );
    Ok(())
}

fn is_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || !matches!(bytes.get(10), Some(b'T' | b't'))
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return false;
    }
    let Some(year) = decimal(bytes, 0, 4) else {
        return false;
    };
    let Some(month) = decimal(bytes, 5, 2) else {
        return false;
    };
    let Some(day) = decimal(bytes, 8, 2) else {
        return false;
    };
    let Some(hour) = decimal(bytes, 11, 2) else {
        return false;
    };
    let Some(minute) = decimal(bytes, 14, 2) else {
        return false;
    };
    let Some(second) = decimal(bytes, 17, 2) else {
        return false;
    };
    if month == 0
        || month > 12
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return false;
    }
    let mut index = 19;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == start {
            return false;
        }
    }
    match bytes.get(index) {
        Some(b'Z' | b'z') => index + 1 == bytes.len(),
        Some(b'+' | b'-') => {
            bytes.len() == index + 6
                && bytes.get(index + 3) == Some(&b':')
                && decimal(bytes, index + 1, 2).is_some_and(|value| value <= 23)
                && decimal(bytes, index + 4, 2).is_some_and(|value| value <= 59)
        }
        _ => false,
    }
}

fn decimal(bytes: &[u8], start: usize, len: usize) -> Option<u32> {
    bytes
        .get(start..start + len)?
        .iter()
        .try_fold(0, |value, byte| {
            byte.is_ascii_digit()
                .then_some(value * 10 + u32::from(byte - b'0'))
        })
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn local_visual_error(code: VisualParseErrorCode, detail: String) -> VisualParseError {
    VisualParseError {
        code,
        message: "the installed cua-perception extension is invalid".to_owned(),
        retryable: false,
        detail: Some(detail),
    }
}

fn visual_error_code(code: VisualParseErrorCode) -> &'static str {
    match code {
        VisualParseErrorCode::NotInstalled => "not_installed",
        VisualParseErrorCode::CaptureNotFound => "capture_not_found",
        VisualParseErrorCode::CaptureExpired => "capture_expired",
        VisualParseErrorCode::CaptureStale => "capture_stale",
        VisualParseErrorCode::CaptureGenerationMismatch => "capture_generation_mismatch",
        VisualParseErrorCode::UnsupportedTarget => "unsupported_target",
        VisualParseErrorCode::UnsupportedPlatform => "unsupported_platform",
        VisualParseErrorCode::IncompatibleProtocol => "incompatible_protocol",
        VisualParseErrorCode::InvalidFrame => "invalid_frame",
        VisualParseErrorCode::WorkerLaunchFailed => "worker_launch_failed",
        VisualParseErrorCode::WorkerCrashed => "worker_crashed",
        VisualParseErrorCode::WorkerCancelled => "worker_cancelled",
        VisualParseErrorCode::Timeout => "timeout",
        VisualParseErrorCode::ResourceLimitExceeded => "resource_limit_exceeded",
        VisualParseErrorCode::ArtifactInvalid => "artifact_invalid",
        VisualParseErrorCode::InferenceFailed => "inference_failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_require_both_paths_and_json() {
        assert!(parse_args(&["parse".into(), "--json".into()]).is_err());
        assert!(parse_args(&[
            "parse".into(),
            "--image".into(),
            "image.png".into(),
            "--json".into(),
        ])
        .is_err());
        assert!(parse_args(&[
            "parse".into(),
            "--image".into(),
            "image.png".into(),
            "--capture".into(),
            "capture.json".into(),
        ])
        .is_err());
        assert!(parse_args(&[
            "parse".into(),
            "--image".into(),
            "image.png".into(),
            "--capture".into(),
            "capture.json".into(),
            "--json".into(),
        ])
        .is_ok());
    }

    #[test]
    fn validates_rfc3339_timestamps() {
        for valid in [
            "2026-09-18T12:00:00Z",
            "2024-02-29t23:59:60.123+05:30",
            "2026-09-18T12:00:00-00:00",
        ] {
            assert!(is_rfc3339(valid), "rejected {valid}");
        }
        for invalid in [
            "2026-09-18",
            "2026-02-29T12:00:00Z",
            "2026-09-18T24:00:00Z",
            "2026-09-18T12:00:00",
            "2026-09-18T12:00:00.Z",
        ] {
            assert!(!is_rfc3339(invalid), "accepted {invalid}");
        }
    }
}
