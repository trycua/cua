//! Protobuf contract for cua-spacesd (`cua.env.v1`) and the local cua
//! daemon (`cua.daemon.v1`), with generated prost messages and tonic
//! clients and servers.
//!
//! The `.proto` files in `libs/cua/proto` are the source of truth; this
//! crate only generates code from them. Applications should not use the
//! generated client directly: the ergonomic client lives in `cua-spacesd-client` and is
//! exposed to other languages only through the cua SDK.
//!
//! Features:
//! - `client` (default): `*_service_client` modules.
//! - `server` (default): `*_service_server` modules.
//! - `serde`: canonical proto3-JSON `Serialize`/`Deserialize` for every
//!   message and enum (via pbjson).

/// Well-known types (`google.protobuf.*`) used by the generated code.
pub use pbjson_types as wkt;

/// `cua.env.v1`: the in-sandbox spacesd contract.
pub mod env {
    /// Version 1 of the spacesd contract.
    #[allow(clippy::all)] // generated code
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/cua.env.v1.rs"));
        #[cfg(feature = "serde")]
        include!(concat!(env!("OUT_DIR"), "/cua.env.v1.serde.rs"));
    }
}

/// `cua.daemon.v1`: the local cua daemon contract (skeleton).
pub mod daemon {
    /// Version 1 of the daemon contract.
    #[allow(clippy::all)] // generated code
    pub mod v1 {
        include!(concat!(env!("OUT_DIR"), "/cua.daemon.v1.rs"));
        #[cfg(feature = "serde")]
        include!(concat!(env!("OUT_DIR"), "/cua.daemon.v1.serde.rs"));
    }
}

/// Serialized `google.protobuf.FileDescriptorSet` of every contract file
/// (with source info). Feed it to `tonic-reflection` or use it to inspect the
/// contract at runtime.
pub const FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/cua_descriptor.bin"));

/// Default TCP port of cua-spacesd (gRPC, gRPC-Web and HTTP routes).
pub const SPACESD_DEFAULT_PORT: u16 = 3211;

/// Default UDP port of the direct QUIC media listener (gRPC port + 1).
pub const SPACESD_DEFAULT_QUIC_PORT: u16 = 3212;

/// Value of `GetCapabilitiesResponse.protocol_version` for `cua.env.v1`.
pub const ENV_PROTOCOL_VERSION: u32 = 1;

/// Value of `GetCapabilitiesResponse.protocol_revision` for this revision of
/// the contract. Bump on every additive change to `cua.env.v1`.
///
/// Revisions: 1 = initial contract; 2 = audio tracks (`AudioOptions`,
/// `NegotiatedAudio`, audio preferences, `InitRequest.audio_uplink`);
/// 3 = `SystemService.Diagnose` / `DiagnoseOnce` (image self-test report);
/// 4 = `SystemService.CreateViewerTicket` (the `/viewer` web viewer) and
/// `ClipboardContent.image_png`; 5 = presence cursor shapes
/// (`CursorShape`, `CursorShapeChanged`), roster heartbeats, cursor batches,
/// `LeaveReason` and `InitRequest.presence`; 6 = `OperatingSystem.pretty_name`
/// and `MetricsResponse.memory_limited` / `disk_limited`; 7 =
/// `SystemService.AttachRelay` / `DetachRelay` (a running driver joins a
/// relay so its Space can be shared); 8 = `HostSpacesService` (a host
/// provides Spaces through the relay).
pub const ENV_PROTOCOL_REVISION: u32 = 8;

/// `DiagnoseReport.schema_version` of this revision of the contract.
pub const DIAGNOSE_REPORT_SCHEMA_VERSION: u32 = 1;

/// Media plane wire version (rcdp wire v2, see `libs/cua/proto/MEDIA.md`).
pub const MEDIA_WIRE_VERSION: u32 = 2;

/// Media plane binary-message magics (MEDIA.md §12). A binary media message
/// whose first four bytes equal [`AUDIO_PACKET_MAGIC`] is an audio packet;
/// otherwise it is a length-prefixed v1-style packet (whose first byte is
/// always 0 because headers are at most 1 MiB).
pub const AUDIO_PACKET_MAGIC: [u8; 4] = *b"RAU2";

/// Size in bytes of the fixed audio packet header (MEDIA.md §12.2).
pub const AUDIO_PACKET_HEADER_LEN: usize = 24;

/// Unit of the shared media clock: every video `capture_timestamp_us` and
/// audio `pts_us` is in microseconds on one per-driver monotonic clock.
pub const MEDIA_CLOCK_HZ: u64 = 1_000_000;

/// Well-known metadata keys and HTTP paths.
pub mod metadata {
    /// Bearer token header: `authorization: Bearer <token>`.
    pub const AUTHORIZATION: &str = "authorization";
    /// Alternate spacesd token header, `x-cua-env-authorization: Bearer <token>`,
    /// for paths where `authorization` belongs to an outer hop (the Fleet
    /// gateway consumes and strips it). Servers accept either header.
    pub const ENV_AUTHORIZATION: &str = "x-cua-env-authorization";
    /// Binary metadata carrying a serialized `cua.env.v1.Principal`.
    pub const PRINCIPAL_BIN: &str = "x-cua-principal-bin";
    /// Plain-HTTP, unauthenticated health route (204 when healthy).
    pub const HEALTH_PATH: &str = "/health";
    /// Media WebSocket path (ticket-authenticated).
    pub const MEDIA_WS_PATH: &str = "/media";
    /// TCP-over-WebSocket tunnel path (ticket-authenticated).
    pub const TUNNEL_WS_PATH: &str = "/tunnel";
    /// Reverse-SOCKS hotspot WebSocket path (ticket-authenticated).
    pub const HOTSPOT_WS_PATH: &str = "/hotspot";
    /// Cua Volume WebSocket path (ticket-authenticated): the client serves
    /// the guest's volume mount over it.
    pub const VOLUME_WS_PATH: &str = "/volume";
    /// Signed-URL file route.
    pub const FILES_PATH: &str = "/files";
    /// Streamable-HTTP MCP endpoint for the cua-driver tool registry.
    pub const MCP_PATH: &str = "/mcp";
    /// The web viewer (static page, unauthenticated; it reads its viewer
    /// ticket from the URL fragment). Served with and without the trailing
    /// slash; assets live under `/viewer/`.
    pub const VIEWER_PATH: &str = "/viewer";
    /// gRPC methods a viewer ticket (`SystemService.CreateViewerTicket`)
    /// may call. Clipboard and filesystem methods additionally need the
    /// ticket's grant; everything else is refused with PERMISSION_DENIED.
    pub const VIEWER_GRPC_METHODS: &[&str] = &[
        "/cua.env.v1.SystemService/GetCapabilities",
        "/cua.env.v1.SystemService/Health",
        "/cua.env.v1.StreamService/ListTargets",
        "/cua.env.v1.StreamService/OpenMedia",
        "/cua.env.v1.StreamService/SetPreferences",
        "/cua.env.v1.StreamService/RequestKeyframe",
        "/cua.env.v1.StreamService/CloseMedia",
        "/cua.env.v1.ComputerService/ListDisplays",
        "/cua.env.v1.ComputerService/GetClipboard",
        "/cua.env.v1.ComputerService/SetClipboard",
        "/cua.env.v1.PresenceService/Join",
        "/cua.env.v1.PresenceService/UpdateCursor",
        "/cua.env.v1.PresenceService/Leave",
        "/cua.env.v1.FilesystemService/Stat",
        "/cua.env.v1.FilesystemService/ListDir",
        "/cua.env.v1.FilesystemService/MakeDir",
        "/cua.env.v1.FilesystemService/Move",
        "/cua.env.v1.FilesystemService/Remove",
        "/cua.env.v1.FilesystemService/CreateWatcher",
        "/cua.env.v1.FilesystemService/GetWatcherEvents",
        "/cua.env.v1.FilesystemService/RemoveWatcher",
        "/cua.env.v1.FilesystemService/ReadFile",
        "/cua.env.v1.FilesystemService/BeginUpload",
        "/cua.env.v1.FilesystemService/UploadChunk",
        "/cua.env.v1.FilesystemService/CommitUpload",
        "/cua.env.v1.FilesystemService/AbortUpload",
    ];
}

/// Client-streaming RPCs and the unary RPC that is their gRPC-Web fallback.
///
/// gRPC-Web cannot carry client streams, so every client-streaming method in
/// the contract must appear here. `tests/descriptor.rs` enforces it.
pub const CLIENT_STREAM_FALLBACKS: &[(&str, &str)] = &[
    (
        "/cua.env.v1.ProcessService/StreamInput",
        "/cua.env.v1.ProcessService/SendInput",
    ),
    (
        "/cua.env.v1.FilesystemService/WriteFile",
        "/cua.env.v1.FilesystemService/UploadChunk",
    ),
];
