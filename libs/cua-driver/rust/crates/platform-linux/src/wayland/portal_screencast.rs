//! Per-window capture via `xdg-desktop-portal` ScreenCast + PipeWire.
//!
//! Reaches GNOME/Mutter and KDE/KWin per-window selection — the
//! cross-DE complement to wlroots `ext-image-copy-capture-v1`. Sequence:
//!
//! 1. ashpd ScreenCast::create_session.
//! 2. select_sources with types=Window|Monitor, cursor_mode=Embedded,
//!    persist_mode=Application (consent dialog fires ONCE per process,
//!    cached for the lifetime of the requesting binary).
//! 3. start(session, parent_window) — user picks the source window in
//!    the portal's system dialog the first time; subsequent calls within
//!    the same process reuse the cached choice.
//! 4. open_pipe_wire_remote returns a Unix file descriptor; Streams
//!    carries the published node ids.
//! 5. Connect a pipewire client to the fd, open a Stream targeting the
//!    advertised node id, negotiate Video/Raw format, dequeue one frame
//!    with `STREAM_FLAG_MAP_BUFFERS`, copy pixels, PNG-encode, stop.
//!
//! The session + restore_token are persisted in a process-local
//! `OnceLock<PortalScreencastSession>` so only the first call across the
//! process lifetime triggers the consent dialog; later calls reuse the
//! same PipeWire node id.

use std::os::fd::AsRawFd;
use std::sync::OnceLock;

use ashpd::desktop::screencast::{
    CursorMode, OpenPipeWireRemoteOptions, Screencast,
    SelectSourcesOptions, SourceType, StartCastOptions, Streams,
};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use ashpd::enumflags2::BitFlags;

/// A live portal ScreenCast session held across calls. Expensive to
/// create (system consent dialog) and cheap to reuse — the OnceLock
/// keeps it alive until the requesting process exits.
struct PortalScreencastSession {
    _session: Session<Screencast>,
    streams: Streams,
    fd: std::os::fd::OwnedFd,
}

unsafe impl Send for PortalScreencastSession {}
unsafe impl Sync for PortalScreencastSession {}

static SESSION: OnceLock<PortalScreencastSession> = OnceLock::new();

/// Capture the user-selected window via `xdg-desktop-portal` ScreenCast.
/// On first invocation per process the portal dialog asks the user to
/// pick a window or monitor; subsequent calls reuse the same node
/// transparently. Returns PNG bytes of the latest frame on the stream.
pub fn screenshot_window_via_portal() -> anyhow::Result<Vec<u8>> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime for ashpd: {e}"))?;

    if SESSION.get().is_none() {
        let sess = rt.block_on(create_portal_session())?;
        let _ = SESSION.set(sess);
    }
    let session = SESSION
        .get()
        .ok_or_else(|| anyhow::anyhow!("portal ScreenCast session unavailable"))?;

    let node_id = session
        .streams
        .streams()
        .iter()
        .next()
        .map(|s| s.pipe_wire_node_id())
        .ok_or_else(|| anyhow::anyhow!("portal advertised no streams"))?;

    capture_one_frame(session.fd.as_raw_fd(), node_id)
}

async fn create_portal_session() -> anyhow::Result<PortalScreencastSession> {
    let proxy = Screencast::new()
        .await
        .map_err(|e| anyhow::anyhow!("portal ScreenCast proxy unreachable: {e}. Install xdg-desktop-portal-gnome / xdg-desktop-portal-kde."))?;

    let session = proxy
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("portal create_session failed: {e}"))?;

    let select_opts = SelectSourcesOptions::default()
        .set_sources(BitFlags::<SourceType>::from(SourceType::Window) | SourceType::Monitor)
        .set_cursor_mode(CursorMode::Embedded)
        .set_persist_mode(PersistMode::Application);

    proxy
        .select_sources(&session, select_opts)
        .await
        .map_err(|e| anyhow::anyhow!("portal select_sources failed: {e}"))?
        .response()
        .map_err(|e| anyhow::anyhow!("portal select_sources response error: {e}"))?;

    let streams = proxy
        .start(&session, None, StartCastOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("portal start failed: {e}"))?
        .response()
        .map_err(|e| anyhow::anyhow!("portal start response error (user denied?): {e}"))?;

    let fd = proxy
        .open_pipe_wire_remote(&session, OpenPipeWireRemoteOptions::default())
        .await
        .map_err(|e| anyhow::anyhow!("portal open_pipe_wire_remote failed: {e}"))?;

    Ok(PortalScreencastSession {
        _session: session,
        streams,
        fd,
    })
}

/// Pull one frame off the PipeWire stream advertised by the portal.
///
/// Implementation note: pipewire-rs frame-pull plumbing requires non-
/// trivial SPA format pod negotiation (SPA_VIDEO_FORMAT_BGRx at the
/// announced w/h, MediaType::Video / MediaSubtype::Raw). The deps are
/// in tree (pipewire 0.8 + libspa 0.8 in Cargo.toml), but the inner
/// callback dance is best driven by a real GNOME/KDE host smoke test
/// to refine timing + format selection. v1 surfaces a typed error so
/// callers fall through cleanly; the portal handshake up to this point
/// IS exercised — consent + stream node id obtained.
///
/// Follow-up (~250 LOC):
///   1. pipewire::MainLoop::new(None)?
///   2. pipewire::Context::new(&main_loop)?
///   3. pipewire::Core::connect_fd(_pipewire_fd, None)?
///   4. Stream::new(&core, "cua-driver-capture", properties{
///        media.type=Video, media.category=Capture, media.role=Screen})
///   5. SPA EnumFormat pod via libspa::param::format_utils:
///      MediaType::Video / MediaSubtype::Raw / VideoFormat::BGRx
///      VideoSize at announced w/h
///   6. stream.connect(Direction::Input, Some(node_id),
///        StreamFlags::AUTOCONNECT | MAP_BUFFERS, params)
///   7. on_process callback: dequeue buffer, copy pixels into
///      Mutex<Option<Vec<u8>>>, signal main_loop.quit()
///   8. main_loop.run() blocks until quit
///   9. PNG-encode the bytes via the image crate (already a workspace
///      dep — same path used by ext_screencopy::encode_buffer_to_png)
fn capture_one_frame(_pipewire_fd: i32, _node_id: u32) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!(
        "portal ScreenCast → PipeWire frame extraction is not yet implemented; \
         portal handshake succeeded (consent granted, stream node id obtained) \
         but the pipewire-rs Stream + libspa format negotiation has not landed \
         yet. Use ext-image-copy-capture-v1 (GNOME 47+ / KDE 6.2+ / wlroots) \
         in the meantime, or fall through to xdg-desktop-portal Screenshot for \
         display-level capture."
    )
}

/// Probe whether the portal ScreenCast interface is reachable on the
/// session bus without taking an actual capture (avoids cached consent).
pub fn probe_screencast_portal() -> anyhow::Result<bool> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime: {e}"))?;
    rt.block_on(async {
        let conn = zbus::Connection::session()
            .await
            .map_err(|e| anyhow::anyhow!("session bus unreachable: {e}"))?;
        let proxy = zbus::fdo::DBusProxy::new(&conn)
            .await
            .map_err(|e| anyhow::anyhow!("dbus proxy creation failed: {e}"))?;
        let bus_name: zbus::names::BusName = "org.freedesktop.portal.Desktop"
            .try_into()
            .map_err(|e| anyhow::anyhow!("bus name parse failed: {e}"))?;
        let has_owner = proxy
            .name_has_owner(bus_name)
            .await
            .map_err(|e| anyhow::anyhow!("name_has_owner failed: {e}"))?;
        Ok(has_owner)
    })
}
