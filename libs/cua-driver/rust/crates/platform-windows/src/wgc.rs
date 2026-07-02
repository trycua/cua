//! Windows.Graphics.Capture (WGC) backend for window screenshots.
//!
//! Used when PrintWindow returns black (UWP / DirectComposition) AND/OR
//! when the target window is occluded by another window. WGC captures
//! the window's own composited frames out of DWM regardless of z-order,
//! which is the only correct way to screenshot UWP apps like Calculator
//! when they're behind the user's terminal.
//!
//! Constraints:
//! - Minimum OS: Windows 10 version 1903 (~April 2019). ~100% of users
//!   today; we surface a structured error on older builds.
//! - Cannot capture **minimized** windows. They have no rendered content.
//!   `get_window_state` still returns the UIA tree for a minimized target
//!   (the screenshot is reported unavailable). We surface this as an
//!   error with the recommended fallback.
//! - First call per process pays ~50ms for D3D11 device creation +
//!   COM activation factory lookup. Subsequent calls don't cache the
//!   device — could be optimized later, but a single-shot screenshot
//!   tool isn't the place to fight that.

use anyhow::{bail, Context, Result};
use std::time::Duration;

use windows::{
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem},
        DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
    },
    Win32::{
        Foundation::HWND,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
                D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
            },
            Dxgi::IDXGIDevice,
        },
        System::WinRT::{
            Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
            Graphics::Capture::IGraphicsCaptureItemInterop,
        },
        UI::WindowsAndMessaging::IsIconic,
    },
    core::Interface,
};

/// Capture a window via WGC, returning BGRA pixels + (width, height).
/// Caller is responsible for encoding to PNG (or whatever format) via
/// `cua_driver_core::image_utils::encode_bgra_to_png`.
pub fn screenshot_window_via_wgc(hwnd: u64) -> Result<(Vec<u8>, u32, u32)> {
    unsafe { wgc_capture_impl(HWND(hwnd as *mut _)) }
}

unsafe fn wgc_capture_impl(hwnd: HWND) -> Result<(Vec<u8>, u32, u32)> {
    if IsIconic(hwnd).as_bool() {
        bail!(
            "WGC cannot capture a minimized window (no rendered content). \
             Restore the window first — `get_window_state` still returns the \
             UIA tree for a minimized window (the screenshot is reported \
             unavailable)."
        );
    }

    // 1. D3D11 device — feature level 11.0 + BGRA support (required by WGC).
    let mut d3d_device: Option<ID3D11Device> = None;
    let mut d3d_context: Option<ID3D11DeviceContext> = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        windows::Win32::Foundation::HMODULE(std::ptr::null_mut()),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(&[D3D_FEATURE_LEVEL_11_0]),
        D3D11_SDK_VERSION,
        Some(&mut d3d_device),
        None,
        Some(&mut d3d_context),
    )
    .context("D3D11CreateDevice failed (hardware feature-level 11.0 + BGRA)")?;
    let d3d_device = d3d_device.context("D3D11CreateDevice returned a None device")?;
    let d3d_context = d3d_context.context("D3D11CreateDevice returned a None context")?;

    // 2. DXGI device interface for the D3D11 device, then the WinRT
    //    IDirect3DDevice wrapper that GraphicsCaptureFramePool needs.
    let dxgi_device: IDXGIDevice = d3d_device.cast().context("ID3D11Device → IDXGIDevice cast")?;
    let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)
        .context("CreateDirect3D11DeviceFromDXGIDevice failed")?;
    let direct3d_device: IDirect3DDevice = inspectable
        .cast()
        .context("CreateDirect3D11DeviceFromDXGIDevice → IDirect3DDevice cast")?;

    // 3. GraphicsCaptureItem for our target HWND.
    let interop_factory: IGraphicsCaptureItemInterop =
        windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .context("IGraphicsCaptureItemInterop activation factory")?;
    let item: GraphicsCaptureItem = interop_factory
        .CreateForWindow(hwnd)
        .context(
            "IGraphicsCaptureItemInterop::CreateForWindow failed — \
             requires Win10 1903+ and the target window must exist",
        )?;
    let item_size = item.Size().context("GraphicsCaptureItem::Size")?;
    if item_size.Width <= 0 || item_size.Height <= 0 {
        bail!(
            "WGC item size is {}x{} — target may be hidden/cloaked. \
             Try restoring the window or use UIA.",
            item_size.Width,
            item_size.Height
        );
    }

    // 4. Frame pool + capture session.
    //    `CreateFreeThreaded` (not `Create`) — `Create` requires the
    //    calling thread to have a CoreDispatcher/event loop running
    //    for FrameArrived to fire, which we don't have inside a
    //    daemon's spawn_blocking thread. `CreateFreeThreaded`
    //    dispatches FrameArrived on a system thread-pool thread, which
    //    is exactly what our mpsc channel handler expects. Without
    //    this, FrameArrived silently never fires and our 1500 ms wait
    //    always times out.
    //
    //    Frame count 2 — WGC docs recommend ≥2 to avoid GPU stalls
    //    while the consumer reads the previous frame. We only consume
    //    one but the GPU side is happier with breathing room.
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &direct3d_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        item_size,
    )
    .context("Direct3D11CaptureFramePool::CreateFreeThreaded")?;
    let session = pool
        .CreateCaptureSession(&item)
        .context("Direct3D11CaptureFramePool::CreateCaptureSession")?;

    // 5. Hide the WGC capture indicator (yellow border) if the build
    //    supports it. Available since Win11 (IsBorderRequired property).
    //    Best-effort — older Win10 won't have the setter; ignore failure.
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(false);

    // 6. Start capture, then poll TryGetNextFrame until a frame is
    //    available or we time out. We tried FrameArrived events first
    //    (both `Create` and `CreateFreeThreaded` pool variants) but
    //    the event never fired in our daemon's thread environment —
    //    likely an STA/COM-apartment mismatch we couldn't isolate.
    //    Polling is what windows-capture and OBS-style consumers do
    //    for single-shot capture anyway: WGC drops frames in the pool
    //    as DWM composes them, and TryGetNextFrame returns Ok(None) /
    //    NULL until one's available. 50 ms polling interval keeps the
    //    "frame finally arrived" detection latency low without burning
    //    CPU on a tight spin.
    session.StartCapture().context("GraphicsCaptureSession::StartCapture")?;

    let deadline = std::time::Instant::now() + Duration::from_millis(1500);
    let mut frame_opt = None;
    while std::time::Instant::now() < deadline {
        match pool.TryGetNextFrame() {
            Ok(f) => { frame_opt = Some(f); break; }
            Err(_) => {
                // TryGetNextFrame returns an error when no frame is
                // available yet. Wait briefly, retry.
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let frame = frame_opt.ok_or_else(|| anyhow::anyhow!(
        "WGC TryGetNextFrame returned no frame within 1500 ms — \
         DWM may not be compositing this window (cloaked / not actually \
         rendered). Fall through to screen-region BitBlt."
    ))?;

    // 8. Pull the underlying ID3D11Texture2D out of the WinRT frame
    //    surface via the IDirect3DDxgiInterfaceAccess shim.
    let surface = frame.Surface().context("frame.Surface")?;
    let access: IDirect3DDxgiInterfaceAccess = surface
        .cast()
        .context("IDirect3DSurface → IDirect3DDxgiInterfaceAccess cast")?;
    let frame_texture: ID3D11Texture2D = access
        .GetInterface()
        .context("IDirect3DDxgiInterfaceAccess::GetInterface(ID3D11Texture2D)")?;

    // 9. Create a staging texture we can map for CPU read, copy frame in.
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    frame_texture.GetDesc(&mut desc);
    desc.Usage = D3D11_USAGE_STAGING;
    desc.BindFlags = 0;
    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    desc.MiscFlags = 0;
    let mut staging: Option<ID3D11Texture2D> = None;
    d3d_device
        .CreateTexture2D(&desc, None, Some(&mut staging))
        .context("CreateTexture2D(staging)")?;
    let staging = staging.context("CreateTexture2D returned a None texture")?;
    d3d_context.CopyResource(&staging, &frame_texture);

    // 10. Map the staging texture, copy BGRA out row-by-row (RowPitch may
    //     be padded — can't just copy `Width * 4 * Height` straight).
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    d3d_context
        .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .context("ID3D11DeviceContext::Map")?;
    let width = desc.Width as usize;
    let height = desc.Height as usize;
    let stride = mapped.RowPitch as usize;
    let mut bgra = vec![0u8; width * height * 4];
    let src_base = mapped.pData as *const u8;
    for row in 0..height {
        std::ptr::copy_nonoverlapping(
            src_base.add(row * stride),
            bgra.as_mut_ptr().add(row * width * 4),
            width * 4,
        );
    }
    d3d_context.Unmap(&staging, 0);

    // 11. Cleanup: close the session and pool explicitly. RAII would
    //     handle this but being explicit makes the lifetimes easier to
    //     audit. (No FrameArrived to unhook — we polled instead.)
    let _ = session.Close();
    let _ = pool.Close();

    Ok((bgra, desc.Width, desc.Height))
}
