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
//!   For minimized targets, the caller should use UIA via
//!   `get_window_state` `capture_mode:"ax"`. We surface this as an
//!   error with the recommended fallback.
//! - First call per process pays ~50ms for D3D11 device creation +
//!   COM activation factory lookup. Subsequent calls don't cache the
//!   device — could be optimized later, but a single-shot screenshot
//!   tool isn't the place to fight that.

use anyhow::{bail, Context, Result};
use std::sync::mpsc;
use std::time::Duration;

use windows::{
    Foundation::TypedEventHandler,
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
/// `mcp_server::image_utils::encode_bgra_to_png`.
pub fn screenshot_window_via_wgc(hwnd: u64) -> Result<(Vec<u8>, u32, u32)> {
    unsafe { wgc_capture_impl(HWND(hwnd as *mut _)) }
}

unsafe fn wgc_capture_impl(hwnd: HWND) -> Result<(Vec<u8>, u32, u32)> {
    if IsIconic(hwnd).as_bool() {
        bail!(
            "WGC cannot capture a minimized window (no rendered content). \
             Restore the window first, or use `get_window_state` with \
             `capture_mode:\"ax\"` for UIA tree access — that works on \
             minimized windows."
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

    // 4. Frame pool + capture session. 1-frame pool because we only need
    //    a single shot; using 2+ frames would just waste GPU memory.
    let pool = Direct3D11CaptureFramePool::Create(
        &direct3d_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        item_size,
    )
    .context("Direct3D11CaptureFramePool::Create")?;
    let session = pool
        .CreateCaptureSession(&item)
        .context("Direct3D11CaptureFramePool::CreateCaptureSession")?;

    // 5. Subscribe to FrameArrived BEFORE StartCapture so we don't miss
    //    the first frame. The handler just signals the main thread.
    let (tx, rx) = mpsc::channel::<()>();
    let handler = TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(
        move |_pool, _args| {
            let _ = tx.send(());
            Ok(())
        },
    );
    let token = pool
        .FrameArrived(&handler)
        .context("FrameArrived subscription")?;

    // 6. Hide the WGC capture indicator (yellow border) if the build
    //    supports it. Available since Win11 (IsBorderRequired property).
    //    Best-effort — older Win10 won't have the setter; ignore failure.
    let _ = session.SetIsBorderRequired(false);
    let _ = session.SetIsCursorCaptureEnabled(false);

    // 7. Start capture, wait for the first frame.
    session.StartCapture().context("GraphicsCaptureSession::StartCapture")?;
    // 1500 ms is generous — first frames usually arrive in <50 ms on a
    // healthy GPU. Larger budget covers cold-start D3D11 cases.
    rx.recv_timeout(Duration::from_millis(1500))
        .context("WGC FrameArrived timed out after 1500 ms — GPU stalled?")?;

    let frame = pool
        .TryGetNextFrame()
        .context("Direct3D11CaptureFramePool::TryGetNextFrame")?;

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

    // 11. Cleanup: detach the event handler, close the session and pool.
    //     RAII would handle this but being explicit is cheaper to audit.
    let _ = pool.RemoveFrameArrived(token);
    let _ = session.Close();
    let _ = pool.Close();

    Ok((bgra, desc.Width, desc.Height))
}
