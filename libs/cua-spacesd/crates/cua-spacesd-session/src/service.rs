// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#[cfg(target_os = "macos")]
use std::collections::HashSet;
use std::collections::{HashMap, VecDeque};
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cua_media_protocol::{
    AccessibilitySnapshotId, ActionBasis, ActionError, ActionErrorCode, ActionFrameCorrelation,
    ActionRequest, ActionResult, AppIconDescriptor, ClientMessage, ClipboardFile, CodecEpoch,
    GeometryEpoch, Hello, InteractiveInputAcknowledgement, InteractiveInputBatch, OpenSession,
    ServerErrorCode, ServerMessage, SessionOpened, StreamPreferences, StreamStats,
    SuspensionReason, TargetEpoch, TargetHandle, VideoCodec, VideoFrameDescriptor,
    WindowGeometryControl, WindowGeometryRequest, WindowGeometryResult, WindowLifecycleEvent,
    WindowSessionId, WindowState, MAX_CLIPBOARD_TEXT_BYTES, PROTOCOL_NAME, PROTOCOL_VERSION,
};
#[cfg(target_os = "macos")]
use cua_media_protocol::{
    MAX_CLIPBOARD_FILES, MAX_CLIPBOARD_FILES_BYTES, MAX_CLIPBOARD_FILE_BYTES,
};
use cua_spacesd_provider_api::{
    enforce_action_policy, AccessibilityProvider, ActionInvocation, ActionProvider, CaptureConfig,
    CaptureEvent, CaptureLease, CaptureProvider, CaptureSink, InteractiveInputLease,
    InteractiveInputProvider, PickTargetRequest, PixelFormat, ProviderError, ProviderErrorCode,
    ProviderTarget, ProviderTargetId, TargetProvider, TargetQuery,
    UnsupportedInteractiveInputProvider, WindowGeometryProvider,
};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSFilenamesPboardType, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeString,
    NSPasteboardWriting,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol as _, NSString, NSURL};
#[cfg(target_os = "macos")]
use objc2_v5::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_v5::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use sha2::{Digest as _, Sha256};

use crate::{
    ActionDispatch, ActionSequence, FramePayload, RuntimeLifecycleEvent, SessionEvent,
    SessionFrame, WindowSessionHub, WindowSessionSubscriber,
};

const ACTION_FRAME_WAIT: Duration = Duration::from_millis(250);
const MAX_PENDING_RUNTIME_EVENTS: usize = 256;
const MAX_PENDING_PROVIDER_ACTIONS: usize = 64;
const MIN_WINDOW_DIMENSION_POINTS: u32 = 64;
const MAX_WINDOW_DIMENSION_POINTS: u32 = 8_192;
const MAX_CAPTURE_FPS: u16 = 240;
const MAX_CAPTURE_DIMENSION: u32 = 8_192;
const MIN_CAPTURE_BITRATE_KBPS: u32 = 250;
const MAX_CAPTURE_BITRATE_KBPS: u32 = 100_000;
const MAX_APP_ICON_BYTES: usize = 8 * 1024 * 1024;

#[cfg(target_os = "macos")]
fn platform_clipboard_snapshot() -> Result<(u64, Option<String>), String> {
    // SAFETY: NSPasteboard's general pasteboard and string accessors return
    // retained Objective-C objects and are valid from this process thread.
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    // SAFETY: `pasteboard` remains retained for both messages.
    let generation = unsafe { pasteboard.changeCount() };
    // SAFETY: `NSPasteboardTypeString` is the system UTF-8 text pasteboard type.
    let text = unsafe { pasteboard.stringForType(NSPasteboardTypeString) }
        .map(|value| value.to_string())
        .filter(|value| value.len() <= MAX_CLIPBOARD_TEXT_BYTES);
    Ok((u64::try_from(generation).unwrap_or_default(), text))
}

#[cfg(target_os = "macos")]
fn platform_clipboard_set(text: &str) -> Result<u64, String> {
    // SAFETY: NSPasteboard's general pasteboard returns a retained object.
    let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
    let text = NSString::from_str(text);
    // SAFETY: Both the pasteboard and string are retained for these synchronous
    // messages, and the pasteboard copies the supplied string.
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
fn platform_clipboard_file_paths() -> Result<(u64, Vec<PathBuf>), String> {
    // SAFETY: Access occurs synchronously while the GUI user's pasteboard
    // service owns the returned immutable property list.
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
fn platform_clipboard_set_files(paths: &[PathBuf]) -> Result<u64, String> {
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
    // SAFETY: NSURL implements NSPasteboardWriting. The pasteboard copies the
    // retained URL objects during this synchronous call.
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
fn encode_clipboard_files(paths: &[PathBuf]) -> Result<(Vec<ClipboardFile>, Vec<u8>), String> {
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
fn materialize_clipboard_files(
    files: &[ClipboardFile],
    payload: &[u8],
    generation: u64,
) -> Result<Vec<PathBuf>, String> {
    validate_clipboard_files(files, payload.len() as u64, payload)?;
    let cache_root = clipboard_cache_root();
    prune_clipboard_cache(&cache_root, 3)?;
    let root = cache_root.join(format!("incoming-{generation}"));
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
fn safe_clipboard_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 255
        && Path::new(name).file_name().is_some_and(|leaf| leaf == name)
}

#[cfg(target_os = "macos")]
fn validate_clipboard_files(
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
fn clipboard_cache_root() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .map(|path| path.join("Library/Caches/cua-spacesd/clipboard"))
        .unwrap_or_else(|| std::env::temp_dir().join("rcdp-clipboard"))
}

#[cfg(target_os = "macos")]
fn prune_clipboard_cache(root: &Path, keep: usize) -> Result<(), String> {
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
                .unwrap_or(UNIX_EPOCH),
        )
    });
    for entry in directories.into_iter().skip(keep) {
        std::fs::remove_dir_all(entry.path())
            .map_err(|error| format!("clipboard cache pruning failed: {error}"))?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn platform_clipboard_snapshot() -> Result<(u64, Option<String>), String> {
    Err("text clipboard synchronization is not available on this host".into())
}

#[cfg(not(target_os = "macos"))]
fn platform_clipboard_set(_: &str) -> Result<u64, String> {
    Err("text clipboard synchronization is not available on this host".into())
}

#[derive(Debug, Clone)]
pub enum OutboundPacket {
    Control(ServerMessage),
    Video {
        descriptor: VideoFrameDescriptor,
        payload: Arc<[u8]>,
    },
    AppIcon {
        descriptor: AppIconDescriptor,
        payload: Arc<[u8]>,
    },
    ClipboardFiles {
        message: ServerMessage,
        payload: Arc<[u8]>,
    },
}

pub struct ServerRuntime {
    targets: Arc<dyn TargetProvider>,
    captures: Arc<dyn CaptureProvider>,
    actions: Arc<dyn ActionProvider>,
    accessibility: Arc<dyn AccessibilityProvider>,
    geometry: Arc<dyn WindowGeometryProvider>,
    inputs: Arc<dyn InteractiveInputProvider>,
    policy_ceiling: cua_media_protocol::SessionPolicy,
    geometry_owners: std::sync::Mutex<HashMap<ProviderTargetId, WindowSessionId>>,
    next_connection_id: AtomicU64,
    next_session_id: AtomicU64,
}

impl ServerRuntime {
    pub fn new(
        targets: Arc<dyn TargetProvider>,
        captures: Arc<dyn CaptureProvider>,
        actions: Arc<dyn ActionProvider>,
        accessibility: Arc<dyn AccessibilityProvider>,
        geometry: Arc<dyn WindowGeometryProvider>,
    ) -> Self {
        Self::new_with_policy_ceiling_and_interactive_input(
            targets,
            captures,
            actions,
            accessibility,
            geometry,
            Arc::new(UnsupportedInteractiveInputProvider),
            cua_media_protocol::SessionPolicy::AllowActivation,
        )
    }

    pub fn new_with_interactive_input(
        targets: Arc<dyn TargetProvider>,
        captures: Arc<dyn CaptureProvider>,
        actions: Arc<dyn ActionProvider>,
        accessibility: Arc<dyn AccessibilityProvider>,
        geometry: Arc<dyn WindowGeometryProvider>,
        inputs: Arc<dyn InteractiveInputProvider>,
    ) -> Self {
        Self::new_with_policy_ceiling_and_interactive_input(
            targets,
            captures,
            actions,
            accessibility,
            geometry,
            inputs,
            cua_media_protocol::SessionPolicy::AllowActivation,
        )
    }

    pub fn new_with_policy_ceiling(
        targets: Arc<dyn TargetProvider>,
        captures: Arc<dyn CaptureProvider>,
        actions: Arc<dyn ActionProvider>,
        accessibility: Arc<dyn AccessibilityProvider>,
        geometry: Arc<dyn WindowGeometryProvider>,
        policy_ceiling: cua_media_protocol::SessionPolicy,
    ) -> Self {
        Self::new_with_policy_ceiling_and_interactive_input(
            targets,
            captures,
            actions,
            accessibility,
            geometry,
            Arc::new(UnsupportedInteractiveInputProvider),
            policy_ceiling,
        )
    }

    pub fn new_with_policy_ceiling_and_interactive_input(
        targets: Arc<dyn TargetProvider>,
        captures: Arc<dyn CaptureProvider>,
        actions: Arc<dyn ActionProvider>,
        accessibility: Arc<dyn AccessibilityProvider>,
        geometry: Arc<dyn WindowGeometryProvider>,
        inputs: Arc<dyn InteractiveInputProvider>,
        policy_ceiling: cua_media_protocol::SessionPolicy,
    ) -> Self {
        Self {
            targets,
            captures,
            actions,
            accessibility,
            geometry,
            inputs,
            policy_ceiling,
            geometry_owners: std::sync::Mutex::new(HashMap::new()),
            next_connection_id: AtomicU64::new(1),
            next_session_id: AtomicU64::new(1),
        }
    }

    pub fn connect(self: &Arc<Self>) -> Connection {
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let hub = WindowSessionHub::new();
        let outbound_ready = Arc::new(tokio::sync::Notify::new());
        let runtime_mailbox = Arc::new(RuntimeMailbox::new(outbound_ready.clone()));
        let provider_action_mailbox = Arc::new(ProviderActionMailbox::new(outbound_ready.clone()));
        let (provider_action_tx, mut provider_action_rx) =
            tokio::sync::mpsc::channel::<ProviderActionJob>(MAX_PENDING_PROVIDER_ACTIONS);
        let actions = self.actions.clone();
        let action_completions = provider_action_mailbox.clone();
        tokio::spawn(async move {
            while let Some(job) = provider_action_rx.recv().await {
                let outcome = actions
                    .perform(&job.target, job.invocation, job.policy)
                    .await;
                action_completions.push(ProviderActionCompletion {
                    action_sequence: job.action_sequence,
                    action_id: job.action_id,
                    target: job.target,
                    outcome,
                });
            }
        });
        let subscriber_id = hub.subscribe(
            format!("connection-{connection_id}"),
            ChannelSubscriber(runtime_mailbox.clone()),
        );
        Connection {
            runtime: self.clone(),
            hub,
            subscriber_id,
            negotiated: false,
            sessions: HashMap::new(),
            runtime_mailbox,
            provider_action_mailbox,
            provider_action_tx,
            outbound_ready,
            pending_action_correlations: HashMap::new(),
            runtime_overflow_reported: false,
        }
    }
}

enum RuntimeItem {
    Frame(Arc<SessionFrame>),
    Event(Arc<SessionEvent>),
}

struct RuntimeMailbox {
    state: std::sync::Mutex<RuntimeMailboxState>,
    outbound_ready: Arc<tokio::sync::Notify>,
}

#[derive(Default)]
struct RuntimeMailboxState {
    latest_frames: HashMap<ProviderTargetId, Arc<SessionFrame>>,
    replaced_frames: HashMap<ProviderTargetId, u64>,
    events: VecDeque<Arc<SessionEvent>>,
    overflowed: bool,
}

struct ProviderActionCompletion {
    action_sequence: ActionSequence,
    action_id: String,
    target: ProviderTargetId,
    outcome: Result<cua_spacesd_provider_api::ActionOutcome, ProviderError>,
}

struct ProviderActionJob {
    action_sequence: ActionSequence,
    action_id: String,
    target: ProviderTargetId,
    invocation: ActionInvocation,
    policy: cua_media_protocol::SessionPolicy,
}

struct ProviderActionMailbox {
    completions: std::sync::Mutex<VecDeque<ProviderActionCompletion>>,
    outbound_ready: Arc<tokio::sync::Notify>,
}

impl Default for RuntimeMailbox {
    fn default() -> Self {
        Self::new(Arc::new(tokio::sync::Notify::new()))
    }
}

impl RuntimeMailbox {
    fn new(outbound_ready: Arc<tokio::sync::Notify>) -> Self {
        Self {
            state: std::sync::Mutex::new(RuntimeMailboxState::default()),
            outbound_ready,
        }
    }
}

impl Default for ProviderActionMailbox {
    fn default() -> Self {
        Self::new(Arc::new(tokio::sync::Notify::new()))
    }
}

impl ProviderActionMailbox {
    fn new(outbound_ready: Arc<tokio::sync::Notify>) -> Self {
        Self {
            completions: std::sync::Mutex::new(VecDeque::new()),
            outbound_ready,
        }
    }

    fn push(&self, completion: ProviderActionCompletion) {
        self.completions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(completion);
        self.outbound_ready.notify_one();
    }

    fn take(&self) -> VecDeque<ProviderActionCompletion> {
        std::mem::take(
            &mut *self
                .completions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

impl RuntimeMailbox {
    fn push_frame(&self, frame: Arc<SessionFrame>) {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let target = frame.target.clone();
            if state.latest_frames.insert(target.clone(), frame).is_some() {
                *state.replaced_frames.entry(target).or_default() += 1;
            }
        }
        self.outbound_ready.notify_one();
    }

    fn push_event(&self, event: Arc<SessionEvent>) {
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.events.len() == MAX_PENDING_RUNTIME_EVENTS {
                state.overflowed = true;
            } else {
                state.events.push_back(event);
            }
        }
        self.outbound_ready.notify_one();
    }

    fn take(&self) -> (Vec<RuntimeItem>, HashMap<ProviderTargetId, u64>, bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut items = state
            .events
            .drain(..)
            .map(RuntimeItem::Event)
            .collect::<Vec<_>>();
        items.extend(
            state
                .latest_frames
                .drain()
                .map(|(_, frame)| RuntimeItem::Frame(frame)),
        );
        let replaced_frames = std::mem::take(&mut state.replaced_frames);
        let overflowed = std::mem::take(&mut state.overflowed);
        (items, replaced_frames, overflowed)
    }
}

struct ChannelSubscriber(Arc<RuntimeMailbox>);

impl WindowSessionSubscriber for ChannelSubscriber {
    fn on_frame(&self, frame: Arc<SessionFrame>) {
        self.0.push_frame(frame);
    }

    fn on_event(&self, event: Arc<SessionEvent>) {
        self.0.push_event(event);
    }
}

struct HubCaptureSink {
    target: ProviderTargetId,
    hub: WindowSessionHub,
    geometry: std::sync::Mutex<cua_media_protocol::SurfaceGeometry>,
    active_generation: Arc<AtomicU64>,
    generation: u64,
}

impl HubCaptureSink {
    fn new(
        target: ProviderTargetId,
        hub: WindowSessionHub,
        geometry: cua_media_protocol::SurfaceGeometry,
        active_generation: Arc<AtomicU64>,
        generation: u64,
    ) -> Self {
        Self {
            target,
            hub,
            geometry: std::sync::Mutex::new(geometry),
            active_generation,
            generation,
        }
    }
}

impl CaptureSink for HubCaptureSink {
    fn on_event(&self, event: CaptureEvent) {
        if self.active_generation.load(Ordering::Acquire) != self.generation {
            return;
        }
        match event {
            CaptureEvent::Frame(frame) => {
                if frame.validate().is_err() {
                    self.hub
                        .mark_source_suspended(&self.target, "invalid_frame");
                    return;
                }
                let scale_factor = self
                    .geometry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .scale_factor;
                let geometry = cua_media_protocol::SurfaceGeometry {
                    width_px: frame.width_px,
                    height_px: frame.height_px,
                    scale_factor,
                };
                let payload = match frame.format {
                    PixelFormat::Png => FramePayload::Png(frame.bytes),
                    PixelFormat::Bgra8 => FramePayload::Bgra {
                        bytes: frame.bytes,
                        bytes_per_row: frame
                            .bytes_per_row
                            .expect("validated BGRA frame has a row stride"),
                    },
                    PixelFormat::H264AnnexB => FramePayload::H264 {
                        bytes: frame.bytes,
                        codec_epoch: CodecEpoch(frame.codec_epoch),
                        keyframe: frame.keyframe,
                        encode_duration_us: frame.encode_duration_us,
                    },
                };
                self.hub.publish_live_frame(
                    self.target.clone(),
                    geometry,
                    frame.capture_timestamp_us,
                    payload,
                );
            }
            CaptureEvent::GeometryChanged(geometry) => {
                *self
                    .geometry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = geometry.clone();
                self.hub.mark_geometry_changed(&self.target, geometry);
            }
            CaptureEvent::TitleChanged(title) => {
                self.hub.mark_title_changed(&self.target, title);
            }
            CaptureEvent::Suspended(reason) => {
                self.hub.mark_source_suspended(&self.target, reason);
            }
            CaptureEvent::Resumed => self.hub.mark_live_source(self.target.clone()),
            CaptureEvent::Closed => {
                self.hub.mark_target_closed(&self.target);
            }
        }
    }
}

struct ActiveSession {
    target: ProviderTarget,
    opened: SessionOpened,
    capture: Arc<dyn CaptureLease>,
    interactive_input: Option<Arc<dyn InteractiveInputLease>>,
    last_input_sequence: Option<u64>,
    input_events_dispatched: u64,
    latest_snapshot: Option<AccessibilitySnapshotId>,
    frames_emitted: u64,
    bytes_emitted: u64,
    frames_replaced: u64,
    keyframe_requests: u64,
    actions_dispatched: u64,
    action_frame_timeouts: u64,
    preference_updates: u64,
    last_geometry_revision: u64,
    awaiting_keyframe: bool,
    capture_format: PixelFormat,
    capture_generation: Arc<AtomicU64>,
    codec_epoch_offset: u64,
}

pub struct Connection {
    runtime: Arc<ServerRuntime>,
    hub: WindowSessionHub,
    subscriber_id: u64,
    negotiated: bool,
    sessions: HashMap<WindowSessionId, ActiveSession>,
    runtime_mailbox: Arc<RuntimeMailbox>,
    provider_action_mailbox: Arc<ProviderActionMailbox>,
    provider_action_tx: tokio::sync::mpsc::Sender<ProviderActionJob>,
    outbound_ready: Arc<tokio::sync::Notify>,
    pending_action_correlations: HashMap<ActionSequence, PendingActionCorrelation>,
    runtime_overflow_reported: bool,
}

struct PendingActionCorrelation {
    action_id: String,
    session_id: WindowSessionId,
    deadline: Option<Instant>,
    first_frame_sequence_after: Option<cua_media_protocol::FrameSequence>,
}

impl Connection {
    /// Notification that frames, lifecycle events, or provider action results
    /// are ready to drain. Transports keep a slower timer only for correlation
    /// deadlines instead of polling every frame.
    pub fn outbound_ready(&self) -> Arc<tokio::sync::Notify> {
        self.outbound_ready.clone()
    }

    pub async fn handle(&mut self, message: ClientMessage) -> Vec<OutboundPacket> {
        self.handle_packet(message, Vec::new()).await
    }

    pub async fn handle_packet(
        &mut self,
        message: ClientMessage,
        payload: Vec<u8>,
    ) -> Vec<OutboundPacket> {
        if let ClientMessage::GetClipboardFiles { known_generation } = message {
            let mut output = match self.clipboard_files(known_generation) {
                Ok(packet) => vec![packet],
                Err((code, message)) => vec![OutboundPacket::Control(ServerMessage::Error {
                    code,
                    message,
                })],
            };
            output.extend(self.drain_outbound());
            return output;
        }
        if let ClientMessage::SetClipboardFiles { files, byte_len } = message {
            let mut output = vec![OutboundPacket::Control(
                match self.set_clipboard_files(files, byte_len, &payload) {
                    Ok(message) => message,
                    Err((code, message)) => ServerMessage::Error { code, message },
                },
            )];
            output.extend(self.drain_outbound());
            return output;
        }
        if !payload.is_empty() {
            return vec![OutboundPacket::Control(ServerMessage::Error {
                code: ServerErrorCode::UnsupportedMessage,
                message: "only file clipboard messages may carry a payload".into(),
            })];
        }
        if let ClientMessage::InteractiveInput(batch) = message {
            let mut output = vec![OutboundPacket::Control(
                self.dispatch_interactive_input(batch).await,
            )];
            output.extend(self.drain_outbound());
            return output;
        }
        if let ClientMessage::Action(action) = message {
            let mut output = match self.dispatch_action(action) {
                Ok(Some(message)) => vec![OutboundPacket::Control(message)],
                Ok(None) => Vec::new(),
                Err((code, message)) => vec![OutboundPacket::Control(ServerMessage::Error {
                    code,
                    message,
                })],
            };
            output.extend(self.drain_outbound());
            return output;
        }
        if let ClientMessage::GetAppIcon {
            window,
            target_epoch,
        } = message
        {
            let mut output = match self.app_icon(window, target_epoch) {
                Ok(packet) => vec![packet],
                Err((code, message)) => vec![OutboundPacket::Control(ServerMessage::Error {
                    code,
                    message,
                })],
            };
            output.extend(self.drain_outbound());
            return output;
        }
        let response = match message {
            ClientMessage::Hello(hello) => self.handle_hello(hello),
            ClientMessage::ListWindows { on_screen_only } => {
                self.require_negotiated().and_then(|_| {
                    self.runtime
                        .targets
                        .enumerate(&TargetQuery { on_screen_only })
                        .map(|targets| ServerMessage::Windows {
                            windows: targets
                                .into_iter()
                                .map(|target| target.descriptor)
                                .collect(),
                        })
                        .map_err(server_error)
                })
            }
            ClientMessage::GetAppIcon { .. } => unreachable!("app icons return as binary assets"),
            ClientMessage::PickWindow { prompt } => self.require_negotiated().and_then(|_| {
                self.runtime
                    .targets
                    .pick(&PickTargetRequest { prompt })
                    .map(|target| ServerMessage::WindowSelected {
                        window: target.descriptor,
                        grant: target.grant,
                    })
                    .map_err(server_error)
            }),
            ClientMessage::RestoreWindow { grant } => self.require_negotiated().and_then(|_| {
                self.runtime
                    .targets
                    .restore(&grant)
                    .map(|target| ServerMessage::WindowSelected {
                        window: target.descriptor,
                        grant: target.grant,
                    })
                    .map_err(server_error)
            }),
            ClientMessage::OpenSession(open) => self.open_session(open),
            ClientMessage::CloseSession { session_id } => self.close_session(&session_id),
            ClientMessage::InteractiveInput(_) => {
                unreachable!("interactive input is dispatched above")
            }
            ClientMessage::Action(_) => unreachable!("actions return through the async mailbox"),
            ClientMessage::RequestKeyframe { session_id } => self.request_keyframe(&session_id),
            ClientMessage::SetStreamPreferences(preferences) => {
                self.set_stream_preferences(preferences)
            }
            ClientMessage::SetWindowGeometry(request) => self.set_window_geometry(request).await,
            ClientMessage::GetWindowState { session_id } => self.window_state(&session_id).await,
            ClientMessage::GetStats { session_id } => self.stats(&session_id),
            ClientMessage::GetClipboard { known_generation } => self.clipboard(known_generation),
            ClientMessage::SetClipboard { text } => self.set_clipboard(text),
            ClientMessage::GetClipboardFiles { .. } => {
                unreachable!("file clipboard polls return as binary assets")
            }
            ClientMessage::SetClipboardFiles { .. } => {
                unreachable!("file clipboard payloads are dispatched above")
            }
            ClientMessage::Authenticate { .. } => Err((
                ServerErrorCode::AlreadyAuthenticated,
                "authentication belongs to the transport binding".into(),
            )),
            ClientMessage::Join { .. }
            | ClientMessage::Cursor { .. }
            | ClientMessage::ListApps
            | ClientMessage::LaunchApp { .. } => Err((
                ServerErrorCode::Unsupported,
                "presence and app-menu messages belong to the daemon binding".into(),
            )),
            ClientMessage::Unsupported => Err((
                ServerErrorCode::UnsupportedMessage,
                "unsupported client message".into(),
            )),
        };

        let mut output = vec![OutboundPacket::Control(match response {
            Ok(message) => message,
            Err((code, message)) => ServerMessage::Error { code, message },
        })];
        output.extend(self.drain_outbound());
        output
    }

    fn app_icon(
        &self,
        window: TargetHandle,
        target_epoch: TargetEpoch,
    ) -> Result<OutboundPacket, ServerFailure> {
        self.require_negotiated()?;
        let icon = self
            .runtime
            .targets
            .app_icon(&window, target_epoch)
            .map_err(server_error)?
            .ok_or((
                ServerErrorCode::Unsupported,
                "the target provider does not expose an application icon".into(),
            ))?;
        if icon.bytes.is_empty() || icon.bytes.len() > MAX_APP_ICON_BYTES {
            return Err((
                ServerErrorCode::InvalidFrame,
                format!("application icon size must be between 1 and {MAX_APP_ICON_BYTES} bytes"),
            ));
        }
        Ok(OutboundPacket::AppIcon {
            descriptor: AppIconDescriptor {
                window,
                target_epoch,
                media_type: icon.media_type,
                byte_len: icon.bytes.len() as u64,
            },
            payload: icon.bytes,
        })
    }

    pub fn drain_outbound(&mut self) -> Vec<OutboundPacket> {
        let (runtime_items, replaced_frames, overflowed) = self.runtime_mailbox.take();
        for (target, replaced) in replaced_frames {
            for session in self.sessions.values_mut() {
                if session.target.id == target {
                    session.frames_replaced = session.frames_replaced.saturating_add(replaced);
                    if session.opened.codec == VideoCodec::H264 {
                        session.awaiting_keyframe = true;
                        session.keyframe_requests = session.keyframe_requests.saturating_add(1);
                        session.capture.request_keyframe();
                    }
                }
            }
        }
        let mut packets = Vec::new();
        let mut closed = Vec::new();
        let mut frames = Vec::new();
        for item in runtime_items {
            match item {
                RuntimeItem::Frame(frame) => frames.push(frame),
                RuntimeItem::Event(event) => match event.as_ref() {
                    SessionEvent::Lifecycle(lifecycle) => {
                        let target = lifecycle_target(lifecycle);
                        for (session_id, session) in &self.sessions {
                            if &session.target.id != target {
                                continue;
                            }
                            if let Some(event) = protocol_lifecycle(lifecycle) {
                                if matches!(event, WindowLifecycleEvent::Closed) {
                                    closed.push(session_id.clone());
                                }
                                packets.push(OutboundPacket::Control(ServerMessage::Lifecycle {
                                    session_id: session_id.clone(),
                                    event,
                                }));
                            }
                        }
                    }
                    SessionEvent::FirstFrameAfterActions {
                        frame_sequence,
                        actions,
                        ..
                    } => {
                        for action in actions {
                            let ready = self
                                .pending_action_correlations
                                .get_mut(action)
                                .is_some_and(|pending| {
                                    pending.first_frame_sequence_after = Some(*frame_sequence);
                                    pending.deadline.is_some()
                                });
                            if ready {
                                let pending = self
                                    .pending_action_correlations
                                    .remove(action)
                                    .expect("ready action correlation exists");
                                packets.push(OutboundPacket::Control(
                                    ServerMessage::ActionFrameCorrelation(ActionFrameCorrelation {
                                        action_id: pending.action_id,
                                        session_id: pending.session_id,
                                        first_frame_sequence_after: pending
                                            .first_frame_sequence_after,
                                    }),
                                ));
                            }
                        }
                    }
                    SessionEvent::Action(_) => {}
                },
            }
        }
        for session_id in closed {
            if let Some(session) = self.sessions.remove(&session_id) {
                session.capture.stop();
                self.release_geometry_owner(&session.target.id, &session_id);
            }
            self.pending_action_correlations
                .retain(|_, pending| pending.session_id != session_id);
        }
        for completion in self.provider_action_mailbox.take() {
            let Some(pending) = self
                .pending_action_correlations
                .get_mut(&completion.action_sequence)
            else {
                continue;
            };
            match completion.outcome {
                Ok(outcome) if outcome.delivered => {
                    packets.push(OutboundPacket::Control(ServerMessage::ActionResult(
                        ActionResult {
                            action_id: completion.action_id,
                            delivered: true,
                            error: None,
                            first_frame_sequence_after: None,
                        },
                    )));
                    if pending.first_frame_sequence_after.is_some() {
                        let pending = self
                            .pending_action_correlations
                            .remove(&completion.action_sequence)
                            .expect("completed action correlation exists");
                        packets.push(OutboundPacket::Control(
                            ServerMessage::ActionFrameCorrelation(ActionFrameCorrelation {
                                action_id: pending.action_id,
                                session_id: pending.session_id,
                                first_frame_sequence_after: pending.first_frame_sequence_after,
                            }),
                        ));
                    } else {
                        pending.deadline = Some(Instant::now() + ACTION_FRAME_WAIT);
                    }
                }
                Ok(_) => {
                    self.pending_action_correlations
                        .remove(&completion.action_sequence);
                    packets.push(OutboundPacket::Control(ServerMessage::ActionResult(
                        ActionResult {
                            action_id: completion.action_id,
                            delivered: false,
                            error: Some(action_error(
                                ActionErrorCode::DeliveryFailed,
                                "provider did not deliver the action",
                                self.hub.target_snapshot(&completion.target),
                            )),
                            first_frame_sequence_after: None,
                        },
                    )));
                }
                Err(error) => {
                    self.pending_action_correlations
                        .remove(&completion.action_sequence);
                    packets.push(OutboundPacket::Control(ServerMessage::ActionResult(
                        ActionResult {
                            action_id: completion.action_id,
                            delivered: false,
                            error: Some(provider_action_error(
                                error,
                                self.hub.target_snapshot(&completion.target),
                            )),
                            first_frame_sequence_after: None,
                        },
                    )));
                }
            }
        }
        let now = Instant::now();
        let expired = self
            .pending_action_correlations
            .iter()
            .filter_map(|(action, pending)| {
                pending
                    .deadline
                    .is_some_and(|deadline| deadline <= now)
                    .then_some(*action)
            })
            .collect::<Vec<_>>();
        for action in expired {
            let Some(pending) = self.pending_action_correlations.remove(&action) else {
                continue;
            };
            if let Some(session) = self.sessions.get_mut(&pending.session_id) {
                session.action_frame_timeouts = session.action_frame_timeouts.saturating_add(1);
            }
            packets.push(OutboundPacket::Control(
                ServerMessage::ActionFrameCorrelation(ActionFrameCorrelation {
                    action_id: pending.action_id,
                    session_id: pending.session_id,
                    first_frame_sequence_after: None,
                }),
            ));
        }
        if overflowed && !self.runtime_overflow_reported {
            self.runtime_overflow_reported = true;
            packets.push(OutboundPacket::Control(ServerMessage::Error {
                code: ServerErrorCode::Internal,
                message: "connection event backlog exceeded its bounded capacity".into(),
            }));
        }
        for frame in frames {
            for (session_id, session) in &mut self.sessions {
                if session.target.id != frame.target {
                    continue;
                }
                if let Some(WireFramePayload {
                    codec,
                    payload,
                    codec_epoch: source_codec_epoch,
                    keyframe,
                    encode_duration_us,
                }) = frame_payload(&frame, &session.opened.codec)
                {
                    let codec_epoch = if codec == VideoCodec::H264 {
                        CodecEpoch(
                            session
                                .codec_epoch_offset
                                .saturating_add(source_codec_epoch.0),
                        )
                    } else {
                        source_codec_epoch
                    };
                    if codec == VideoCodec::H264 && session.awaiting_keyframe && !keyframe {
                        session.frames_replaced = session.frames_replaced.saturating_add(1);
                        continue;
                    }
                    if codec == VideoCodec::H264 && keyframe {
                        session.awaiting_keyframe = false;
                    }
                    session.opened.codec_epoch = codec_epoch;
                    session.frames_emitted = session.frames_emitted.saturating_add(1);
                    session.bytes_emitted = session
                        .bytes_emitted
                        .saturating_add(u64::try_from(payload.len()).unwrap_or(u64::MAX));
                    packets.push(OutboundPacket::Video {
                        descriptor: VideoFrameDescriptor {
                            session_id: session_id.clone(),
                            sequence: frame.sequence,
                            geometry_epoch: frame.geometry_epoch,
                            codec_epoch,
                            width_px: frame.geometry.width_px,
                            height_px: frame.geometry.height_px,
                            capture_timestamp_us: frame.capture_timestamp_us,
                            encode_duration_us,
                            codec,
                            keyframe,
                        },
                        payload,
                    });
                }
            }
        }
        packets
    }

    fn handle_hello(&mut self, hello: Hello) -> Result<ServerMessage, ServerFailure> {
        if self.negotiated {
            return Err((
                ServerErrorCode::AlreadyNegotiated,
                "hello has already completed".into(),
            ));
        }
        if hello.protocol_name != PROTOCOL_NAME
            || !hello.protocol_versions.contains(&PROTOCOL_VERSION)
        {
            return Err((
                ServerErrorCode::UnsupportedProtocol,
                "no compatible RCDP protocol version".into(),
            ));
        }
        self.negotiated = true;
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut capabilities = vec![
            "targets.enumerate".into(),
            "targets.pick".into(),
            "targets.restore".into(),
            "app.icon.binary".into(),
            "actions.policy".into(),
            "window.geometry.opt_in".into(),
        ];
        #[cfg(target_os = "macos")]
        {
            capabilities.push("clipboard.text.v1".into());
            capabilities.push("clipboard.files.v1".into());
        }
        Ok(ServerMessage::Hello(Hello {
            protocol_name: PROTOCOL_NAME.into(),
            protocol_versions: vec![PROTOCOL_VERSION],
            capabilities,
            build_revision: Some(cua_media_protocol::build_revision().into()),
        }))
    }

    fn clipboard(&self, known_generation: Option<u64>) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        let (generation, text) = platform_clipboard_snapshot().map_err(|message| {
            (
                ServerErrorCode::Internal,
                format!("clipboard read failed: {message}"),
            )
        })?;
        Ok(ServerMessage::Clipboard {
            generation,
            text: (known_generation != Some(generation))
                .then_some(text)
                .flatten(),
        })
    }

    fn set_clipboard(&self, text: String) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        if text.len() > MAX_CLIPBOARD_TEXT_BYTES {
            return Err((
                ServerErrorCode::RateLimited,
                format!("clipboard text exceeds {MAX_CLIPBOARD_TEXT_BYTES} UTF-8 bytes"),
            ));
        }
        let generation = platform_clipboard_set(&text).map_err(|message| {
            (
                ServerErrorCode::Internal,
                format!("clipboard write failed: {message}"),
            )
        })?;
        Ok(ServerMessage::Clipboard {
            generation,
            text: Some(text),
        })
    }

    #[cfg(target_os = "macos")]
    fn clipboard_files(
        &self,
        known_generation: Option<u64>,
    ) -> Result<OutboundPacket, ServerFailure> {
        self.require_negotiated()?;
        let (generation, paths) = platform_clipboard_file_paths().map_err(|message| {
            (
                ServerErrorCode::Internal,
                format!("file clipboard read failed: {message}"),
            )
        })?;
        let (files, payload) = if known_generation == Some(generation) || paths.is_empty() {
            (None, Vec::new())
        } else {
            let (files, payload) = encode_clipboard_files(&paths)
                .map_err(|message| (ServerErrorCode::RateLimited, message))?;
            (Some(files), payload)
        };
        Ok(OutboundPacket::ClipboardFiles {
            message: ServerMessage::ClipboardFiles {
                generation,
                files,
                byte_len: payload.len() as u64,
            },
            payload: payload.into(),
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn clipboard_files(&self, _: Option<u64>) -> Result<OutboundPacket, ServerFailure> {
        Err((
            ServerErrorCode::Unsupported,
            "file clipboard synchronization is not available on this host".into(),
        ))
    }

    #[cfg(target_os = "macos")]
    fn set_clipboard_files(
        &self,
        files: Vec<ClipboardFile>,
        byte_len: u64,
        payload: &[u8],
    ) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        validate_clipboard_files(&files, byte_len, payload)
            .map_err(|message| (ServerErrorCode::RateLimited, message))?;
        let transfer = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0);
        let paths = materialize_clipboard_files(&files, payload, transfer)
            .map_err(|message| (ServerErrorCode::Internal, message))?;
        let generation = platform_clipboard_set_files(&paths).map_err(|message| {
            (
                ServerErrorCode::Internal,
                format!("file clipboard write failed: {message}"),
            )
        })?;
        Ok(ServerMessage::ClipboardFiles {
            generation,
            files: None,
            byte_len: 0,
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn set_clipboard_files(
        &self,
        _: Vec<ClipboardFile>,
        _: u64,
        _: &[u8],
    ) -> Result<ServerMessage, ServerFailure> {
        Err((
            ServerErrorCode::Unsupported,
            "file clipboard synchronization is not available on this host".into(),
        ))
    }

    fn open_session(&mut self, open: OpenSession) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        if !self.runtime.policy_ceiling.allows(open.policy) {
            return Err((
                ServerErrorCode::InvalidOpen,
                format!(
                    "requested policy {:?} exceeds the server ceiling {:?}",
                    open.policy, self.runtime.policy_ceiling
                ),
            ));
        }
        if open.max_fps == 0
            || open.max_fps > MAX_CAPTURE_FPS
            || open.max_dimension == 0
            || open.max_dimension > MAX_CAPTURE_DIMENSION
            || open.target_bitrate_kbps.is_some_and(|bitrate| {
                !(MIN_CAPTURE_BITRATE_KBPS..=MAX_CAPTURE_BITRATE_KBPS).contains(&bitrate)
            })
        {
            return Err((
                ServerErrorCode::InvalidOpen,
                format!(
                    "capture limits must be within 1..={MAX_CAPTURE_FPS} fps, 1..={MAX_CAPTURE_DIMENSION} pixels, and {MIN_CAPTURE_BITRATE_KBPS}..={MAX_CAPTURE_BITRATE_KBPS} kbps when a bitrate is set"
                ),
            ));
        }
        let target = self
            .runtime
            .targets
            .resolve(&open.window, open.target_epoch)
            .map_err(server_error)?;
        let geometry_control = match open.geometry_control {
            WindowGeometryControl::ObserveOnly => WindowGeometryControl::ObserveOnly,
            WindowGeometryControl::Bidirectional => {
                if open.policy == cua_media_protocol::SessionPolicy::ViewOnly {
                    return Err((
                        ServerErrorCode::InvalidOpen,
                        "bidirectional geometry requires a controllable session policy".into(),
                    ));
                }
                if !self
                    .runtime
                    .geometry
                    .supports(&target.id)
                    .map_err(server_error)?
                {
                    return Err((
                        ServerErrorCode::InvalidOpen,
                        "the target does not support background-safe window resizing".into(),
                    ));
                }
                WindowGeometryControl::Bidirectional
            }
            WindowGeometryControl::Unknown => {
                return Err((
                    ServerErrorCode::InvalidOpen,
                    "unknown window geometry control mode".into(),
                ));
            }
        };
        if self
            .sessions
            .values()
            .any(|session| session.target.id == target.id)
        {
            return Err((
                ServerErrorCode::InvalidOpen,
                "this connection already owns a session for the target".into(),
            ));
        }
        let formats = self
            .runtime
            .captures
            .formats(&target.id)
            .map_err(server_error)?;
        let (codec, format) = select_codec(&open.accepted_codecs, &formats).ok_or_else(|| {
            (
                ServerErrorCode::CodecUnavailable,
                "no mutually supported v1 frame format".into(),
            )
        })?;
        let action_capabilities = self
            .runtime
            .actions
            .capabilities(&target.id)
            .map_err(server_error)?;
        let interactive_input = self
            .runtime
            .inputs
            .open(&target.id, open.policy)
            .map_err(server_error)?;
        let session_id = WindowSessionId(format!(
            "session-{}",
            self.runtime.next_session_id.fetch_add(1, Ordering::Relaxed)
        ));
        if geometry_control == WindowGeometryControl::Bidirectional {
            let mut owners = self
                .runtime
                .geometry_owners
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if owners.contains_key(&target.id) {
                return Err((
                    ServerErrorCode::InvalidOpen,
                    "another session already controls this host window's geometry".into(),
                ));
            }
            owners.insert(target.id.clone(), session_id.clone());
        }
        let capture_generation = Arc::new(AtomicU64::new(1));
        let sink = Arc::new(HubCaptureSink::new(
            target.id.clone(),
            self.hub.clone(),
            target.descriptor.geometry.clone(),
            capture_generation.clone(),
            1,
        ));
        let capture = match self.runtime.captures.start(
            &target.id,
            &CaptureConfig {
                max_fps: open.max_fps,
                max_dimension: open.max_dimension,
                target_bitrate_kbps: open.target_bitrate_kbps,
                accepted_formats: vec![format],
            },
            sink,
        ) {
            Ok(capture) => capture,
            Err(error) => {
                self.release_geometry_owner(&target.id, &session_id);
                return Err(server_error(error));
            }
        };
        let snapshot = self.hub.target_snapshot(&target.id);
        let awaiting_keyframe = codec == VideoCodec::H264;
        let mut capabilities = vec![
            "video.latest_frame".into(),
            "video.preferences.runtime".into(),
        ];
        if geometry_control == WindowGeometryControl::Bidirectional {
            capabilities.push("window.geometry.bidirectional".into());
        }
        if interactive_input.is_some() {
            capabilities.push("input.interactive.v1".into());
        }
        if codec == VideoCodec::H264 {
            capabilities.extend([
                "video.h264.annex_b".into(),
                "video.codec_epoch".into(),
                "video.keyframe_request".into(),
            ]);
        }
        let opened = SessionOpened {
            session_id: session_id.clone(),
            target_epoch: target.id.epoch,
            geometry_epoch: snapshot
                .as_ref()
                .map_or(GeometryEpoch(0), |state| state.geometry_epoch),
            codec_epoch: CodecEpoch(1),
            geometry: snapshot
                .and_then(|state| state.geometry)
                .unwrap_or_else(|| target.descriptor.geometry.clone()),
            codec,
            max_fps: open.max_fps,
            max_dimension: open.max_dimension,
            target_bitrate_kbps: open.target_bitrate_kbps,
            capabilities,
            action_capabilities,
            policy: open.policy,
            geometry_control,
        };
        self.sessions.insert(
            session_id,
            ActiveSession {
                target,
                opened: opened.clone(),
                capture,
                interactive_input,
                last_input_sequence: None,
                input_events_dispatched: 0,
                latest_snapshot: None,
                frames_emitted: 0,
                bytes_emitted: 0,
                frames_replaced: 0,
                keyframe_requests: 0,
                actions_dispatched: 0,
                action_frame_timeouts: 0,
                preference_updates: 0,
                last_geometry_revision: 0,
                awaiting_keyframe,
                capture_format: format,
                capture_generation,
                codec_epoch_offset: 0,
            },
        );
        Ok(ServerMessage::SessionOpened(opened))
    }

    fn close_session(
        &mut self,
        session_id: &WindowSessionId,
    ) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        let session = self.sessions.remove(session_id).ok_or_else(|| {
            (
                ServerErrorCode::UnknownSession,
                "session does not exist".into(),
            )
        })?;
        let target = session.target.id.clone();
        session.capture.stop();
        self.release_geometry_owner(&target, session_id);
        self.hub.forget_target(&target);
        self.pending_action_correlations
            .retain(|_, pending| &pending.session_id != session_id);
        Ok(ServerMessage::SessionClosed {
            session_id: session_id.clone(),
        })
    }

    async fn set_window_geometry(
        &mut self,
        request: WindowGeometryRequest,
    ) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        tracing::debug!(
            target: "cua_spacesd_client::geometry",
            session_id = ?request.session_id,
            revision = request.revision,
            width_points = request.width_points,
            height_points = request.height_points,
            "host received window resize request"
        );
        let Some(session) = self.sessions.get_mut(&request.session_id) else {
            return Err((
                ServerErrorCode::UnknownSession,
                "session does not exist".into(),
            ));
        };
        if session.opened.geometry_control != WindowGeometryControl::Bidirectional {
            return Ok(ServerMessage::WindowGeometryResult(WindowGeometryResult {
                session_id: request.session_id,
                revision: request.revision,
                applied: false,
                width_points: request.width_points,
                height_points: request.height_points,
                error: Some("bidirectional window geometry was not granted".into()),
            }));
        }
        if request.revision <= session.last_geometry_revision {
            return Ok(ServerMessage::WindowGeometryResult(WindowGeometryResult {
                session_id: request.session_id,
                revision: request.revision,
                applied: false,
                width_points: request.width_points,
                height_points: request.height_points,
                error: Some("stale or duplicate geometry revision".into()),
            }));
        }
        session.last_geometry_revision = request.revision;
        if !(MIN_WINDOW_DIMENSION_POINTS..=MAX_WINDOW_DIMENSION_POINTS)
            .contains(&request.width_points)
            || !(MIN_WINDOW_DIMENSION_POINTS..=MAX_WINDOW_DIMENSION_POINTS)
                .contains(&request.height_points)
        {
            return Ok(ServerMessage::WindowGeometryResult(WindowGeometryResult {
                session_id: request.session_id,
                revision: request.revision,
                applied: false,
                width_points: request.width_points,
                height_points: request.height_points,
                error: Some(format!(
                    "window dimensions must be between {MIN_WINDOW_DIMENSION_POINTS} and {MAX_WINDOW_DIMENSION_POINTS} points"
                )),
            }));
        }
        let target = session.target.id.clone();
        let outcome = self
            .runtime
            .geometry
            .resize(&target, request.width_points, request.height_points)
            .await;
        match &outcome {
            Ok(applied) => tracing::debug!(
                target: "cua_spacesd_client::geometry",
                session_id = ?request.session_id,
                revision = request.revision,
                width_points = applied.width_points,
                height_points = applied.height_points,
                "host applied window resize"
            ),
            Err(error) => tracing::warn!(
                target: "cua_spacesd_client::geometry",
                session_id = ?request.session_id,
                revision = request.revision,
                error = %error.message,
                "host rejected window resize"
            ),
        }
        Ok(ServerMessage::WindowGeometryResult(match outcome {
            Ok(applied) => WindowGeometryResult {
                session_id: request.session_id,
                revision: request.revision,
                applied: true,
                width_points: applied.width_points,
                height_points: applied.height_points,
                error: None,
            },
            Err(error) => WindowGeometryResult {
                session_id: request.session_id,
                revision: request.revision,
                applied: false,
                width_points: request.width_points,
                height_points: request.height_points,
                error: Some(error.message),
            },
        }))
    }

    fn release_geometry_owner(&self, target: &ProviderTargetId, session_id: &WindowSessionId) {
        let mut owners = self
            .runtime
            .geometry_owners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owners.get(target) == Some(session_id) {
            owners.remove(target);
        }
    }

    async fn dispatch_interactive_input(&mut self, batch: InteractiveInputBatch) -> ServerMessage {
        let dispatch_started = Instant::now();
        tracing::debug!(
            target: "cua_spacesd_client::host_input",
            session_id = ?batch.session_id,
            first_sequence = batch.first_sequence,
            event_count = batch.events.len(),
            "host received interactive input batch"
        );
        if let Err((code, message)) = self.require_negotiated() {
            return ServerMessage::Error { code, message };
        }
        let session_id = batch.session_id.clone();
        let (target, lease, previous_sequence) = match self.sessions.get(&session_id) {
            Some(session) => (
                session.target.id.clone(),
                session.interactive_input.clone(),
                session.last_input_sequence,
            ),
            None => {
                return ServerMessage::Error {
                    code: ServerErrorCode::UnknownSession,
                    message: "session does not exist".into(),
                };
            }
        };
        let snapshot = self.hub.target_snapshot(&target);
        let through_sequence = match batch.validate() {
            Ok(sequence) => sequence,
            Err(message) => {
                return ServerMessage::InteractiveInputAcknowledgement(
                    InteractiveInputAcknowledgement {
                        session_id,
                        through_sequence: previous_sequence.unwrap_or(0),
                        delivered: false,
                        error: Some(action_error(
                            ActionErrorCode::DeliveryFailed,
                            message,
                            snapshot,
                        )),
                        host_dispatch_us: None,
                    },
                );
            }
        };
        let expected_sequence = match previous_sequence {
            None => 1,
            Some(sequence) => match sequence.checked_add(1) {
                Some(sequence) => sequence,
                None => {
                    return ServerMessage::InteractiveInputAcknowledgement(
                        InteractiveInputAcknowledgement {
                            session_id,
                            through_sequence: u64::MAX,
                            delivered: false,
                            error: Some(action_error(
                                ActionErrorCode::DeliveryFailed,
                                "interactive input sequence is exhausted",
                                snapshot,
                            )),
                            host_dispatch_us: None,
                        },
                    );
                }
            },
        };
        if batch.first_sequence != expected_sequence {
            return ServerMessage::InteractiveInputAcknowledgement(
                InteractiveInputAcknowledgement {
                    session_id,
                    through_sequence: previous_sequence.unwrap_or(0),
                    delivered: false,
                    error: Some(action_error(
                        ActionErrorCode::StaleTarget,
                        format!(
                            "input sequence must start at {expected_sequence}, received {}",
                            batch.first_sequence
                        ),
                        snapshot,
                    )),
                    host_dispatch_us: None,
                },
            );
        }
        let Some(lease) = lease else {
            return ServerMessage::InteractiveInputAcknowledgement(
                InteractiveInputAcknowledgement {
                    session_id,
                    through_sequence: previous_sequence.unwrap_or(0),
                    delivered: false,
                    error: Some(action_error(
                        ActionErrorCode::Unsupported,
                        "interactive input is not available for this session",
                        snapshot,
                    )),
                    host_dispatch_us: None,
                },
            );
        };

        // The batch is now accepted into this session's ordered stream. Even a
        // native rejection consumes its sequence so the client can continue
        // after reporting the failed range instead of deadlocking on retries.
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.last_input_sequence = Some(through_sequence);
        }
        let event_count = batch.events.len() as u64;
        let result = tokio::task::spawn_blocking(move || lease.dispatch(&batch)).await;
        match result {
            Ok(Ok(outcome)) if outcome.through_sequence == through_sequence => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.input_events_dispatched =
                        session.input_events_dispatched.saturating_add(event_count);
                }
                tracing::debug!(
                    target: "cua_spacesd_client::host_input",
                    ?session_id,
                    through_sequence,
                    event_count,
                    provider_dispatch_us = outcome.dispatch_micros,
                    host_total_us = dispatch_started.elapsed().as_micros(),
                    "host delivered interactive input batch"
                );
                ServerMessage::InteractiveInputAcknowledgement(InteractiveInputAcknowledgement {
                    session_id,
                    through_sequence,
                    delivered: true,
                    error: None,
                    host_dispatch_us: Some(outcome.dispatch_micros),
                })
            }
            Ok(Ok(_)) => {
                tracing::error!(
                    target: "cua_spacesd_client::host_input",
                    ?session_id,
                    through_sequence,
                    "interactive input provider acknowledged the wrong sequence"
                );
                ServerMessage::InteractiveInputAcknowledgement(InteractiveInputAcknowledgement {
                    session_id,
                    through_sequence,
                    delivered: false,
                    error: Some(action_error(
                        ActionErrorCode::DeliveryFailed,
                        "interactive input provider acknowledged the wrong sequence",
                        self.hub.target_snapshot(&target),
                    )),
                    host_dispatch_us: None,
                })
            }
            Ok(Err(error)) => {
                tracing::error!(
                    target: "cua_spacesd_client::host_input",
                    ?session_id,
                    through_sequence,
                    %error,
                    "interactive input provider rejected a batch"
                );
                ServerMessage::InteractiveInputAcknowledgement(InteractiveInputAcknowledgement {
                    session_id,
                    through_sequence,
                    delivered: false,
                    error: Some(provider_action_error(
                        error,
                        self.hub.target_snapshot(&target),
                    )),
                    host_dispatch_us: None,
                })
            }
            Err(error) => {
                tracing::error!(
                    target: "cua_spacesd_client::host_input",
                    ?session_id,
                    through_sequence,
                    %error,
                    "interactive input worker failed"
                );
                ServerMessage::InteractiveInputAcknowledgement(InteractiveInputAcknowledgement {
                    session_id,
                    through_sequence,
                    delivered: false,
                    error: Some(action_error(
                        ActionErrorCode::DeliveryFailed,
                        format!("interactive input worker failed: {error}"),
                        self.hub.target_snapshot(&target),
                    )),
                    host_dispatch_us: None,
                })
            }
        }
    }

    fn dispatch_action(
        &mut self,
        request: ActionRequest,
    ) -> Result<Option<ServerMessage>, ServerFailure> {
        self.require_negotiated()?;
        let session = self.sessions.get(&request.session_id).ok_or_else(|| {
            (
                ServerErrorCode::UnknownSession,
                "session does not exist".into(),
            )
        })?;
        let target = session.target.id.clone();
        let policy = session.opened.policy;
        let capabilities = session.opened.action_capabilities.clone();
        let latest_snapshot = session.latest_snapshot;
        let coordinate_space = self
            .hub
            .target_snapshot(&target)
            .and_then(|snapshot| snapshot.geometry)
            .or_else(|| Some(session.opened.geometry.clone()));

        let error = if contains_native_target_key(&request.arguments) {
            Some(action_error(
                ActionErrorCode::NativeTargetRejected,
                "native target identifiers are server-owned",
                self.hub.target_snapshot(&target),
            ))
        } else if let Err(error) = validate_basis(
            &request.basis,
            latest_snapshot,
            self.hub.target_snapshot(&target),
        ) {
            Some(error)
        } else {
            let capability = capabilities
                .iter()
                .find(|capability| capability.action == request.tool);
            match capability {
                None => Some(action_error(
                    ActionErrorCode::Unsupported,
                    "action is not advertised for this target",
                    self.hub.target_snapshot(&target),
                )),
                Some(capability) => enforce_action_policy(policy, capability)
                    .err()
                    .map(|error| provider_action_error(error, self.hub.target_snapshot(&target))),
            }
        };
        if let Some(error) = error {
            return Ok(Some(ServerMessage::ActionResult(ActionResult {
                action_id: request.action_id,
                delivered: false,
                error: Some(error),
                first_frame_sequence_after: None,
            })));
        }

        let action_id = request.action_id.clone();
        let session_id = request.session_id.clone();
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.actions_dispatched = session.actions_dispatched.saturating_add(1);
        }
        let action_sequence = self.hub.publish_action(ActionDispatch {
            target: Some(target.clone()),
            tool: request.tool.clone(),
            label: request.tool.clone(),
            wall_timestamp_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        });
        self.pending_action_correlations.insert(
            action_sequence,
            PendingActionCorrelation {
                action_id: action_id.clone(),
                session_id: session_id.clone(),
                deadline: None,
                first_frame_sequence_after: None,
            },
        );
        let invocation = ActionInvocation {
            action_id: request.action_id,
            action: request.tool,
            arguments: request.arguments,
            basis: request.basis,
            coordinate_space,
        };
        if self
            .provider_action_tx
            .try_send(ProviderActionJob {
                action_sequence,
                action_id: action_id.clone(),
                target: target.clone(),
                invocation,
                policy,
            })
            .is_err()
        {
            self.pending_action_correlations.remove(&action_sequence);
            return Ok(Some(ServerMessage::ActionResult(ActionResult {
                action_id,
                delivered: false,
                error: Some(action_error(
                    ActionErrorCode::RateLimited,
                    "the bounded provider action queue is full",
                    self.hub.target_snapshot(&target),
                )),
                first_frame_sequence_after: None,
            })));
        }
        Ok(None)
    }

    fn request_keyframe(
        &mut self,
        session_id: &WindowSessionId,
    ) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        let session = self.sessions.get_mut(session_id).ok_or_else(|| {
            (
                ServerErrorCode::UnknownSession,
                "session does not exist".into(),
            )
        })?;
        session.keyframe_requests = session.keyframe_requests.saturating_add(1);
        session.awaiting_keyframe = session.opened.codec == VideoCodec::H264;
        session.capture.request_keyframe();
        Ok(ServerMessage::KeyframeRequested {
            session_id: session_id.clone(),
        })
    }

    fn set_stream_preferences(
        &mut self,
        preferences: StreamPreferences,
    ) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        if preferences.max_fps == 0
            || preferences.max_fps > MAX_CAPTURE_FPS
            || preferences.max_dimension == 0
            || preferences.max_dimension > MAX_CAPTURE_DIMENSION
            || preferences.target_bitrate_kbps.is_some_and(|bitrate| {
                !(MIN_CAPTURE_BITRATE_KBPS..=MAX_CAPTURE_BITRATE_KBPS).contains(&bitrate)
            })
        {
            return Err((
                ServerErrorCode::InvalidOpen,
                format!(
                    "stream preferences must be within 1..={MAX_CAPTURE_FPS} fps, 1..={MAX_CAPTURE_DIMENSION} pixels, and {MIN_CAPTURE_BITRATE_KBPS}..={MAX_CAPTURE_BITRATE_KBPS} kbps when a bitrate is set"
                ),
            ));
        }
        let session = self.sessions.get(&preferences.session_id).ok_or_else(|| {
            (
                ServerErrorCode::UnknownSession,
                "session does not exist".into(),
            )
        })?;
        if session.opened.max_fps == preferences.max_fps
            && session.opened.max_dimension == preferences.max_dimension
            && session.opened.target_bitrate_kbps == preferences.target_bitrate_kbps
        {
            return Ok(ServerMessage::StreamPreferencesApplied(preferences));
        }
        let target = session.target.id.clone();
        let format = session.capture_format;
        let geometry = session.opened.geometry.clone();
        let generation_state = session.capture_generation.clone();
        let old_generation = generation_state.load(Ordering::Acquire);
        let Some(new_generation) = old_generation.checked_add(1) else {
            return Err((
                ServerErrorCode::Internal,
                "capture generation exhausted".into(),
            ));
        };
        generation_state.store(new_generation, Ordering::Release);
        let sink = Arc::new(HubCaptureSink::new(
            target.clone(),
            self.hub.clone(),
            geometry,
            generation_state.clone(),
            new_generation,
        ));
        let replacement = match self.runtime.captures.start(
            &target,
            &CaptureConfig {
                max_fps: preferences.max_fps,
                max_dimension: preferences.max_dimension,
                target_bitrate_kbps: preferences.target_bitrate_kbps,
                accepted_formats: vec![format],
            },
            sink,
        ) {
            Ok(capture) => capture,
            Err(error) => {
                generation_state.store(old_generation, Ordering::Release);
                return Err(server_error(error));
            }
        };

        let session = self
            .sessions
            .get_mut(&preferences.session_id)
            .expect("session was validated above");
        let retired = std::mem::replace(&mut session.capture, replacement);
        session.opened.max_fps = preferences.max_fps;
        session.opened.max_dimension = preferences.max_dimension;
        session.opened.target_bitrate_kbps = preferences.target_bitrate_kbps;
        session.preference_updates = session.preference_updates.saturating_add(1);
        if session.opened.codec == VideoCodec::H264 {
            session.codec_epoch_offset = session
                .opened
                .codec_epoch
                .0
                .max(session.codec_epoch_offset.saturating_add(1));
            session.awaiting_keyframe = true;
        }
        retired.stop();
        Ok(ServerMessage::StreamPreferencesApplied(preferences))
    }

    async fn window_state(
        &mut self,
        session_id: &WindowSessionId,
    ) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        let target = self
            .sessions
            .get(session_id)
            .ok_or_else(|| {
                (
                    ServerErrorCode::UnknownSession,
                    "session does not exist".into(),
                )
            })?
            .target
            .id
            .clone();
        let snapshot = self
            .runtime
            .accessibility
            .snapshot(&target)
            .await
            .map_err(server_error)?;
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.latest_snapshot = Some(snapshot.snapshot_id);
        }
        Ok(ServerMessage::WindowState(WindowState {
            session_id: session_id.clone(),
            snapshot_id: snapshot.snapshot_id,
            state: snapshot.state,
        }))
    }

    fn stats(&self, session_id: &WindowSessionId) -> Result<ServerMessage, ServerFailure> {
        self.require_negotiated()?;
        let session = self.sessions.get(session_id).ok_or_else(|| {
            (
                ServerErrorCode::UnknownSession,
                "session does not exist".into(),
            )
        })?;
        Ok(ServerMessage::Stats(StreamStats {
            session_id: session_id.clone(),
            frames_emitted: session.frames_emitted,
            frames_replaced: session.frames_replaced,
            keyframe_requests: session.keyframe_requests,
            pending_frames: 0,
            bytes_emitted: session.bytes_emitted,
            actions_dispatched: session.actions_dispatched,
            action_frame_timeouts: session.action_frame_timeouts,
            preference_updates: session.preference_updates,
            input_events_dispatched: session.input_events_dispatched,
        }))
    }

    fn require_negotiated(&self) -> Result<(), ServerFailure> {
        if self.negotiated {
            Ok(())
        } else {
            Err((
                ServerErrorCode::HelloRequired,
                "hello must complete first".into(),
            ))
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let sessions = self.sessions.drain().collect::<Vec<_>>();
        for (session_id, session) in sessions {
            session.capture.stop();
            self.release_geometry_owner(&session.target.id, &session_id);
        }
        self.hub.unsubscribe(self.subscriber_id);
    }
}

type ServerFailure = (ServerErrorCode, String);

fn server_error(error: ProviderError) -> ServerFailure {
    let code = match error.code {
        ProviderErrorCode::StaleTarget | ProviderErrorCode::TargetUnavailable => {
            ServerErrorCode::UnknownWindow
        }
        ProviderErrorCode::PermissionDenied | ProviderErrorCode::ConsentRequired => {
            ServerErrorCode::CaptureFailed
        }
        ProviderErrorCode::CaptureFailed => ServerErrorCode::CaptureFailed,
        ProviderErrorCode::Unsupported
        | ProviderErrorCode::ViewOnly
        | ProviderErrorCode::WouldRequireActivation => ServerErrorCode::Unsupported,
        ProviderErrorCode::DeliveryFailed | ProviderErrorCode::Internal => {
            ServerErrorCode::Internal
        }
    };
    (code, error.message)
}

fn provider_action_error(
    error: ProviderError,
    snapshot: Option<crate::TargetSnapshot>,
) -> ActionError {
    let code = match error.code {
        ProviderErrorCode::StaleTarget => ActionErrorCode::StaleTarget,
        ProviderErrorCode::PermissionDenied | ProviderErrorCode::ConsentRequired => {
            ActionErrorCode::PermissionDenied
        }
        ProviderErrorCode::TargetUnavailable => ActionErrorCode::WindowUnavailable,
        ProviderErrorCode::Unsupported => ActionErrorCode::Unsupported,
        ProviderErrorCode::ViewOnly => ActionErrorCode::ViewOnly,
        ProviderErrorCode::WouldRequireActivation => ActionErrorCode::WouldRequireActivation,
        ProviderErrorCode::DeliveryFailed
        | ProviderErrorCode::CaptureFailed
        | ProviderErrorCode::Internal => ActionErrorCode::DeliveryFailed,
    };
    action_error(code, error.message, snapshot)
}

fn action_error(
    code: ActionErrorCode,
    message: impl Into<String>,
    snapshot: Option<crate::TargetSnapshot>,
) -> ActionError {
    ActionError {
        code,
        message: message.into(),
        current_geometry_epoch: snapshot.map(|snapshot| snapshot.geometry_epoch),
    }
}

fn validate_basis(
    basis: &ActionBasis,
    latest_snapshot: Option<AccessibilitySnapshotId>,
    target: Option<crate::TargetSnapshot>,
) -> Result<(), ActionError> {
    match basis {
        ActionBasis::Pixel {
            geometry_epoch,
            frame_sequence,
        } => {
            let Some(target) = target else {
                return Err(action_error(
                    ActionErrorCode::WindowUnavailable,
                    "target has no captured frame",
                    None,
                ));
            };
            if target.geometry_epoch != *geometry_epoch
                || target.frame_sequence.0 < frame_sequence.0
            {
                return Err(action_error(
                    ActionErrorCode::StaleGeometry,
                    "pixel basis does not match current target geometry",
                    Some(target),
                ));
            }
        }
        ActionBasis::Accessibility { snapshot_id } => {
            if latest_snapshot != Some(*snapshot_id) {
                return Err(action_error(
                    ActionErrorCode::StaleAccessibilitySnapshot,
                    "accessibility snapshot is stale",
                    target,
                ));
            }
        }
        ActionBasis::None => {}
    }
    Ok(())
}

fn contains_native_target_key(value: &cua_media_protocol::Value) -> bool {
    match value {
        cua_media_protocol::Value::Object(map) => map.iter().any(|(key, value)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "pid" | "window_id" | "native_window_id" | "hwnd" | "portal_token"
            ) || contains_native_target_key(value)
        }),
        cua_media_protocol::Value::Array(values) => values.iter().any(contains_native_target_key),
        _ => false,
    }
}

fn select_codec(
    accepted: &[VideoCodec],
    formats: &[PixelFormat],
) -> Option<(VideoCodec, PixelFormat)> {
    accepted.iter().find_map(|codec| match codec {
        VideoCodec::Bgra if formats.contains(&PixelFormat::Bgra8) => {
            Some((VideoCodec::Bgra, PixelFormat::Bgra8))
        }
        VideoCodec::Png if formats.contains(&PixelFormat::Png) => {
            Some((VideoCodec::Png, PixelFormat::Png))
        }
        VideoCodec::H264 if formats.contains(&PixelFormat::H264AnnexB) => {
            Some((VideoCodec::H264, PixelFormat::H264AnnexB))
        }
        VideoCodec::Unknown | VideoCodec::Bgra | VideoCodec::Png | VideoCodec::H264 => None,
    })
}

struct WireFramePayload {
    codec: VideoCodec,
    payload: Arc<[u8]>,
    codec_epoch: CodecEpoch,
    keyframe: bool,
    encode_duration_us: Option<u32>,
}

fn frame_payload(frame: &SessionFrame, selected: &VideoCodec) -> Option<WireFramePayload> {
    match (&frame.payload, selected) {
        (FramePayload::Png(bytes), VideoCodec::Png) => Some(WireFramePayload {
            codec: VideoCodec::Png,
            payload: bytes.clone(),
            codec_epoch: CodecEpoch(1),
            keyframe: true,
            encode_duration_us: None,
        }),
        (FramePayload::Bgra { bytes, .. }, VideoCodec::Bgra) => Some(WireFramePayload {
            codec: VideoCodec::Bgra,
            payload: bytes.clone(),
            codec_epoch: CodecEpoch(1),
            keyframe: true,
            encode_duration_us: None,
        }),
        (
            FramePayload::H264 {
                bytes,
                codec_epoch,
                keyframe,
                encode_duration_us,
            },
            VideoCodec::H264,
        ) => Some(WireFramePayload {
            codec: VideoCodec::H264,
            payload: bytes.clone(),
            codec_epoch: *codec_epoch,
            keyframe: *keyframe,
            encode_duration_us: *encode_duration_us,
        }),
        _ => None,
    }
}

fn lifecycle_target(event: &RuntimeLifecycleEvent) -> &ProviderTargetId {
    match event {
        RuntimeLifecycleEvent::Opened { target, .. }
        | RuntimeLifecycleEvent::GeometryChanged { target, .. }
        | RuntimeLifecycleEvent::TitleChanged { target, .. }
        | RuntimeLifecycleEvent::Suspended { target, .. }
        | RuntimeLifecycleEvent::Resumed { target }
        | RuntimeLifecycleEvent::Closed { target } => target,
    }
}

fn protocol_lifecycle(event: &RuntimeLifecycleEvent) -> Option<WindowLifecycleEvent> {
    match event {
        RuntimeLifecycleEvent::Opened { .. } => None,
        RuntimeLifecycleEvent::GeometryChanged {
            geometry_epoch,
            geometry,
            ..
        } => Some(WindowLifecycleEvent::GeometryChanged {
            geometry_epoch: *geometry_epoch,
            geometry: geometry.clone(),
        }),
        RuntimeLifecycleEvent::TitleChanged { title, .. } => {
            Some(WindowLifecycleEvent::TitleChanged {
                title: title.clone(),
            })
        }
        RuntimeLifecycleEvent::Suspended { reason, .. } => Some(WindowLifecycleEvent::Suspended {
            reason: match reason.as_str() {
                "minimized" => SuspensionReason::Minimized,
                "consent_required" => SuspensionReason::ConsentRequired,
                "consent_revoked" => SuspensionReason::ConsentRevoked,
                "window_unavailable" => SuspensionReason::WindowUnavailable,
                _ => SuspensionReason::CaptureFailed,
            },
        }),
        RuntimeLifecycleEvent::Resumed { .. } => Some(WindowLifecycleEvent::Resumed),
        RuntimeLifecycleEvent::Closed { .. } => Some(WindowLifecycleEvent::Closed),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Mutex;

    use cua_media_protocol::{
        ActionCapability, ActionDeliveryGuarantee, FrameSequence, InputKeyState,
        InteractiveInputEvent, SessionPolicy, SurfaceGeometry, TargetEpoch, TargetGrant,
        TargetHandle, WindowDescriptor,
    };
    use cua_spacesd_provider_api::{
        AccessibilitySnapshot, ActionOutcome, AppliedWindowGeometry, BackendTargetKey, OwnedFrame,
        ProviderAppIcon, ProviderFuture,
    };

    use super::*;

    struct MockLease(AtomicBool);

    impl CaptureLease for MockLease {
        fn stop(&self) {
            self.0.store(true, Ordering::Release);
        }
    }

    struct MockProvider {
        target: Mutex<ProviderTarget>,
        sinks: Mutex<Vec<Arc<dyn CaptureSink>>>,
        configs: Mutex<Vec<CaptureConfig>>,
        guarantee: Mutex<ActionDeliveryGuarantee>,
        perform_count: AtomicUsize,
        perform_delay_ms: AtomicU64,
        emit_after_action: AtomicBool,
        last_coordinate_space: Mutex<Option<SurfaceGeometry>>,
        geometry_requests: Mutex<Vec<(u32, u32)>>,
        input_batches: Arc<Mutex<Vec<InteractiveInputBatch>>>,
    }

    impl MockProvider {
        fn new() -> Self {
            let descriptor = WindowDescriptor {
                window: TargetHandle("target-opaque".into()),
                target_epoch: TargetEpoch(1),
                app_name: "Fixture".into(),
                title: "Window".into(),
                geometry: SurfaceGeometry {
                    width_px: 2,
                    height_px: 2,
                    scale_factor: 2.0,
                },
                visible: true,
            };
            Self {
                target: Mutex::new(ProviderTarget {
                    id: ProviderTargetId {
                        key: BackendTargetKey::new("pid=42;window=99"),
                        epoch: TargetEpoch(1),
                    },
                    descriptor,
                    grant: Some(TargetGrant("grant-opaque".into())),
                }),
                sinks: Mutex::new(Vec::new()),
                configs: Mutex::new(Vec::new()),
                guarantee: Mutex::new(ActionDeliveryGuarantee::Background),
                perform_count: AtomicUsize::new(0),
                perform_delay_ms: AtomicU64::new(0),
                emit_after_action: AtomicBool::new(true),
                last_coordinate_space: Mutex::new(None),
                geometry_requests: Mutex::new(Vec::new()),
                input_batches: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn emit_frame(&self, timestamp: u64) {
            let sinks = self
                .sinks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            for sink in sinks {
                sink.on_event(CaptureEvent::Frame(OwnedFrame {
                    bytes: Arc::from(vec![0; 16]),
                    format: PixelFormat::Bgra8,
                    width_px: 2,
                    height_px: 2,
                    bytes_per_row: Some(8),
                    capture_timestamp_us: timestamp,
                    encode_duration_us: None,
                    codec_epoch: 1,
                    keyframe: true,
                }));
            }
        }
    }

    impl TargetProvider for MockProvider {
        fn enumerate(&self, _query: &TargetQuery) -> Result<Vec<ProviderTarget>, ProviderError> {
            Ok(vec![self
                .target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()])
        }

        fn pick(&self, _request: &PickTargetRequest) -> Result<ProviderTarget, ProviderError> {
            Ok(self
                .target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }

        fn restore(&self, grant: &TargetGrant) -> Result<ProviderTarget, ProviderError> {
            let target = self
                .target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if target.grant.as_ref() == Some(grant) {
                Ok(target)
            } else {
                Err(ProviderError::new(
                    ProviderErrorCode::TargetUnavailable,
                    "grant is unknown",
                ))
            }
        }

        fn resolve(
            &self,
            handle: &TargetHandle,
            epoch: TargetEpoch,
        ) -> Result<ProviderTarget, ProviderError> {
            let target = self
                .target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if &target.descriptor.window != handle || target.id.epoch != epoch {
                return Err(ProviderError::new(
                    ProviderErrorCode::StaleTarget,
                    "target handle or epoch is stale",
                ));
            }
            Ok(target)
        }

        fn app_icon(
            &self,
            handle: &TargetHandle,
            epoch: TargetEpoch,
        ) -> Result<Option<ProviderAppIcon>, ProviderError> {
            self.resolve(handle, epoch)?;
            Ok(Some(ProviderAppIcon {
                media_type: "application/x-apple-icns".into(),
                bytes: Arc::from([1, 2, 3, 4].as_slice()),
            }))
        }
    }

    impl CaptureProvider for MockProvider {
        fn formats(&self, _target: &ProviderTargetId) -> Result<Vec<PixelFormat>, ProviderError> {
            Ok(vec![PixelFormat::Bgra8])
        }

        fn start(
            &self,
            _target: &ProviderTargetId,
            config: &CaptureConfig,
            sink: Arc<dyn CaptureSink>,
        ) -> Result<Arc<dyn CaptureLease>, ProviderError> {
            self.configs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(config.clone());
            self.sinks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(sink);
            self.emit_frame(1);
            Ok(Arc::new(MockLease(AtomicBool::new(false))))
        }
    }

    impl ActionProvider for MockProvider {
        fn capabilities(
            &self,
            _target: &ProviderTargetId,
        ) -> Result<Vec<ActionCapability>, ProviderError> {
            Ok(vec![ActionCapability {
                action: "type_text".into(),
                guarantee: *self
                    .guarantee
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            }])
        }

        fn perform<'a>(
            &'a self,
            _target: &'a ProviderTargetId,
            action: ActionInvocation,
            _policy: SessionPolicy,
        ) -> ProviderFuture<'a, Result<ActionOutcome, ProviderError>> {
            Box::pin(async move {
                self.perform_count.fetch_add(1, Ordering::Relaxed);
                let delay_ms = self.perform_delay_ms.load(Ordering::Acquire);
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                *self
                    .last_coordinate_space
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = action.coordinate_space;
                if self.emit_after_action.load(Ordering::Acquire) {
                    self.emit_frame(2);
                }
                Ok(ActionOutcome {
                    delivered: true,
                    detail: None,
                })
            })
        }
    }

    impl AccessibilityProvider for MockProvider {
        fn snapshot<'a>(
            &'a self,
            _target: &'a ProviderTargetId,
        ) -> ProviderFuture<'a, Result<AccessibilitySnapshot, ProviderError>> {
            Box::pin(async {
                Ok(AccessibilitySnapshot {
                    snapshot_id: AccessibilitySnapshotId(7),
                    state: cua_media_protocol::Value::Object(Default::default()),
                })
            })
        }
    }

    impl WindowGeometryProvider for MockProvider {
        fn supports(&self, _target: &ProviderTargetId) -> Result<bool, ProviderError> {
            Ok(true)
        }

        fn resize<'a>(
            &'a self,
            _target: &'a ProviderTargetId,
            width_points: u32,
            height_points: u32,
        ) -> ProviderFuture<'a, Result<AppliedWindowGeometry, ProviderError>> {
            Box::pin(async move {
                self.geometry_requests
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((width_points, height_points));
                Ok(AppliedWindowGeometry {
                    width_points,
                    height_points,
                })
            })
        }
    }

    struct MockInputLease(Arc<Mutex<Vec<InteractiveInputBatch>>>);

    impl InteractiveInputLease for MockInputLease {
        fn dispatch(
            &self,
            batch: &InteractiveInputBatch,
        ) -> Result<cua_spacesd_provider_api::InteractiveInputOutcome, ProviderError> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(batch.clone());
            Ok(cua_spacesd_provider_api::InteractiveInputOutcome {
                through_sequence: batch.validate().expect("fixture batch is valid"),
                event_count: batch.events.len(),
                dispatch_micros: 42,
            })
        }
    }

    impl InteractiveInputProvider for MockProvider {
        fn open(
            &self,
            _target: &ProviderTargetId,
            policy: SessionPolicy,
        ) -> Result<Option<Arc<dyn InteractiveInputLease>>, ProviderError> {
            Ok((policy != SessionPolicy::ViewOnly).then(|| {
                Arc::new(MockInputLease(self.input_batches.clone()))
                    as Arc<dyn InteractiveInputLease>
            }))
        }
    }

    fn runtime(provider: Arc<MockProvider>) -> Arc<ServerRuntime> {
        ServerRuntime::new_with_interactive_input(
            provider.clone(),
            provider.clone(),
            provider.clone(),
            provider.clone(),
            provider.clone(),
            provider,
        )
        .into()
    }

    async fn hello(connection: &mut Connection) {
        let output = connection
            .handle(ClientMessage::Hello(Hello::default()))
            .await;
        assert!(matches!(
            output.first(),
            Some(OutboundPacket::Control(ServerMessage::Hello(_)))
        ));
    }

    #[tokio::test]
    async fn application_icon_is_target_validated_and_binary() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider).connect();
        hello(&mut connection).await;
        let output = connection
            .handle(ClientMessage::GetAppIcon {
                window: TargetHandle("target-opaque".into()),
                target_epoch: TargetEpoch(1),
            })
            .await;
        assert!(matches!(
            output.as_slice(),
            [OutboundPacket::AppIcon {
                descriptor: AppIconDescriptor {
                    media_type,
                    byte_len: 4,
                    ..
                },
                payload,
            }] if media_type == "application/x-apple-icns" && payload.as_ref() == [1, 2, 3, 4]
        ));
    }

    #[tokio::test]
    async fn oversized_clipboard_is_rejected_before_native_access() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider).connect();
        hello(&mut connection).await;
        let output = connection
            .handle(ClientMessage::SetClipboard {
                text: "x".repeat(MAX_CLIPBOARD_TEXT_BYTES + 1),
            })
            .await;
        assert!(matches!(
            output.as_slice(),
            [OutboundPacket::Control(ServerMessage::Error {
                code: ServerErrorCode::RateLimited,
                ..
            })]
        ));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn file_clipboard_rejects_paths_before_materializing_contents() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider).connect();
        hello(&mut connection).await;
        let bytes = b"safe contents";
        let output = connection
            .handle_packet(
                ClientMessage::SetClipboardFiles {
                    files: vec![ClipboardFile {
                        name: "../escape.txt".into(),
                        offset: 0,
                        byte_len: bytes.len() as u64,
                        sha256: format!("{:x}", Sha256::digest(bytes)),
                    }],
                    byte_len: bytes.len() as u64,
                },
                bytes.to_vec(),
            )
            .await;
        assert!(matches!(
            output.as_slice(),
            [OutboundPacket::Control(ServerMessage::Error {
                code: ServerErrorCode::RateLimited,
                ..
            })]
        ));
    }

    fn open_request(policy: SessionPolicy, epoch: u64) -> OpenSession {
        OpenSession {
            window: TargetHandle("target-opaque".into()),
            target_epoch: TargetEpoch(epoch),
            accepted_codecs: vec![VideoCodec::Bgra],
            max_fps: 15,
            max_dimension: 1280,
            target_bitrate_kbps: None,
            policy,
            geometry_control: WindowGeometryControl::ObserveOnly,
        }
    }

    async fn open(connection: &mut Connection, policy: SessionPolicy) -> WindowSessionId {
        let output = connection
            .handle(ClientMessage::OpenSession(open_request(policy, 1)))
            .await;
        match output.first() {
            Some(OutboundPacket::Control(ServerMessage::SessionOpened(opened))) => {
                opened.session_id.clone()
            }
            other => panic!("unexpected open response: {other:?}"),
        }
    }

    #[tokio::test]
    async fn interactive_input_is_ordered_acknowledged_and_counted() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider.clone()).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::AllowActivation).await;
        let batch = InteractiveInputBatch {
            session_id: session_id.clone(),
            first_sequence: 1,
            events: vec![
                InteractiveInputEvent::Key {
                    key: "left".into(),
                    state: InputKeyState::Down,
                    modifiers: Vec::new(),
                    repeat: false,
                },
                InteractiveInputEvent::Key {
                    key: "left".into(),
                    state: InputKeyState::Up,
                    modifiers: Vec::new(),
                    repeat: false,
                },
            ],
        };
        let output = connection
            .handle(ClientMessage::InteractiveInput(batch.clone()))
            .await;
        assert!(matches!(
            output.first(),
            Some(OutboundPacket::Control(
                ServerMessage::InteractiveInputAcknowledgement(acknowledgement)
            )) if acknowledgement.delivered
                && acknowledgement.through_sequence == 2
                && acknowledgement.host_dispatch_us == Some(42)
        ));
        assert_eq!(
            provider
                .input_batches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[batch]
        );

        let rejected = connection
            .handle(ClientMessage::InteractiveInput(InteractiveInputBatch {
                session_id: session_id.clone(),
                first_sequence: 4,
                events: vec![InteractiveInputEvent::TextCommit { text: "x".into() }],
            }))
            .await;
        assert!(matches!(
            rejected.first(),
            Some(OutboundPacket::Control(
                ServerMessage::InteractiveInputAcknowledgement(acknowledgement)
            )) if !acknowledgement.delivered && acknowledgement.through_sequence == 2
        ));

        let stats = connection
            .handle(ClientMessage::GetStats { session_id })
            .await;
        assert!(matches!(
            stats.first(),
            Some(OutboundPacket::Control(ServerMessage::Stats(stats)))
                if stats.input_events_dispatched == 2
        ));
    }

    async fn wait_for_video(
        connection: &mut Connection,
        width_px: u32,
        height_px: u32,
        capture_timestamp_us: Option<u64>,
    ) -> VideoFrameDescriptor {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(descriptor) =
                    connection
                        .drain_outbound()
                        .into_iter()
                        .find_map(|packet| match packet {
                            OutboundPacket::Video { descriptor, .. }
                                if descriptor.width_px == width_px
                                    && descriptor.height_px == height_px
                                    && capture_timestamp_us.is_none_or(|expected| {
                                        descriptor.capture_timestamp_us == expected
                                    }) =>
                            {
                                Some(descriptor)
                            }
                            _ => None,
                        })
                {
                    return descriptor;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("expected video frame arrives")
    }

    async fn wait_for_action_correlation(
        connection: &mut Connection,
        initial: &[OutboundPacket],
        expected_action_id: &str,
    ) -> ActionFrameCorrelation {
        if let Some(correlation) = initial.iter().find_map(|packet| match packet {
            OutboundPacket::Control(ServerMessage::ActionFrameCorrelation(correlation))
                if correlation.action_id == expected_action_id =>
            {
                Some(correlation.clone())
            }
            _ => None,
        }) {
            return correlation;
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(correlation) =
                    connection
                        .drain_outbound()
                        .into_iter()
                        .find_map(|packet| match packet {
                            OutboundPacket::Control(ServerMessage::ActionFrameCorrelation(
                                correlation,
                            )) if correlation.action_id == expected_action_id => Some(correlation),
                            _ => None,
                        })
                {
                    return correlation;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("expected action correlation arrives")
    }

    async fn wait_for_action_result(
        connection: &mut Connection,
        initial: &[OutboundPacket],
        expected_action_id: &str,
    ) -> (ActionResult, Vec<OutboundPacket>) {
        let mut packets = initial.to_vec();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(result) = packets.iter().find_map(|packet| match packet {
                    OutboundPacket::Control(ServerMessage::ActionResult(result))
                        if result.action_id == expected_action_id =>
                    {
                        Some(result.clone())
                    }
                    _ => None,
                }) {
                    return (result, packets);
                }
                packets.extend(connection.drain_outbound());
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("expected action result arrives")
    }

    fn bgra_frame(width_px: u32, height_px: u32, timestamp: u64) -> CaptureEvent {
        let stride = width_px.saturating_mul(4);
        CaptureEvent::Frame(OwnedFrame {
            bytes: Arc::from(vec![
                0;
                usize::try_from(stride.saturating_mul(height_px))
                    .expect("fixture frame fits usize")
            ]),
            format: PixelFormat::Bgra8,
            width_px,
            height_px,
            bytes_per_row: Some(stride),
            capture_timestamp_us: timestamp,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: true,
        })
    }

    #[tokio::test]
    async fn runtime_policy_ceiling_rejects_activation_session() {
        let fixture = Arc::new(MockProvider::new());
        let runtime = Arc::new(ServerRuntime::new_with_policy_ceiling(
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture.clone(),
            fixture,
            SessionPolicy::BackgroundOnly,
        ));
        let mut connection = runtime.connect();
        hello(&mut connection).await;

        let output = connection
            .handle(ClientMessage::OpenSession(open_request(
                SessionPolicy::AllowActivation,
                1,
            )))
            .await;
        assert!(matches!(
            output.as_slice(),
            [OutboundPacket::Control(ServerMessage::Error {
                code: ServerErrorCode::InvalidOpen,
                ..
            })]
        ));
    }

    #[tokio::test]
    async fn clients_keep_independent_capture_geometry_and_epochs() {
        let provider = Arc::new(MockProvider::new());
        let runtime = runtime(provider.clone());
        let mut compact = runtime.connect();
        let mut detailed = runtime.connect();
        hello(&mut compact).await;
        hello(&mut detailed).await;

        let mut compact_open = open_request(SessionPolicy::BackgroundOnly, 1);
        compact_open.max_dimension = 640;
        let compact_output = compact
            .handle(ClientMessage::OpenSession(compact_open))
            .await;
        assert!(matches!(
            compact_output.first(),
            Some(OutboundPacket::Control(ServerMessage::SessionOpened(_)))
        ));

        let mut detailed_open = open_request(SessionPolicy::BackgroundOnly, 1);
        detailed_open.max_dimension = 1_280;
        let detailed_output = detailed
            .handle(ClientMessage::OpenSession(detailed_open))
            .await;
        assert!(matches!(
            detailed_output.first(),
            Some(OutboundPacket::Control(ServerMessage::SessionOpened(_)))
        ));

        assert_eq!(
            provider
                .configs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|config| config.max_dimension)
                .collect::<Vec<_>>(),
            vec![640, 1_280]
        );
        let sinks = provider
            .sinks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let compact_sink = sinks[0].clone();
        let detailed_sink = sinks[1].clone();

        compact_sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
            width_px: 1,
            height_px: 1,
            scale_factor: 1.0,
        }));
        compact_sink.on_event(bgra_frame(1, 1, 10));
        detailed_sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
            width_px: 3,
            height_px: 2,
            scale_factor: 1.0,
        }));
        detailed_sink.on_event(bgra_frame(3, 2, 11));

        let compact_frame = wait_for_video(&mut compact, 1, 1, None).await;
        let detailed_frame = wait_for_video(&mut detailed, 3, 2, None).await;
        assert!(compact_frame.geometry_epoch.0 >= 1);
        assert!(detailed_frame.geometry_epoch.0 >= 1);
        let compact_epoch = compact_frame.geometry_epoch;

        detailed.drain_outbound();
        compact_sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
            width_px: 4,
            height_px: 1,
            scale_factor: 1.0,
        }));
        compact_sink.on_event(bgra_frame(4, 1, 12));
        let compact_frame = wait_for_video(&mut compact, 4, 1, None).await;
        assert_eq!(
            compact_frame.geometry_epoch,
            GeometryEpoch(compact_epoch.0.saturating_add(1))
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(detailed.drain_outbound().into_iter().all(|packet| {
            !matches!(
                packet,
                OutboundPacket::Video {
                    descriptor: VideoFrameDescriptor { width_px: 4, .. },
                    ..
                }
            )
        }));
    }

    #[tokio::test]
    async fn connection_rejects_duplicate_capture_ownership_for_one_target() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider.clone()).connect();
        hello(&mut connection).await;
        open(&mut connection, SessionPolicy::BackgroundOnly).await;

        let duplicate = connection
            .handle(ClientMessage::OpenSession(open_request(
                SessionPolicy::BackgroundOnly,
                1,
            )))
            .await;
        assert!(matches!(
            duplicate.first(),
            Some(OutboundPacket::Control(ServerMessage::Error {
                code: ServerErrorCode::InvalidOpen,
                ..
            }))
        ));
        assert_eq!(
            provider
                .configs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn explicitly_closed_target_can_reopen_without_stale_close_event() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::BackgroundOnly).await;

        connection
            .handle(ClientMessage::CloseSession { session_id })
            .await;
        let reopened = connection
            .handle(ClientMessage::OpenSession(open_request(
                SessionPolicy::BackgroundOnly,
                1,
            )))
            .await;
        assert!(matches!(
            reopened.first(),
            Some(OutboundPacket::Control(ServerMessage::SessionOpened(_)))
        ));
        assert!(!reopened.iter().any(|packet| matches!(
            packet,
            OutboundPacket::Control(ServerMessage::Lifecycle {
                event: WindowLifecycleEvent::Closed,
                ..
            })
        )));
    }

    #[tokio::test]
    async fn runtime_preferences_restart_only_the_active_capture_generation() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider.clone()).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::BackgroundOnly).await;
        let retired_sink = provider
            .sinks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[0]
            .clone();

        let preferences = StreamPreferences {
            session_id: session_id.clone(),
            max_fps: 30,
            max_dimension: 640,
            target_bitrate_kbps: Some(2_400),
        };
        let applied = connection
            .handle(ClientMessage::SetStreamPreferences(preferences.clone()))
            .await;
        assert!(matches!(
            applied.first(),
            Some(OutboundPacket::Control(
                ServerMessage::StreamPreferencesApplied(actual)
            )) if actual == &preferences
        ));
        assert_eq!(
            provider
                .configs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|config| {
                    (
                        config.max_fps,
                        config.max_dimension,
                        config.target_bitrate_kbps,
                    )
                })
                .collect::<Vec<_>>(),
            vec![(15, 1_280, None), (30, 640, Some(2_400))]
        );
        let active_sink = provider
            .sinks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last()
            .expect("replacement capture registered")
            .clone();

        retired_sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
            width_px: 9,
            height_px: 9,
            scale_factor: 1.0,
        }));
        retired_sink.on_event(bgra_frame(9, 9, 20));
        active_sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
            width_px: 3,
            height_px: 2,
            scale_factor: 1.0,
        }));
        active_sink.on_event(bgra_frame(3, 2, 21));
        wait_for_video(&mut connection, 3, 2, None).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(connection.drain_outbound().into_iter().all(|packet| {
            !matches!(
                packet,
                OutboundPacket::Video {
                    descriptor: VideoFrameDescriptor { width_px: 9, .. },
                    ..
                }
            )
        }));

        let stats = connection
            .handle(ClientMessage::GetStats { session_id })
            .await;
        assert!(matches!(
            stats.first(),
            Some(OutboundPacket::Control(ServerMessage::Stats(StreamStats {
                preference_updates: 1,
                ..
            })))
        ));
    }

    #[tokio::test]
    async fn bidirectional_geometry_is_revisioned_and_provider_owned() {
        let provider = Arc::new(MockProvider::new());
        let runtime = runtime(provider.clone());
        let mut connection = runtime.connect();
        hello(&mut connection).await;
        let mut request = open_request(SessionPolicy::BackgroundOnly, 1);
        request.geometry_control = WindowGeometryControl::Bidirectional;
        let output = connection.handle(ClientMessage::OpenSession(request)).await;
        let session_id = match output.first() {
            Some(OutboundPacket::Control(ServerMessage::SessionOpened(opened))) => {
                assert_eq!(
                    opened.geometry_control,
                    WindowGeometryControl::Bidirectional
                );
                assert!(opened
                    .capabilities
                    .iter()
                    .any(|capability| capability == "window.geometry.bidirectional"));
                opened.session_id.clone()
            }
            other => panic!("unexpected open response: {other:?}"),
        };

        let applied = connection
            .handle(ClientMessage::SetWindowGeometry(WindowGeometryRequest {
                session_id: session_id.clone(),
                revision: 1,
                width_points: 960,
                height_points: 640,
            }))
            .await;
        assert!(matches!(
            applied.first(),
            Some(OutboundPacket::Control(
                ServerMessage::WindowGeometryResult(WindowGeometryResult {
                    revision: 1,
                    applied: true,
                    width_points: 960,
                    height_points: 640,
                    error: None,
                    ..
                })
            ))
        ));

        let stale = connection
            .handle(ClientMessage::SetWindowGeometry(WindowGeometryRequest {
                session_id,
                revision: 1,
                width_points: 800,
                height_points: 600,
            }))
            .await;
        assert!(matches!(
            stale.first(),
            Some(OutboundPacket::Control(
                ServerMessage::WindowGeometryResult(WindowGeometryResult {
                    revision: 1,
                    applied: false,
                    ..
                })
            ))
        ));
        assert_eq!(
            *provider
                .geometry_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![(960, 640)]
        );
    }

    #[tokio::test]
    async fn geometry_control_has_one_owner_and_drop_releases_the_lease() {
        let provider = Arc::new(MockProvider::new());
        let runtime = runtime(provider);
        let mut request = open_request(SessionPolicy::BackgroundOnly, 1);
        request.geometry_control = WindowGeometryControl::Bidirectional;

        let mut owner = runtime.connect();
        hello(&mut owner).await;
        assert!(matches!(
            owner
                .handle(ClientMessage::OpenSession(request.clone()))
                .await
                .first(),
            Some(OutboundPacket::Control(ServerMessage::SessionOpened(_)))
        ));

        let mut contender = runtime.connect();
        hello(&mut contender).await;
        assert!(matches!(
            contender
                .handle(ClientMessage::OpenSession(request.clone()))
                .await
                .first(),
            Some(OutboundPacket::Control(ServerMessage::Error {
                code: ServerErrorCode::InvalidOpen,
                ..
            }))
        ));

        drop(owner);
        assert!(matches!(
            contender
                .handle(ClientMessage::OpenSession(request))
                .await
                .first(),
            Some(OutboundPacket::Control(ServerMessage::SessionOpened(_)))
        ));
    }

    #[tokio::test]
    async fn repeated_h264_preference_restarts_reserve_new_codec_epochs() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::BackgroundOnly).await;
        let session = connection.sessions.get_mut(&session_id).unwrap();
        session.opened.codec = VideoCodec::H264;
        session.capture_format = PixelFormat::H264AnnexB;

        for (index, max_dimension) in [640, 800].into_iter().enumerate() {
            connection
                .handle(ClientMessage::SetStreamPreferences(StreamPreferences {
                    session_id: session_id.clone(),
                    max_fps: 30,
                    max_dimension,
                    target_bitrate_kbps: None,
                }))
                .await;
            assert_eq!(
                connection.sessions[&session_id].codec_epoch_offset,
                u64::try_from(index + 1).unwrap()
            );
            assert!(connection.sessions[&session_id].awaiting_keyframe);
        }
    }

    #[tokio::test]
    async fn discovery_pick_and_restore_never_expose_native_keys() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider.clone()).connect();
        hello(&mut connection).await;

        for message in [
            ClientMessage::ListWindows {
                on_screen_only: false,
            },
            ClientMessage::PickWindow { prompt: None },
            ClientMessage::RestoreWindow {
                grant: TargetGrant("grant-opaque".into()),
            },
        ] {
            let output = connection.handle(message).await;
            let json = format!("{:?}", output.first());
            assert!(!json.contains("pid=42"));
            assert!(!json.contains("window=99"));
        }
    }

    #[tokio::test]
    async fn stale_target_epoch_is_rejected_before_capture() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider).connect();
        hello(&mut connection).await;
        let output = connection
            .handle(ClientMessage::OpenSession(open_request(
                SessionPolicy::BackgroundOnly,
                2,
            )))
            .await;
        assert!(matches!(
            output.first(),
            Some(OutboundPacket::Control(ServerMessage::Error {
                code: ServerErrorCode::UnknownWindow,
                ..
            }))
        ));
    }

    #[tokio::test]
    async fn background_only_rejects_activation_without_calling_provider() {
        let provider = Arc::new(MockProvider::new());
        *provider
            .guarantee
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            ActionDeliveryGuarantee::MayActivate;
        let mut connection = runtime(provider.clone()).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::BackgroundOnly).await;

        let output = connection
            .handle(ClientMessage::Action(ActionRequest {
                action_id: "action-1".into(),
                session_id,
                tool: "type_text".into(),
                arguments: cua_media_protocol::Value::Object(Default::default()),
                basis: ActionBasis::None,
            }))
            .await;
        assert!(matches!(
            output.first(),
            Some(OutboundPacket::Control(ServerMessage::ActionResult(
                ActionResult {
                    delivered: false,
                    error: Some(ActionError {
                        code: ActionErrorCode::WouldRequireActivation,
                        ..
                    }),
                    ..
                }
            )))
        ));
        assert_eq!(provider.perform_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn background_action_correlates_first_subsequent_frame() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider.clone()).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::BackgroundOnly).await;

        let output = connection
            .handle(ClientMessage::Action(ActionRequest {
                action_id: "action-1".into(),
                session_id: session_id.clone(),
                tool: "type_text".into(),
                arguments: cua_media_protocol::Value::Object(Default::default()),
                basis: ActionBasis::None,
            }))
            .await;
        let (result, output) = wait_for_action_result(&mut connection, &output, "action-1").await;
        assert!(result.delivered);
        assert_eq!(result.first_frame_sequence_after, None);
        let correlation = wait_for_action_correlation(&mut connection, &output, "action-1").await;
        assert_eq!(
            correlation.first_frame_sequence_after,
            Some(FrameSequence(2))
        );
        assert_eq!(
            *provider
                .last_coordinate_space
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Some(SurfaceGeometry {
                width_px: 2,
                height_px: 2,
                scale_factor: 2.0,
            })
        );

        connection
            .handle(ClientMessage::RequestKeyframe {
                session_id: session_id.clone(),
            })
            .await;
        let stats = connection
            .handle(ClientMessage::GetStats { session_id })
            .await;
        assert!(matches!(
            stats.first(),
            Some(OutboundPacket::Control(ServerMessage::Stats(StreamStats {
                frames_emitted,
                bytes_emitted,
                keyframe_requests: 1,
                actions_dispatched: 1,
                action_frame_timeouts: 0,
                ..
            }))) if *frames_emitted >= 1 && *bytes_emitted >= 16
        ));
    }

    #[tokio::test]
    async fn native_target_arguments_are_rejected() {
        let provider = Arc::new(MockProvider::new());
        let mut connection = runtime(provider.clone()).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::AllowActivation).await;
        let output = connection
            .handle(ClientMessage::Action(ActionRequest {
                action_id: "action-1".into(),
                session_id,
                tool: "type_text".into(),
                arguments: cua_media_protocol::Value::Object(
                    [("pid".into(), cua_media_protocol::Value::from(42))]
                        .into_iter()
                        .collect(),
                ),
                basis: ActionBasis::None,
            }))
            .await;
        assert!(matches!(
            output.first(),
            Some(OutboundPacket::Control(ServerMessage::ActionResult(
                ActionResult {
                    delivered: false,
                    error: Some(ActionError {
                        code: ActionErrorCode::NativeTargetRejected,
                        ..
                    }),
                    ..
                }
            )))
        ));
        assert_eq!(provider.perform_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn stats_report_missing_post_action_frame() {
        let provider = Arc::new(MockProvider::new());
        provider.emit_after_action.store(false, Ordering::Release);
        let mut connection = runtime(provider).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::BackgroundOnly).await;

        let dispatched = tokio::time::timeout(
            Duration::from_millis(50),
            connection.handle(ClientMessage::Action(ActionRequest {
                action_id: "action-without-frame".into(),
                session_id: session_id.clone(),
                tool: "type_text".into(),
                arguments: cua_media_protocol::Value::Object(Default::default()),
                basis: ActionBasis::None,
            })),
        )
        .await
        .expect("action dispatch does not wait for provider completion");
        let (result, _) =
            wait_for_action_result(&mut connection, &dispatched, "action-without-frame").await;
        assert!(result.delivered);
        assert_eq!(result.first_frame_sequence_after, None);

        tokio::time::sleep(ACTION_FRAME_WAIT + Duration::from_millis(10)).await;
        assert!(matches!(
            connection.drain_outbound().first(),
            Some(OutboundPacket::Control(
                ServerMessage::ActionFrameCorrelation(ActionFrameCorrelation {
                    action_id,
                    first_frame_sequence_after: None,
                    ..
                })
            )) if action_id == "action-without-frame"
        ));

        let stats = connection
            .handle(ClientMessage::GetStats { session_id })
            .await;
        assert!(matches!(
            stats.first(),
            Some(OutboundPacket::Control(ServerMessage::Stats(StreamStats {
                actions_dispatched: 1,
                action_frame_timeouts: 1,
                ..
            })))
        ));
    }

    #[tokio::test]
    async fn slow_provider_action_does_not_block_video_drain() {
        let provider = Arc::new(MockProvider::new());
        provider.perform_delay_ms.store(150, Ordering::Release);
        provider.emit_after_action.store(false, Ordering::Release);
        let mut connection = runtime(provider.clone()).connect();
        hello(&mut connection).await;
        let session_id = open(&mut connection, SessionPolicy::BackgroundOnly).await;

        let started = Instant::now();
        let dispatched = connection
            .handle(ClientMessage::Action(ActionRequest {
                action_id: "slow-action".into(),
                session_id,
                tool: "type_text".into(),
                arguments: cua_media_protocol::Value::Object(Default::default()),
                basis: ActionBasis::None,
            }))
            .await;
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(!dispatched.iter().any(|packet| matches!(
            packet,
            OutboundPacket::Control(ServerMessage::ActionResult(_))
        )));

        provider.emit_frame(9);
        let frame = wait_for_video(&mut connection, 2, 2, Some(9)).await;
        assert_eq!(frame.capture_timestamp_us, 9);
        assert!(started.elapsed() < Duration::from_millis(150));

        let (result, _) = wait_for_action_result(&mut connection, &[], "slow-action").await;
        assert!(result.delivered);
    }

    #[test]
    fn connection_mailbox_replaces_frames_per_target() {
        let target = ProviderTargetId {
            key: BackendTargetKey::new("fixture"),
            epoch: TargetEpoch(1),
        };
        let mailbox = RuntimeMailbox::default();
        for sequence in 1..=100 {
            mailbox.push_frame(Arc::new(SessionFrame {
                target: target.clone(),
                sequence: FrameSequence(sequence),
                geometry_epoch: GeometryEpoch(1),
                geometry: SurfaceGeometry {
                    width_px: 1,
                    height_px: 1,
                    scale_factor: 1.0,
                },
                capture_timestamp_us: sequence,
                first_after_actions: Vec::new(),
                payload: FramePayload::Bgra {
                    bytes: Arc::from(vec![0; 4]),
                    bytes_per_row: 4,
                },
            }));
        }
        let (items, replaced, overflowed) = mailbox.take();
        assert!(!overflowed);
        assert_eq!(items.len(), 1);
        assert_eq!(replaced.get(&target), Some(&99));
        assert!(matches!(
            items.first(),
            Some(RuntimeItem::Frame(frame)) if frame.sequence == FrameSequence(100)
        ));
    }

    #[tokio::test]
    async fn connection_mailbox_notifies_transport_without_polling() {
        let target = ProviderTargetId {
            key: BackendTargetKey::new("fixture"),
            epoch: TargetEpoch(1),
        };
        let outbound_ready = Arc::new(tokio::sync::Notify::new());
        let mailbox = RuntimeMailbox::new(outbound_ready.clone());
        mailbox.push_frame(Arc::new(SessionFrame {
            target,
            sequence: FrameSequence(1),
            geometry_epoch: GeometryEpoch(1),
            geometry: SurfaceGeometry {
                width_px: 1,
                height_px: 1,
                scale_factor: 1.0,
            },
            capture_timestamp_us: 1,
            first_after_actions: Vec::new(),
            payload: FramePayload::Bgra {
                bytes: Arc::from(vec![0; 4]),
                bytes_per_row: 4,
            },
        }));
        tokio::time::timeout(Duration::from_millis(10), outbound_ready.notified())
            .await
            .expect("frame publication should wake the transport immediately");
    }

    #[test]
    fn connection_mailbox_drains_lossless_control_before_video() {
        let target = ProviderTargetId {
            key: BackendTargetKey::new("fixture"),
            epoch: TargetEpoch(1),
        };
        let mailbox = RuntimeMailbox::default();
        mailbox.push_frame(Arc::new(SessionFrame {
            target: target.clone(),
            sequence: FrameSequence(1),
            geometry_epoch: GeometryEpoch(1),
            geometry: SurfaceGeometry {
                width_px: 1,
                height_px: 1,
                scale_factor: 1.0,
            },
            capture_timestamp_us: 1,
            first_after_actions: Vec::new(),
            payload: FramePayload::Bgra {
                bytes: Arc::from(vec![0; 4]),
                bytes_per_row: 4,
            },
        }));
        mailbox.push_event(Arc::new(SessionEvent::Action(crate::ActionObservation {
            sequence: ActionSequence(1),
            target: Some(target),
            tool: "click".into(),
            label: "click".into(),
            wall_timestamp_ms: 1,
        })));

        let (items, _, _) = mailbox.take();
        assert!(matches!(items.first(), Some(RuntimeItem::Event(_))));
        assert!(matches!(items.get(1), Some(RuntimeItem::Frame(_))));
    }

    #[test]
    fn connection_mailbox_bounds_lossless_event_backlog() {
        let target = ProviderTargetId {
            key: BackendTargetKey::new("fixture"),
            epoch: TargetEpoch(1),
        };
        let mailbox = RuntimeMailbox::default();
        for _ in 0..=MAX_PENDING_RUNTIME_EVENTS {
            mailbox.push_event(Arc::new(SessionEvent::Lifecycle(
                RuntimeLifecycleEvent::Resumed {
                    target: target.clone(),
                },
            )));
        }
        let (items, replaced, overflowed) = mailbox.take();
        assert!(overflowed);
        assert!(replaced.is_empty());
        assert_eq!(items.len(), MAX_PENDING_RUNTIME_EVENTS);
    }
}
