// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#[cfg(target_os = "macos")]
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
use std::error::Error;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::telemetry::{percentile_f64, unix_timestamp_ms, MetricBucket};
#[cfg(target_os = "macos")]
use block2::RcBlock;
use cua_media_protocol::{
    ActionBasis, ActionDeliveryGuarantee, ActionRequest, ClientMessage, ClipboardFile,
    FrameSequence, GeometryEpoch, Hello, InputGesturePhase, InputKeyState, InputModifier,
    InputPointerButton, InputPointerPhase, InteractiveInputBatch, InteractiveInputEvent,
    OpenSession, ServerMessage, SessionPolicy, StreamPreferences, SurfaceGeometry, VideoCodec,
    VideoFrameDescriptor, WindowGeometryControl, WindowGeometryRequest, WindowLifecycleEvent,
    WireHeader, MAX_CLIPBOARD_TEXT_BYTES, PROTOCOL_NAME,
};
#[cfg(target_os = "macos")]
use cua_media_protocol::{
    MAX_CLIPBOARD_FILES, MAX_CLIPBOARD_FILES_BYTES, MAX_CLIPBOARD_FILE_BYTES,
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSApp, NSAutoresizingMaskOptions, NSEvent, NSEventMask, NSEventModifierFlags, NSEventPhase,
    NSEventType, NSFilenamesPboardType, NSPasteboard, NSPasteboardTypeFileURL,
    NSPasteboardTypeString, NSPasteboardWriting, NSTextAlignment, NSTextField, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol as _, NSPoint, NSRect, NSSize, NSString,
    NSURL,
};
#[cfg(target_os = "macos")]
use objc2_v5::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_v5::runtime::ProtocolObject;
use pixels::wgpu::{BlendState, Color, TextureFormat};
use pixels::{Pixels, PixelsBuilder, ScalingMode, SurfaceTexture};
#[cfg(target_os = "macos")]
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
#[cfg(target_os = "macos")]
use winit::platform::macos::WindowBuilderExtMacOS as _;
#[cfg(target_os = "macos")]
use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use winit::window::{Window, WindowBuilder};

#[cfg(target_os = "macos")]
#[path = "macos/h264.rs"]
mod h264;

#[path = "client_transport.rs"]
mod client_transport;

#[cfg(target_os = "macos")]
#[path = "macos/metal.rs"]
mod metal;

use client_transport::{ClientTransport, TransportEvent};
#[cfg(target_os = "macos")]
use h264::H264Decoder;
#[cfg(target_os = "macos")]
use metal::MetalFrameRenderer;

#[cfg(target_os = "windows")]
mod windows_h264;

#[cfg(target_os = "windows")]
use windows_h264::H264Decoder;

type ClientResult<T> = Result<T, Box<dyn Error>>;

const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_INPUT_TO_PRESENT_SAMPLE: Duration = Duration::from_secs(1);

#[cfg(target_os = "macos")]
fn local_clipboard_snapshot() -> Result<(u64, Option<String>), String> {
    // SAFETY: NSPasteboard's general pasteboard and string accessors return
    // retained Objective-C objects and are valid from this process thread.
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    // SAFETY: `pasteboard` remains retained for both messages.
    let generation = unsafe { pasteboard.changeCount() };
    // SAFETY: `NSPasteboardTypeString` is the system UTF-8 text pasteboard type.
    let text =
        unsafe { pasteboard.stringForType(NSPasteboardTypeString) }.map(|value| value.to_string());
    Ok((u64::try_from(generation).unwrap_or_default(), text))
}

#[cfg(target_os = "macos")]
fn set_local_clipboard(text: &str) -> Result<u64, String> {
    // SAFETY: NSPasteboard's general pasteboard returns a retained object.
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    let text = NSString::from_str(text);
    // SAFETY: Both objects are retained for these synchronous messages, and
    // the pasteboard copies the supplied string.
    unsafe {
        pasteboard.clearContents();
        if !pasteboard.setString_forType(&text, NSPasteboardTypeString) {
            return Err("the native pasteboard rejected text".into());
        }
    }
    // SAFETY: `pasteboard` remains retained.
    let generation = unsafe { pasteboard.changeCount() };
    Ok(u64::try_from(generation).unwrap_or_default())
}

#[cfg(target_os = "macos")]
fn local_file_clipboard_paths() -> Result<(u64, Vec<PathBuf>), String> {
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    let generation = unsafe { pasteboard.changeCount() };
    if let Some(items) = unsafe { pasteboard.pasteboardItems() } {
        let paths = (0..items.count())
            .filter_map(|index| {
                let item = unsafe { items.objectAtIndex(index) };
                let value = unsafe { item.stringForType(NSPasteboardTypeFileURL) }?;
                let url = unsafe { NSURL::URLWithString(&value) }?;
                if !unsafe { url.isFileURL() } {
                    return None;
                }
                unsafe { url.path() }.map(|path| PathBuf::from(path.to_string()))
            })
            .collect::<Vec<_>>();
        if !paths.is_empty() {
            return Ok((u64::try_from(generation).unwrap_or_default(), paths));
        }
    }
    let file_type = unsafe { NSFilenamesPboardType };
    let Some(property_list) = pasteboard.propertyListForType(file_type) else {
        return Ok((u64::try_from(generation).unwrap_or_default(), Vec::new()));
    };
    // SAFETY: Cocoa property lists are NSObject instances.
    let object = unsafe { &*(property_list.as_ref() as *const _ as *const NSObject) };
    if !object.is_kind_of::<NSArray<NSString>>() {
        return Ok((u64::try_from(generation).unwrap_or_default(), Vec::new()));
    }
    // SAFETY: Objective-C generics are erased. The runtime class check above
    // establishes NSArray; Finder's filename contract establishes NSStrings.
    let paths = unsafe { &*(property_list.as_ref() as *const _ as *const NSArray<NSString>) };
    Ok((
        u64::try_from(generation).unwrap_or_default(),
        (0..paths.count())
            .map(|index| PathBuf::from(unsafe { paths.objectAtIndex(index) }.to_string()))
            .collect(),
    ))
}

#[cfg(target_os = "macos")]
fn set_local_file_clipboard(paths: &[PathBuf]) -> Result<u64, String> {
    if paths.is_empty() {
        return Err("file clipboard requires at least one file".into());
    }
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    let objects = paths
        .iter()
        .map(|path| {
            let path = NSString::from_str(&path.to_string_lossy());
            let url = unsafe { NSURL::fileURLWithPath(&path) };
            let object: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
                ProtocolObject::from_retained(url);
            object
        })
        .collect::<Vec<_>>();
    let objects = NSArray::from_vec(objects);
    unsafe {
        pasteboard.clearContents();
        if !pasteboard.writeObjects(&objects) {
            return Err("the native pasteboard rejected file URLs".into());
        }
    }
    let generation = unsafe { pasteboard.changeCount() };
    Ok(u64::try_from(generation).unwrap_or_default())
}

#[cfg(target_os = "macos")]
fn encode_local_clipboard_files(
    paths: &[PathBuf],
) -> Result<(Vec<ClipboardFile>, Vec<u8>), String> {
    if paths.len() > MAX_CLIPBOARD_FILES {
        return Err(format!(
            "clipboard contains more than {MAX_CLIPBOARD_FILES} files"
        ));
    }
    let mut total = 0_usize;
    let mut files = Vec::new();
    let mut payload = Vec::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| safe_clipboard_filename(name))
            .ok_or_else(|| "clipboard file has an unsafe or non-UTF-8 name".to_owned())?;
        let bytes =
            std::fs::read(path).map_err(|error| format!("clipboard file read failed: {error}"))?;
        if bytes.len() > MAX_CLIPBOARD_FILE_BYTES {
            return Err(format!(
                "clipboard file exceeds {MAX_CLIPBOARD_FILE_BYTES} bytes"
            ));
        }
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| "clipboard file size overflow".to_owned())?;
        if total > MAX_CLIPBOARD_FILES_BYTES {
            return Err(format!(
                "clipboard files exceed {MAX_CLIPBOARD_FILES_BYTES} bytes"
            ));
        }
        files.push(ClipboardFile {
            name: name.into(),
            offset: total.saturating_sub(bytes.len()) as u64,
            byte_len: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes.as_slice())),
        });
        payload.extend_from_slice(&bytes);
    }
    Ok((files, payload))
}

#[cfg(target_os = "macos")]
fn materialize_local_clipboard_files(
    files: &[ClipboardFile],
    payload: &[u8],
    generation: u64,
) -> Result<Vec<PathBuf>, String> {
    validate_local_clipboard_files(files, payload.len() as u64, payload)?;
    let cache_root = local_clipboard_cache_root();
    prune_local_clipboard_cache(&cache_root, 3)?;
    let root = cache_root.join(format!("remote-{generation}"));
    std::fs::create_dir_all(&root)
        .map_err(|error| format!("clipboard cache creation failed: {error}"))?;
    let mut paths = Vec::with_capacity(files.len());
    for file in files {
        let start = file.offset as usize;
        let end = start + file.byte_len as usize;
        let path = root.join(&file.name);
        std::fs::write(&path, &payload[start..end])
            .map_err(|error| format!("clipboard file write failed: {error}"))?;
        paths.push(path);
    }
    Ok(paths)
}

#[cfg(target_os = "macos")]
fn validate_local_clipboard_files(
    files: &[ClipboardFile],
    byte_len: u64,
    payload: &[u8],
) -> Result<(), String> {
    if files.is_empty() || files.len() > MAX_CLIPBOARD_FILES {
        return Err(format!(
            "clipboard must contain 1..={MAX_CLIPBOARD_FILES} files"
        ));
    }
    let mut total = 0_usize;
    let mut names = HashSet::new();
    let mut expected_offset = 0_u64;
    for file in files {
        if !safe_clipboard_filename(&file.name) || !names.insert(file.name.as_str()) {
            return Err("clipboard file names are unsafe or duplicated".into());
        }
        if file.offset != expected_offset || file.byte_len as usize > MAX_CLIPBOARD_FILE_BYTES {
            return Err("clipboard file length is invalid".into());
        }
        let end = file
            .offset
            .checked_add(file.byte_len)
            .ok_or_else(|| "clipboard file range overflow".to_owned())?;
        let bytes = payload
            .get(file.offset as usize..end as usize)
            .ok_or_else(|| "clipboard file range exceeds payload".to_owned())?;
        if format!("{:x}", Sha256::digest(bytes)) != file.sha256 {
            return Err("clipboard file checksum does not match".into());
        }
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| "clipboard file size overflow".to_owned())?;
        if total > MAX_CLIPBOARD_FILES_BYTES {
            return Err(format!(
                "clipboard files exceed {MAX_CLIPBOARD_FILES_BYTES} bytes"
            ));
        }
        expected_offset = end;
    }
    if byte_len != payload.len() as u64 || expected_offset != byte_len {
        return Err("clipboard payload length does not match its manifest".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn safe_clipboard_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 255
        && Path::new(name).file_name().is_some_and(|leaf| leaf == name)
}

#[cfg(target_os = "macos")]
fn local_clipboard_cache_root() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .map(|path| path.join("Library/Caches/cua-viewer/clipboard"))
        .unwrap_or_else(|| std::env::temp_dir().join("rcdp-clipboard"))
}

#[cfg(target_os = "macos")]
fn prune_local_clipboard_cache(root: &Path, keep: usize) -> Result<(), String> {
    std::fs::create_dir_all(root)
        .map_err(|error| format!("clipboard cache creation failed: {error}"))?;
    let mut directories = std::fs::read_dir(root)
        .map_err(|error| format!("clipboard cache read failed: {error}"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .collect::<Vec<_>>();
    directories.sort_by_key(|entry| {
        std::cmp::Reverse(
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH),
        )
    });
    for entry in directories.into_iter().skip(keep) {
        std::fs::remove_dir_all(entry.path())
            .map_err(|error| format!("clipboard cache pruning failed: {error}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn local_clipboard_snapshot() -> Result<(u64, Option<String>), String> {
    Err("text clipboard synchronization is not available on this client".into())
}

#[cfg(not(target_os = "macos"))]
fn set_local_clipboard(_: &str) -> Result<u64, String> {
    Err("text clipboard synchronization is not available on this client".into())
}

#[cfg(not(target_os = "macos"))]
fn local_file_clipboard_paths() -> Result<(u64, Vec<PathBuf>), String> {
    Err("file clipboard synchronization is not available on this client".into())
}

#[cfg(not(target_os = "macos"))]
fn set_local_file_clipboard(_: &[PathBuf]) -> Result<u64, String> {
    Err("file clipboard synchronization is not available on this client".into())
}

#[cfg(not(target_os = "macos"))]
fn encode_local_clipboard_files(_: &[PathBuf]) -> Result<(Vec<ClipboardFile>, Vec<u8>), String> {
    Err("file clipboard synchronization is not available on this client".into())
}

#[cfg(not(target_os = "macos"))]
fn materialize_local_clipboard_files(
    _: &[ClipboardFile],
    _: &[u8],
    _: u64,
) -> Result<Vec<PathBuf>, String> {
    Err("file clipboard synchronization is not available on this client".into())
}

#[derive(Debug, Clone)]
struct Arguments {
    endpoints: Vec<ConnectionEndpoint>,
    target: Option<String>,
    max_fps: u16,
    max_dimension: u32,
    max_bitrate_kbps: u32,
    protocol_name: String,
    sync_window_size: bool,
    show_stats: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectionEndpoint {
    url: String,
    token: Option<String>,
    quic_cert_sha256: Option<String>,
}

#[derive(Debug, Default)]
struct BundledRemoteAppConfig {
    url: Option<String>,
    token: Option<String>,
    max_fps: Option<u16>,
    max_dimension: Option<u32>,
    max_bitrate_kbps: Option<u32>,
    protocol_name: Option<String>,
    quic_cert_sha256: Option<String>,
    fallback_endpoints: Vec<ConnectionEndpoint>,
    sync_window_size: bool,
    show_stats: bool,
}

fn connection_endpoint_from_json(value: &serde_json::Value) -> ClientResult<ConnectionEndpoint> {
    let url = value
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or("fallback endpoint requires a string url")?
        .to_owned();
    validate_connection_url(&url)?;
    Ok(ConnectionEndpoint {
        url,
        token: value
            .get("token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        quic_cert_sha256: value
            .get("quic_cert_sha256")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    })
}

fn validate_connection_url(url: &str) -> ClientResult<()> {
    if url.starts_with("ws://")
        || url.starts_with("wss://")
        || url.starts_with("quic://")
        || url.starts_with("http://")
        || url.starts_with("https://")
    {
        Ok(())
    } else {
        Err(
            "connection URL must use http(s):// (cua-spacesd, wire v2), ws://, wss://, or quic://"
                .into(),
        )
    }
}

fn bundled_remote_app_config() -> ClientResult<BundledRemoteAppConfig> {
    let executable = std::env::current_exe()?;
    let Some(contents) = executable
        .parent()
        .and_then(std::path::Path::parent)
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
    else {
        return Ok(BundledRemoteAppConfig::default());
    };
    let path: PathBuf = contents.join("Resources/remote-app.json");
    if !path.is_file() {
        return Ok(BundledRemoteAppConfig::default());
    }
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let fallback_endpoints = value
        .get("fallback_endpoints")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(connection_endpoint_from_json)
                .collect::<ClientResult<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(BundledRemoteAppConfig {
        url: value
            .get("url")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        token: value
            .get("token")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        max_fps: value
            .get("max_fps")
            .and_then(|value| value.as_u64())
            .and_then(|value| u16::try_from(value).ok()),
        max_dimension: value
            .get("max_dimension")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok()),
        max_bitrate_kbps: value
            .get("max_bitrate_kbps")
            .and_then(|value| value.as_u64())
            .and_then(|value| u32::try_from(value).ok()),
        protocol_name: value
            .get("protocol_name")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        quic_cert_sha256: value
            .get("quic_cert_sha256")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        fallback_endpoints,
        sync_window_size: value
            .get("sync_window_size")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        show_stats: value
            .get("show_stats")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
    })
}

impl Arguments {
    fn parse() -> ClientResult<Self> {
        let values = std::env::args().skip(1).collect::<Vec<_>>();
        if values
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
        {
            println!(
                "Usage: cua-viewer --url URL [--target OPAQUE_HANDLE|display:ID] [--token TOKEN] \
                 (URL: http(s)://HOST:3211[/?quic=1&audio=0] for cua-spacesd wire v2; \
                 ws(s):// or quic:// for a v1 share) \
                 [--max-fps FPS] [--max-dimension PIXELS] [--max-bitrate-kbps KBPS] \
                 [--protocol-name cua-media] \
                 [--quic-cert-sha256 HEX] \
                 [--sync-window-size] [--show-stats]"
            );
            std::process::exit(0);
        }
        let option = |name: &str| {
            values
                .iter()
                .position(|value| value == name)
                .and_then(|index| values.get(index + 1))
                .cloned()
        };
        let bundled = bundled_remote_app_config()?;
        let url = option("--url")
            .or(bundled.url)
            .ok_or("--url is required (or install a bundle with Resources/remote-app.json)")?;
        validate_connection_url(&url)?;
        let mut endpoints = vec![ConnectionEndpoint {
            url,
            token: option("--token").or(bundled.token),
            quic_cert_sha256: option("--quic-cert-sha256").or(bundled.quic_cert_sha256),
        }];
        endpoints.extend(bundled.fallback_endpoints);
        let max_fps = option("--max-fps")
            .map(|value| value.parse())
            .transpose()?
            .or(bundled.max_fps)
            .unwrap_or(60);
        if !(1..=240).contains(&max_fps) {
            return Err("--max-fps must be between 1 and 240".into());
        }
        let max_dimension = option("--max-dimension")
            .map(|value| value.parse())
            .transpose()?
            .or(bundled.max_dimension)
            .unwrap_or(1920);
        if max_dimension == 0 {
            return Err("--max-dimension must be non-zero".into());
        }
        let max_bitrate_kbps = option("--max-bitrate-kbps")
            .map(|value| value.parse())
            .transpose()?
            .or(bundled.max_bitrate_kbps)
            .unwrap_or(8_000);
        if !(250..=100_000).contains(&max_bitrate_kbps) {
            return Err("--max-bitrate-kbps must be between 250 and 100000".into());
        }
        let protocol_name = option("--protocol-name")
            .or(bundled.protocol_name)
            .unwrap_or_else(|| PROTOCOL_NAME.into());
        if protocol_name != PROTOCOL_NAME {
            return Err(format!("--protocol-name must be {PROTOCOL_NAME}").into());
        }
        Ok(Self {
            endpoints,
            target: option("--target"),
            max_fps,
            max_dimension,
            max_bitrate_kbps,
            protocol_name,
            sync_window_size: values
                .iter()
                .any(|argument| argument == "--sync-window-size")
                || bundled.sync_window_size,
            show_stats: values.iter().any(|argument| argument == "--show-stats")
                || bundled.show_stats,
        })
    }
}

#[derive(Debug)]
enum UiWake {
    Network,
}

#[derive(Debug)]
enum UiEvent {
    Status(String),
    Opened {
        title: String,
        geometry: SurfaceGeometry,
        geometry_control: bool,
    },
    Geometry(SurfaceGeometry),
    Title(String),
    Suspended(String),
    Resumed,
    Closed,
    Telemetry(String),
    Fatal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionPhase {
    Connecting,
    WaitingForVideo,
    Live,
    Reconnecting,
    Closed,
    Failed,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn connection_overlay_text(phase: ConnectionPhase, detail: &str) -> Option<String> {
    match phase {
        ConnectionPhase::Live => None,
        ConnectionPhase::Connecting => Some("Connecting to remote Mac…".into()),
        ConnectionPhase::WaitingForVideo => Some("Waiting for remote video…".into()),
        ConnectionPhase::Reconnecting => Some("Connection interrupted · reconnecting…".into()),
        ConnectionPhase::Closed => Some("Remote window closed".into()),
        ConnectionPhase::Failed => Some(if detail.trim().is_empty() {
            "Remote Mac is unreachable".into()
        } else {
            detail.into()
        }),
    }
}

#[cfg(target_os = "macos")]
struct ConnectionOverlay {
    scrim: Retained<NSVisualEffectView>,
    label: Retained<NSTextField>,
}

#[cfg(target_os = "macos")]
impl ConnectionOverlay {
    fn new(window: &Window) -> Option<Self> {
        let main_thread = MainThreadMarker::new()?;
        let handle = window.window_handle().ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        // SAFETY: Winit owns this NSView for the lifetime of the window. This
        // constructor runs synchronously on AppKit's main event-loop thread.
        let content_view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        let bounds = content_view.bounds();
        let scrim = unsafe {
            NSVisualEffectView::initWithFrame(
                MainThreadMarker::alloc::<NSVisualEffectView>(main_thread),
                bounds,
            )
        };
        unsafe {
            scrim.setMaterial(NSVisualEffectMaterial::HUDWindow);
            scrim.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
            scrim.setState(NSVisualEffectState::Active);
            scrim.setAlphaValue(0.86);
            scrim.setAutoresizingMask(
                NSAutoresizingMaskOptions::NSViewWidthSizable
                    | NSAutoresizingMaskOptions::NSViewHeightSizable,
            );
        }

        let label_height = 40.0;
        let label_frame = NSRect::new(
            NSPoint::new(0.0, ((bounds.size.height - label_height) / 2.0).max(0.0)),
            NSSize::new(bounds.size.width, label_height),
        );
        let label = unsafe {
            NSTextField::labelWithString(
                &NSString::from_str("Connecting to remote Mac…"),
                main_thread,
            )
        };
        unsafe {
            label.setFrame(label_frame);
            label.setAlignment(NSTextAlignment::Center);
            label.setAutoresizingMask(
                NSAutoresizingMaskOptions::NSViewWidthSizable
                    | NSAutoresizingMaskOptions::NSViewMinYMargin
                    | NSAutoresizingMaskOptions::NSViewMaxYMargin,
            );
            scrim.addSubview(&label);
            content_view.addSubview(&scrim);
        }
        Some(Self { scrim, label })
    }

    fn update(&self, phase: ConnectionPhase, detail: &str) {
        let Some(text) = connection_overlay_text(phase, detail) else {
            self.scrim.setHidden(true);
            return;
        };
        // SAFETY: UI event draining occurs on AppKit's main event-loop thread;
        // the retained field and view stay alive for the full window lifetime.
        unsafe { self.label.setStringValue(&NSString::from_str(&text)) };
        self.scrim.setHidden(false);
    }
}

#[derive(Debug)]
struct NativeFrame {
    width_px: u32,
    height_px: u32,
    data: NativeFrameData,
    geometry_epoch: GeometryEpoch,
    sequence: FrameSequence,
    received_at: Instant,
}

#[derive(Debug)]
enum NativeFrameData {
    CpuBgra(Vec<u8>),
    #[cfg(target_os = "macos")]
    PixelBuffer(h264::DecodedPixelBuffer),
}

#[derive(Debug)]
struct LatencyTelemetry {
    window_started: Instant,
    frames_received: u64,
    bytes_received: u64,
    host_encode_us: u64,
    host_encode_samples: u64,
    decode_us: u64,
    frames_replaced: u64,
    frames_presented: u64,
    receive_to_present_us: u64,
    action_results: u64,
    action_us: u64,
    last_action_average_us: Option<u64>,
    control_rtt_us: Option<u64>,
    server_frames_replaced: u64,
    last_server_frames_replaced: Option<u64>,
    network_incomplete_frames: u64,
    encode_samples_ms: Vec<f64>,
    decode_samples_ms: Vec<f64>,
    present_samples_ms: Vec<f64>,
    input_ack_samples_ms: Vec<f64>,
    host_dispatch_samples_ms: Vec<f64>,
    input_to_present_samples_ms: Vec<f64>,
    scroll_gap_samples_ms: Vec<f64>,
    input_events_captured: u64,
    input_events_sent: u64,
    input_batches_sent: u64,
    max_input_in_flight: u32,
    max_input_queue_depth: u32,
    scroll_events_captured: u64,
    scroll_events_sent: u64,
    last_scroll_captured: Option<Instant>,
    pending_input_started: Option<Instant>,
    /// Session totals for `cua_stream_stats` (never reset).
    totals: crate::telemetry::StreamTotals,
}

impl LatencyTelemetry {
    fn new(now: Instant) -> Self {
        Self {
            window_started: now,
            frames_received: 0,
            bytes_received: 0,
            host_encode_us: 0,
            host_encode_samples: 0,
            decode_us: 0,
            frames_replaced: 0,
            frames_presented: 0,
            receive_to_present_us: 0,
            action_results: 0,
            action_us: 0,
            last_action_average_us: None,
            control_rtt_us: None,
            server_frames_replaced: 0,
            last_server_frames_replaced: None,
            network_incomplete_frames: 0,
            encode_samples_ms: Vec::new(),
            decode_samples_ms: Vec::new(),
            present_samples_ms: Vec::new(),
            input_ack_samples_ms: Vec::new(),
            host_dispatch_samples_ms: Vec::new(),
            input_to_present_samples_ms: Vec::new(),
            scroll_gap_samples_ms: Vec::new(),
            input_events_captured: 0,
            input_events_sent: 0,
            input_batches_sent: 0,
            max_input_in_flight: 0,
            max_input_queue_depth: 0,
            scroll_events_captured: 0,
            scroll_events_sent: 0,
            last_scroll_captured: None,
            pending_input_started: None,
            totals: crate::telemetry::StreamTotals::new(now),
        }
    }

    fn record_frame(
        &mut self,
        bytes: usize,
        host_encode_us: Option<u32>,
        decode: Duration,
        replaced: bool,
    ) {
        self.frames_received = self.frames_received.saturating_add(1);
        self.totals.frames = self.totals.frames.saturating_add(1);
        self.bytes_received = self
            .bytes_received
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        if let Some(host_encode_us) = host_encode_us {
            self.host_encode_us = self
                .host_encode_us
                .saturating_add(u64::from(host_encode_us));
            self.host_encode_samples = self.host_encode_samples.saturating_add(1);
            push_sample(
                &mut self.encode_samples_ms,
                f64::from(host_encode_us) / 1_000.0,
            );
        }
        self.decode_us = self
            .decode_us
            .saturating_add(u64::try_from(decode.as_micros()).unwrap_or(u64::MAX));
        push_sample(&mut self.decode_samples_ms, decode.as_secs_f64() * 1_000.0);
        self.frames_replaced = self.frames_replaced.saturating_add(u64::from(replaced));
    }

    fn record_presentation(&mut self, receive_to_present: Duration) {
        self.frames_presented = self.frames_presented.saturating_add(1);
        self.receive_to_present_us = self
            .receive_to_present_us
            .saturating_add(u64::try_from(receive_to_present.as_micros()).unwrap_or(u64::MAX));
        push_sample(
            &mut self.present_samples_ms,
            receive_to_present.as_secs_f64() * 1_000.0,
        );
        if let Some(started) = self.pending_input_started.take() {
            let elapsed = started.elapsed();
            if elapsed <= MAX_INPUT_TO_PRESENT_SAMPLE {
                push_sample(
                    &mut self.input_to_present_samples_ms,
                    elapsed.as_secs_f64() * 1_000.0,
                );
            }
        }
    }

    fn record_action(&mut self, elapsed: Duration) {
        self.action_results = self.action_results.saturating_add(1);
        self.action_us = self
            .action_us
            .saturating_add(u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
        push_sample(
            &mut self.input_ack_samples_ms,
            elapsed.as_secs_f64() * 1_000.0,
        );
    }

    fn record_host_dispatch(&mut self, dispatch_us: Option<u64>) {
        if let Some(dispatch_us) = dispatch_us {
            push_sample(
                &mut self.host_dispatch_samples_ms,
                dispatch_us as f64 / 1_000.0,
            );
        }
    }

    fn record_input_captured(&mut self, events: u64, scroll: bool, now: Instant) {
        self.input_events_captured = self.input_events_captured.saturating_add(events);
        if scroll {
            self.scroll_events_captured = self.scroll_events_captured.saturating_add(events);
            if let Some(previous) = self.last_scroll_captured.replace(now) {
                push_sample(
                    &mut self.scroll_gap_samples_ms,
                    now.saturating_duration_since(previous).as_secs_f64() * 1_000.0,
                );
            }
        }
    }

    fn record_input_sent(
        &mut self,
        events: u64,
        scroll_events: u64,
        in_flight: usize,
        queue_depth: usize,
        expects_visual_change: bool,
    ) {
        self.input_events_sent = self.input_events_sent.saturating_add(events);
        self.scroll_events_sent = self.scroll_events_sent.saturating_add(scroll_events);
        self.input_batches_sent = self.input_batches_sent.saturating_add(1);
        self.max_input_in_flight = self
            .max_input_in_flight
            .max(u32::try_from(in_flight).unwrap_or(u32::MAX));
        self.max_input_queue_depth = self
            .max_input_queue_depth
            .max(u32::try_from(queue_depth).unwrap_or(u32::MAX));
        if expects_visual_change {
            // The protocol does not carry a frame-to-input causal id. Track
            // the newest actionable input so a burst of typing or scrolling
            // is not reported as the latency of its oldest queued event.
            self.pending_input_started = Some(Instant::now());
        }
    }

    fn record_control_rtt(&mut self, elapsed: Duration) {
        self.control_rtt_us = Some(
            u64::try_from(elapsed.as_micros())
                .unwrap_or(u64::MAX)
                .min(60_000_000),
        );
    }

    fn record_server_replacements(&mut self, total: u64) {
        if let Some(previous) = self.last_server_frames_replaced {
            self.server_frames_replaced = self
                .server_frames_replaced
                .saturating_add(total.saturating_sub(previous));
        }
        self.last_server_frames_replaced = Some(total);
    }

    fn record_network_loss(&mut self, incomplete_frames: u64) {
        self.network_incomplete_frames = self
            .network_incomplete_frames
            .saturating_add(incomplete_frames);
    }

    fn summary_and_reset(&mut self, now: Instant) -> TelemetrySummary {
        let seconds = now
            .saturating_duration_since(self.window_started)
            .as_secs_f64()
            .max(0.001);
        let fps = self.frames_received as f64 / seconds;
        let mbps = self.bytes_received as f64 * 8.0 / seconds / 1_000_000.0;
        let decode_ms = average_ms(self.decode_us, self.frames_received);
        let encode_ms = average_ms(self.host_encode_us, self.host_encode_samples);
        let present_ms = average_ms(self.receive_to_present_us, self.frames_presented);
        if let Some(average) = self.action_us.checked_div(self.action_results) {
            self.last_action_average_us = Some(average);
        }
        let rtt_ms = self
            .control_rtt_us
            .map(|value| format!("{:.0}", value as f64 / 1_000.0))
            .unwrap_or_else(|| "—".into());
        let action_ms = self
            .last_action_average_us
            .map(|value| format!("{:.0}", value as f64 / 1_000.0))
            .unwrap_or_else(|| "—".into());
        let title = format!(
            "{fps:.0} fps · {mbps:.1} Mbps · RTT {rtt_ms} ms · encode {:.1} ms · decode {:.1} ms · present {:.1} ms · input {action_ms} ms · drops {}/{} · net {}",
            encode_ms.unwrap_or(0.0),
            decode_ms.unwrap_or(0.0),
            present_ms.unwrap_or(0.0),
            self.frames_replaced,
            self.server_frames_replaced,
            self.network_incomplete_frames,
        );
        let mut metric = MetricBucket {
            timestamp_ms: unix_timestamp_ms(),
            duration_ms: now
                .saturating_duration_since(self.window_started)
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            fps,
            bitrate_mbps: mbps,
            control_rtt_ms: self.control_rtt_us.map(|value| value as f64 / 1_000.0),
            frames_received: self.frames_received,
            frames_presented: self.frames_presented,
            client_frames_replaced: self.frames_replaced,
            server_frames_replaced: self.server_frames_replaced,
            network_incomplete_frames: self.network_incomplete_frames,
            input_events_captured: self.input_events_captured,
            input_events_sent: self.input_events_sent,
            input_batches_sent: self.input_batches_sent,
            max_input_in_flight: self.max_input_in_flight,
            max_input_queue_depth: self.max_input_queue_depth,
            scroll_events_captured: self.scroll_events_captured,
            scroll_events_sent: self.scroll_events_sent,
            ..MetricBucket::default()
        };
        set_percentiles(
            &self.encode_samples_ms,
            &mut metric.encode_p50_ms,
            &mut metric.encode_p95_ms,
            &mut metric.encode_p99_ms,
        );
        set_percentiles(
            &self.decode_samples_ms,
            &mut metric.decode_p50_ms,
            &mut metric.decode_p95_ms,
            &mut metric.decode_p99_ms,
        );
        set_percentiles(
            &self.present_samples_ms,
            &mut metric.present_p50_ms,
            &mut metric.present_p95_ms,
            &mut metric.present_p99_ms,
        );
        set_percentiles(
            &self.input_ack_samples_ms,
            &mut metric.input_ack_p50_ms,
            &mut metric.input_ack_p95_ms,
            &mut metric.input_ack_p99_ms,
        );
        set_percentiles(
            &self.host_dispatch_samples_ms,
            &mut metric.host_dispatch_p50_ms,
            &mut metric.host_dispatch_p95_ms,
            &mut metric.host_dispatch_p99_ms,
        );
        set_percentiles(
            &self.input_to_present_samples_ms,
            &mut metric.input_to_present_p50_ms,
            &mut metric.input_to_present_p95_ms,
            &mut metric.input_to_present_p99_ms,
        );
        metric.scroll_gap_p95_ms = percentile_f64(&self.scroll_gap_samples_ms, 0.95);
        self.totals.observe_p95(metric.input_to_present_p95_ms);
        self.window_started = now;
        self.frames_received = 0;
        self.bytes_received = 0;
        self.host_encode_us = 0;
        self.host_encode_samples = 0;
        self.decode_us = 0;
        self.frames_replaced = 0;
        self.frames_presented = 0;
        self.receive_to_present_us = 0;
        self.action_results = 0;
        self.action_us = 0;
        self.server_frames_replaced = 0;
        self.network_incomplete_frames = 0;
        self.encode_samples_ms.clear();
        self.decode_samples_ms.clear();
        self.present_samples_ms.clear();
        self.input_ack_samples_ms.clear();
        self.host_dispatch_samples_ms.clear();
        self.input_to_present_samples_ms.clear();
        self.scroll_gap_samples_ms.clear();
        self.input_events_captured = 0;
        self.input_events_sent = 0;
        self.input_batches_sent = 0;
        self.max_input_in_flight = 0;
        self.max_input_queue_depth = 0;
        self.scroll_events_captured = 0;
        self.scroll_events_sent = 0;
        TelemetrySummary { title, metric }
    }
}

#[derive(Debug)]
struct TelemetrySummary {
    title: String,
    /// Structured form of the window the title summarizes. Only tests read it
    /// now that the local dogfood store is gone.
    #[cfg_attr(not(test), allow(dead_code))]
    metric: MetricBucket,
}

fn push_sample(samples: &mut Vec<f64>, value: f64) {
    const MAX_SAMPLES_PER_BUCKET: usize = 4_096;
    if value.is_finite() && samples.len() < MAX_SAMPLES_PER_BUCKET {
        samples.push(value);
    }
}

fn set_percentiles(
    samples: &[f64],
    p50: &mut Option<f64>,
    p95: &mut Option<f64>,
    p99: &mut Option<f64>,
) {
    *p50 = percentile_f64(samples, 0.50);
    *p95 = percentile_f64(samples, 0.95);
    *p99 = percentile_f64(samples, 0.99);
}

fn telemetry_record_capture(telemetry: &Mutex<LatencyTelemetry>, events: u64, scroll: bool) {
    telemetry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .record_input_captured(events, scroll, Instant::now());
}

fn average_ms(total_us: u64, samples: u64) -> Option<f64> {
    (samples > 0).then(|| total_us as f64 / samples as f64 / 1_000.0)
}

#[cfg(target_os = "macos")]
fn native_window_inner_size(window: &Window) -> PhysicalSize<u32> {
    let Ok(handle) = window.window_handle() else {
        return window.inner_size();
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return window.inner_size();
    };
    // SAFETY: Winit owns this NSView for the lifetime of `window`, and the
    // native event loop invokes this helper on AppKit's main thread.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let Some(ns_window) = view.window() else {
        return window.inner_size();
    };
    let content = ns_window.contentRectForFrameRect(ns_window.frame());
    // SAFETY: The window and its backing conversion remain valid during this
    // synchronous main-thread query.
    let backing = unsafe { ns_window.convertRectToBacking(content) };
    PhysicalSize::new(
        backing.size.width.round().clamp(1.0, f64::from(u32::MAX)) as u32,
        backing.size.height.round().clamp(1.0, f64::from(u32::MAX)) as u32,
    )
}

#[cfg(not(target_os = "macos"))]
fn native_window_inner_size(window: &Window) -> PhysicalSize<u32> {
    window.inner_size()
}

#[derive(Debug)]
enum InputCommand {
    Action {
        tool: &'static str,
        arguments: serde_json::Value,
        basis: ActionBasis,
    },
    Text(String),
    Interactive(InteractiveInputEvent),
    Resize {
        revision: u64,
        width_points: u32,
        height_points: u32,
    },
    Close,
}

#[derive(Debug)]
struct ScrollCommand {
    x: u32,
    y: u32,
    direction: &'static str,
    amount: u32,
    basis: ActionBasis,
}

#[derive(Clone)]
struct InteractiveScrollSender {
    events: Arc<Mutex<VecDeque<InteractiveInputEvent>>>,
    wake: watch::Sender<()>,
}

struct InteractiveScrollReceiver {
    events: Arc<Mutex<VecDeque<InteractiveInputEvent>>>,
    wake: watch::Receiver<()>,
}

fn interactive_scroll_channel() -> (InteractiveScrollSender, InteractiveScrollReceiver) {
    let events = Arc::new(Mutex::new(VecDeque::new()));
    let (wake, wake_rx) = watch::channel(());
    (
        InteractiveScrollSender {
            events: events.clone(),
            wake,
        },
        InteractiveScrollReceiver {
            events,
            wake: wake_rx,
        },
    )
}

impl InteractiveScrollSender {
    fn send(&self, event: InteractiveInputEvent) {
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(previous) = events.back_mut() {
            if coalesce_latest_interactive_sample(std::slice::from_mut(previous), event.clone()) {
                drop(events);
                self.wake.send_replace(());
                return;
            }
        }
        events.push_back(event);
        drop(events);
        self.wake.send_replace(());
    }
}

impl InteractiveScrollReceiver {
    fn try_recv(&mut self) -> Option<InteractiveInputEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    fn len(&self) -> usize {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[derive(Debug, Clone, Copy)]
struct Viewport {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

impl Viewport {
    fn fit(window: PhysicalSize<u32>, frame_width: u32, frame_height: u32) -> Option<Self> {
        if window.width == 0 || window.height == 0 || frame_width == 0 || frame_height == 0 {
            return None;
        }
        let scale = (window.width as f64 / frame_width as f64)
            .min(window.height as f64 / frame_height as f64);
        let width = (frame_width as f64 * scale).floor().max(1.0) as u32;
        let height = (frame_height as f64 * scale).floor().max(1.0) as u32;
        Some(Self {
            left: (window.width - width) / 2,
            top: (window.height - height) / 2,
            width,
            height,
        })
    }

    fn frame_point(
        self,
        position: PhysicalPosition<f64>,
        frame_width: u32,
        frame_height: u32,
    ) -> Option<(u32, u32)> {
        let x = position.x - f64::from(self.left);
        let y = position.y - f64::from(self.top);
        if x < 0.0 || y < 0.0 || x >= f64::from(self.width) || y >= f64::from(self.height) {
            return None;
        }
        Some((
            ((x * f64::from(frame_width) / f64::from(self.width)).floor() as u32)
                .min(frame_width - 1),
            ((y * f64::from(frame_height) / f64::from(self.height)).floor() as u32)
                .min(frame_height - 1),
        ))
    }
}

const ADAPTATION_INTERVAL: Duration = Duration::from_millis(500);
const RECOVERY_WINDOWS: u8 = 6;
const MIN_ADAPTIVE_FPS: u16 = 15;
const MIN_ADAPTIVE_DIMENSION: u32 = 640;
const MIN_ADAPTIVE_BITRATE_KBPS: u32 = 500;
const NATIVE_SESSION_POLICY: SessionPolicy = SessionPolicy::AllowActivation;
// Automation actions can take more than a second on compatibility hosts. Keep
// only one in flight so live input cannot turn into seconds of stale replay.
const MAX_IN_FLIGHT_ACTIONS: usize = 1;
// Critical input edges may use the full bounded window, but pointer motion and
// scroll samples are replaceable state. Keeping only two continuous batches in
// flight prevents movement from trapping clicks or text behind seconds of
// stale replay when a host dispatch path or mobile network stalls.
const MAX_IN_FLIGHT_INPUT_BATCHES: usize = 64;
const MAX_IN_FLIGHT_CONTINUOUS_INPUT_BATCHES: usize = 2;
const MAX_CLIENT_INPUT_BATCH_EVENTS: usize = 32;
#[cfg(target_os = "macos")]
const MAX_BUFFERED_NATIVE_SCROLL_EVENTS: usize = 32;
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(120);
const REMOTE_RESIZE_SUPPRESSION: Duration = Duration::from_millis(750);
const WINDOW_SIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONNECTION_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(4);
const SESSION_HEALTH_INTERVAL: Duration = Duration::from_secs(1);
const SESSION_VIDEO_IDLE_PROBE_AFTER: Duration = Duration::from_secs(5);
const SESSION_VIDEO_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const RECONNECT_INITIAL_DELAY: Duration = Duration::from_millis(250);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug, Default)]
struct ReconnectBackoff {
    failures: u32,
}

impl ReconnectBackoff {
    fn next_delay(&mut self) -> Duration {
        let exponent = self.failures.min(5);
        self.failures = self.failures.saturating_add(1);
        RECONNECT_INITIAL_DELAY
            .saturating_mul(1_u32 << exponent)
            .min(RECONNECT_MAX_DELAY)
    }

    fn reset(&mut self) {
        self.failures = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VideoHealthAction {
    None,
    RequestKeyframe,
    Stall,
}

fn video_health_action(
    last_video: Instant,
    probe_started: Option<Instant>,
    suspended: bool,
    now: Instant,
) -> VideoHealthAction {
    if suspended {
        return VideoHealthAction::None;
    }
    if let Some(probe_started) = probe_started {
        return if now.saturating_duration_since(probe_started) >= SESSION_VIDEO_PROBE_TIMEOUT {
            VideoHealthAction::Stall
        } else {
            VideoHealthAction::None
        };
    }
    if now.saturating_duration_since(last_video) >= SESSION_VIDEO_IDLE_PROBE_AFTER {
        VideoHealthAction::RequestKeyframe
    } else {
        VideoHealthAction::None
    }
}

fn accepted_video_codecs() -> Vec<VideoCodec> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        vec![VideoCodec::H264, VideoCodec::Bgra]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        vec![VideoCodec::Bgra]
    }
}

#[derive(Debug, Clone, Copy)]
struct PendingResize {
    width_points: u32,
    height_points: u32,
    deadline: Instant,
}

#[derive(Debug, Clone, Copy)]
struct RemoteResizeSuppression {
    expected: PhysicalSize<u32>,
    deadline: Instant,
}

struct AdaptivePreferences {
    ceiling_fps: u16,
    ceiling_dimension: u32,
    ceiling_bitrate_kbps: u32,
    current_fps: u16,
    current_dimension: u32,
    current_bitrate_kbps: u32,
    window_started: Instant,
    frames: u32,
    replaced: u32,
    network_losses: u32,
    decode_us: u64,
    min_rtt: Option<Duration>,
    latest_rtt: Option<Duration>,
    stable_windows: u8,
    congested_windows: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StreamTuning {
    max_fps: u16,
    max_dimension: u32,
    target_bitrate_kbps: u32,
}

impl AdaptivePreferences {
    fn new(max_fps: u16, max_dimension: u32, max_bitrate_kbps: u32, now: Instant) -> Self {
        Self {
            ceiling_fps: max_fps,
            ceiling_dimension: max_dimension,
            ceiling_bitrate_kbps: max_bitrate_kbps,
            current_fps: max_fps,
            current_dimension: max_dimension,
            current_bitrate_kbps: max_bitrate_kbps,
            window_started: now,
            frames: 0,
            replaced: 0,
            network_losses: 0,
            decode_us: 0,
            min_rtt: None,
            latest_rtt: None,
            stable_windows: 0,
            congested_windows: 0,
        }
    }

    fn observe_frame(
        &mut self,
        replaced: bool,
        decode: Duration,
        rtt: Option<Duration>,
        now: Instant,
    ) -> Option<StreamTuning> {
        self.frames = self.frames.saturating_add(1);
        self.replaced = self.replaced.saturating_add(u32::from(replaced));
        self.decode_us = self
            .decode_us
            .saturating_add(u64::try_from(decode.as_micros()).unwrap_or(u64::MAX));
        self.observe_rtt(rtt);
        self.evaluate(now)
    }

    fn observe_network_loss(
        &mut self,
        incomplete_frames: u64,
        rtt: Option<Duration>,
        now: Instant,
    ) -> Option<StreamTuning> {
        self.network_losses = self
            .network_losses
            .saturating_add(u32::try_from(incomplete_frames).unwrap_or(u32::MAX));
        self.observe_rtt(rtt);
        self.evaluate(now)
    }

    fn observe_rtt(&mut self, rtt: Option<Duration>) {
        let Some(rtt) = rtt else {
            return;
        };
        self.latest_rtt = Some(rtt);
        self.min_rtt = Some(self.min_rtt.map_or(rtt, |minimum| minimum.min(rtt)));
    }

    fn evaluate(&mut self, now: Instant) -> Option<StreamTuning> {
        if now.saturating_duration_since(self.window_started) < ADAPTATION_INTERVAL {
            return None;
        }

        let enough_samples = self.frames >= 3;
        let replacement_pressure =
            enough_samples && self.replaced > 0 && self.replaced.saturating_mul(10) >= self.frames;
        let average_decode_us = self.decode_us.checked_div(u64::from(self.frames.max(1)));
        let frame_budget_us = 1_000_000_u64 / u64::from(self.current_fps.max(1));
        let decode_pressure = enough_samples
            && average_decode_us
                .is_some_and(|decode| decode.saturating_mul(4) >= frame_budget_us * 3);
        let rtt_pressure = self
            .latest_rtt
            .zip(self.min_rtt)
            .is_some_and(|(latest, minimum)| {
                latest > minimum + Duration::from_millis(12).max(minimum / 3)
            });
        let network_pressure = self.network_losses > 0;
        let congested = network_pressure || replacement_pressure || decode_pressure || rtt_pressure;
        let stable = enough_samples && self.replaced == 0 && !congested;
        let mut changed = false;
        if congested {
            self.congested_windows = self.congested_windows.saturating_add(1);
            let minimum_bitrate = self.ceiling_bitrate_kbps.min(MIN_ADAPTIVE_BITRATE_KBPS);
            let next_bitrate = self
                .current_bitrate_kbps
                .saturating_mul(3)
                .checked_div(4)
                .unwrap_or(self.current_bitrate_kbps)
                .max(minimum_bitrate);
            changed |= next_bitrate != self.current_bitrate_kbps;
            self.current_bitrate_kbps = next_bitrate;

            let minimum_dimension = self.ceiling_dimension.min(MIN_ADAPTIVE_DIMENSION);
            if self.congested_windows >= 2 || replacement_pressure || decode_pressure {
                let next_fps = self
                    .current_fps
                    .saturating_mul(4)
                    .checked_div(5)
                    .unwrap_or(self.current_fps)
                    .max(self.ceiling_fps.min(MIN_ADAPTIVE_FPS));
                let next_dimension = self
                    .current_dimension
                    .saturating_mul(9)
                    .checked_div(10)
                    .unwrap_or(self.current_dimension)
                    .max(minimum_dimension);
                changed |= next_fps != self.current_fps || next_dimension != self.current_dimension;
                self.current_fps = next_fps;
                self.current_dimension = next_dimension;
            }
            self.stable_windows = 0;
        } else if stable {
            self.congested_windows = 0;
            self.stable_windows = self.stable_windows.saturating_add(1);
            if self.stable_windows >= RECOVERY_WINDOWS {
                let next_fps = self.current_fps.saturating_add(5).min(self.ceiling_fps);
                let next_dimension = self
                    .current_dimension
                    .saturating_add(256)
                    .min(self.ceiling_dimension);
                let next_bitrate = self
                    .current_bitrate_kbps
                    .saturating_add((self.ceiling_bitrate_kbps / 10).max(250))
                    .min(self.ceiling_bitrate_kbps);
                changed = next_fps != self.current_fps
                    || next_dimension != self.current_dimension
                    || next_bitrate != self.current_bitrate_kbps;
                self.current_fps = next_fps;
                self.current_dimension = next_dimension;
                self.current_bitrate_kbps = next_bitrate;
                self.stable_windows = 0;
            }
        } else {
            self.stable_windows = 0;
            self.congested_windows = 0;
        }
        self.frames = 0;
        self.replaced = 0;
        self.network_losses = 0;
        self.decode_us = 0;
        self.window_started = now;
        changed.then_some(StreamTuning {
            max_fps: self.current_fps,
            max_dimension: self.current_dimension,
            target_bitrate_kbps: self.current_bitrate_kbps,
        })
    }
}

struct AppState {
    frame: Option<NativeFrame>,
    frame_dirty: bool,
    viewport: Option<Viewport>,
    cursor: Option<PhysicalPosition<f64>>,
    modifiers: ModifiersState,
    last_pointer_press: Option<PointerPress>,
    active_pointer_gesture: Option<PointerGesture>,
    interactive_pressed_button: Option<MouseButton>,
    scroll_residual_x: f64,
    scroll_residual_y: f64,
    geometry_control: bool,
    pending_resize: Option<PendingResize>,
    remote_resize_suppression: Option<RemoteResizeSuppression>,
    local_resize_active_until: Option<Instant>,
    last_observed_window_size: Option<PhysicalSize<u32>>,
    next_resize_revision: u64,
    title: String,
    status: String,
    connection_phase: ConnectionPhase,
    telemetry: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            frame: None,
            frame_dirty: false,
            viewport: None,
            cursor: None,
            modifiers: ModifiersState::empty(),
            last_pointer_press: None,
            active_pointer_gesture: None,
            interactive_pressed_button: None,
            scroll_residual_x: 0.0,
            scroll_residual_y: 0.0,
            geometry_control: false,
            pending_resize: None,
            remote_resize_suppression: None,
            local_resize_active_until: None,
            last_observed_window_size: None,
            next_resize_revision: 1,
            title: "Cua Viewer".into(),
            status: "Connecting".into(),
            connection_phase: ConnectionPhase::Connecting,
            telemetry: None,
        }
    }
}

pub(super) fn run() -> ClientResult<()> {
    let arguments = Arguments::parse()?;
    tracing::info!(
        primary_transport = arguments.endpoints[0]
            .url
            .split(':')
            .next()
            .unwrap_or("unknown"),
        endpoint_count = arguments.endpoints.len(),
        max_fps = arguments.max_fps,
        max_dimension = arguments.max_dimension,
        max_bitrate_kbps = arguments.max_bitrate_kbps,
        sync_window_size = arguments.sync_window_size,
        "starting native remote-app window"
    );
    let event_loop = EventLoopBuilder::<UiWake>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let window_builder = WindowBuilder::new()
        .with_title("RCDP — Connecting…")
        .with_inner_size(LogicalSize::new(960.0, 600.0))
        .with_min_inner_size(LogicalSize::new(320.0, 200.0));
    #[cfg(target_os = "macos")]
    let window_builder = window_builder
        .with_titlebar_transparent(true)
        .with_title_hidden(true)
        .with_fullsize_content_view(true);
    let window = Arc::new(window_builder.build(&event_loop)?);
    window.set_ime_allowed(true);

    let surface_size = window.inner_size();
    let surface_texture = SurfaceTexture::new(
        surface_size.width.max(1),
        surface_size.height.max(1),
        window.clone(),
    );
    let mut renderer: Pixels<'static> = PixelsBuilder::new(1, 1, surface_texture)
        .texture_format(TextureFormat::Bgra8UnormSrgb)
        .blend_state(BlendState::REPLACE)
        .clear_color(Color {
            r: 0.0627,
            g: 0.0667,
            b: 0.0784,
            a: 1.0,
        })
        .build()?;
    renderer.set_scaling_mode(ScalingMode::Fill);
    let mut texture_size = (1, 1);
    #[cfg(target_os = "macos")]
    let mut metal_frame_renderer =
        MetalFrameRenderer::new(renderer.device(), renderer.surface_texture_format())?;
    #[cfg(target_os = "macos")]
    let connection_overlay = ConnectionOverlay::new(&window);
    #[cfg(target_os = "macos")]
    if connection_overlay.is_none() {
        tracing::warn!("could not attach native connection-status overlay");
    }
    let (ui_tx, ui_rx) = mpsc::channel();
    let latest_frame = Arc::new(Mutex::new(None));
    let (input_tx, input_rx) = unbounded_channel();
    let (pointer_move_tx, pointer_move_rx) = watch::channel(None);
    let (interactive_scroll_tx, interactive_scroll_rx) = interactive_scroll_channel();
    let (scroll_tx, scroll_rx) = unbounded_channel();
    let interactive_input_enabled = Arc::new(AtomicBool::new(false));
    let telemetry = Arc::new(Mutex::new(LatencyTelemetry::new(Instant::now())));
    #[cfg(target_os = "macos")]
    let native_scroll_events = Arc::new(Mutex::new(VecDeque::new()));
    #[cfg(target_os = "macos")]
    let shortcut_sender = input_tx.clone();
    #[cfg(target_os = "macos")]
    let shortcut_interactive_input = interactive_input_enabled.clone();
    #[cfg(target_os = "macos")]
    let shortcut_telemetry = telemetry.clone();
    #[cfg(target_os = "macos")]
    let monitored_scroll_events = native_scroll_events.clone();
    #[cfg(target_os = "macos")]
    let shortcut_block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit invokes local event monitors with a valid NSEvent for
        // the duration of this callback on the application main thread.
        let event = unsafe { event.as_ref() };
        if unsafe { event.r#type() } == NSEventType::ScrollWheel {
            let mut events = monitored_scroll_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if events.len() >= MAX_BUFFERED_NATIVE_SCROLL_EVENTS {
                events.pop_front();
            }
            events.push_back(precise_scroll_event_from_appkit(event));
            return event as *const NSEvent as *mut NSEvent;
        }
        if send_appkit_command_shortcut(
            &shortcut_sender,
            event,
            shortcut_interactive_input.load(Ordering::Acquire),
        ) {
            telemetry_record_capture(&shortcut_telemetry, 1, false);
            std::ptr::null_mut()
        } else {
            event as *const NSEvent as *mut NSEvent
        }
    });
    // Winit documents that some Command-modified key events never reach its
    // NSView keyDown path. The same monitor also snapshots scroll phases while
    // AppKit is synchronously dispatching the native event; consulting
    // NSApp.currentEvent later from winit's queued callback can read the next
    // event and duplicate momentum boundaries.
    // SAFETY: The block and returned monitor token are retained for the full
    // event-loop lifetime below.
    #[cfg(target_os = "macos")]
    let shortcut_monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::ScrollWheel,
            &shortcut_block,
        )
    };
    spawn_network(
        arguments,
        NetworkContext {
            proxy,
            ui_tx,
            latest_frame: latest_frame.clone(),
            telemetry: telemetry.clone(),
            interactive_input_enabled: interactive_input_enabled.clone(),
        },
        NetworkReceivers {
            input_rx,
            pointer_move_rx,
            interactive_scroll_rx,
            scroll_rx,
        },
    );

    let mut state = AppState::default();
    event_loop.run(move |event, target| {
        #[cfg(target_os = "macos")]
        let _keep_shortcut_monitor_alive = &shortcut_monitor;
        target.set_control_flow(ControlFlow::Wait);
        match event {
            Event::UserEvent(UiWake::Network) => {
                drain_ui_events(
                    &ui_rx,
                    &latest_frame,
                    &window,
                    &mut state,
                    target,
                    #[cfg(target_os = "macos")]
                    connection_overlay.as_ref(),
                );
                window.request_redraw();
            }
            Event::WindowEvent { window_id, event } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => {
                    tracing::info!("client window close requested");
                    let _ = input_tx.send(InputCommand::Close);
                    let totals = telemetry
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .totals
                        .clone();
                    totals.send(Instant::now(), Duration::from_millis(400));
                    target.exit();
                }
                WindowEvent::Resized(size) => {
                    let now = Instant::now();
                    observe_local_window_size(&window, &mut state, size, now);
                    flush_pending_resize(&input_tx, &mut state, now);
                    window.request_redraw();
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    let now = Instant::now();
                    observe_local_window_size(
                        &window,
                        &mut state,
                        native_window_inner_size(&window),
                        now,
                    );
                    flush_pending_resize(&input_tx, &mut state, now);
                    window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    state.cursor = Some(position);
                    if interactive_input_enabled.load(Ordering::Acquire) {
                        telemetry_record_capture(&telemetry, 1, false);
                        send_latest_interactive_pointer(
                            &pointer_move_tx,
                            &state,
                            state.interactive_pressed_button,
                        );
                    }
                }
                WindowEvent::CursorLeft { .. }
                    if interactive_input_enabled.load(Ordering::Acquire)
                        && state.interactive_pressed_button.is_some() =>
                {
                    let button = state.interactive_pressed_button.take();
                    pointer_move_tx.send_replace(None);
                    send_interactive_pointer(&input_tx, &state, InputPointerPhase::Cancel, button);
                }
                WindowEvent::ModifiersChanged(modifiers) => state.modifiers = modifiers.state(),
                WindowEvent::MouseInput {
                    state: element_state,
                    button,
                    ..
                } => {
                    telemetry_record_capture(&telemetry, 1, false);
                    // AppKit does not emit mouseMoved before every mouseDown. In
                    // particular, background CGEvent delivery can otherwise use
                    // an old winit CursorMoved position and click the wrong row.
                    if let Some(position) = current_event_pointer_position() {
                        state.cursor = Some(position);
                    }
                    if interactive_input_enabled.load(Ordering::Acquire) {
                        // Down/up carry their own exact coordinates. Clear any
                        // stale motion sample so an old hover/drag update can
                        // never be replayed after this lossless edge.
                        pointer_move_tx.send_replace(None);
                        handle_interactive_pointer_input(
                            &input_tx,
                            &mut state,
                            element_state,
                            button,
                        );
                    } else {
                        handle_pointer_input(&input_tx, &mut state, element_state, button);
                    }
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    telemetry_record_capture(&telemetry, 1, true);
                    tracing::debug!(
                        target: "cua_spacesd_client::input",
                        ?delta,
                        interactive = interactive_input_enabled.load(Ordering::Acquire),
                        "captured client scroll"
                    );
                    #[cfg(target_os = "macos")]
                    let native_scroll = native_scroll_events
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .pop_front();
                    #[cfg(not(target_os = "macos"))]
                    let native_scroll = None;
                    if interactive_input_enabled.load(Ordering::Acquire) {
                        send_interactive_scroll(
                            &interactive_scroll_tx,
                            &mut state,
                            delta,
                            native_scroll,
                        );
                    } else {
                        send_scroll(&scroll_tx, &mut state, delta);
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    telemetry_record_capture(&telemetry, 1, false);
                    tracing::debug!(
                        target: "cua_spacesd_client::input",
                        key_kind = input_key_kind(&event.logical_key),
                        ?event.state,
                        repeat = event.repeat,
                        text_bytes = event.text.as_deref().map_or(0, str::len),
                        interactive = interactive_input_enabled.load(Ordering::Acquire),
                        "captured client keyboard event"
                    );
                    if let Some(modifiers) = current_event_modifiers() {
                        state.modifiers = modifiers;
                    }
                    if interactive_input_enabled.load(Ordering::Acquire) {
                        send_interactive_keyboard(
                            &input_tx,
                            &state,
                            &event.logical_key,
                            event.state,
                            event.repeat,
                            event.text.as_deref(),
                        );
                    } else if event.state == ElementState::Pressed
                        && !send_key(&input_tx, &state, &event.logical_key)
                    {
                        if let Some(text) = event.text.as_deref() {
                            send_text(&input_tx, text);
                        }
                    }
                }
                WindowEvent::Ime(winit::event::Ime::Commit(text)) if !text.is_empty() => {
                    telemetry_record_capture(&telemetry, 1, false);
                    tracing::debug!(
                        target: "cua_spacesd_client::input",
                        text_bytes = text.len(),
                        interactive = interactive_input_enabled.load(Ordering::Acquire),
                        "captured client IME commit"
                    );
                    if interactive_input_enabled.load(Ordering::Acquire) {
                        send_interactive_text(&input_tx, &text);
                    } else {
                        send_text(&input_tx, &text);
                    }
                }
                WindowEvent::RedrawRequested => {
                    if let Err(error) = draw(
                        &window,
                        &mut renderer,
                        &mut texture_size,
                        #[cfg(target_os = "macos")]
                        &mut metal_frame_renderer,
                        &mut state,
                        &telemetry,
                    ) {
                        state.status = format!("Render error: {error}");
                        window.set_title(&format!("RCDP — {}", state.status));
                    }
                }
                _ => {}
            },
            Event::AboutToWait => {
                let now = Instant::now();
                observe_local_window_size(
                    &window,
                    &mut state,
                    native_window_inner_size(&window),
                    now,
                );
                flush_pending_resize(&input_tx, &mut state, now);
                let poll_deadline = now + WINDOW_SIZE_POLL_INTERVAL;
                let wake_at = state
                    .pending_resize
                    .map_or(poll_deadline, |pending| pending.deadline.min(poll_deadline));
                target.set_control_flow(ControlFlow::WaitUntil(wake_at));
            }
            _ => {}
        }
    })?;
    Ok(())
}

struct NetworkContext {
    proxy: EventLoopProxy<UiWake>,
    ui_tx: mpsc::Sender<UiEvent>,
    latest_frame: Arc<Mutex<Option<NativeFrame>>>,
    telemetry: Arc<Mutex<LatencyTelemetry>>,
    interactive_input_enabled: Arc<AtomicBool>,
}

struct NetworkReceivers {
    input_rx: UnboundedReceiver<InputCommand>,
    pointer_move_rx: watch::Receiver<Option<InteractiveInputEvent>>,
    interactive_scroll_rx: InteractiveScrollReceiver,
    scroll_rx: UnboundedReceiver<ScrollCommand>,
}

fn spawn_network(arguments: Arguments, context: NetworkContext, receivers: NetworkReceivers) {
    thread::Builder::new()
        .name("cua-viewer-network".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let result = match runtime {
                Ok(runtime) => runtime.block_on(network_main(arguments, &context, receivers)),
                Err(error) => Err(error.into()),
            };
            if let Err(error) = result {
                tracing::error!(%error, "client network worker stopped");
                emit(
                    &context.proxy,
                    &context.ui_tx,
                    UiEvent::Fatal(error.to_string()),
                );
            }
        })
        .expect("network thread starts");
}

fn discard_stale_session_input(receivers: &mut NetworkReceivers) -> bool {
    discard_stale_session_input_parts(
        &mut receivers.input_rx,
        &mut receivers.pointer_move_rx,
        &mut receivers.interactive_scroll_rx,
        &mut receivers.scroll_rx,
    )
}

fn discard_stale_session_input_parts(
    input_rx: &mut UnboundedReceiver<InputCommand>,
    pointer_move_rx: &mut watch::Receiver<Option<InteractiveInputEvent>>,
    interactive_scroll_rx: &mut InteractiveScrollReceiver,
    scroll_rx: &mut UnboundedReceiver<ScrollCommand>,
) -> bool {
    let mut close_requested = false;
    let mut discarded_commands = 0_usize;
    while let Ok(command) = input_rx.try_recv() {
        close_requested |= matches!(command, InputCommand::Close);
        discarded_commands = discarded_commands.saturating_add(1);
    }
    let _ = pointer_move_rx.borrow_and_update().clone();
    let mut discarded_scroll_events = 0_usize;
    while interactive_scroll_rx.try_recv().is_some() {
        discarded_scroll_events = discarded_scroll_events.saturating_add(1);
    }
    let mut discarded_compatibility_scrolls = 0_usize;
    while scroll_rx.try_recv().is_ok() {
        discarded_compatibility_scrolls = discarded_compatibility_scrolls.saturating_add(1);
    }
    if discarded_commands > 0 || discarded_scroll_events > 0 || discarded_compatibility_scrolls > 0
    {
        tracing::debug!(
            target: "cua_spacesd_client::input",
            discarded_commands,
            discarded_scroll_events,
            discarded_compatibility_scrolls,
            close_requested,
            "discarded input queued for a disconnected session"
        );
    }
    close_requested || input_rx.is_closed()
}

fn emit(proxy: &EventLoopProxy<UiWake>, sender: &mpsc::Sender<UiEvent>, event: UiEvent) {
    if sender.send(event).is_ok() {
        let _ = proxy.send_event(UiWake::Network);
    }
}

fn publish_frame(
    proxy: &EventLoopProxy<UiWake>,
    latest_frame: &Mutex<Option<NativeFrame>>,
    frame: NativeFrame,
) -> bool {
    let replaced = latest_frame
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .replace(frame)
        .is_some();
    let _ = proxy.send_event(UiWake::Network);
    replaced
}

async fn next_input_command(
    deferred: &mut Option<InputCommand>,
    input_rx: &mut UnboundedReceiver<InputCommand>,
    pointer_move_rx: &mut watch::Receiver<Option<InteractiveInputEvent>>,
    interactive_scroll_rx: &mut InteractiveScrollReceiver,
    allow_continuous_input: bool,
) -> Option<InputCommand> {
    if let Some(command) = deferred.take() {
        return Some(command);
    }
    if let Ok(command) = input_rx.try_recv() {
        return Some(command);
    }
    if !allow_continuous_input {
        return input_rx.recv().await;
    }
    if let Some(event) = interactive_scroll_rx.try_recv() {
        return Some(InputCommand::Interactive(event));
    }
    loop {
        tokio::select! {
            biased;
            command = input_rx.recv() => return command,
            changed = interactive_scroll_rx.wake.changed() => {
                if changed.is_err() {
                    return input_rx.recv().await;
                }
                if let Some(event) = interactive_scroll_rx.try_recv() {
                    return Some(InputCommand::Interactive(event));
                }
            }
            changed = pointer_move_rx.changed() => {
                if changed.is_err() {
                    return input_rx.recv().await;
                }
                if let Some(event) = pointer_move_rx.borrow_and_update().clone() {
                    return Some(InputCommand::Interactive(event));
                }
            }
        }
    }
}

async fn network_main(
    arguments: Arguments,
    context: &NetworkContext,
    mut receivers: NetworkReceivers,
) -> ClientResult<()> {
    let mut backoff = ReconnectBackoff::default();
    let mut reconnect_attempt = 0_u64;
    let mut endpoint_index = 0_usize;
    loop {
        context
            .interactive_input_enabled
            .store(false, Ordering::Release);
        let mut established = false;
        let endpoint = &arguments.endpoints[endpoint_index];
        let failed_transport = endpoint.url.split(':').next().unwrap_or("unknown");
        match network_session(
            &arguments,
            endpoint,
            context,
            &mut receivers,
            &mut established,
        )
        .await
        {
            Ok(()) => {
                return Ok(());
            }
            Err(error) => {
                context
                    .interactive_input_enabled
                    .store(false, Ordering::Release);
                if discard_stale_session_input(&mut receivers) {
                    tracing::info!("client closed while remote transport was unavailable");
                    return Ok(());
                }
                if established {
                    backoff.reset();
                }
                endpoint_index = (endpoint_index + 1) % arguments.endpoints.len();
                let completed_endpoint_cycle = endpoint_index == 0;
                let has_alternate_endpoint = arguments.endpoints.len() > 1;
                let try_alternate_immediately =
                    has_alternate_endpoint && (!completed_endpoint_cycle || established);
                let delay = if try_alternate_immediately {
                    Duration::ZERO
                } else {
                    backoff.next_delay()
                };
                let next_transport = arguments.endpoints[endpoint_index]
                    .url
                    .split(':')
                    .next()
                    .unwrap_or("unknown");
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                tracing::warn!(
                    %error,
                    reconnect_attempt,
                    reconnect_delay_ms = delay.as_millis(),
                    established,
                    failed_transport,
                    next_transport,
                    "remote transport interrupted; reconnecting"
                );
                let message = if try_alternate_immediately {
                    format!("connection lost; trying alternate {next_transport} path")
                } else {
                    format!(
                        "connection lost; reconnecting in {:.2}s",
                        delay.as_secs_f64()
                    )
                };
                emit(&context.proxy, &context.ui_tx, UiEvent::Suspended(message));
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

async fn network_session(
    arguments: &Arguments,
    endpoint: &ConnectionEndpoint,
    context: &NetworkContext,
    receivers: &mut NetworkReceivers,
    established: &mut bool,
) -> ClientResult<()> {
    let NetworkContext {
        proxy,
        ui_tx,
        latest_frame,
        telemetry,
        interactive_input_enabled,
    } = context;
    let NetworkReceivers {
        input_rx,
        pointer_move_rx,
        interactive_scroll_rx,
        scroll_rx,
    } = receivers;

    emit(proxy, ui_tx, UiEvent::Status("Connecting".into()));
    let transport_name = endpoint.url.split(':').next().unwrap_or("unknown");
    telemetry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .totals
        .transport = crate::telemetry::StreamTotals::transport_of(&endpoint.url);
    tracing::info!(transport = transport_name, "connecting remote transport");
    let mut transport = tokio::time::timeout(
        CONNECTION_ATTEMPT_TIMEOUT,
        ClientTransport::connect(
            &endpoint.url,
            endpoint.quic_cert_sha256.as_deref(),
            endpoint.token.as_deref(),
        ),
    )
    .await
    .map_err(|_| "remote transport connection attempt timed out")??;
    tracing::info!(transport = transport_name, "remote transport connected");

    if let Some(token) = &endpoint.token {
        send_client(
            &mut transport,
            ClientMessage::Authenticate {
                token: token.clone(),
            },
        )
        .await?;
        wait_for_control(&mut transport, |message| {
            matches!(message, ServerMessage::Authenticated)
        })
        .await?;
    }
    send_client(
        &mut transport,
        ClientMessage::Hello(Hello {
            protocol_name: arguments.protocol_name.clone(),
            ..Hello::default()
        }),
    )
    .await?;
    let server_hello = wait_for_control(&mut transport, |message| {
        matches!(message, ServerMessage::Hello(_))
    })
    .await?;
    let ServerMessage::Hello(server_hello) = server_hello else {
        unreachable!("hello predicate only accepts hello messages");
    };
    let supports_clipboard = server_hello
        .capabilities
        .iter()
        .any(|capability| capability == "clipboard.text.v1");
    let supports_file_clipboard = server_hello
        .capabilities
        .iter()
        .any(|capability| capability == "clipboard.files.v1");
    send_client(
        &mut transport,
        ClientMessage::ListWindows {
            on_screen_only: true,
        },
    )
    .await?;
    let windows = wait_for_windows(&mut transport).await?;
    let descriptor = match arguments.target.as_deref() {
        Some(expected) => windows
            .into_iter()
            .find(|window| window.window.0 == expected)
            .ok_or("requested target is not available")?,
        None => preferred_window(windows).ok_or("the share has no visible windows")?,
    };

    send_client(
        &mut transport,
        ClientMessage::OpenSession(OpenSession {
            window: descriptor.window.clone(),
            target_epoch: descriptor.target_epoch,
            accepted_codecs: accepted_video_codecs(),
            max_fps: arguments.max_fps,
            max_dimension: arguments.max_dimension,
            target_bitrate_kbps: Some(arguments.max_bitrate_kbps),
            policy: NATIVE_SESSION_POLICY,
            geometry_control: if arguments.sync_window_size {
                WindowGeometryControl::Bidirectional
            } else {
                WindowGeometryControl::ObserveOnly
            },
        }),
    )
    .await?;
    let opened = wait_for_opened(&mut transport).await?;
    if discard_stale_session_input_parts(
        input_rx,
        pointer_move_rx,
        interactive_scroll_rx,
        scroll_rx,
    ) {
        send_client(
            &mut transport,
            ClientMessage::CloseSession {
                session_id: opened.session_id,
            },
        )
        .await?;
        return Ok(());
    }
    let geometry_control = arguments.sync_window_size
        && opened.geometry_control == WindowGeometryControl::Bidirectional
        && opened
            .capabilities
            .iter()
            .any(|capability| capability == "window.geometry.bidirectional");
    emit(
        proxy,
        ui_tx,
        UiEvent::Opened {
            title: display_title(&descriptor.app_name, &descriptor.title),
            geometry: opened.geometry.clone(),
            geometry_control,
        },
    );
    let session_id = opened.session_id;
    let supports_interactive_input = opened
        .capabilities
        .iter()
        .any(|capability| capability == "input.interactive.v1");
    interactive_input_enabled.store(supports_interactive_input, Ordering::Release);
    *established = true;
    tracing::info!(
        session_id = ?session_id,
        supports_interactive_input,
        supports_clipboard,
        supports_file_clipboard,
        geometry_control,
        codec = ?opened.codec,
        "remote window session opened"
    );
    let supports_runtime_preferences = opened
        .capabilities
        .iter()
        .any(|capability| capability == "video.preferences.runtime");
    let supports_scroll = opened.action_capabilities.iter().any(|capability| {
        capability.action == "scroll"
            && capability.guarantee != ActionDeliveryGuarantee::Unsupported
    });
    let mut adaptive_preferences = AdaptivePreferences::new(
        opened.max_fps,
        opened.max_dimension,
        opened.target_bitrate_kbps.unwrap_or(8_000),
        Instant::now(),
    );
    let mut action_counter = 0_u64;
    let mut pending_actions = HashMap::new();
    let mut input_sequence = 1_u64;
    let mut pending_input_batches = HashMap::new();
    let mut deferred_input: Option<InputCommand> = None;
    let mut h264_decoder = H264Decoder::new();
    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    stats_tick.tick().await;
    let mut clipboard_tick = tokio::time::interval(CLIPBOARD_POLL_INTERVAL);
    clipboard_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    clipboard_tick.tick().await;
    let mut health_tick = tokio::time::interval(SESSION_HEALTH_INTERVAL);
    health_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    health_tick.tick().await;
    let mut local_clipboard_generation = None;
    let mut remote_clipboard_generation = None;
    let mut pending_stats = None;
    let mut last_loss_keyframe_request: Option<Instant> = None;
    let mut last_video = Instant::now();
    let mut video_probe_started: Option<Instant> = None;
    let mut video_suspended = false;

    loop {
        tokio::select! {
            biased;
            command = next_input_command(
                &mut deferred_input,
                input_rx,
                pointer_move_rx,
                interactive_scroll_rx,
                pending_input_batches.len() < MAX_IN_FLIGHT_CONTINUOUS_INPUT_BATCHES,
            ), if if supports_interactive_input {
                pending_input_batches.len() < MAX_IN_FLIGHT_INPUT_BATCHES
            } else {
                pending_actions.len() < MAX_IN_FLIGHT_ACTIONS
            } => {
                let Some(command) = command else { return Ok(()); };
                match command {
                    InputCommand::Close => {
                        tracing::info!(session_id = ?session_id, "closing remote session");
                        send_client(&mut transport, ClientMessage::CloseSession { session_id }).await?;
                        return Ok(());
                    }
                    InputCommand::Action { tool, arguments, basis } => {
                        action_counter = action_counter.saturating_add(1);
                        let action_id = format!("native-{action_counter}");
                        send_client(&mut transport, ClientMessage::Action(ActionRequest {
                            action_id: action_id.clone(),
                            session_id: session_id.clone(),
                            tool: tool.into(),
                            arguments,
                            basis,
                        })).await?;
                        tracing::debug!(
                            target: "cua_spacesd_client::input",
                            %action_id,
                            tool,
                            in_flight = pending_actions.len() + 1,
                            "sent compatibility input action"
                        );
                        pending_actions.insert(action_id, Instant::now());
                        telemetry
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .record_input_sent(1, 0, pending_actions.len(), input_rx.len(), true);
                    }
                    InputCommand::Text(text) => {
                        let text = coalesce_text_input(text, input_rx, &mut deferred_input);
                        action_counter = action_counter.saturating_add(1);
                        let action_id = format!("native-{action_counter}");
                        send_client(&mut transport, ClientMessage::Action(ActionRequest {
                            action_id: action_id.clone(),
                            session_id: session_id.clone(),
                            tool: "type_text".into(),
                            arguments: serde_json::json!({"text": text, "delay_ms": 0}),
                            basis: ActionBasis::None,
                        })).await?;
                        tracing::debug!(
                            target: "cua_spacesd_client::input",
                            %action_id,
                            text_bytes = text.len(),
                            in_flight = pending_actions.len() + 1,
                            "sent compatibility text action"
                        );
                        pending_actions.insert(action_id, Instant::now());
                        telemetry
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .record_input_sent(1, 0, pending_actions.len(), input_rx.len(), true);
                    }
                    InputCommand::Interactive(event) => {
                        if !supports_interactive_input {
                            continue;
                        }
                        let events = coalesce_interactive_input(
                            event,
                            input_rx,
                            &mut deferred_input,
                        );
                        let event_count = u64::try_from(events.len())?;
                        let scroll_event_count = events
                            .iter()
                            .filter(|event| matches!(event, InteractiveInputEvent::Scroll { .. }))
                            .count() as u64;
                        let expects_visual_change = events.iter().any(|event| {
                            !matches!(
                                event,
                                InteractiveInputEvent::Pointer {
                                    phase: InputPointerPhase::Move,
                                    ..
                                }
                            )
                        });
                        let event_summary = interactive_event_summary(&events);
                        let through_sequence = input_sequence
                            .checked_add(event_count.saturating_sub(1))
                            .ok_or("interactive input sequence exhausted")?;
                        send_client(
                            &mut transport,
                            ClientMessage::InteractiveInput(InteractiveInputBatch {
                                session_id: session_id.clone(),
                                first_sequence: input_sequence,
                                events,
                            }),
                        )
                        .await?;
                        tracing::debug!(
                            target: "cua_spacesd_client::input",
                            first_sequence = input_sequence,
                            through_sequence,
                            event_count,
                            events = %event_summary,
                            queued_commands = input_rx.len(),
                            latest_pointer_pending = pointer_move_rx.has_changed().unwrap_or(false),
                            queued_scroll_events = interactive_scroll_rx.len(),
                            in_flight = pending_input_batches.len() + 1,
                            "sent interactive input batch"
                        );
                        pending_input_batches.insert(through_sequence, Instant::now());
                        telemetry
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .record_input_sent(
                                event_count,
                                scroll_event_count,
                                pending_input_batches.len(),
                                input_rx
                                    .len()
                                    .saturating_add(interactive_scroll_rx.len()),
                                expects_visual_change,
                            );
                        input_sequence = through_sequence
                            .checked_add(1)
                            .ok_or("interactive input sequence exhausted")?;
                    }
                    InputCommand::Resize { revision, width_points, height_points } => {
                        send_client(
                            &mut transport,
                            ClientMessage::SetWindowGeometry(WindowGeometryRequest {
                                session_id: session_id.clone(),
                                revision,
                                width_points,
                                height_points,
                            }),
                        )
                        .await?;
                        tracing::debug!(
                            target: "cua_spacesd_client::geometry",
                            session_id = ?session_id,
                            revision,
                            width_points,
                            height_points,
                            "sent host window resize request"
                        );
                        telemetry
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .record_input_sent(
                                1,
                                0,
                                pending_input_batches.len(),
                                input_rx.len(),
                                false,
                            );
                    }
                }
            }
            command = scroll_rx.recv(), if pending_actions.len() < MAX_IN_FLIGHT_ACTIONS => {
                let Some(command) = command else {
                    continue;
                };
                // Compatibility scroll actions are slow automation calls. A
                // queued delta is a stale velocity sample, not work that must
                // be replayed, so always keep the newest sample without adding
                // older force values to it.
                let command = latest_scroll_command(command, scroll_rx);
                let ScrollCommand { x, y, direction, amount, basis } = command;
                action_counter = action_counter.saturating_add(1);
                let (tool, arguments, basis) = if supports_scroll {
                    (
                        "scroll",
                        serde_json::json!({
                            "x": x,
                            "y": y,
                            "direction": direction,
                            "amount": amount,
                        }),
                        basis,
                    )
                } else {
                    (
                        "press_key",
                        serde_json::json!({"key": direction, "modifiers": []}),
                        ActionBasis::None,
                    )
                };
                let action_id = format!("native-{action_counter}");
                send_client(&mut transport, ClientMessage::Action(ActionRequest {
                    action_id: action_id.clone(),
                    session_id: session_id.clone(),
                    tool: tool.into(),
                    arguments,
                    basis,
                }))
                .await?;
                tracing::debug!(
                    target: "cua_spacesd_client::input",
                    %action_id,
                    tool,
                    direction,
                    amount,
                    "sent compatibility scroll action"
                );
                pending_actions.insert(action_id, Instant::now());
                telemetry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .record_input_sent(1, 1, pending_actions.len(), scroll_rx.len(), true);
            }
            _ = clipboard_tick.tick(), if supports_clipboard || supports_file_clipboard => {
                match local_clipboard_snapshot() {
                    Ok((generation, text)) if local_clipboard_generation != Some(generation) => {
                        local_clipboard_generation = Some(generation);
                        let file_paths = if supports_file_clipboard {
                            local_file_clipboard_paths()
                                .ok()
                                .filter(|(file_generation, _)| *file_generation == generation)
                                .map(|(_, paths)| paths)
                                .unwrap_or_default()
                        } else {
                            Vec::new()
                        };
                        if !file_paths.is_empty() {
                            match encode_local_clipboard_files(&file_paths) {
                                Ok((files, payload)) if !files.is_empty() => {
                                    let file_count = files.len();
                                    let total_bytes = files.iter().map(|file| file.byte_len).sum::<u64>();
                                    send_client_payload(
                                        &mut transport,
                                        ClientMessage::SetClipboardFiles {
                                            files,
                                            byte_len: payload.len() as u64,
                                        },
                                        &payload,
                                    )
                                    .await?;
                                    tracing::info!(
                                        target: "cua_spacesd_client::clipboard",
                                        generation,
                                        file_count,
                                        total_bytes,
                                        "sent client file clipboard update"
                                    );
                                }
                                Ok(_) => tracing::warn!(
                                    target: "cua_spacesd_client::clipboard",
                                    generation,
                                    "file clipboard contained no transferable regular files"
                                ),
                                Err(error) => tracing::warn!(
                                    target: "cua_spacesd_client::clipboard",
                                    generation,
                                    %error,
                                    "client file clipboard exceeds transfer policy"
                                ),
                            }
                        } else if supports_clipboard {
                            match text {
                            Some(text) if text.len() <= MAX_CLIPBOARD_TEXT_BYTES => {
                                let text_bytes = text.len();
                                send_client(
                                    &mut transport,
                                    ClientMessage::SetClipboard { text },
                                )
                                .await?;
                                tracing::debug!(
                                    target: "cua_spacesd_client::clipboard",
                                    generation,
                                    text_bytes,
                                    "sent client clipboard update"
                                );
                            }
                            Some(text) => {
                                tracing::warn!(
                                    target: "cua_spacesd_client::clipboard",
                                    generation,
                                    text_bytes = text.len(),
                                    max_text_bytes = MAX_CLIPBOARD_TEXT_BYTES,
                                    "client clipboard text exceeds synchronization limit"
                                );
                            }
                            None => {
                                send_client(
                                    &mut transport,
                                    ClientMessage::GetClipboard {
                                        known_generation: remote_clipboard_generation,
                                    },
                                )
                                .await?;
                            }
                            }
                        }
                    }
                    Ok(_) if supports_clipboard => {
                        send_client(
                            &mut transport,
                            ClientMessage::GetClipboard {
                                known_generation: remote_clipboard_generation,
                            },
                        )
                        .await?;
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(
                            target: "cua_spacesd_client::clipboard",
                            %error,
                            "failed to read client clipboard"
                        );
                    }
                }
                if supports_file_clipboard {
                    send_client(
                        &mut transport,
                        ClientMessage::GetClipboardFiles {
                            known_generation: remote_clipboard_generation,
                        },
                    )
                    .await?;
                }
            }
            _ = stats_tick.tick() => {
                let now = Instant::now();
                if let Some(rtt) = transport.transport_rtt() {
                    telemetry
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .record_control_rtt(rtt);
                }
                let summary = telemetry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .summary_and_reset(now);
                if arguments.show_stats {
                    emit(proxy, ui_tx, UiEvent::Telemetry(summary.title));
                }
                if pending_stats.is_none() {
                    send_client(
                        &mut transport,
                        ClientMessage::GetStats {
                            session_id: session_id.clone(),
                        },
                    )
                    .await?;
                    pending_stats = Some(Instant::now());
                }
            }
            _ = health_tick.tick() => {
                let now = Instant::now();
                match video_health_action(last_video, video_probe_started, video_suspended, now) {
                    VideoHealthAction::None => {}
                    VideoHealthAction::RequestKeyframe => {
                        tracing::debug!(
                            transport = transport_name,
                            idle_for_ms = now.saturating_duration_since(last_video).as_millis(),
                            "probing an idle remote video stream with a keyframe request"
                        );
                        send_client(
                            &mut transport,
                            ClientMessage::RequestKeyframe {
                                session_id: session_id.clone(),
                            },
                        )
                        .await?;
                        video_probe_started = Some(now);
                    }
                    VideoHealthAction::Stall => {
                        tracing::warn!(
                            transport = transport_name,
                            stalled_for_ms = now.saturating_duration_since(last_video).as_millis(),
                            probe_wait_ms = video_probe_started
                                .map(|started| now.saturating_duration_since(started).as_millis()),
                            "remote video stream did not answer a keyframe probe"
                        );
                        return Err("remote video stream stalled after keyframe probe".into());
                    }
                }
            }
            event = transport.next() => {
                match event? {
                    TransportEvent::Packet(WireHeader::Video(frame), payload) => {
                            if frame.session_id == session_id {
                                last_video = Instant::now();
                                video_probe_started = None;
                                let received_at = Instant::now();
                                let payload_bytes = payload.len();
                                {
                                    let mut t = telemetry
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    t.totals.max_height = t.totals.max_height.max(frame.height_px);
                                    if t.totals.codec.is_empty() {
                                        t.totals.codec = format!("{:?}", frame.codec).to_ascii_lowercase();
                                    }
                                }
                                let host_encode_us = frame.encode_duration_us;
                                let decode_started = Instant::now();
                                let decoded = match frame.codec {
                                    VideoCodec::Bgra => Some(decode_bgra(frame, payload)?),
                                    VideoCodec::H264 => {
                                        #[cfg(any(target_os = "macos", target_os = "windows"))]
                                        {
                                            match h264_decoder.decode(frame, &payload) {
                                                Ok(frame) => frame,
                                                Err(error) => {
                                                    emit(proxy, ui_tx, UiEvent::Status(format!("H.264 recovery: {error}")));
                                                    None
                                                }
                                            }
                                        }
                                        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                                        {
                                            return Err("this native client did not negotiate H.264".into());
                                        }
                                    }
                                    codec => return Err(format!("native renderer received unsupported codec {codec:?}").into()),
                                };
                                if h264_decoder.take_keyframe_request() {
                                    send_client(
                                        &mut transport,
                                        ClientMessage::RequestKeyframe {
                                            session_id: session_id.clone(),
                                        },
                                    )
                                    .await?;
                                }
                                if let Some(mut frame) = decoded {
                                    let decode_elapsed = decode_started.elapsed();
                                    frame.received_at = received_at;
                                    let replaced = publish_frame(proxy, latest_frame, frame);
                                    telemetry
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .record_frame(
                                            payload_bytes,
                                            host_encode_us,
                                            decode_elapsed,
                                            replaced,
                                        );
                                    if supports_runtime_preferences {
                                        if let Some(tuning) = adaptive_preferences.observe_frame(
                                            replaced,
                                            decode_elapsed,
                                            transport.transport_rtt(),
                                            Instant::now(),
                                        ) {
                                            send_client(
                                                &mut transport,
                                                ClientMessage::SetStreamPreferences(StreamPreferences {
                                                    session_id: session_id.clone(),
                                                    max_fps: tuning.max_fps,
                                                    max_dimension: tuning.max_dimension,
                                                    target_bitrate_kbps: Some(tuning.target_bitrate_kbps),
                                                }),
                                            )
                                            .await?;
                                        }
                                    }
                                }
                            }
                    }
                    TransportEvent::Packet(WireHeader::Server(message), packet_payload) => {
                            match message {
                                ServerMessage::Lifecycle { session_id: event_session, event }
                                    if event_session == session_id => match event {
                                        WindowLifecycleEvent::TitleChanged { title } => {
                                            emit(proxy, ui_tx, UiEvent::Title(display_title(&descriptor.app_name, &title)));
                                        }
                                        WindowLifecycleEvent::Suspended { reason } => {
                                            video_suspended = true;
                                            video_probe_started = None;
                                            emit(proxy, ui_tx, UiEvent::Suspended(format!("{reason:?}").to_lowercase()));
                                        }
                                        WindowLifecycleEvent::Resumed => {
                                            video_suspended = false;
                                            last_video = Instant::now();
                                            video_probe_started = None;
                                            emit(proxy, ui_tx, UiEvent::Resumed);
                                        }
                                        WindowLifecycleEvent::Closed => {
                                            emit(proxy, ui_tx, UiEvent::Closed);
                                            return Ok(());
                                        }
                                        WindowLifecycleEvent::GeometryChanged { geometry, .. } => {
                                            emit(proxy, ui_tx, UiEvent::Geometry(geometry));
                                        }
                                        WindowLifecycleEvent::Unknown => {}
                                    },
                                ServerMessage::ActionResult(result) => {
                                    let round_trip = pending_actions
                                        .remove(&result.action_id)
                                        .map(|sent_at| sent_at.elapsed());
                                    if let Some(elapsed) = round_trip {
                                        telemetry
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                                            .record_action(elapsed);
                                    }
                                    tracing::debug!(
                                        target: "cua_spacesd_client::input",
                                        action_id = %result.action_id,
                                        delivered = result.delivered,
                                        round_trip_ms = round_trip.map(|elapsed| elapsed.as_secs_f64() * 1_000.0),
                                        in_flight = pending_actions.len(),
                                        "received compatibility input acknowledgement"
                                    );
                                    if !result.delivered {
                                        if let Some(error) = result.error {
                                            emit(proxy, ui_tx, UiEvent::Status(format!("Input rejected: {}", error.message)));
                                        }
                                    }
                                }
                                ServerMessage::InteractiveInputAcknowledgement(acknowledgement)
                                    if acknowledgement.session_id == session_id => {
                                        let round_trip = pending_input_batches
                                            .remove(&acknowledgement.through_sequence)
                                            .map(|sent_at| sent_at.elapsed());
                                        if let Some(elapsed) = round_trip {
                                            telemetry
                                                .lock()
                                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                                .record_action(elapsed);
                                        }
                                        telemetry
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                                            .record_host_dispatch(acknowledgement.host_dispatch_us);
                                        pending_input_batches.retain(|sequence, _| {
                                            *sequence > acknowledgement.through_sequence
                                        });
                                        tracing::debug!(
                                            target: "cua_spacesd_client::input",
                                            through_sequence = acknowledgement.through_sequence,
                                            delivered = acknowledgement.delivered,
                                            matched_batch = round_trip.is_some(),
                                            round_trip_ms = round_trip.map(|elapsed| elapsed.as_secs_f64() * 1_000.0),
                                            host_dispatch_us = acknowledgement.host_dispatch_us,
                                            in_flight = pending_input_batches.len(),
                                            queued_commands = input_rx.len(),
                                            "received interactive input acknowledgement"
                                        );
                                        if !acknowledgement.delivered {
                                            if let Some(error) = acknowledgement.error {
                                                emit(
                                                    proxy,
                                                    ui_tx,
                                                    UiEvent::Status(format!(
                                                        "Input rejected: {}",
                                                        error.message
                                                    )),
                                                );
                                            }
                                        }
                                    }
                                ServerMessage::StreamPreferencesApplied(preferences)
                                    if preferences.session_id == session_id => {
                                        emit(
                                            proxy,
                                            ui_tx,
                                            UiEvent::Status(format!(
                                                "Live · {} fps · {} px · {:.1} Mbps",
                                                preferences.max_fps,
                                                preferences.max_dimension,
                                                preferences.target_bitrate_kbps.unwrap_or(0) as f64 / 1_000.0,
                                            )),
                                        );
                                    }
                                ServerMessage::WindowGeometryResult(result)
                                    if result.session_id == session_id => {
                                        tracing::debug!(
                                            target: "cua_spacesd_client::geometry",
                                            revision = result.revision,
                                            applied = result.applied,
                                            width_points = result.width_points,
                                            height_points = result.height_points,
                                            error = result.error.as_deref(),
                                            "received host window resize result"
                                        );
                                        if !result.applied {
                                            emit(
                                                proxy,
                                                ui_tx,
                                                UiEvent::Status(format!(
                                                    "Host resize rejected: {}",
                                                    result.error.unwrap_or_else(|| "unknown error".into())
                                                )),
                                            );
                                        }
                                    }
                                ServerMessage::Stats(stats) if stats.session_id == session_id => {
                                    if let Some(sent_at) = pending_stats.take() {
                                        telemetry
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                                            .record_control_rtt(sent_at.elapsed());
                                    }
                                    telemetry
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .record_server_replacements(stats.frames_replaced);
                                }
                                ServerMessage::Clipboard { generation, text } => {
                                    remote_clipboard_generation = Some(generation);
                                    if let Some(text) = text {
                                        let text_bytes = text.len();
                                        if text_bytes <= MAX_CLIPBOARD_TEXT_BYTES {
                                            match local_clipboard_snapshot() {
                                                Ok((local_generation, current))
                                                    if current.as_deref() == Some(text.as_str()) =>
                                                {
                                                    local_clipboard_generation =
                                                        Some(local_generation);
                                                }
                                                Ok(_) => match set_local_clipboard(&text) {
                                                    Ok(local_generation) => {
                                                        local_clipboard_generation =
                                                            Some(local_generation);
                                                        tracing::debug!(
                                                            target: "cua_spacesd_client::clipboard",
                                                            generation,
                                                            local_generation,
                                                            text_bytes,
                                                            "applied host clipboard update"
                                                        );
                                                    }
                                                    Err(error) => {
                                                        tracing::warn!(
                                                            target: "cua_spacesd_client::clipboard",
                                                            %error,
                                                            "failed to write client clipboard"
                                                        );
                                                    }
                                                },
                                                Err(error) => {
                                                    tracing::warn!(
                                                        target: "cua_spacesd_client::clipboard",
                                                        %error,
                                                        "failed to compare client clipboard"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                                ServerMessage::ClipboardFiles { generation, files, byte_len } => {
                                    remote_clipboard_generation = Some(generation);
                                    if let Some(files) = files {
                                        let file_count = files.len();
                                        let total_bytes = files.iter().map(|file| file.byte_len).sum::<u64>();
                                        let materialized = if byte_len == packet_payload.len() as u64 {
                                            materialize_local_clipboard_files(
                                                &files,
                                                &packet_payload,
                                                generation,
                                            )
                                        } else {
                                            Err("file clipboard payload length does not match header".into())
                                        };
                                        match materialized
                                            .and_then(|paths| set_local_file_clipboard(&paths))
                                        {
                                            Ok(local_generation) => {
                                                local_clipboard_generation = Some(local_generation);
                                                tracing::info!(
                                                    target: "cua_spacesd_client::clipboard",
                                                    generation,
                                                    local_generation,
                                                    file_count,
                                                    total_bytes,
                                                    "applied host file clipboard update"
                                                );
                                            }
                                            Err(error) => tracing::warn!(
                                                target: "cua_spacesd_client::clipboard",
                                                generation,
                                                %error,
                                                "failed to apply host file clipboard update"
                                            ),
                                        }
                                    }
                                }
                                ServerMessage::Error { code, message } => {
                                    return Err(format!("server error {code:?}: {message}").into());
                                }
                                _ => {}
                            }
                    }
                    TransportEvent::VideoLoss { incomplete_frames, lost_keyframe } => {
                        telemetry
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .record_network_loss(incomplete_frames);
                        let now = Instant::now();
                        if supports_runtime_preferences {
                            if let Some(tuning) = adaptive_preferences.observe_network_loss(
                                incomplete_frames,
                                transport.transport_rtt(),
                                now,
                            ) {
                                send_client(
                                    &mut transport,
                                    ClientMessage::SetStreamPreferences(StreamPreferences {
                                        session_id: session_id.clone(),
                                        max_fps: tuning.max_fps,
                                        max_dimension: tuning.max_dimension,
                                        target_bitrate_kbps: Some(tuning.target_bitrate_kbps),
                                    }),
                                )
                                .await?;
                            }
                        }
                        let recovery_due = last_loss_keyframe_request.is_none_or(|previous| {
                            now.saturating_duration_since(previous) >= Duration::from_millis(250)
                        });
                        if lost_keyframe || recovery_due {
                            send_client(
                                &mut transport,
                                ClientMessage::RequestKeyframe {
                                    session_id: session_id.clone(),
                                },
                            )
                            .await?;
                            last_loss_keyframe_request = Some(now);
                        }
                    }
                    TransportEvent::Ping(payload) => transport.send_pong(payload).await?,
                    TransportEvent::Closed => return Err("server closed the stream".into()),
                    TransportEvent::Packet(_, _) => {}
                }
            }
        }
    }
}

fn preferred_window(
    windows: Vec<cua_media_protocol::WindowDescriptor>,
) -> Option<cua_media_protocol::WindowDescriptor> {
    windows.into_iter().max_by_key(|window| {
        let area = u64::from(window.geometry.width_px) * u64::from(window.geometry.height_px);
        (window.visible, !window.title.trim().is_empty(), area)
    })
}

async fn send_client(transport: &mut ClientTransport, message: ClientMessage) -> ClientResult<()> {
    transport.send(message).await
}

async fn send_client_payload(
    transport: &mut ClientTransport,
    message: ClientMessage,
    payload: &[u8],
) -> ClientResult<()> {
    transport.send_payload(message, payload).await
}

async fn next_server(transport: &mut ClientTransport) -> ClientResult<ServerMessage> {
    loop {
        match transport.next().await? {
            TransportEvent::Packet(WireHeader::Server(message), _) => {
                if let ServerMessage::Error { code, message } = &message {
                    return Err(format!("server error {code:?}: {message}").into());
                }
                return Ok(message);
            }
            TransportEvent::Ping(payload) => transport.send_pong(payload).await?,
            TransportEvent::Closed => return Err("server closed during handshake".into()),
            TransportEvent::Packet(_, _) | TransportEvent::VideoLoss { .. } => {}
        }
    }
}

async fn wait_for_control(
    transport: &mut ClientTransport,
    predicate: impl Fn(&ServerMessage) -> bool,
) -> ClientResult<ServerMessage> {
    loop {
        let message = next_server(transport).await?;
        if predicate(&message) {
            return Ok(message);
        }
    }
}

async fn wait_for_windows(
    transport: &mut ClientTransport,
) -> ClientResult<Vec<cua_media_protocol::WindowDescriptor>> {
    match wait_for_control(transport, |message| {
        matches!(message, ServerMessage::Windows { .. })
    })
    .await?
    {
        ServerMessage::Windows { windows } => Ok(windows),
        _ => unreachable!(),
    }
}

async fn wait_for_opened(
    transport: &mut ClientTransport,
) -> ClientResult<cua_media_protocol::SessionOpened> {
    match wait_for_control(transport, |message| {
        matches!(message, ServerMessage::SessionOpened(_))
    })
    .await?
    {
        ServerMessage::SessionOpened(opened) => Ok(opened),
        _ => unreachable!(),
    }
}

fn decode_bgra(descriptor: VideoFrameDescriptor, payload: Vec<u8>) -> ClientResult<NativeFrame> {
    if descriptor.codec != VideoCodec::Bgra {
        return Err(format!("native BGRA renderer received {:?}", descriptor.codec).into());
    }
    let pixel_count = usize::try_from(descriptor.width_px)?
        .checked_mul(usize::try_from(descriptor.height_px)?)
        .ok_or("frame dimensions overflow")?;
    if payload.len() != pixel_count.checked_mul(4).ok_or("frame size overflow")? {
        return Err("packed BGRA frame has the wrong payload size".into());
    }
    Ok(NativeFrame {
        width_px: descriptor.width_px,
        height_px: descriptor.height_px,
        data: NativeFrameData::CpuBgra(payload),
        geometry_epoch: descriptor.geometry_epoch,
        sequence: descriptor.sequence,
        received_at: Instant::now(),
    })
}

fn display_title(app_name: &str, title: &str) -> String {
    let title = title.trim();
    if title.is_empty() || title == app_name {
        format!("{app_name} — Remote")
    } else {
        format!("{app_name} — {title} — Remote")
    }
}

fn drain_ui_events(
    receiver: &mpsc::Receiver<UiEvent>,
    latest_frame: &Mutex<Option<NativeFrame>>,
    window: &Window,
    state: &mut AppState,
    target: &winit::event_loop::EventLoopWindowTarget<UiWake>,
    #[cfg(target_os = "macos")] connection_overlay: Option<&ConnectionOverlay>,
) {
    while let Ok(event) = receiver.try_recv() {
        match event {
            UiEvent::Status(status) => {
                if status == "Connecting" {
                    state.connection_phase = ConnectionPhase::Connecting;
                } else if status.starts_with("H.264 recovery") {
                    state.connection_phase = ConnectionPhase::WaitingForVideo;
                }
                state.status = status;
            }
            UiEvent::Opened {
                title,
                geometry,
                geometry_control,
            } => {
                reset_local_input_state(state);
                state.status = "Waiting for remote video".into();
                state.connection_phase = ConnectionPhase::WaitingForVideo;
                state.title.clone_from(&title);
                state.geometry_control = geometry_control;
                window.set_title(&title);
                apply_remote_geometry(window, &geometry, state);
            }
            UiEvent::Geometry(geometry) => {
                let now = Instant::now();
                if state
                    .local_resize_active_until
                    .is_some_and(|deadline| now <= deadline)
                {
                    tracing::debug!(
                        target: "cua_spacesd_client::geometry",
                        width_px = geometry.width_px,
                        height_px = geometry.height_px,
                        "ignored host geometry echo during local resize"
                    );
                } else {
                    state.local_resize_active_until = None;
                    apply_remote_geometry(window, &geometry, state);
                }
            }
            UiEvent::Title(title) => {
                state.title.clone_from(&title);
                window.set_title(&title);
            }
            UiEvent::Suspended(reason) => {
                state.status = format!("Suspended: {reason}");
                state.connection_phase = ConnectionPhase::Reconnecting;
                reset_local_input_state(state);
                window.set_title(&format!("{} — {}", state.title, state.status));
            }
            UiEvent::Resumed => {
                state.status = "Waiting for remote video".into();
                state.connection_phase = ConnectionPhase::WaitingForVideo;
                window.set_title(&format!("{} — {}", state.title, state.status));
            }
            UiEvent::Closed => {
                state.status = "Remote window closed".into();
                state.connection_phase = ConnectionPhase::Closed;
                window.set_title("RCDP — Remote window closed");
            }
            UiEvent::Telemetry(summary) => {
                state.telemetry = Some(summary);
                refresh_window_title(window, state);
            }
            UiEvent::Fatal(message) => {
                state.status = message;
                state.connection_phase = ConnectionPhase::Failed;
                window.set_title(&format!("RCDP — {}", state.status));
            }
        }
    }
    if let Some(frame) = latest_frame
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        state.frame = Some(frame);
        state.frame_dirty = true;
        if !matches!(
            state.connection_phase,
            ConnectionPhase::Reconnecting | ConnectionPhase::Closed | ConnectionPhase::Failed
        ) {
            state.connection_phase = ConnectionPhase::Live;
            state.status = "Live".into();
            refresh_window_title(window, state);
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(connection_overlay) = connection_overlay {
        connection_overlay.update(state.connection_phase, &state.status);
    }
    if matches!(state.status.as_str(), "Remote window closed") {
        target.set_control_flow(ControlFlow::Wait);
    }
}

fn reset_local_input_state(state: &mut AppState) {
    state.last_pointer_press = None;
    state.active_pointer_gesture = None;
    state.interactive_pressed_button = None;
    state.scroll_residual_x = 0.0;
    state.scroll_residual_y = 0.0;
}

fn apply_remote_geometry(window: &Window, geometry: &SurfaceGeometry, state: &mut AppState) {
    if let Some(size) = remote_logical_size(geometry) {
        let expected = size.to_physical(window.scale_factor());
        state.remote_resize_suppression = Some(RemoteResizeSuppression {
            expected,
            deadline: Instant::now() + REMOTE_RESIZE_SUPPRESSION,
        });
        state.pending_resize = None;
        let _ = window.request_inner_size(size);
    }
}

fn refresh_window_title(window: &Window, state: &AppState) {
    match state.telemetry.as_deref() {
        Some(telemetry) if state.status.starts_with("Live") => {
            window.set_title(&format!("{} — {telemetry}", state.title));
        }
        _ => window.set_title(&state.title),
    }
}

fn handle_local_resize(
    window: &Window,
    state: &mut AppState,
    size: PhysicalSize<u32>,
    now: Instant,
) {
    if state.remote_resize_suppression.is_some_and(|suppression| {
        now <= suppression.deadline && physical_sizes_near(size, suppression.expected)
    }) {
        state.remote_resize_suppression = None;
        state.pending_resize = None;
        return;
    }
    state.remote_resize_suppression = None;
    if !state.geometry_control || size.width == 0 || size.height == 0 {
        return;
    }
    let scale = window.scale_factor();
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }
    let pending = schedule_pending_resize(
        state.pending_resize,
        (f64::from(size.width) / scale).round().max(1.0) as u32,
        (f64::from(size.height) / scale).round().max(1.0) as u32,
        now,
    );
    state.local_resize_active_until = Some(now + REMOTE_RESIZE_SUPPRESSION);
    tracing::trace!(
        target: "cua_spacesd_client::geometry",
        width_px = size.width,
        height_px = size.height,
        scale,
        width_points = pending.width_points,
        height_points = pending.height_points,
        "observed local proxy resize"
    );
    state.pending_resize = Some(pending);
}

fn schedule_pending_resize(
    previous: Option<PendingResize>,
    width_points: u32,
    height_points: u32,
    now: Instant,
) -> PendingResize {
    PendingResize {
        width_points,
        height_points,
        deadline: previous.map_or(now + RESIZE_DEBOUNCE, |pending| pending.deadline),
    }
}

fn observe_local_window_size(
    window: &Window,
    state: &mut AppState,
    size: PhysicalSize<u32>,
    now: Instant,
) {
    if state.last_observed_window_size == Some(size) {
        return;
    }
    state.last_observed_window_size = Some(size);
    handle_local_resize(window, state, size, now);
}

fn flush_pending_resize(
    sender: &UnboundedSender<InputCommand>,
    state: &mut AppState,
    now: Instant,
) {
    let Some(pending) = state
        .pending_resize
        .filter(|pending| pending.deadline <= now)
    else {
        return;
    };
    state.pending_resize = None;
    let revision = state.next_resize_revision;
    state.next_resize_revision = state.next_resize_revision.saturating_add(1);
    tracing::debug!(
        target: "cua_spacesd_client::geometry",
        revision,
        width_points = pending.width_points,
        height_points = pending.height_points,
        "queued debounced host window resize"
    );
    let _ = sender.send(InputCommand::Resize {
        revision,
        width_points: pending.width_points,
        height_points: pending.height_points,
    });
}

fn physical_sizes_near(left: PhysicalSize<u32>, right: PhysicalSize<u32>) -> bool {
    left.width.abs_diff(right.width) <= 2 && left.height.abs_diff(right.height) <= 2
}

fn remote_logical_size(geometry: &SurfaceGeometry) -> Option<LogicalSize<f64>> {
    if geometry.width_px == 0
        || geometry.height_px == 0
        || !geometry.scale_factor.is_finite()
        || geometry.scale_factor <= 0.0
    {
        return None;
    }
    Some(LogicalSize::new(
        f64::from(geometry.width_px) / geometry.scale_factor,
        f64::from(geometry.height_px) / geometry.scale_factor,
    ))
}

fn draw(
    window: &Window,
    renderer: &mut Pixels<'static>,
    texture_size: &mut (u32, u32),
    #[cfg(target_os = "macos")] metal_frame_renderer: &mut MetalFrameRenderer,
    state: &mut AppState,
    telemetry: &Mutex<LatencyTelemetry>,
) -> ClientResult<()> {
    let size = native_window_inner_size(window);
    if size.width == 0 || size.height == 0 {
        return Ok(());
    }
    renderer.resize_surface(size.width, size.height)?;
    let Some(frame) = state.frame.as_ref() else {
        renderer.render()?;
        return Ok(());
    };
    let Some(viewport) = Viewport::fit(size, frame.width_px, frame.height_px) else {
        renderer.render()?;
        return Ok(());
    };
    let presenting_new_frame = state.frame_dirty;
    match &frame.data {
        NativeFrameData::CpuBgra(bgra) => {
            let next_texture_size = (frame.width_px, frame.height_px);
            if *texture_size != next_texture_size {
                renderer.resize_buffer(frame.width_px, frame.height_px)?;
                *texture_size = next_texture_size;
                state.frame_dirty = true;
            }
            if state.frame_dirty {
                if renderer.frame().len() != bgra.len() {
                    return Err("GPU texture and decoded BGRA frame sizes differ".into());
                }
                renderer.frame_mut().copy_from_slice(bgra);
                state.frame_dirty = false;
            }
            renderer.render()?;
        }
        #[cfg(target_os = "macos")]
        NativeFrameData::PixelBuffer(pixel_buffer) => {
            if state.frame_dirty {
                metal_frame_renderer.prepare(
                    renderer.device(),
                    pixel_buffer,
                    frame.width_px,
                    frame.height_px,
                )?;
                state.frame_dirty = false;
            }
            renderer.render_with(|encoder, target, _| {
                metal_frame_renderer.render(encoder, target, viewport);
                Ok(())
            })?;
        }
    }
    state.viewport = Some(viewport);
    if presenting_new_frame {
        telemetry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record_presentation(frame.received_at.elapsed());
    }
    Ok(())
}

fn current_pixel_basis(state: &AppState) -> Option<ActionBasis> {
    state.frame.as_ref().map(|frame| ActionBasis::Pixel {
        geometry_epoch: frame.geometry_epoch,
        frame_sequence: frame.sequence,
    })
}

fn current_frame_point(state: &AppState) -> Option<(u32, u32)> {
    let frame = state.frame.as_ref()?;
    state
        .viewport?
        .frame_point(state.cursor?, frame.width_px, frame.height_px)
}

fn current_normalized_frame_point(state: &AppState) -> Option<(f64, f64)> {
    let frame = state.frame.as_ref()?;
    let (x, y) = current_frame_point(state)?;
    Some((
        f64::from(x) / f64::from(frame.width_px.max(1)),
        f64::from(y) / f64::from(frame.height_px.max(1)),
    ))
}

#[cfg(target_os = "macos")]
fn current_event_pointer_position() -> Option<PhysicalPosition<f64>> {
    let main_thread = MainThreadMarker::new()?;
    let event = NSApp(main_thread).currentEvent()?;
    // SAFETY: The winit event callback runs synchronously on AppKit's main
    // thread, and these getters only inspect the NSEvent currently dispatched.
    let event_window = unsafe { event.window(main_thread) }?;
    let content_view = event_window.contentView()?;
    // SAFETY: `locationInWindow` is a value getter on the current NSEvent.
    let window_point = unsafe { event.locationInWindow() };
    let view_point = content_view.convertPoint_fromView(window_point, None);
    let frame = content_view.frame();
    physical_pointer_from_view_coordinates(
        view_point.x,
        view_point.y,
        frame.size.width,
        frame.size.height,
        event_window.backingScaleFactor(),
    )
}

#[cfg(not(target_os = "macos"))]
fn current_event_pointer_position() -> Option<PhysicalPosition<f64>> {
    None
}

#[cfg(target_os = "macos")]
fn current_event_modifiers() -> Option<ModifiersState> {
    let main_thread = MainThreadMarker::new()?;
    let event = NSApp(main_thread).currentEvent()?;
    // SAFETY: The winit callback is synchronously handling this NSEvent on
    // AppKit's main thread, so reading its immutable modifier flags is valid.
    let flags = unsafe { event.modifierFlags() };
    Some(modifiers_state_from_appkit_flags(flags))
}

#[cfg(not(target_os = "macos"))]
fn current_event_modifiers() -> Option<ModifiersState> {
    None
}

#[derive(Debug, Clone, Copy)]
struct PreciseScrollEvent {
    delta_x: f64,
    delta_y: f64,
    phase: InputGesturePhase,
    momentum_phase: InputGesturePhase,
    precise: bool,
}

#[cfg(target_os = "macos")]
fn precise_scroll_event_from_appkit(event: &NSEvent) -> PreciseScrollEvent {
    // SAFETY: Called synchronously from the local monitor while AppKit owns
    // and dispatches this NSEvent.
    unsafe {
        PreciseScrollEvent {
            delta_x: event.scrollingDeltaX(),
            delta_y: event.scrollingDeltaY(),
            phase: input_gesture_phase_from_appkit(event.phase()),
            momentum_phase: input_gesture_phase_from_appkit(event.momentumPhase()),
            precise: event.hasPreciseScrollingDeltas(),
        }
    }
}

#[cfg(target_os = "macos")]
fn input_gesture_phase_from_appkit(phase: NSEventPhase) -> InputGesturePhase {
    if phase.contains(NSEventPhase::Cancelled) {
        InputGesturePhase::Cancelled
    } else if phase.contains(NSEventPhase::Ended) {
        InputGesturePhase::Ended
    } else if phase.contains(NSEventPhase::Changed) || phase.contains(NSEventPhase::Stationary) {
        InputGesturePhase::Changed
    } else if phase.contains(NSEventPhase::Began) {
        InputGesturePhase::Began
    } else if phase.contains(NSEventPhase::MayBegin) {
        InputGesturePhase::MayBegin
    } else {
        InputGesturePhase::None
    }
}

#[cfg(target_os = "macos")]
fn modifiers_state_from_appkit_flags(flags: NSEventModifierFlags) -> ModifiersState {
    let mut modifiers = ModifiersState::empty();
    modifiers.set(
        ModifiersState::SHIFT,
        flags.contains(NSEventModifierFlags::NSEventModifierFlagShift),
    );
    modifiers.set(
        ModifiersState::CONTROL,
        flags.contains(NSEventModifierFlags::NSEventModifierFlagControl),
    );
    modifiers.set(
        ModifiersState::ALT,
        flags.contains(NSEventModifierFlags::NSEventModifierFlagOption),
    );
    modifiers.set(
        ModifiersState::SUPER,
        flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand),
    );
    modifiers
}

#[cfg(target_os = "macos")]
fn send_appkit_command_shortcut(
    sender: &UnboundedSender<InputCommand>,
    event: &NSEvent,
    interactive: bool,
) -> bool {
    // SAFETY: This is called synchronously by the local NSEvent monitor while
    // AppKit owns and dispatches `event` on the main thread.
    let flags = unsafe { event.modifierFlags() };
    if !flags.contains(NSEventModifierFlags::NSEventModifierFlagCommand) {
        return false;
    }
    // SAFETY: These immutable NSEvent getters are valid for the callback's
    // event lifetime.
    let Some(characters) = (unsafe { event.charactersIgnoringModifiers() }) else {
        return false;
    };
    let key = appkit_shortcut_key(&characters.to_string());
    let Some(key) = key else {
        return false;
    };
    // Keep the viewer's conventional lifecycle shortcut local. Command-W and
    // editing shortcuts intentionally go to the shared remote application.
    if key == "q" {
        return false;
    }
    let modifier_state = modifiers_state_from_appkit_flags(flags);
    if interactive {
        let modifiers = input_modifiers(modifier_state);
        // SAFETY: Immutable event metadata is valid for the monitor callback.
        let event_type = unsafe { event.r#type() };
        let state = if event_type == NSEventType::KeyUp {
            InputKeyState::Up
        } else {
            InputKeyState::Down
        };
        let repeat = unsafe { event.isARepeat() };
        let _ = sender.send(InputCommand::Interactive(InteractiveInputEvent::Key {
            key,
            state,
            modifiers,
            repeat,
        }));
    } else {
        // Legacy automation already emits a complete press on key-down.
        if unsafe { event.r#type() } == NSEventType::KeyUp {
            return false;
        }
        let modifiers = modifier_names(modifier_state);
        let _ = sender.send(InputCommand::Action {
            tool: "press_key",
            arguments: serde_json::json!({"key": key, "modifiers": modifiers}),
            basis: ActionBasis::None,
        });
    }
    true
}

#[cfg(target_os = "macos")]
fn appkit_shortcut_key(characters: &str) -> Option<String> {
    let key = match characters {
        "\r" => "return",
        "\t" => "tab",
        "\u{1b}" => "escape",
        "\u{7f}" => "backspace",
        "\u{f700}" => "up",
        "\u{f701}" => "down",
        "\u{f702}" => "left",
        "\u{f703}" => "right",
        "\u{f729}" => "home",
        "\u{f72b}" => "end",
        "\u{f72c}" => "pageup",
        "\u{f72d}" => "pagedown",
        text if text.chars().count() == 1 => return Some(text.to_lowercase()),
        _ => return None,
    };
    Some(key.into())
}

#[cfg(target_os = "macos")]
fn physical_pointer_from_view_coordinates(
    x: f64,
    y_from_bottom: f64,
    width: f64,
    height: f64,
    scale_factor: f64,
) -> Option<PhysicalPosition<f64>> {
    if !x.is_finite()
        || !y_from_bottom.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || !scale_factor.is_finite()
        || width <= 0.0
        || height <= 0.0
        || scale_factor <= 0.0
        || x < 0.0
        || y_from_bottom < 0.0
        || x > width
        || y_from_bottom > height
    {
        return None;
    }
    Some(PhysicalPosition::new(
        x * scale_factor,
        (height - y_from_bottom) * scale_factor,
    ))
}

#[derive(Debug, Clone, Copy)]
struct PointerPress {
    at: Instant,
    button: MouseButton,
    point: (u32, u32),
}

#[derive(Debug, Clone, Copy)]
struct PointerGesture {
    button: MouseButton,
    start: (u32, u32),
    geometry_epoch: GeometryEpoch,
}

const SYNTHETIC_POINTER_DUPLICATE_WINDOW: Duration = Duration::from_millis(8);
const DRAG_THRESHOLD_PX: u32 = 3;
const SCROLL_PIXELS_PER_NOTCH: f64 = 10.0;
const MAX_SCROLL_BATCH: u32 = 8;

fn accept_pointer_press(
    previous: &mut Option<PointerPress>,
    button: MouseButton,
    point: (u32, u32),
    now: Instant,
) -> bool {
    let duplicate = previous.is_some_and(|press| {
        press.button == button
            && press.point == point
            && now.saturating_duration_since(press.at) < SYNTHETIC_POINTER_DUPLICATE_WINDOW
    });
    *previous = Some(PointerPress {
        at: now,
        button,
        point,
    });
    !duplicate
}

fn handle_interactive_pointer_input(
    sender: &UnboundedSender<InputCommand>,
    state: &mut AppState,
    element_state: ElementState,
    button: MouseButton,
) {
    if input_pointer_button(button).is_none() {
        return;
    }
    if element_state == ElementState::Pressed {
        let Some(point) = current_frame_point(state) else {
            return;
        };
        if !accept_pointer_press(&mut state.last_pointer_press, button, point, Instant::now()) {
            return;
        }
        state.interactive_pressed_button = Some(button);
        send_interactive_pointer(sender, state, InputPointerPhase::Down, Some(button));
    } else {
        send_interactive_pointer(sender, state, InputPointerPhase::Up, Some(button));
        if state.interactive_pressed_button == Some(button) {
            state.interactive_pressed_button = None;
        }
    }
}

fn input_key_kind(key: &Key) -> &'static str {
    match key {
        Key::Named(_) => "named",
        Key::Character(_) => "character",
        Key::Unidentified(_) => "unidentified",
        Key::Dead(_) => "dead",
    }
}

fn send_interactive_pointer(
    sender: &UnboundedSender<InputCommand>,
    state: &AppState,
    phase: InputPointerPhase,
    button: Option<MouseButton>,
) {
    let Some(event) = interactive_pointer_event(state, phase, button) else {
        return;
    };
    if let InteractiveInputEvent::Pointer {
        button,
        x_normalized,
        y_normalized,
        ..
    } = &event
    {
        tracing::debug!(
            target: "cua_spacesd_client::input",
            ?phase,
            ?button,
            x_normalized,
            y_normalized,
            "queued client pointer edge"
        );
    }
    let _ = sender.send(InputCommand::Interactive(event));
}

fn send_latest_interactive_pointer(
    sender: &watch::Sender<Option<InteractiveInputEvent>>,
    state: &AppState,
    button: Option<MouseButton>,
) {
    if let Some(event) = interactive_pointer_event(state, InputPointerPhase::Move, button) {
        sender.send_replace(Some(event));
    }
}

fn interactive_pointer_event(
    state: &AppState,
    phase: InputPointerPhase,
    button: Option<MouseButton>,
) -> Option<InteractiveInputEvent> {
    let (x_normalized, y_normalized) = current_normalized_frame_point(state)?;
    let button = match button {
        Some(button) => Some(input_pointer_button(button)?),
        None => None,
    };
    Some(InteractiveInputEvent::Pointer {
        phase,
        button,
        x_normalized,
        y_normalized,
        modifiers: input_modifiers(state.modifiers),
    })
}

fn input_pointer_button(button: MouseButton) -> Option<InputPointerButton> {
    match button {
        MouseButton::Left => Some(InputPointerButton::Left),
        MouseButton::Right => Some(InputPointerButton::Right),
        MouseButton::Middle => Some(InputPointerButton::Middle),
        _ => None,
    }
}

fn handle_pointer_input(
    sender: &UnboundedSender<InputCommand>,
    state: &mut AppState,
    element_state: ElementState,
    button: MouseButton,
) {
    match (element_state, button) {
        (ElementState::Pressed, MouseButton::Left) => begin_pointer_gesture(state, button),
        (ElementState::Released, MouseButton::Left) => {
            finish_pointer_gesture(sender, state, button)
        }
        (ElementState::Pressed, _) => send_click(sender, state, button),
        (ElementState::Released, _) => {}
    }
}

fn begin_pointer_gesture(state: &mut AppState, button: MouseButton) {
    let Some((x, y)) = current_frame_point(state) else {
        return;
    };
    if !accept_pointer_press(
        &mut state.last_pointer_press,
        button,
        (x, y),
        Instant::now(),
    ) {
        return;
    }
    let Some(ActionBasis::Pixel { geometry_epoch, .. }) = current_pixel_basis(state) else {
        return;
    };
    state.active_pointer_gesture = Some(PointerGesture {
        button,
        start: (x, y),
        geometry_epoch,
    });
}

fn finish_pointer_gesture(
    sender: &UnboundedSender<InputCommand>,
    state: &mut AppState,
    button: MouseButton,
) {
    let Some(gesture) = state.active_pointer_gesture.take() else {
        return;
    };
    if gesture.button != button {
        return;
    }
    let Some(end) = current_frame_point(state) else {
        return;
    };
    let Some(basis @ ActionBasis::Pixel { geometry_epoch, .. }) = current_pixel_basis(state) else {
        return;
    };
    if geometry_epoch != gesture.geometry_epoch {
        return;
    }

    let dx = i64::from(end.0) - i64::from(gesture.start.0);
    let dy = i64::from(end.1) - i64::from(gesture.start.1);
    let distance_squared = dx * dx + dy * dy;
    if distance_squared < i64::from(DRAG_THRESHOLD_PX * DRAG_THRESHOLD_PX) {
        send_click_at(sender, gesture.start, button, basis);
        return;
    }

    let distance = (distance_squared as f64).sqrt();
    let duration_ms = (distance * 0.8).round().clamp(60.0, 300.0) as u64;
    let steps = (distance / 8.0).ceil().clamp(4.0, 32.0) as u64;
    let modifiers = modifier_names(state.modifiers);
    let _ = sender.send(InputCommand::Action {
        tool: "drag",
        arguments: serde_json::json!({
            "from_x": gesture.start.0,
            "from_y": gesture.start.1,
            "to_x": end.0,
            "to_y": end.1,
            "button": "left",
            "duration_ms": duration_ms,
            "steps": steps,
            "modifier": modifiers,
        }),
        basis,
    });
}

fn send_click(sender: &UnboundedSender<InputCommand>, state: &mut AppState, button: MouseButton) {
    let Some(point) = current_frame_point(state) else {
        return;
    };
    if !accept_pointer_press(&mut state.last_pointer_press, button, point, Instant::now()) {
        return;
    }
    let Some(basis) = current_pixel_basis(state) else {
        return;
    };
    send_click_at(sender, point, button, basis);
}

fn send_click_at(
    sender: &UnboundedSender<InputCommand>,
    (x, y): (u32, u32),
    button: MouseButton,
    basis: ActionBasis,
) {
    let button = match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        _ => return,
    };
    let _ = sender.send(InputCommand::Action {
        tool: "click",
        arguments: serde_json::json!({"x": x, "y": y, "button": button, "count": 1}),
        basis,
    });
}

fn send_interactive_scroll(
    sender: &InteractiveScrollSender,
    state: &mut AppState,
    delta: MouseScrollDelta,
    native_scroll: Option<PreciseScrollEvent>,
) {
    if let Some(position) = current_event_pointer_position() {
        state.cursor = Some(position);
    }
    let Some((x_normalized, y_normalized)) = current_normalized_frame_point(state) else {
        return;
    };
    let scroll = native_scroll.unwrap_or_else(|| match delta {
        MouseScrollDelta::LineDelta(x, y) => PreciseScrollEvent {
            delta_x: f64::from(x),
            delta_y: f64::from(y),
            phase: InputGesturePhase::None,
            momentum_phase: InputGesturePhase::None,
            precise: false,
        },
        MouseScrollDelta::PixelDelta(position) => PreciseScrollEvent {
            delta_x: position.x,
            delta_y: position.y,
            phase: InputGesturePhase::None,
            momentum_phase: InputGesturePhase::None,
            precise: true,
        },
    });
    tracing::debug!(
        target: "cua_spacesd_client::input",
        delta_x = scroll.delta_x,
        delta_y = scroll.delta_y,
        ?scroll.phase,
        ?scroll.momentum_phase,
        precise = scroll.precise,
        x_normalized,
        y_normalized,
        "queued client scroll event"
    );
    sender.send(InteractiveInputEvent::Scroll {
        x_normalized,
        y_normalized,
        delta_x: scroll.delta_x,
        delta_y: scroll.delta_y,
        phase: scroll.phase,
        momentum_phase: scroll.momentum_phase,
        precise: scroll.precise,
    });
}

fn send_scroll(
    sender: &UnboundedSender<ScrollCommand>,
    state: &mut AppState,
    delta: MouseScrollDelta,
) {
    let Some((x, y)) = current_frame_point(state) else {
        return;
    };
    let Some(basis) = current_pixel_basis(state) else {
        return;
    };
    let (horizontal, vertical) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (f64::from(x), f64::from(y)),
        MouseScrollDelta::PixelDelta(position) => (
            position.x / SCROLL_PIXELS_PER_NOTCH,
            position.y / SCROLL_PIXELS_PER_NOTCH,
        ),
    };
    let (direction, residual, delta) = if vertical.abs() >= horizontal.abs() {
        (
            if vertical > 0.0 { "up" } else { "down" },
            &mut state.scroll_residual_y,
            vertical,
        )
    } else {
        (
            if horizontal > 0.0 { "left" } else { "right" },
            &mut state.scroll_residual_x,
            horizontal,
        )
    };
    *residual += delta;
    let amount = residual.abs().floor() as u32;
    if amount == 0 {
        return;
    }
    *residual -= residual.signum() * f64::from(amount);
    let _ = sender.send(ScrollCommand {
        x,
        y,
        direction,
        amount: amount.min(MAX_SCROLL_BATCH),
        basis,
    });
}

fn modifier_names(modifiers: ModifiersState) -> Vec<&'static str> {
    let mut names = Vec::new();
    if modifiers.control_key() {
        names.push("ctrl");
    }
    if modifiers.alt_key() {
        names.push("alt");
    }
    if modifiers.shift_key() {
        names.push("shift");
    }
    if modifiers.super_key() {
        names.push(native_super_modifier_name());
    }
    names
}

fn input_modifiers(modifiers: ModifiersState) -> Vec<InputModifier> {
    let mut result = Vec::new();
    if modifiers.control_key() {
        result.push(InputModifier::Control);
    }
    if modifiers.alt_key() {
        result.push(InputModifier::Option);
    }
    if modifiers.shift_key() {
        result.push(InputModifier::Shift);
    }
    if modifiers.super_key() {
        result.push(InputModifier::Command);
    }
    result
}

fn native_super_modifier_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "win"
    } else {
        "cmd"
    }
}

fn send_key(sender: &UnboundedSender<InputCommand>, state: &AppState, key: &Key) -> bool {
    let modifiers = modifier_names(state.modifiers);
    let key_name = match key {
        Key::Named(NamedKey::Enter) => Some("enter"),
        Key::Named(NamedKey::Backspace) => Some("backspace"),
        Key::Named(NamedKey::Delete) => Some("delete"),
        Key::Named(NamedKey::Tab) => Some("tab"),
        Key::Named(NamedKey::Escape) => Some("escape"),
        Key::Named(NamedKey::ArrowUp) => Some("up"),
        Key::Named(NamedKey::ArrowDown) => Some("down"),
        Key::Named(NamedKey::ArrowLeft) => Some("left"),
        Key::Named(NamedKey::ArrowRight) => Some("right"),
        Key::Named(NamedKey::Home) => Some("home"),
        Key::Named(NamedKey::End) => Some("end"),
        Key::Named(NamedKey::PageUp) => Some("pageup"),
        Key::Named(NamedKey::PageDown) => Some("pagedown"),
        Key::Character(text) if state.modifiers.control_key() || state.modifiers.super_key() => {
            Some(text.as_str())
        }
        _ => None,
    };
    let Some(key) = key_name else {
        return false;
    };
    let _ = sender.send(InputCommand::Action {
        tool: "press_key",
        arguments: serde_json::json!({"key": key, "modifiers": modifiers}),
        basis: ActionBasis::None,
    });
    true
}

fn send_interactive_keyboard(
    sender: &UnboundedSender<InputCommand>,
    state: &AppState,
    key: &Key,
    element_state: ElementState,
    repeat: bool,
    text: Option<&str>,
) {
    let key_name = match key {
        Key::Named(NamedKey::Enter) => Some("enter"),
        Key::Named(NamedKey::Backspace) => Some("backspace"),
        Key::Named(NamedKey::Delete) => Some("delete"),
        Key::Named(NamedKey::Tab) => Some("tab"),
        Key::Named(NamedKey::Escape) => Some("escape"),
        Key::Named(NamedKey::ArrowUp) => Some("up"),
        Key::Named(NamedKey::ArrowDown) => Some("down"),
        Key::Named(NamedKey::ArrowLeft) => Some("left"),
        Key::Named(NamedKey::ArrowRight) => Some("right"),
        Key::Named(NamedKey::Home) => Some("home"),
        Key::Named(NamedKey::End) => Some("end"),
        Key::Named(NamedKey::PageUp) => Some("pageup"),
        Key::Named(NamedKey::PageDown) => Some("pagedown"),
        Key::Named(NamedKey::Shift) => Some("shift"),
        Key::Named(NamedKey::Control) => Some("control"),
        Key::Named(NamedKey::Alt) => Some("option"),
        Key::Named(NamedKey::Super) => Some("command"),
        Key::Named(NamedKey::CapsLock) => Some("capslock"),
        Key::Named(NamedKey::F1) => Some("f1"),
        Key::Named(NamedKey::F2) => Some("f2"),
        Key::Named(NamedKey::F3) => Some("f3"),
        Key::Named(NamedKey::F4) => Some("f4"),
        Key::Named(NamedKey::F5) => Some("f5"),
        Key::Named(NamedKey::F6) => Some("f6"),
        Key::Named(NamedKey::F7) => Some("f7"),
        Key::Named(NamedKey::F8) => Some("f8"),
        Key::Named(NamedKey::F9) => Some("f9"),
        Key::Named(NamedKey::F10) => Some("f10"),
        Key::Named(NamedKey::F11) => Some("f11"),
        Key::Named(NamedKey::F12) => Some("f12"),
        Key::Character(value) if state.modifiers.control_key() || state.modifiers.super_key() => {
            Some(value.as_str())
        }
        _ => None,
    };
    if let Some(key_name) = key_name {
        let key_state = match element_state {
            ElementState::Pressed => InputKeyState::Down,
            ElementState::Released => InputKeyState::Up,
        };
        tracing::debug!(
            target: "cua_spacesd_client::input",
            key_kind = if matches!(key, Key::Character(_)) {
                "shortcut_character"
            } else {
                "named"
            },
            ?key_state,
            repeat,
            modifier_count = input_modifiers(state.modifiers).len(),
            "queued client key event"
        );
        let _ = sender.send(InputCommand::Interactive(InteractiveInputEvent::Key {
            key: key_name.to_lowercase(),
            state: key_state,
            modifiers: input_modifiers(state.modifiers),
            repeat,
        }));
    } else if element_state == ElementState::Pressed {
        if let Some(text) = text {
            send_interactive_text(sender, text);
        }
    }
}

fn send_interactive_text(sender: &UnboundedSender<InputCommand>, text: &str) {
    if text.is_empty() || text.chars().all(char::is_control) {
        return;
    }
    tracing::debug!(
        target: "cua_spacesd_client::input",
        text_bytes = text.len(),
        text_chars = text.chars().count(),
        "queued client text commit"
    );
    let _ = sender.send(InputCommand::Interactive(
        InteractiveInputEvent::TextCommit { text: text.into() },
    ));
}

fn interactive_event_summary(events: &[InteractiveInputEvent]) -> String {
    let mut text = 0usize;
    let mut key = 0usize;
    let mut pointer_move = 0usize;
    let mut pointer_edge = 0usize;
    let mut scroll = 0usize;
    for event in events {
        match event {
            InteractiveInputEvent::TextCommit { .. } => text += 1,
            InteractiveInputEvent::Key { .. } => key += 1,
            InteractiveInputEvent::Pointer {
                phase: InputPointerPhase::Move,
                ..
            } => pointer_move += 1,
            InteractiveInputEvent::Pointer { .. } => pointer_edge += 1,
            InteractiveInputEvent::Scroll { .. } => scroll += 1,
        }
    }
    format!(
        "text={text},key={key},pointer_move={pointer_move},pointer_edge={pointer_edge},scroll={scroll}"
    )
}

fn send_text(sender: &UnboundedSender<InputCommand>, text: &str) {
    if text.is_empty() || text.chars().all(char::is_control) {
        return;
    }
    let _ = sender.send(InputCommand::Text(text.into()));
}

fn coalesce_text_input(
    mut text: String,
    receiver: &mut UnboundedReceiver<InputCommand>,
    deferred: &mut Option<InputCommand>,
) -> String {
    while let Ok(next) = receiver.try_recv() {
        match next {
            InputCommand::Text(next_text) => text.push_str(&next_text),
            command => {
                *deferred = Some(command);
                break;
            }
        }
    }
    text
}

fn latest_scroll_command(
    mut command: ScrollCommand,
    receiver: &mut UnboundedReceiver<ScrollCommand>,
) -> ScrollCommand {
    while let Ok(next) = receiver.try_recv() {
        command = next;
    }
    command
}

fn coalesce_interactive_input(
    first: InteractiveInputEvent,
    receiver: &mut UnboundedReceiver<InputCommand>,
    deferred: &mut Option<InputCommand>,
) -> Vec<InteractiveInputEvent> {
    let mut events = vec![first];
    while let Ok(next) = receiver.try_recv() {
        match next {
            InputCommand::Interactive(event) => {
                if coalesce_latest_interactive_sample(&mut events, event.clone()) {
                    continue;
                }
                if events.len() >= MAX_CLIENT_INPUT_BATCH_EVENTS {
                    *deferred = Some(InputCommand::Interactive(event));
                    break;
                }
                events.push(event);
            }
            command => {
                *deferred = Some(command);
                break;
            }
        }
    }
    events
}

fn coalesce_latest_interactive_sample(
    events: &mut [InteractiveInputEvent],
    next: InteractiveInputEvent,
) -> bool {
    let Some(previous) = events.last_mut() else {
        return false;
    };
    match next {
        InteractiveInputEvent::TextCommit { text } => {
            let InteractiveInputEvent::TextCommit { text: previous } = previous else {
                return false;
            };
            previous.push_str(&text);
            true
        }
        next @ InteractiveInputEvent::Pointer {
            phase: InputPointerPhase::Move,
            ..
        } if matches!(
            previous,
            InteractiveInputEvent::Pointer {
                phase: InputPointerPhase::Move,
                ..
            }
        ) =>
        {
            *previous = next;
            true
        }
        InteractiveInputEvent::Scroll {
            x_normalized,
            y_normalized,
            delta_x,
            delta_y,
            phase: next_phase,
            momentum_phase: next_momentum_phase,
            precise,
        } => {
            let InteractiveInputEvent::Scroll {
                x_normalized: previous_x,
                y_normalized: previous_y,
                delta_x: previous_delta_x,
                delta_y: previous_delta_y,
                phase: previous_phase,
                momentum_phase: previous_momentum_phase,
                precise: previous_precise,
            } = previous
            else {
                return false;
            };
            if *previous_precise != precise
                || !replaceable_scroll_sample(*previous_phase, *previous_momentum_phase)
                || !replaceable_scroll_sample(next_phase, next_momentum_phase)
            {
                return false;
            }

            *previous_x = x_normalized;
            *previous_y = y_normalized;
            // Scroll deltas are incremental displacement, not replaceable
            // velocity samples. Preserve the full gesture distance when a
            // congested link forces adjacent samples to share one event.
            *previous_delta_x += delta_x;
            *previous_delta_y += delta_y;
            *previous_phase = next_phase;
            *previous_momentum_phase = next_momentum_phase;
            true
        }
        _ => false,
    }
}

fn replaceable_scroll_sample(phase: InputGesturePhase, momentum_phase: InputGesturePhase) -> bool {
    matches!(phase, InputGesturePhase::None | InputGesturePhase::Changed)
        && matches!(
            momentum_phase,
            InputGesturePhase::None | InputGesturePhase::Changed
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fallback_connection_endpoint() {
        let endpoint = connection_endpoint_from_json(&serde_json::json!({
            "url": "ws://100.109.252.43:7543",
            "token": "secret",
            "quic_cert_sha256": "unused-for-websocket"
        }))
        .unwrap();
        assert_eq!(endpoint.url, "ws://100.109.252.43:7543");
        assert_eq!(endpoint.token.as_deref(), Some("secret"));
        assert_eq!(
            endpoint.quic_cert_sha256.as_deref(),
            Some("unused-for-websocket")
        );
    }

    #[test]
    fn rejects_invalid_fallback_connection_endpoint() {
        let missing_url = connection_endpoint_from_json(&serde_json::json!({
            "token": "secret"
        }));
        assert!(missing_url.is_err());

        // http(s) selects wire v2 (see `connection_endpoint_from_json`), so
        // use a scheme the viewer genuinely cannot speak.
        let unsupported_scheme = connection_endpoint_from_json(&serde_json::json!({
            "url": "ftp://100.109.252.43:7543"
        }));
        assert!(unsupported_scheme.is_err());
    }

    #[test]
    fn reconnect_backoff_is_fast_then_bounded() {
        let mut backoff = ReconnectBackoff::default();
        assert_eq!(backoff.next_delay(), Duration::from_millis(250));
        assert_eq!(backoff.next_delay(), Duration::from_millis(500));
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
        assert_eq!(backoff.next_delay(), Duration::from_secs(2));
        assert_eq!(backoff.next_delay(), Duration::from_secs(4));
        assert_eq!(backoff.next_delay(), RECONNECT_MAX_DELAY);
        assert_eq!(backoff.next_delay(), RECONNECT_MAX_DELAY);
        backoff.reset();
        assert_eq!(backoff.next_delay(), RECONNECT_INITIAL_DELAY);
    }

    #[test]
    fn video_stall_watchdog_probes_static_windows_before_reconnecting() {
        let start = Instant::now();
        let probe_at = start + SESSION_VIDEO_IDLE_PROBE_AFTER;
        assert_eq!(
            video_health_action(start, None, false, probe_at - Duration::from_millis(1)),
            VideoHealthAction::None
        );
        assert_eq!(
            video_health_action(start, None, false, probe_at),
            VideoHealthAction::RequestKeyframe
        );
        assert_eq!(
            video_health_action(
                start,
                Some(probe_at),
                false,
                probe_at + SESSION_VIDEO_PROBE_TIMEOUT - Duration::from_millis(1)
            ),
            VideoHealthAction::None
        );
        assert_eq!(
            video_health_action(
                start,
                Some(probe_at),
                false,
                probe_at + SESSION_VIDEO_PROBE_TIMEOUT
            ),
            VideoHealthAction::Stall
        );
        assert_eq!(
            video_health_action(
                start,
                Some(probe_at),
                true,
                probe_at + SESSION_VIDEO_PROBE_TIMEOUT
            ),
            VideoHealthAction::None
        );
    }

    #[test]
    fn connection_overlay_only_disappears_for_live_video() {
        assert_eq!(
            connection_overlay_text(ConnectionPhase::Connecting, "Connecting").as_deref(),
            Some("Connecting to remote Mac…")
        );
        assert_eq!(
            connection_overlay_text(ConnectionPhase::Reconnecting, "details").as_deref(),
            Some("Connection interrupted · reconnecting…")
        );
        assert_eq!(
            connection_overlay_text(ConnectionPhase::WaitingForVideo, "details").as_deref(),
            Some("Waiting for remote video…")
        );
        assert_eq!(connection_overlay_text(ConnectionPhase::Live, "Live"), None);
        assert_eq!(
            connection_overlay_text(ConnectionPhase::Failed, "remote refused connection")
                .as_deref(),
            Some("remote refused connection")
        );
    }

    #[test]
    fn reconnect_discards_stale_session_input_and_preserves_close_intent() {
        let (input_tx, input_rx) = unbounded_channel();
        let (pointer_move_tx, pointer_move_rx) = watch::channel(None);
        let (interactive_scroll_tx, interactive_scroll_rx) = interactive_scroll_channel();
        let (scroll_tx, scroll_rx) = unbounded_channel();
        input_tx.send(InputCommand::Text("stale".into())).unwrap();
        input_tx.send(InputCommand::Close).unwrap();
        pointer_move_tx.send_replace(Some(InteractiveInputEvent::Pointer {
            x_normalized: 0.5,
            y_normalized: 0.5,
            phase: InputPointerPhase::Move,
            button: None,
            modifiers: Vec::new(),
        }));
        interactive_scroll_tx.send(InteractiveInputEvent::Scroll {
            x_normalized: 0.5,
            y_normalized: 0.5,
            delta_x: 0.0,
            delta_y: 4.0,
            phase: InputGesturePhase::Changed,
            momentum_phase: InputGesturePhase::None,
            precise: true,
        });
        scroll_tx
            .send(ScrollCommand {
                x: 10,
                y: 20,
                direction: "down",
                amount: 3,
                basis: ActionBasis::None,
            })
            .unwrap();
        let mut receivers = NetworkReceivers {
            input_rx,
            pointer_move_rx,
            interactive_scroll_rx,
            scroll_rx,
        };

        assert!(discard_stale_session_input(&mut receivers));
        assert!(receivers.input_rx.try_recv().is_err());
        assert!(!receivers.pointer_move_rx.has_changed().unwrap());
        assert_eq!(receivers.interactive_scroll_rx.len(), 0);
        assert!(receivers.scroll_rx.try_recv().is_err());
    }

    fn interactive_state(cursor: (f64, f64)) -> AppState {
        AppState {
            frame: Some(NativeFrame {
                width_px: 100,
                height_px: 100,
                data: NativeFrameData::CpuBgra(vec![0; 100 * 100 * 4]),
                geometry_epoch: GeometryEpoch(3),
                sequence: FrameSequence(9),
                received_at: Instant::now(),
            }),
            viewport: Some(Viewport {
                left: 0,
                top: 0,
                width: 100,
                height: 100,
            }),
            cursor: Some(PhysicalPosition::new(cursor.0, cursor.1)),
            ..AppState::default()
        }
    }

    #[test]
    fn viewport_maps_letterboxed_native_pixels_to_frame_pixels() {
        let viewport = Viewport::fit(PhysicalSize::new(1000, 1000), 1280, 640).unwrap();
        assert_eq!(
            (viewport.left, viewport.top, viewport.width, viewport.height),
            (0, 250, 1000, 500)
        );
        assert_eq!(
            viewport.frame_point(PhysicalPosition::new(500.0, 500.0), 1280, 640),
            Some((640, 320))
        );
        assert_eq!(
            viewport.frame_point(PhysicalPosition::new(500.0, 100.0), 1280, 640),
            None
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn appkit_mouse_down_position_uses_top_left_backing_pixels() {
        assert_eq!(
            physical_pointer_from_view_coordinates(30.0, 440.0, 800.0, 500.0, 2.0),
            Some(PhysicalPosition::new(60.0, 120.0))
        );
        assert_eq!(
            physical_pointer_from_view_coordinates(-1.0, 440.0, 800.0, 500.0, 2.0),
            None
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn appkit_key_event_flags_recover_missing_modifier_change_events() {
        let flags = NSEventModifierFlags::NSEventModifierFlagCommand
            | NSEventModifierFlags::NSEventModifierFlagShift;
        let modifiers = modifiers_state_from_appkit_flags(flags);
        assert!(modifiers.super_key());
        assert!(modifiers.shift_key());
        assert!(!modifiers.alt_key());
        assert!(!modifiers.control_key());
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn appkit_command_monitor_maps_editing_and_navigation_shortcuts() {
        assert_eq!(appkit_shortcut_key("A").as_deref(), Some("a"));
        assert_eq!(appkit_shortcut_key("\u{f702}").as_deref(), Some("left"));
        assert_eq!(appkit_shortcut_key("multiple"), None);
    }

    #[test]
    fn remote_geometry_uses_host_points_instead_of_client_backing_scale() {
        let size = remote_logical_size(&SurfaceGeometry {
            width_px: 1280,
            height_px: 640,
            scale_factor: 4.0 / 3.0,
        })
        .unwrap();
        assert_eq!(size, LogicalSize::new(960.0, 480.0));
        assert!(remote_logical_size(&SurfaceGeometry {
            width_px: 1280,
            height_px: 640,
            scale_factor: 0.0,
        })
        .is_none());
    }

    #[test]
    fn debounced_local_resize_emits_one_revisioned_host_request() {
        let (sender, mut receiver) = unbounded_channel();
        let start = Instant::now();
        let mut state = AppState {
            geometry_control: true,
            pending_resize: Some(PendingResize {
                width_points: 900,
                height_points: 700,
                deadline: start + RESIZE_DEBOUNCE,
            }),
            ..AppState::default()
        };
        flush_pending_resize(&sender, &mut state, start + RESIZE_DEBOUNCE);
        assert!(matches!(
            receiver.try_recv(),
            Ok(InputCommand::Resize {
                revision: 1,
                width_points: 900,
                height_points: 700,
            })
        ));
        flush_pending_resize(&sender, &mut state, start + RESIZE_DEBOUNCE * 2);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn live_resize_throttle_keeps_deadline_and_latest_dimensions() {
        let start = Instant::now();
        let first = schedule_pending_resize(None, 900, 700, start);
        let latest =
            schedule_pending_resize(Some(first), 1_200, 800, start + Duration::from_millis(50));

        assert_eq!(latest.width_points, 1_200);
        assert_eq!(latest.height_points, 800);
        assert_eq!(latest.deadline, start + RESIZE_DEBOUNCE);
    }

    #[test]
    fn remote_resize_suppression_tolerates_backing_rounding_only() {
        assert!(physical_sizes_near(
            PhysicalSize::new(1800, 1400),
            PhysicalSize::new(1802, 1399)
        ));
        assert!(!physical_sizes_near(
            PhysicalSize::new(1800, 1400),
            PhysicalSize::new(1810, 1400)
        ));
    }

    #[test]
    fn adaptive_preferences_downshift_on_replacement_and_recover_slowly() {
        let start = Instant::now();
        let mut preferences = AdaptivePreferences::new(30, 1_920, 8_000, start);
        assert_eq!(
            preferences.observe_frame(true, Duration::from_millis(2), None, start),
            None
        );
        assert_eq!(
            preferences.observe_frame(
                true,
                Duration::from_millis(2),
                None,
                start + Duration::from_millis(100),
            ),
            None
        );
        assert_eq!(
            preferences.observe_frame(
                false,
                Duration::from_millis(2),
                None,
                start + Duration::from_millis(200),
            ),
            None
        );
        assert_eq!(
            preferences.observe_frame(
                false,
                Duration::from_millis(2),
                None,
                start + ADAPTATION_INTERVAL,
            ),
            Some(StreamTuning {
                max_fps: 24,
                max_dimension: 1_728,
                target_bitrate_kbps: 6_000,
            })
        );

        let mut boundary = start + ADAPTATION_INTERVAL;
        for stable_window in 1..=RECOVERY_WINDOWS {
            for offset in 1..=2 {
                assert_eq!(
                    preferences.observe_frame(
                        false,
                        Duration::from_millis(2),
                        None,
                        boundary + Duration::from_millis(offset),
                    ),
                    None
                );
            }
            boundary += ADAPTATION_INTERVAL;
            let update = preferences.observe_frame(false, Duration::from_millis(2), None, boundary);
            if stable_window < RECOVERY_WINDOWS {
                assert_eq!(update, None);
            } else {
                assert_eq!(
                    update,
                    Some(StreamTuning {
                        max_fps: 29,
                        max_dimension: 1_920,
                        target_bitrate_kbps: 6_800,
                    })
                );
            }
        }
    }

    #[test]
    fn adaptive_preferences_reduce_bitrate_first_for_network_loss() {
        let start = Instant::now();
        let mut preferences = AdaptivePreferences::new(60, 1_920, 8_000, start);
        assert_eq!(
            preferences.observe_network_loss(
                1,
                Some(Duration::from_millis(20)),
                start + ADAPTATION_INTERVAL,
            ),
            Some(StreamTuning {
                max_fps: 60,
                max_dimension: 1_920,
                target_bitrate_kbps: 6_000,
            })
        );
        assert_eq!(
            preferences.observe_network_loss(
                1,
                Some(Duration::from_millis(50)),
                start + ADAPTATION_INTERVAL * 2,
            ),
            Some(StreamTuning {
                max_fps: 48,
                max_dimension: 1_728,
                target_bitrate_kbps: 4_500,
            })
        );
    }

    #[test]
    fn latency_telemetry_reports_interval_rates_and_resets_samples() {
        let start = Instant::now();
        let mut telemetry = LatencyTelemetry::new(start);
        telemetry.record_frame(125_000, Some(1_000), Duration::from_millis(2), true);
        telemetry.record_frame(125_000, Some(3_000), Duration::from_millis(4), false);
        telemetry.record_presentation(Duration::from_millis(8));
        telemetry.record_action(Duration::from_millis(120));
        telemetry.record_control_rtt(Duration::from_millis(130));
        telemetry.record_server_replacements(5);
        telemetry.record_server_replacements(7);
        telemetry.record_network_loss(3);

        let summary = telemetry.summary_and_reset(start + Duration::from_secs(1));

        assert!(summary.title.contains("2 fps"));
        assert!(summary.title.contains("2.0 Mbps"));
        assert!(summary.title.contains("RTT 130 ms"));
        assert!(summary.title.contains("encode 2.0 ms"));
        assert!(summary.title.contains("decode 3.0 ms"));
        assert!(summary.title.contains("present 8.0 ms"));
        assert!(summary.title.contains("input 120 ms"));
        assert!(summary.title.contains("drops 1/2"));
        assert!(summary.title.contains("net 3"));
        assert_eq!(summary.metric.input_ack_p95_ms, Some(120.0));
        assert_eq!(telemetry.frames_received, 0);
        assert_eq!(telemetry.frames_replaced, 0);
        assert_eq!(telemetry.last_server_frames_replaced, Some(7));
    }

    #[test]
    fn input_to_present_ignores_hover_and_unrelated_late_frames() {
        let start = Instant::now();
        let mut telemetry = LatencyTelemetry::new(start);
        telemetry.record_input_captured(1, false, start);
        assert!(telemetry.pending_input_started.is_none());

        telemetry.record_input_sent(1, 0, 1, 0, false);
        assert!(telemetry.pending_input_started.is_none());

        telemetry.record_input_sent(1, 0, 1, 0, true);
        assert!(telemetry.pending_input_started.is_some());
        telemetry.pending_input_started = Some(Instant::now() - MAX_INPUT_TO_PRESENT_SAMPLE * 2);
        telemetry.record_presentation(Duration::from_millis(1));
        assert!(telemetry.input_to_present_samples_ms.is_empty());
    }

    #[test]
    fn bgra_decode_preserves_gpu_texture_bytes_and_validates_length() {
        let descriptor = VideoFrameDescriptor {
            session_id: cua_media_protocol::WindowSessionId("session".into()),
            sequence: FrameSequence(7),
            geometry_epoch: GeometryEpoch(2),
            codec_epoch: cua_media_protocol::CodecEpoch(1),
            width_px: 2,
            height_px: 1,
            capture_timestamp_us: 1,
            encode_duration_us: None,
            codec: VideoCodec::Bgra,
            keyframe: true,
        };
        let frame = decode_bgra(
            descriptor.clone(),
            vec![0x33, 0x22, 0x11, 0xff, 0xcc, 0xbb, 0xaa, 0xff],
        )
        .unwrap();
        #[cfg(target_os = "macos")]
        let bgra = match frame.data {
            NativeFrameData::CpuBgra(bgra) => bgra,
            NativeFrameData::PixelBuffer(_) => {
                panic!("raw BGRA unexpectedly decoded into a native surface")
            }
        };
        #[cfg(not(target_os = "macos"))]
        let NativeFrameData::CpuBgra(bgra) = frame.data;
        assert_eq!(bgra, vec![0x33, 0x22, 0x11, 0xff, 0xcc, 0xbb, 0xaa, 0xff]);
        assert!(decode_bgra(descriptor, vec![0; 4]).is_err());
    }

    #[test]
    fn native_client_requests_allow_activation() {
        assert_eq!(NATIVE_SESSION_POLICY, SessionPolicy::AllowActivation);
    }

    #[test]
    fn modified_character_becomes_press_key() {
        let (sender, mut receiver) = unbounded_channel();
        let state = AppState {
            modifiers: ModifiersState::SUPER,
            ..AppState::default()
        };

        send_key(&sender, &state, &Key::Character("c".into()));

        let InputCommand::Action {
            tool,
            arguments,
            basis,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected an action");
        };
        assert_eq!(tool, "press_key");
        assert_eq!(
            arguments,
            serde_json::json!({"key": "c", "modifiers": [native_super_modifier_name()]})
        );
        assert_eq!(basis, ActionBasis::None);
    }

    #[test]
    fn plain_character_uses_text_lane_instead_of_press_key() {
        let (sender, mut receiver) = unbounded_channel();
        assert!(!send_key(
            &sender,
            &AppState::default(),
            &Key::Character("a".into())
        ));
        assert!(receiver.try_recv().is_err());

        send_text(&sender, "a");
        let InputCommand::Text(text) = receiver.try_recv().unwrap() else {
            panic!("expected a text action");
        };
        assert_eq!(text, "a");
    }

    #[test]
    fn interactive_selection_key_preserves_down_up_and_modifiers() {
        let (sender, mut receiver) = unbounded_channel();
        let state = AppState {
            modifiers: ModifiersState::SHIFT,
            ..AppState::default()
        };
        send_interactive_keyboard(
            &sender,
            &state,
            &Key::Named(NamedKey::ArrowLeft),
            ElementState::Pressed,
            false,
            None,
        );
        send_interactive_keyboard(
            &sender,
            &state,
            &Key::Named(NamedKey::ArrowLeft),
            ElementState::Released,
            false,
            None,
        );
        for expected_state in [InputKeyState::Down, InputKeyState::Up] {
            let InputCommand::Interactive(InteractiveInputEvent::Key {
                key,
                state,
                modifiers,
                ..
            }) = receiver.try_recv().unwrap()
            else {
                panic!("expected an interactive key event");
            };
            assert_eq!(key, "left");
            assert_eq!(state, expected_state);
            assert_eq!(modifiers, vec![InputModifier::Shift]);
        }
    }

    #[test]
    fn interactive_pointer_streams_live_drag_samples() {
        let (sender, mut receiver) = unbounded_channel();
        let mut state = interactive_state((10.0, 20.0));
        handle_interactive_pointer_input(
            &sender,
            &mut state,
            ElementState::Pressed,
            MouseButton::Left,
        );
        state.cursor = Some(PhysicalPosition::new(40.0, 50.0));
        send_interactive_pointer(
            &sender,
            &state,
            InputPointerPhase::Move,
            state.interactive_pressed_button,
        );
        handle_interactive_pointer_input(
            &sender,
            &mut state,
            ElementState::Released,
            MouseButton::Left,
        );

        let phases = [
            InputPointerPhase::Down,
            InputPointerPhase::Move,
            InputPointerPhase::Up,
        ];
        for expected in phases {
            let InputCommand::Interactive(InteractiveInputEvent::Pointer { phase, .. }) =
                receiver.try_recv().unwrap()
            else {
                panic!("expected an interactive pointer event");
            };
            assert_eq!(phase, expected);
        }
    }

    #[test]
    fn latest_pointer_lane_replaces_stale_motion() {
        let (sender, mut receiver) = watch::channel(None);
        let mut state = interactive_state((10.0, 20.0));
        send_latest_interactive_pointer(&sender, &state, None);
        state.cursor = Some(PhysicalPosition::new(80.0, 90.0));
        send_latest_interactive_pointer(&sender, &state, Some(MouseButton::Left));

        assert!(receiver.has_changed().unwrap());
        let Some(InteractiveInputEvent::Pointer {
            phase,
            button,
            x_normalized,
            y_normalized,
            ..
        }) = receiver.borrow_and_update().clone()
        else {
            panic!("expected the latest pointer motion");
        };
        assert_eq!(phase, InputPointerPhase::Move);
        assert_eq!(button, Some(InputPointerButton::Left));
        assert_eq!((x_normalized, y_normalized), (0.8, 0.9));
    }

    #[tokio::test]
    async fn lossless_pointer_edge_precedes_latest_motion() {
        let (sender, mut receiver) = unbounded_channel();
        let (pointer_sender, mut pointer_receiver) = watch::channel(None);
        let (_scroll_sender, mut scroll_receiver) = interactive_scroll_channel();
        let mut state = interactive_state((10.0, 20.0));
        send_latest_interactive_pointer(&pointer_sender, &state, None);
        pointer_sender.send_replace(None);
        handle_interactive_pointer_input(
            &sender,
            &mut state,
            ElementState::Pressed,
            MouseButton::Left,
        );
        state.cursor = Some(PhysicalPosition::new(40.0, 50.0));
        send_latest_interactive_pointer(&pointer_sender, &state, state.interactive_pressed_button);

        let mut deferred = None;
        assert!(matches!(
            next_input_command(
                &mut deferred,
                &mut receiver,
                &mut pointer_receiver,
                &mut scroll_receiver,
                true,
            )
            .await,
            Some(InputCommand::Interactive(InteractiveInputEvent::Pointer {
                phase: InputPointerPhase::Down,
                ..
            }))
        ));
        assert!(matches!(
            next_input_command(
                &mut deferred,
                &mut receiver,
                &mut pointer_receiver,
                &mut scroll_receiver,
                true,
            )
            .await,
            Some(InputCommand::Interactive(InteractiveInputEvent::Pointer {
                phase: InputPointerPhase::Move,
                button: Some(InputPointerButton::Left),
                ..
            }))
        ));
    }

    #[tokio::test]
    async fn scroll_lane_accumulates_displacement_and_keeps_gesture_boundaries() {
        let scroll = |delta_y, phase| InteractiveInputEvent::Scroll {
            x_normalized: 0.5,
            y_normalized: 0.5,
            delta_x: 0.0,
            delta_y,
            phase,
            momentum_phase: InputGesturePhase::None,
            precise: true,
        };
        let (_input_sender, mut input_receiver) = unbounded_channel();
        let (_, mut pointer_receiver) = watch::channel(None);
        let (scroll_sender, mut scroll_receiver) = interactive_scroll_channel();
        scroll_sender.send(scroll(1.0, InputGesturePhase::Began));
        scroll_sender.send(scroll(2.0, InputGesturePhase::Changed));
        scroll_sender.send(scroll(18.0, InputGesturePhase::Changed));
        scroll_sender.send(scroll(0.0, InputGesturePhase::Ended));
        assert_eq!(scroll_receiver.len(), 3);

        let mut deferred = None;
        assert!(matches!(
            next_input_command(
                &mut deferred,
                &mut input_receiver,
                &mut pointer_receiver,
                &mut scroll_receiver,
                true,
            )
            .await,
            Some(InputCommand::Interactive(InteractiveInputEvent::Scroll {
                phase: InputGesturePhase::Began,
                ..
            }))
        ));
        assert!(matches!(
            next_input_command(
                &mut deferred,
                &mut input_receiver,
                &mut pointer_receiver,
                &mut scroll_receiver,
                true,
            )
            .await,
            Some(InputCommand::Interactive(InteractiveInputEvent::Scroll {
                delta_y: 20.0,
                phase: InputGesturePhase::Changed,
                ..
            }))
        ));
        assert!(matches!(
            next_input_command(
                &mut deferred,
                &mut input_receiver,
                &mut pointer_receiver,
                &mut scroll_receiver,
                true,
            )
            .await,
            Some(InputCommand::Interactive(InteractiveInputEvent::Scroll {
                phase: InputGesturePhase::Ended,
                ..
            }))
        ));
    }

    #[tokio::test]
    async fn continuous_input_waits_without_blocking_critical_commands() {
        let (input_sender, mut input_receiver) = unbounded_channel();
        let (pointer_sender, mut pointer_receiver) = watch::channel(None);
        let (_scroll_sender, mut scroll_receiver) = interactive_scroll_channel();
        pointer_sender.send_replace(Some(InteractiveInputEvent::Pointer {
            phase: InputPointerPhase::Move,
            button: None,
            x_normalized: 0.5,
            y_normalized: 0.5,
            modifiers: Vec::new(),
        }));
        input_sender
            .send(InputCommand::Text("x".to_owned()))
            .unwrap();

        let mut deferred = None;
        assert!(matches!(
            next_input_command(
                &mut deferred,
                &mut input_receiver,
                &mut pointer_receiver,
                &mut scroll_receiver,
                false,
            )
            .await,
            Some(InputCommand::Text(text)) if text == "x"
        ));
        assert!(tokio::time::timeout(
            Duration::from_millis(10),
            next_input_command(
                &mut deferred,
                &mut input_receiver,
                &mut pointer_receiver,
                &mut scroll_receiver,
                false,
            )
        )
        .await
        .is_err());
        assert!(matches!(
            next_input_command(
                &mut deferred,
                &mut input_receiver,
                &mut pointer_receiver,
                &mut scroll_receiver,
                true,
            )
            .await,
            Some(InputCommand::Interactive(
                InteractiveInputEvent::Pointer { .. }
            ))
        ));
    }

    #[test]
    fn interactive_text_coalesces_without_crossing_non_input_commands() {
        let (sender, mut receiver) = unbounded_channel();
        send_interactive_text(&sender, "r");
        send_interactive_text(&sender, "c");
        sender
            .send(InputCommand::Resize {
                revision: 2,
                width_points: 900,
                height_points: 600,
            })
            .unwrap();
        let InputCommand::Interactive(first) = receiver.try_recv().unwrap() else {
            panic!("expected initial interactive input");
        };
        let mut deferred = None;
        assert_eq!(
            coalesce_interactive_input(first, &mut receiver, &mut deferred),
            vec![InteractiveInputEvent::TextCommit { text: "rc".into() }]
        );
        assert!(matches!(deferred, Some(InputCommand::Resize { .. })));
    }

    #[test]
    fn interactive_scroll_accumulates_displacement_and_keeps_phase_boundaries() {
        let scroll = |delta_y, phase| InteractiveInputEvent::Scroll {
            x_normalized: 0.5,
            y_normalized: 0.5,
            delta_x: 0.0,
            delta_y,
            phase,
            momentum_phase: InputGesturePhase::None,
            precise: true,
        };
        let (sender, mut receiver) = unbounded_channel();
        sender
            .send(InputCommand::Interactive(scroll(
                2.0,
                InputGesturePhase::Changed,
            )))
            .unwrap();
        sender
            .send(InputCommand::Interactive(scroll(
                7.0,
                InputGesturePhase::Changed,
            )))
            .unwrap();
        sender
            .send(InputCommand::Interactive(scroll(
                0.0,
                InputGesturePhase::Ended,
            )))
            .unwrap();

        let InputCommand::Interactive(first) = receiver.try_recv().unwrap() else {
            panic!("expected initial interactive scroll");
        };
        let mut deferred = None;
        let events = coalesce_interactive_input(first, &mut receiver, &mut deferred);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            InteractiveInputEvent::Scroll {
                delta_y: 9.0,
                phase: InputGesturePhase::Changed,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            InteractiveInputEvent::Scroll {
                phase: InputGesturePhase::Ended,
                ..
            }
        ));
        assert!(deferred.is_none());
    }

    #[test]
    fn compatibility_scroll_discards_stale_queued_force() {
        let (sender, mut receiver) = unbounded_channel();
        sender
            .send(ScrollCommand {
                x: 10,
                y: 20,
                direction: "down",
                amount: 3,
                basis: ActionBasis::None,
            })
            .unwrap();
        sender
            .send(ScrollCommand {
                x: 30,
                y: 40,
                direction: "up",
                amount: 6,
                basis: ActionBasis::None,
            })
            .unwrap();

        let first = ScrollCommand {
            x: 1,
            y: 2,
            direction: "down",
            amount: 1,
            basis: ActionBasis::None,
        };
        let latest = latest_scroll_command(first, &mut receiver);
        assert_eq!(latest.x, 30);
        assert_eq!(latest.y, 40);
        assert_eq!(latest.direction, "up");
        assert_eq!(latest.amount, 6);
    }

    #[test]
    fn adjacent_text_is_coalesced_without_overtaking_the_next_action() {
        let (sender, mut receiver) = unbounded_channel();
        sender.send(InputCommand::Text("r".into())).unwrap();
        sender.send(InputCommand::Text("c".into())).unwrap();
        sender
            .send(InputCommand::Action {
                tool: "press_key",
                arguments: serde_json::json!({"key": "return", "modifiers": []}),
                basis: ActionBasis::None,
            })
            .unwrap();
        sender.send(InputCommand::Text("later".into())).unwrap();

        let InputCommand::Text(first) = receiver.try_recv().unwrap() else {
            panic!("expected initial text");
        };
        let mut deferred = None;
        assert_eq!(
            coalesce_text_input(first, &mut receiver, &mut deferred),
            "rc"
        );
        assert!(matches!(deferred, Some(InputCommand::Action { .. })));
        assert!(matches!(
            receiver.try_recv(),
            Ok(InputCommand::Text(text)) if text == "later"
        ));
    }

    #[test]
    fn left_pointer_motion_becomes_a_drag_on_release() {
        let (sender, mut receiver) = unbounded_channel();
        let mut state = interactive_state((10.0, 12.0));

        begin_pointer_gesture(&mut state, MouseButton::Left);
        assert!(receiver.try_recv().is_err());
        state.cursor = Some(PhysicalPosition::new(70.0, 42.0));
        finish_pointer_gesture(&sender, &mut state, MouseButton::Left);

        let InputCommand::Action {
            tool,
            arguments,
            basis,
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected a drag action");
        };
        assert_eq!(tool, "drag");
        assert_eq!(arguments["from_x"], 10);
        assert_eq!(arguments["from_y"], 12);
        assert_eq!(arguments["to_x"], 70);
        assert_eq!(arguments["to_y"], 42);
        assert_eq!(basis, current_pixel_basis(&state).unwrap());
    }

    #[test]
    fn stationary_left_pointer_becomes_a_click_on_release() {
        let (sender, mut receiver) = unbounded_channel();
        let mut state = interactive_state((20.0, 30.0));

        begin_pointer_gesture(&mut state, MouseButton::Left);
        finish_pointer_gesture(&sender, &mut state, MouseButton::Left);

        let InputCommand::Action {
            tool, arguments, ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected a click action");
        };
        assert_eq!(tool, "click");
        assert_eq!(arguments["x"], 20);
        assert_eq!(arguments["y"], 30);
    }

    #[test]
    fn pixel_scroll_accumulates_small_deltas_and_preserves_velocity() {
        let (sender, mut receiver) = unbounded_channel();
        let mut state = interactive_state((50.0, 50.0));

        send_scroll(
            &sender,
            &mut state,
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 4.0)),
        );
        assert!(receiver.try_recv().is_err());
        send_scroll(
            &sender,
            &mut state,
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 7.0)),
        );
        let slow = receiver.try_recv().unwrap();
        assert_eq!(slow.direction, "up");
        assert_eq!(slow.amount, 1);

        send_scroll(
            &sender,
            &mut state,
            MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 65.0)),
        );
        let fast = receiver.try_recv().unwrap();
        assert_eq!(fast.direction, "up");
        assert_eq!(fast.amount, 6);
    }

    #[test]
    fn only_same_run_loop_pointer_duplicates_are_deglitched() {
        let start = Instant::now();
        let mut previous = None;
        assert!(accept_pointer_press(
            &mut previous,
            MouseButton::Left,
            (88, 320),
            start,
        ));
        assert!(!accept_pointer_press(
            &mut previous,
            MouseButton::Left,
            (88, 320),
            start + Duration::from_millis(2),
        ));
        assert!(accept_pointer_press(
            &mut previous,
            MouseButton::Left,
            (88, 320),
            start + Duration::from_millis(20),
        ));
    }

    #[test]
    fn automatic_target_selection_prefers_the_main_visible_window() {
        let window = |name: &str, width_px, height_px| cua_media_protocol::WindowDescriptor {
            window: cua_media_protocol::TargetHandle(name.into()),
            target_epoch: cua_media_protocol::TargetEpoch(1),
            app_name: "Codex".into(),
            title: name.into(),
            geometry: SurfaceGeometry {
                width_px,
                height_px,
                scale_factor: 1.0,
            },
            visible: true,
        };
        let selected =
            preferred_window(vec![window("utility", 66, 20), window("main", 1_920, 960)]).unwrap();
        assert_eq!(selected.window.0, "main");
    }
}
