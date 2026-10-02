// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Streaming Windows.Graphics.Capture backend for one HWND.
//!
//! WGC reads the window's own composited frames out of DWM, so an occluded or
//! backgrounded window still streams. The capture session runs on a dedicated
//! thread with a free-threaded frame pool; frames are copied through a cached
//! staging texture into tightly packed BGRA and handed to the provider sink as
//! owned memory.
//!
//! Frame coordinates match `DWMWA_EXTENDED_FRAME_BOUNDS`, which is the same
//! space the cua-driver pixel tools (`click` x/y, `get_window_state`
//! screenshots) resolve against, so client pixel coordinates on a frame map
//! directly onto action coordinates.
//!
//! Constraints inherited from WGC: Windows 10 1903+, and a minimized window
//! has no rendered content — the monitor loop reports `Suspended("minimized")`
//! and `Resumed` around iconic state instead of failing the stream.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_media_protocol::SurfaceGeometry;
use cua_spacesd_provider_api::{
    CaptureEvent, CaptureLease, CaptureSink, OwnedFrame, PixelFormat, ProviderError,
    ProviderErrorCode,
};
use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindow,
};

use crate::windows_support;

const MONITOR_INTERVAL: Duration = Duration::from_millis(200);
const POLL_SLEEP: Duration = Duration::from_millis(4);

struct WindowsCaptureLease {
    stop: Arc<AtomicBool>,
}

impl CaptureLease for WindowsCaptureLease {
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub(crate) fn start(
    hwnd: u64,
    max_fps: u16,
    sink: Arc<dyn CaptureSink>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("rcdp-wgc-capture".into())
        .spawn(move || capture_thread(hwnd, max_fps, thread_stop, sink, native_geometry, ready_tx))
        .map_err(|error| {
            ProviderError::new(
                ProviderErrorCode::CaptureFailed,
                format!("failed to spawn capture thread: {error}"),
            )
        })?;
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(Arc::new(WindowsCaptureLease { stop })),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(ProviderError::new(
            ProviderErrorCode::CaptureFailed,
            "Windows capture setup timed out",
        )),
    }
}

struct CaptureStream {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: windows::Graphics::Capture::GraphicsCaptureSession,
    pool_size: SizeInt32,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

fn capture_error(message: impl Into<String>) -> ProviderError {
    ProviderError::new(ProviderErrorCode::CaptureFailed, message.into())
}

fn open_stream(hwnd: HWND) -> Result<CaptureStream, ProviderError> {
    unsafe {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE(std::ptr::null_mut()),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|error| capture_error(format!("D3D11CreateDevice failed: {error}")))?;
        let device = device.ok_or_else(|| capture_error("D3D11CreateDevice returned no device"))?;
        let context =
            context.ok_or_else(|| capture_error("D3D11CreateDevice returned no context"))?;

        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|error| capture_error(format!("IDXGIDevice cast failed: {error}")))?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
            .map_err(|error| capture_error(format!("WinRT device wrap failed: {error}")))?;
        let direct3d: IDirect3DDevice = inspectable
            .cast()
            .map_err(|error| capture_error(format!("IDirect3DDevice cast failed: {error}")))?;

        let interop: IGraphicsCaptureItemInterop =
            windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(
                |error| capture_error(format!("capture interop factory failed: {error}")),
            )?;
        let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd).map_err(|error| {
            capture_error(format!(
                "GraphicsCaptureItem::CreateForWindow failed (needs Windows 10 1903+ and a live window): {error}"
            ))
        })?;
        let size = item
            .Size()
            .map_err(|error| capture_error(format!("GraphicsCaptureItem::Size failed: {error}")))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err(capture_error(format!(
                "capture item size is {}x{}; the window may be cloaked",
                size.Width, size.Height
            )));
        }

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct3d,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|error| capture_error(format!("frame pool creation failed: {error}")))?;
        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|error| capture_error(format!("capture session creation failed: {error}")))?;
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        session
            .StartCapture()
            .map_err(|error| capture_error(format!("StartCapture failed: {error}")))?;

        Ok(CaptureStream {
            device,
            context,
            pool,
            session,
            pool_size: size,
            staging: None,
        })
    }
}

fn window_title(hwnd: HWND) -> String {
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..copied.max(0) as usize])
    }
}

fn capture_thread(
    hwnd_value: u64,
    max_fps: u16,
    stop: Arc<AtomicBool>,
    sink: Arc<dyn CaptureSink>,
    native_geometry: Arc<dyn Fn(u32, u32) + Send + Sync>,
    ready: std::sync::mpsc::Sender<Result<(), ProviderError>>,
) {
    let hwnd = HWND(hwnd_value as *mut _);
    let mut stream = match open_stream(hwnd) {
        Ok(stream) => {
            native_geometry(
                stream.pool_size.Width as u32,
                stream.pool_size.Height as u32,
            );
            let _ = ready.send(Ok(()));
            stream
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    let min_frame_interval = Duration::from_millis(1000 / u64::from(max_fps.clamp(1, 60)));
    let epoch = Instant::now();
    let mut last_emit: Option<Instant> = None;
    let mut last_monitor = Instant::now() - MONITOR_INTERVAL;
    let mut last_title = window_title(hwnd);
    let mut suspended = false;

    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }

        if last_monitor.elapsed() >= MONITOR_INTERVAL {
            last_monitor = Instant::now();
            unsafe {
                if !IsWindow(hwnd).as_bool() {
                    sink.on_event(CaptureEvent::Closed);
                    return;
                }
                let iconic = IsIconic(hwnd).as_bool();
                if iconic && !suspended {
                    suspended = true;
                    sink.on_event(CaptureEvent::Suspended("minimized".into()));
                } else if !iconic && suspended {
                    suspended = false;
                    sink.on_event(CaptureEvent::Resumed);
                }
            }
            let title = window_title(hwnd);
            if title != last_title {
                last_title = title.clone();
                sink.on_event(CaptureEvent::TitleChanged(title));
            }
        }

        let frame = match stream.pool.TryGetNextFrame() {
            Ok(frame) => frame,
            Err(_) => {
                std::thread::sleep(POLL_SLEEP);
                continue;
            }
        };

        let content_size = match frame.ContentSize() {
            Ok(size) => size,
            Err(_) => continue,
        };
        if content_size.Width <= 0 || content_size.Height <= 0 {
            continue;
        }
        if content_size != stream.pool_size {
            // The window resized: recreate the pool at the new size and wait
            // for a frame with matching dimensions. The stale frame is
            // discarded rather than published at wrong geometry.
            if stream
                .pool
                .Recreate(
                    &wgc_device(&stream),
                    DirectXPixelFormat::B8G8R8A8UIntNormalized,
                    2,
                    content_size,
                )
                .is_err()
            {
                sink.on_event(CaptureEvent::Suspended("capture_reset_failed".into()));
                return;
            }
            stream.pool_size = content_size;
            stream.staging = None;
            native_geometry(content_size.Width as u32, content_size.Height as u32);
            sink.on_event(CaptureEvent::GeometryChanged(SurfaceGeometry {
                width_px: content_size.Width as u32,
                height_px: content_size.Height as u32,
                scale_factor: windows_support::window_scale_factor(hwnd_value),
            }));
            continue;
        }

        if let Some(last) = last_emit {
            if last.elapsed() < min_frame_interval {
                continue;
            }
        }

        match copy_frame_bgra(&mut stream, &frame, content_size) {
            Ok(Some((bytes, width, height))) => {
                last_emit = Some(Instant::now());
                sink.on_event(CaptureEvent::Frame(OwnedFrame {
                    bytes: Arc::from(bytes),
                    format: PixelFormat::Bgra8,
                    width_px: width,
                    height_px: height,
                    bytes_per_row: Some(width * 4),
                    capture_timestamp_us: epoch.elapsed().as_micros() as u64,
                    encode_duration_us: None,
                    codec_epoch: 1,
                    keyframe: true,
                }));
            }
            Ok(None) => {}
            Err(_) => {
                sink.on_event(CaptureEvent::Suspended("frame_copy_failed".into()));
            }
        }
    }
}

fn wgc_device(stream: &CaptureStream) -> IDirect3DDevice {
    let dxgi: IDXGIDevice = stream.device.cast().expect("device supports IDXGIDevice");
    let inspectable =
        unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.expect("WinRT device wrap");
    inspectable.cast().expect("IDirect3DDevice cast")
}

fn copy_frame_bgra(
    stream: &mut CaptureStream,
    frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
    content_size: SizeInt32,
) -> windows::core::Result<Option<(Vec<u8>, u32, u32)>> {
    unsafe {
        let surface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let texture: ID3D11Texture2D = access.GetInterface()?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let width = content_size.Width as u32;
        let height = content_size.Height as u32;
        if desc.Width < width || desc.Height < height {
            // A frame produced before the pool recreation settled.
            return Ok(None);
        }

        let staging_matches = matches!(
            stream.staging,
            Some((_, staging_width, staging_height))
                if staging_width == desc.Width && staging_height == desc.Height
        );
        if !staging_matches {
            let mut staging_desc = desc;
            staging_desc.Usage = D3D11_USAGE_STAGING;
            staging_desc.BindFlags = 0;
            staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            staging_desc.MiscFlags = 0;
            let mut staging: Option<ID3D11Texture2D> = None;
            stream
                .device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
            let staging = staging.expect("CreateTexture2D succeeded with a texture");
            stream.staging = Some((staging, desc.Width, desc.Height));
        }
        let (staging, _, _) = stream.staging.as_ref().expect("staging texture exists");

        stream.context.CopyResource(staging, &texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        stream
            .context
            .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let stride = mapped.RowPitch as usize;
        let row_bytes = width as usize * 4;
        let mut bytes = vec![0u8; row_bytes * height as usize];
        let base = mapped.pData as *const u8;
        for row in 0..height as usize {
            std::ptr::copy_nonoverlapping(
                base.add(row * stride),
                bytes.as_mut_ptr().add(row * row_bytes),
                row_bytes,
            );
        }
        stream.context.Unmap(staging, 0);
        Ok(Some((bytes, width, height)))
    }
}
