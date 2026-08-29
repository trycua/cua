//! Native Wayland Agent View for compositors exposing layer-shell.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::Duration;

use pip_preview::{PipConfig, PipViewModel};
use wayland_client::{
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_compositor::WlCompositor,
        wl_region::WlRegion,
        wl_registry,
        wl_shm::{Format, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

use super::{render_agent_view, UiMessage};

struct State {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    configured: bool,
    width: u32,
    height: u32,
    pending_buffers: HashMap<u32, (*mut libc::c_void, usize, i32)>,
}

unsafe impl Send for State {}

impl State {
    fn new(width: u32, height: u32) -> Self {
        Self {
            compositor: None,
            shm: None,
            layer_shell: None,
            surface: None,
            layer_surface: None,
            configured: false,
            width,
            height,
            pending_buffers: HashMap::new(),
        }
    }
}

pub(super) fn run_window(
    cfg: PipConfig,
    rx: Receiver<UiMessage>,
    ready: SyncSender<anyhow::Result<()>>,
) {
    if let Err(error) = run(cfg, rx, &ready) {
        let _ = ready.send(Err(error));
    }
}

fn run(
    cfg: PipConfig,
    rx: Receiver<UiMessage>,
    ready: &SyncSender<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue::<State>();
    let qh = queue.handle();
    let _registry = connection.display().get_registry(&qh, ());
    let mut state = State::new(cfg.geometry.width.max(360), cfg.geometry.height.max(260));
    queue.roundtrip(&mut state)?;

    let compositor = state
        .compositor
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Wayland compositor does not expose wl_compositor"))?;
    let layer_shell = state.layer_shell.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "native Wayland Agent View requires zwlr_layer_shell_v1; use XWayland on this compositor"
        )
    })?;
    let surface = compositor.create_surface(&qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        None,
        Layer::Overlay,
        "cua-agent-view".to_owned(),
        &qh,
        (),
    );
    layer_surface.set_anchor(Anchor::Top | Anchor::Right);
    layer_surface.set_size(state.width, state.height);
    layer_surface.set_margin(24, 24, 24, 24);
    layer_surface.set_exclusive_zone(-1);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);
    // Layer-shell cannot provide a normal draggable/resizable window contract.
    // Keep the presentation click-through so it can never block automation;
    // session selection continues to follow MRU on this native-only path.
    let input_region = compositor.create_region(&qh, ());
    surface.set_input_region(Some(&input_region));
    input_region.destroy();
    surface.commit();
    state.surface = Some(surface);
    state.layer_surface = Some(layer_surface);

    for _ in 0..8 {
        queue.roundtrip(&mut state)?;
        if state.configured {
            break;
        }
    }
    anyhow::ensure!(state.configured, "Wayland Agent View was never configured");
    ready
        .send(Ok(()))
        .map_err(|_| anyhow::anyhow!("Agent View startup receiver was dropped"))?;

    let mut model = PipViewModel::new(12);
    let mut running = true;
    let mut dirty = true;
    while running {
        match rx.recv_timeout(Duration::from_millis(16)) {
            Ok(UiMessage::Frame(frame)) => {
                model.upsert(frame);
                dirty = true;
            }
            Ok(UiMessage::RemoveTarget(workspace, target)) => {
                dirty |= model.remove_target(&workspace, &target);
            }
            Ok(UiMessage::RemoveWorkspace(workspace)) => {
                dirty |= !model.remove_workspace(&workspace).is_empty();
            }
            Ok(UiMessage::SetInputTransparent(_, reply)) => {
                // Layer-shell presentation does not cover the automated target
                // at its top-right inset, so no compositor input-region swap is
                // required for injected actions.
                let _ = reply.send(Ok(()));
            }
            Ok(UiMessage::Shutdown) => running = false,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if dirty && running {
            redraw(&mut state, &model, &qh)?;
            dirty = false;
        }
        queue.roundtrip(&mut state)?;
    }
    Ok(())
}

fn redraw(state: &mut State, model: &PipViewModel, qh: &QueueHandle<State>) -> anyhow::Result<()> {
    let surface = state
        .surface
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Wayland Agent View surface is unavailable"))?;
    let shm = state
        .shm
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Wayland compositor does not expose wl_shm"))?;
    let width = state.width.max(1).min(u16::MAX as u32) as u16;
    let height = state.height.max(1).min(u16::MAX as u32) as u16;
    let frames = model.selected_frames();
    let workspaces = model.workspaces();
    let pixels = render_agent_view(
        width,
        height,
        &frames,
        &workspaces,
        model.selected_workspace_id(),
        model.active_view_id(),
    );
    let stride = i32::from(width) * 4;
    let size = pixels.len();
    let (fd, ptr) = crate::wayland::anon_shm(size)?;
    unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr.cast::<u8>(), size) };
    use std::os::fd::AsFd as _;
    let pool_fd = unsafe { crate::wayland::borrowed_fd(fd) };
    let pool = shm.create_pool(pool_fd.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        i32::from(width),
        i32::from(height),
        stride,
        Format::Xrgb8888,
        qh,
        (),
    );
    pool.destroy();
    state
        .pending_buffers
        .insert(buffer.id().protocol_id(), (ptr, size, fd));
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, i32::from(width), i32::from(height));
    surface.commit();
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor =
                        Some(registry.bind::<WlCompositor, _, _>(name, version.min(6), qh, ()))
                }
                "wl_shm" => {
                    state.shm = Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()))
                }
                "zwlr_layer_shell_v1" => {
                    state.layer_shell =
                        Some(registry.bind::<ZwlrLayerShellV1, _, _>(name, version.min(4), qh, ()))
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                if width > 0 {
                    state.width = width;
                }
                if height > 0 {
                    state.height = height;
                }
                state.configured = true;
            }
            zwlr_layer_surface_v1::Event::Closed => state.surface = None,
            _ => {}
        }
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        state: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wl_buffer::Event::Release) {
            if let Some((ptr, size, fd)) = state.pending_buffers.remove(&buffer.id().protocol_id())
            {
                crate::wayland::cleanup_mmap(ptr, size, fd);
            }
            buffer.destroy();
        }
    }
}

macro_rules! empty_dispatch {
    ($ty:ty) => {
        impl Dispatch<$ty, ()> for State {
            fn event(
                _: &mut Self,
                _: &$ty,
                _: <$ty as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    };
}

empty_dispatch!(WlCompositor);
empty_dispatch!(WlShm);
empty_dispatch!(WlShmPool);
empty_dispatch!(WlSurface);
empty_dispatch!(WlRegion);
empty_dispatch!(ZwlrLayerShellV1);
