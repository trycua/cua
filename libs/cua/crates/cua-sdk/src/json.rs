//! The JSON escape hatch: any `cua.env.v1` RPC with proto3-JSON request
//! and response, dispatched through the typed prost codec. The tables are
//! checked against the compiled contract by
//! `exported_methods_cover_every_env_rpc`.

use crate::{CuaError, Result};
use cua_proto::env::v1 as pb;

/// A channel the generated clients (and this dispatcher) can run on:
/// `cua_spacesd_client::GrpcChannel` natively, the fetch-based gRPC-Web client in the
/// browser.
pub(crate) trait Channel:
    tonic::client::GrpcService<
        tonic::body::Body,
        Error: Into<tonic::codegen::StdError>,
        ResponseBody: tonic::codegen::Body<
            Data = tonic::codegen::Bytes,
            Error: Into<tonic::codegen::StdError>,
        > + Send
                          + 'static,
    >
{
}

impl<T> Channel for T where
    T: tonic::client::GrpcService<
            tonic::body::Body,
            Error: Into<tonic::codegen::StdError>,
            ResponseBody: tonic::codegen::Body<
                Data = tonic::codegen::Bytes,
                Error: Into<tonic::codegen::StdError>,
            > + Send
                              + 'static,
        >
{
}

fn status(s: tonic::Status) -> CuaError {
    #[cfg(not(any(target_arch = "wasm32", feature = "web")))]
    {
        s.into()
    }
    #[cfg(any(target_arch = "wasm32", feature = "web"))]
    {
        CuaError::Env(format!("{:?}: {}", s.code(), s.message()))
    }
}

/// Unary RPCs reachable through `SpacesdClient.call_json`.
pub const JSON_UNARY_METHODS: &[&str] = &[
    "/cua.env.v1.AccessibilityService/GetTree",
    "/cua.env.v1.AccessibilityService/Find",
    "/cua.env.v1.AccessibilityService/Act",
    "/cua.env.v1.ComputerService/Screenshot",
    "/cua.env.v1.ComputerService/Pointer",
    "/cua.env.v1.ComputerService/Keyboard",
    "/cua.env.v1.ComputerService/GetClipboard",
    "/cua.env.v1.ComputerService/SetClipboard",
    "/cua.env.v1.ComputerService/GetCursorPosition",
    "/cua.env.v1.ComputerService/ListDisplays",
    "/cua.env.v1.DriverService/ListTools",
    "/cua.env.v1.DriverService/CallTool",
    "/cua.env.v1.FilesystemService/Stat",
    "/cua.env.v1.FilesystemService/ListDir",
    "/cua.env.v1.FilesystemService/MakeDir",
    "/cua.env.v1.FilesystemService/Move",
    "/cua.env.v1.FilesystemService/Remove",
    "/cua.env.v1.FilesystemService/CreateWatcher",
    "/cua.env.v1.FilesystemService/GetWatcherEvents",
    "/cua.env.v1.FilesystemService/RemoveWatcher",
    "/cua.env.v1.FilesystemService/BeginUpload",
    "/cua.env.v1.FilesystemService/UploadChunk",
    "/cua.env.v1.FilesystemService/CommitUpload",
    "/cua.env.v1.FilesystemService/AbortUpload",
    "/cua.env.v1.FilesystemService/CreateSignedUrl",
    "/cua.env.v1.HostSpacesService/GetHostSpaces",
    "/cua.env.v1.HostSpacesService/CreateHostSpace",
    "/cua.env.v1.HostSpacesService/DeleteHostSpace",
    "/cua.env.v1.HostSpacesService/CancelHostSpace",
    "/cua.env.v1.HostSpacesService/SetHostSpacePower",
    "/cua.env.v1.HostSpacesService/DeleteCloudSpace",
    "/cua.env.v1.VolumeService/AttachVolume",
    "/cua.env.v1.VolumeService/DetachVolume",
    "/cua.env.v1.VolumeService/GetVolumeStatus",
    "/cua.env.v1.PresenceService/UpdateCursor",
    "/cua.env.v1.PresenceService/Leave",
    "/cua.env.v1.ProcessService/ListProcesses",
    "/cua.env.v1.ProcessService/SendInput",
    "/cua.env.v1.ProcessService/SignalProcess",
    "/cua.env.v1.ProcessService/CloseStdin",
    "/cua.env.v1.ProcessService/ResizePty",
    "/cua.env.v1.StreamService/ListTargets",
    "/cua.env.v1.StreamService/OpenMedia",
    "/cua.env.v1.StreamService/SetPreferences",
    "/cua.env.v1.StreamService/RequestKeyframe",
    "/cua.env.v1.StreamService/CloseMedia",
    "/cua.env.v1.SystemService/GetCapabilities",
    "/cua.env.v1.SystemService/Init",
    "/cua.env.v1.SystemService/Health",
    "/cua.env.v1.SystemService/Metrics",
    "/cua.env.v1.SystemService/Shutdown",
    "/cua.env.v1.SystemService/DiagnoseOnce",
    "/cua.env.v1.SystemService/CreateViewerTicket",
    "/cua.env.v1.SystemService/AttachRelay",
    "/cua.env.v1.SystemService/DetachRelay",
    "/cua.env.v1.TeleportService/GetManifest",
    "/cua.env.v1.TeleportService/ImportSession",
    "/cua.env.v1.TeleportService/BeginReceiveFiles",
    "/cua.env.v1.TeleportService/ReceiveFilesChunk",
    "/cua.env.v1.TeleportService/CommitReceiveFiles",
    "/cua.env.v1.TeleportService/AbortReceiveFiles",
    "/cua.env.v1.TeleportService/WipeImport",
    "/cua.env.v1.TunnelService/Forward",
    "/cua.env.v1.TunnelService/ListForwards",
    "/cua.env.v1.TunnelService/CloseForward",
    "/cua.env.v1.TunnelService/StartHotspot",
    "/cua.env.v1.TunnelService/StopHotspot",
    "/cua.env.v1.TunnelService/GetHotspotStatus",
    "/cua.env.v1.WindowsService/ListWindows",
    "/cua.env.v1.WindowsService/GetWindow",
    "/cua.env.v1.WindowsService/ActivateWindow",
    "/cua.env.v1.WindowsService/SetWindowBounds",
    "/cua.env.v1.WindowsService/MinimizeWindow",
    "/cua.env.v1.WindowsService/MaximizeWindow",
    "/cua.env.v1.WindowsService/RestoreWindow",
    "/cua.env.v1.WindowsService/CloseWindow",
    "/cua.env.v1.WindowsService/LaunchApp",
    "/cua.env.v1.WindowsService/Open",
];

/// Server-streaming RPCs reachable through `SpacesdClient.call_json_stream`.
pub const JSON_STREAM_METHODS: &[&str] = &[
    "/cua.env.v1.FilesystemService/WatchDir",
    "/cua.env.v1.FilesystemService/ReadFile",
    "/cua.env.v1.PresenceService/Join",
    "/cua.env.v1.SystemService/Diagnose",
    "/cua.env.v1.WindowsService/WatchWindows",
];

/// RPCs exposed only through typed methods (`spawn`, `attach`).
pub const TYPED_ONLY_METHODS: &[&str] = &[
    "/cua.env.v1.ProcessService/ConnectProcess",
    "/cua.env.v1.ProcessService/StartProcess",
];

/// Accepts `/cua.env.v1.Svc/Method`, `cua.env.v1.Svc/Method` or
/// `Svc/Method`.
pub(crate) fn normalize(method: &str) -> Result<String> {
    let m = method.trim().trim_start_matches('/');
    let full = if m.starts_with("cua.env.v1.") {
        format!("/{m}")
    } else {
        format!("/cua.env.v1.{m}")
    };
    if JSON_UNARY_METHODS.contains(&full.as_str()) || JSON_STREAM_METHODS.contains(&full.as_str()) {
        Ok(full)
    } else {
        Err(CuaError::InvalidArgument(format!(
            "{method:?} is not a JSON-callable cua.env.v1 method"
        )))
    }
}

fn parse<T: serde::de::DeserializeOwned>(json: &str) -> Result<T> {
    let json = if json.trim().is_empty() { "{}" } else { json };
    Ok(serde_json::from_str(json)?)
}

async fn unary<C: Channel, Req, Resp>(ch: C, path: &'static str, json: &str) -> Result<String>
where
    Req: prost::Message + serde::de::DeserializeOwned + Send + Sync + 'static,
    Resp: prost::Message + Default + serde::Serialize + Send + Sync + 'static,
{
    let req: Req = parse(json)?;
    let mut grpc = tonic::client::Grpc::new(ch)
        .max_decoding_message_size(MAX_MESSAGE_BYTES)
        .max_encoding_message_size(MAX_MESSAGE_BYTES);
    grpc.ready()
        .await
        .map_err(|e| CuaError::Transport(e.into().to_string()))?;
    let codec = tonic_prost::ProstCodec::<Req, Resp>::default();
    let resp = grpc
        .unary(
            tonic::Request::new(req),
            http::uri::PathAndQuery::from_static(path),
            codec,
        )
        .await
        .map_err(status)?;
    Ok(serde_json::to_string(resp.get_ref())?)
}

#[cfg(not(any(target_arch = "wasm32", feature = "web")))]
async fn stream<C: Channel, Req, Resp>(
    ch: C,
    path: &'static str,
    json: &str,
    max: u32,
    timeout: std::time::Duration,
) -> Result<Vec<String>>
where
    Req: prost::Message + serde::de::DeserializeOwned + Send + Sync + 'static,
    Resp: prost::Message + Default + serde::Serialize + Send + Sync + 'static,
{
    let req: Req = parse(json)?;
    let mut grpc = tonic::client::Grpc::new(ch)
        .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES);
    grpc.ready()
        .await
        .map_err(|e| CuaError::Transport(e.into().to_string()))?;
    let codec = tonic_prost::ProstCodec::<Req, Resp>::default();
    let mut s = grpc
        .server_streaming(
            tonic::Request::new(req),
            http::uri::PathAndQuery::from_static(path),
            codec,
        )
        .await
        .map_err(status)?
        .into_inner();
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    // Bounded: at most `max` messages and at most `timeout`.
    while (out.len() as u32) < max {
        match tokio::time::timeout_at(deadline, s.message()).await {
            Ok(Ok(Some(m))) => out.push(serde_json::to_string(&m)?),
            Ok(Ok(None)) | Err(_) => break,
            Ok(Err(s)) => return Err(status(s)),
        }
    }
    Ok(out)
}

/// Largest message accepted either way (same as cua-spacesd-client).
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

pub(crate) async fn call_unary<C: Channel>(ch: C, path: &str, json: &str) -> Result<String> {
    match path {
        "/cua.env.v1.AccessibilityService/GetTree" => {
            unary::<C, pb::GetTreeRequest, pb::GetTreeResponse>(
                ch,
                "/cua.env.v1.AccessibilityService/GetTree",
                json,
            )
            .await
        }
        "/cua.env.v1.AccessibilityService/Find" => {
            unary::<C, pb::FindRequest, pb::FindResponse>(
                ch,
                "/cua.env.v1.AccessibilityService/Find",
                json,
            )
            .await
        }
        "/cua.env.v1.AccessibilityService/Act" => {
            unary::<C, pb::ActRequest, pb::ActResponse>(
                ch,
                "/cua.env.v1.AccessibilityService/Act",
                json,
            )
            .await
        }
        "/cua.env.v1.ComputerService/Screenshot" => {
            unary::<C, pb::ScreenshotRequest, pb::ScreenshotResponse>(
                ch,
                "/cua.env.v1.ComputerService/Screenshot",
                json,
            )
            .await
        }
        "/cua.env.v1.ComputerService/Pointer" => {
            unary::<C, pb::PointerRequest, pb::PointerResponse>(
                ch,
                "/cua.env.v1.ComputerService/Pointer",
                json,
            )
            .await
        }
        "/cua.env.v1.ComputerService/Keyboard" => {
            unary::<C, pb::KeyboardRequest, pb::KeyboardResponse>(
                ch,
                "/cua.env.v1.ComputerService/Keyboard",
                json,
            )
            .await
        }
        "/cua.env.v1.ComputerService/GetClipboard" => {
            unary::<C, pb::GetClipboardRequest, pb::GetClipboardResponse>(
                ch,
                "/cua.env.v1.ComputerService/GetClipboard",
                json,
            )
            .await
        }
        "/cua.env.v1.ComputerService/SetClipboard" => {
            unary::<C, pb::SetClipboardRequest, pb::SetClipboardResponse>(
                ch,
                "/cua.env.v1.ComputerService/SetClipboard",
                json,
            )
            .await
        }
        "/cua.env.v1.ComputerService/GetCursorPosition" => {
            unary::<C, pb::GetCursorPositionRequest, pb::GetCursorPositionResponse>(
                ch,
                "/cua.env.v1.ComputerService/GetCursorPosition",
                json,
            )
            .await
        }
        "/cua.env.v1.ComputerService/ListDisplays" => {
            unary::<C, pb::ListDisplaysRequest, pb::ListDisplaysResponse>(
                ch,
                "/cua.env.v1.ComputerService/ListDisplays",
                json,
            )
            .await
        }
        "/cua.env.v1.DriverService/ListTools" => {
            unary::<C, pb::ListToolsRequest, pb::ListToolsResponse>(
                ch,
                "/cua.env.v1.DriverService/ListTools",
                json,
            )
            .await
        }
        "/cua.env.v1.DriverService/CallTool" => {
            unary::<C, pb::CallToolRequest, pb::CallToolResponse>(
                ch,
                "/cua.env.v1.DriverService/CallTool",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/Stat" => {
            unary::<C, pb::StatRequest, pb::StatResponse>(
                ch,
                "/cua.env.v1.FilesystemService/Stat",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/ListDir" => {
            unary::<C, pb::ListDirRequest, pb::ListDirResponse>(
                ch,
                "/cua.env.v1.FilesystemService/ListDir",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/MakeDir" => {
            unary::<C, pb::MakeDirRequest, pb::MakeDirResponse>(
                ch,
                "/cua.env.v1.FilesystemService/MakeDir",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/Move" => {
            unary::<C, pb::MoveRequest, pb::MoveResponse>(
                ch,
                "/cua.env.v1.FilesystemService/Move",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/Remove" => {
            unary::<C, pb::RemoveRequest, pb::RemoveResponse>(
                ch,
                "/cua.env.v1.FilesystemService/Remove",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/CreateWatcher" => {
            unary::<C, pb::CreateWatcherRequest, pb::CreateWatcherResponse>(
                ch,
                "/cua.env.v1.FilesystemService/CreateWatcher",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/GetWatcherEvents" => {
            unary::<C, pb::GetWatcherEventsRequest, pb::GetWatcherEventsResponse>(
                ch,
                "/cua.env.v1.FilesystemService/GetWatcherEvents",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/RemoveWatcher" => {
            unary::<C, pb::RemoveWatcherRequest, pb::RemoveWatcherResponse>(
                ch,
                "/cua.env.v1.FilesystemService/RemoveWatcher",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/BeginUpload" => {
            unary::<C, pb::BeginUploadRequest, pb::BeginUploadResponse>(
                ch,
                "/cua.env.v1.FilesystemService/BeginUpload",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/UploadChunk" => {
            unary::<C, pb::UploadChunkRequest, pb::UploadChunkResponse>(
                ch,
                "/cua.env.v1.FilesystemService/UploadChunk",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/CommitUpload" => {
            unary::<C, pb::CommitUploadRequest, pb::CommitUploadResponse>(
                ch,
                "/cua.env.v1.FilesystemService/CommitUpload",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/AbortUpload" => {
            unary::<C, pb::AbortUploadRequest, pb::AbortUploadResponse>(
                ch,
                "/cua.env.v1.FilesystemService/AbortUpload",
                json,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/CreateSignedUrl" => {
            unary::<C, pb::CreateSignedUrlRequest, pb::CreateSignedUrlResponse>(
                ch,
                "/cua.env.v1.FilesystemService/CreateSignedUrl",
                json,
            )
            .await
        }
        "/cua.env.v1.HostSpacesService/GetHostSpaces" => {
            unary::<C, pb::GetHostSpacesRequest, pb::GetHostSpacesResponse>(
                ch,
                "/cua.env.v1.HostSpacesService/GetHostSpaces",
                json,
            )
            .await
        }
        "/cua.env.v1.HostSpacesService/CreateHostSpace" => {
            unary::<C, pb::CreateHostSpaceRequest, pb::CreateHostSpaceResponse>(
                ch,
                "/cua.env.v1.HostSpacesService/CreateHostSpace",
                json,
            )
            .await
        }
        "/cua.env.v1.HostSpacesService/DeleteHostSpace" => {
            unary::<C, pb::DeleteHostSpaceRequest, pb::DeleteHostSpaceResponse>(
                ch,
                "/cua.env.v1.HostSpacesService/DeleteHostSpace",
                json,
            )
            .await
        }
        "/cua.env.v1.HostSpacesService/CancelHostSpace" => {
            unary::<C, pb::CancelHostSpaceRequest, pb::CancelHostSpaceResponse>(
                ch,
                "/cua.env.v1.HostSpacesService/CancelHostSpace",
                json,
            )
            .await
        }
        "/cua.env.v1.HostSpacesService/SetHostSpacePower" => {
            unary::<C, pb::SetHostSpacePowerRequest, pb::SetHostSpacePowerResponse>(
                ch,
                "/cua.env.v1.HostSpacesService/SetHostSpacePower",
                json,
            )
            .await
        }
        "/cua.env.v1.HostSpacesService/DeleteCloudSpace" => {
            unary::<C, pb::DeleteCloudSpaceRequest, pb::DeleteCloudSpaceResponse>(
                ch,
                "/cua.env.v1.HostSpacesService/DeleteCloudSpace",
                json,
            )
            .await
        }
        "/cua.env.v1.VolumeService/AttachVolume" => {
            unary::<C, pb::AttachVolumeRequest, pb::AttachVolumeResponse>(
                ch,
                "/cua.env.v1.VolumeService/AttachVolume",
                json,
            )
            .await
        }
        "/cua.env.v1.VolumeService/DetachVolume" => {
            unary::<C, pb::DetachVolumeRequest, pb::DetachVolumeResponse>(
                ch,
                "/cua.env.v1.VolumeService/DetachVolume",
                json,
            )
            .await
        }
        "/cua.env.v1.VolumeService/GetVolumeStatus" => {
            unary::<C, pb::GetVolumeStatusRequest, pb::GetVolumeStatusResponse>(
                ch,
                "/cua.env.v1.VolumeService/GetVolumeStatus",
                json,
            )
            .await
        }
        "/cua.env.v1.PresenceService/UpdateCursor" => {
            unary::<C, pb::UpdateCursorRequest, pb::UpdateCursorResponse>(
                ch,
                "/cua.env.v1.PresenceService/UpdateCursor",
                json,
            )
            .await
        }
        "/cua.env.v1.PresenceService/Leave" => {
            unary::<C, pb::LeaveRequest, pb::LeaveResponse>(
                ch,
                "/cua.env.v1.PresenceService/Leave",
                json,
            )
            .await
        }
        "/cua.env.v1.ProcessService/ListProcesses" => {
            unary::<C, pb::ListProcessesRequest, pb::ListProcessesResponse>(
                ch,
                "/cua.env.v1.ProcessService/ListProcesses",
                json,
            )
            .await
        }
        "/cua.env.v1.ProcessService/SendInput" => {
            unary::<C, pb::SendInputRequest, pb::SendInputResponse>(
                ch,
                "/cua.env.v1.ProcessService/SendInput",
                json,
            )
            .await
        }
        "/cua.env.v1.ProcessService/SignalProcess" => {
            unary::<C, pb::SignalProcessRequest, pb::SignalProcessResponse>(
                ch,
                "/cua.env.v1.ProcessService/SignalProcess",
                json,
            )
            .await
        }
        "/cua.env.v1.ProcessService/CloseStdin" => {
            unary::<C, pb::CloseStdinRequest, pb::CloseStdinResponse>(
                ch,
                "/cua.env.v1.ProcessService/CloseStdin",
                json,
            )
            .await
        }
        "/cua.env.v1.ProcessService/ResizePty" => {
            unary::<C, pb::ResizePtyRequest, pb::ResizePtyResponse>(
                ch,
                "/cua.env.v1.ProcessService/ResizePty",
                json,
            )
            .await
        }
        "/cua.env.v1.StreamService/ListTargets" => {
            unary::<C, pb::ListTargetsRequest, pb::ListTargetsResponse>(
                ch,
                "/cua.env.v1.StreamService/ListTargets",
                json,
            )
            .await
        }
        "/cua.env.v1.StreamService/OpenMedia" => {
            unary::<C, pb::OpenMediaRequest, pb::OpenMediaResponse>(
                ch,
                "/cua.env.v1.StreamService/OpenMedia",
                json,
            )
            .await
        }
        "/cua.env.v1.StreamService/SetPreferences" => {
            unary::<C, pb::SetPreferencesRequest, pb::SetPreferencesResponse>(
                ch,
                "/cua.env.v1.StreamService/SetPreferences",
                json,
            )
            .await
        }
        "/cua.env.v1.StreamService/RequestKeyframe" => {
            unary::<C, pb::RequestKeyframeRequest, pb::RequestKeyframeResponse>(
                ch,
                "/cua.env.v1.StreamService/RequestKeyframe",
                json,
            )
            .await
        }
        "/cua.env.v1.StreamService/CloseMedia" => {
            unary::<C, pb::CloseMediaRequest, pb::CloseMediaResponse>(
                ch,
                "/cua.env.v1.StreamService/CloseMedia",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/GetCapabilities" => {
            unary::<C, pb::GetCapabilitiesRequest, pb::GetCapabilitiesResponse>(
                ch,
                "/cua.env.v1.SystemService/GetCapabilities",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/Init" => {
            unary::<C, pb::InitRequest, pb::InitResponse>(
                ch,
                "/cua.env.v1.SystemService/Init",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/Health" => {
            unary::<C, pb::HealthRequest, pb::HealthResponse>(
                ch,
                "/cua.env.v1.SystemService/Health",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/Metrics" => {
            unary::<C, pb::MetricsRequest, pb::MetricsResponse>(
                ch,
                "/cua.env.v1.SystemService/Metrics",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/Shutdown" => {
            unary::<C, pb::ShutdownRequest, pb::ShutdownResponse>(
                ch,
                "/cua.env.v1.SystemService/Shutdown",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/CreateViewerTicket" => {
            unary::<C, pb::CreateViewerTicketRequest, pb::CreateViewerTicketResponse>(
                ch,
                "/cua.env.v1.SystemService/CreateViewerTicket",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/AttachRelay" => {
            unary::<C, pb::AttachRelayRequest, pb::AttachRelayResponse>(
                ch,
                "/cua.env.v1.SystemService/AttachRelay",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/DetachRelay" => {
            unary::<C, pb::DetachRelayRequest, pb::DetachRelayResponse>(
                ch,
                "/cua.env.v1.SystemService/DetachRelay",
                json,
            )
            .await
        }
        "/cua.env.v1.SystemService/DiagnoseOnce" => {
            unary::<C, pb::DiagnoseOnceRequest, pb::DiagnoseOnceResponse>(
                ch,
                "/cua.env.v1.SystemService/DiagnoseOnce",
                json,
            )
            .await
        }
        "/cua.env.v1.TeleportService/GetManifest" => {
            unary::<C, pb::GetManifestRequest, pb::GetManifestResponse>(
                ch,
                "/cua.env.v1.TeleportService/GetManifest",
                json,
            )
            .await
        }
        "/cua.env.v1.TeleportService/ImportSession" => {
            unary::<C, pb::ImportSessionRequest, pb::ImportSessionResponse>(
                ch,
                "/cua.env.v1.TeleportService/ImportSession",
                json,
            )
            .await
        }
        "/cua.env.v1.TeleportService/BeginReceiveFiles" => {
            unary::<C, pb::BeginReceiveFilesRequest, pb::BeginReceiveFilesResponse>(
                ch,
                "/cua.env.v1.TeleportService/BeginReceiveFiles",
                json,
            )
            .await
        }
        "/cua.env.v1.TeleportService/ReceiveFilesChunk" => {
            unary::<C, pb::ReceiveFilesChunkRequest, pb::ReceiveFilesChunkResponse>(
                ch,
                "/cua.env.v1.TeleportService/ReceiveFilesChunk",
                json,
            )
            .await
        }
        "/cua.env.v1.TeleportService/CommitReceiveFiles" => {
            unary::<C, pb::CommitReceiveFilesRequest, pb::CommitReceiveFilesResponse>(
                ch,
                "/cua.env.v1.TeleportService/CommitReceiveFiles",
                json,
            )
            .await
        }
        "/cua.env.v1.TeleportService/AbortReceiveFiles" => {
            unary::<C, pb::AbortReceiveFilesRequest, pb::AbortReceiveFilesResponse>(
                ch,
                "/cua.env.v1.TeleportService/AbortReceiveFiles",
                json,
            )
            .await
        }
        "/cua.env.v1.TeleportService/WipeImport" => {
            unary::<C, pb::WipeImportRequest, pb::WipeImportResponse>(
                ch,
                "/cua.env.v1.TeleportService/WipeImport",
                json,
            )
            .await
        }
        "/cua.env.v1.TunnelService/Forward" => {
            unary::<C, pb::ForwardRequest, pb::ForwardResponse>(
                ch,
                "/cua.env.v1.TunnelService/Forward",
                json,
            )
            .await
        }
        "/cua.env.v1.TunnelService/ListForwards" => {
            unary::<C, pb::ListForwardsRequest, pb::ListForwardsResponse>(
                ch,
                "/cua.env.v1.TunnelService/ListForwards",
                json,
            )
            .await
        }
        "/cua.env.v1.TunnelService/CloseForward" => {
            unary::<C, pb::CloseForwardRequest, pb::CloseForwardResponse>(
                ch,
                "/cua.env.v1.TunnelService/CloseForward",
                json,
            )
            .await
        }
        "/cua.env.v1.TunnelService/StartHotspot" => {
            unary::<C, pb::StartHotspotRequest, pb::StartHotspotResponse>(
                ch,
                "/cua.env.v1.TunnelService/StartHotspot",
                json,
            )
            .await
        }
        "/cua.env.v1.TunnelService/StopHotspot" => {
            unary::<C, pb::StopHotspotRequest, pb::StopHotspotResponse>(
                ch,
                "/cua.env.v1.TunnelService/StopHotspot",
                json,
            )
            .await
        }
        "/cua.env.v1.TunnelService/GetHotspotStatus" => {
            unary::<C, pb::GetHotspotStatusRequest, pb::GetHotspotStatusResponse>(
                ch,
                "/cua.env.v1.TunnelService/GetHotspotStatus",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/ListWindows" => {
            unary::<C, pb::ListWindowsRequest, pb::ListWindowsResponse>(
                ch,
                "/cua.env.v1.WindowsService/ListWindows",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/GetWindow" => {
            unary::<C, pb::GetWindowRequest, pb::GetWindowResponse>(
                ch,
                "/cua.env.v1.WindowsService/GetWindow",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/ActivateWindow" => {
            unary::<C, pb::ActivateWindowRequest, pb::ActivateWindowResponse>(
                ch,
                "/cua.env.v1.WindowsService/ActivateWindow",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/SetWindowBounds" => {
            unary::<C, pb::SetWindowBoundsRequest, pb::SetWindowBoundsResponse>(
                ch,
                "/cua.env.v1.WindowsService/SetWindowBounds",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/MinimizeWindow" => {
            unary::<C, pb::MinimizeWindowRequest, pb::MinimizeWindowResponse>(
                ch,
                "/cua.env.v1.WindowsService/MinimizeWindow",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/MaximizeWindow" => {
            unary::<C, pb::MaximizeWindowRequest, pb::MaximizeWindowResponse>(
                ch,
                "/cua.env.v1.WindowsService/MaximizeWindow",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/RestoreWindow" => {
            unary::<C, pb::RestoreWindowRequest, pb::RestoreWindowResponse>(
                ch,
                "/cua.env.v1.WindowsService/RestoreWindow",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/CloseWindow" => {
            unary::<C, pb::CloseWindowRequest, pb::CloseWindowResponse>(
                ch,
                "/cua.env.v1.WindowsService/CloseWindow",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/LaunchApp" => {
            unary::<C, pb::LaunchAppRequest, pb::LaunchAppResponse>(
                ch,
                "/cua.env.v1.WindowsService/LaunchApp",
                json,
            )
            .await
        }
        "/cua.env.v1.WindowsService/Open" => {
            unary::<C, pb::OpenRequest, pb::OpenResponse>(
                ch,
                "/cua.env.v1.WindowsService/Open",
                json,
            )
            .await
        }
        other => Err(CuaError::InvalidArgument(format!(
            "{other} is not a unary JSON method"
        ))),
    }
}

#[cfg(not(any(target_arch = "wasm32", feature = "web")))]
pub(crate) async fn call_stream<C: Channel>(
    ch: C,
    path: &str,
    json: &str,
    max: u32,
    timeout: std::time::Duration,
) -> Result<Vec<String>> {
    match path {
        "/cua.env.v1.FilesystemService/WatchDir" => {
            stream::<C, pb::WatchDirRequest, pb::WatchDirResponse>(
                ch,
                "/cua.env.v1.FilesystemService/WatchDir",
                json,
                max,
                timeout,
            )
            .await
        }
        "/cua.env.v1.FilesystemService/ReadFile" => {
            stream::<C, pb::ReadFileRequest, pb::ReadFileResponse>(
                ch,
                "/cua.env.v1.FilesystemService/ReadFile",
                json,
                max,
                timeout,
            )
            .await
        }
        "/cua.env.v1.PresenceService/Join" => {
            stream::<C, pb::JoinRequest, pb::JoinResponse>(
                ch,
                "/cua.env.v1.PresenceService/Join",
                json,
                max,
                timeout,
            )
            .await
        }
        "/cua.env.v1.SystemService/Diagnose" => {
            stream::<C, pb::DiagnoseRequest, pb::DiagnoseResponse>(
                ch,
                "/cua.env.v1.SystemService/Diagnose",
                json,
                max,
                timeout,
            )
            .await
        }
        "/cua.env.v1.WindowsService/WatchWindows" => {
            stream::<C, pb::WatchWindowsRequest, pb::WatchWindowsResponse>(
                ch,
                "/cua.env.v1.WindowsService/WatchWindows",
                json,
                max,
                timeout,
            )
            .await
        }
        other => Err(CuaError::InvalidArgument(format!(
            "{other} is not a streaming JSON method"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use std::collections::BTreeSet;

    /// Every RPC of the compiled `cua.env.v1` contract is reachable from the
    /// SDK: typed, through the JSON escape hatch, or (client streams) via
    /// its registered unary fallback. Mirrors cua-driver's
    /// `exported_typed_methods_cover_every_published_contract`.
    #[test]
    fn exported_methods_cover_every_env_rpc() {
        let fds = prost_types::FileDescriptorSet::decode(cua_proto::FILE_DESCRIPTOR_SET).unwrap();
        let mut contract = BTreeSet::new();
        let mut client_streams = BTreeSet::new();
        for f in fds.file.iter().filter(|f| f.package() == "cua.env.v1") {
            for s in &f.service {
                for m in &s.method {
                    let path = format!("/cua.env.v1.{}/{}", s.name(), m.name());
                    if m.client_streaming() {
                        client_streams.insert(path);
                    } else {
                        contract.insert(path);
                    }
                }
            }
        }
        let exported: BTreeSet<String> = JSON_UNARY_METHODS
            .iter()
            .chain(JSON_STREAM_METHODS)
            .chain(TYPED_ONLY_METHODS)
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            exported, contract,
            "regenerate native/json.rs from the contract"
        );
        for cs in client_streams {
            let fallback = cua_proto::CLIENT_STREAM_FALLBACKS
                .iter()
                .find(|(s, _)| *s == cs)
                .map(|(_, f)| *f)
                .unwrap_or_else(|| panic!("{cs} has no unary fallback"));
            assert!(
                JSON_UNARY_METHODS.contains(&fallback),
                "{cs}: fallback {fallback} is not callable"
            );
        }
    }

    #[test]
    fn normalizes_method_names() {
        assert_eq!(
            normalize("WindowsService/ListWindows").unwrap(),
            "/cua.env.v1.WindowsService/ListWindows"
        );
        assert!(normalize("/cua.env.v1.ProcessService/StartProcess").is_err());
        assert!(normalize("Nope/Nope").is_err());
    }
}
