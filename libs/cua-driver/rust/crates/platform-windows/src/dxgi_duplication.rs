// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! DXGI Desktop Duplication backend for fullscreen windows.
//!
//! `PrintWindow` and GDI `BitBlt` capture the contents DWM last composed for
//! an HWND. That is up to date for ordinary windows, but a fullscreen,
//! flip-model Direct3D swap chain — the presentation mode used by Unity /
//! Unreal games today, even in "fullscreen windowed" — is scanned out
//! directly by the display controller instead of being composed through the
//! window's DWM redirection surface. DWM keeps only the last composed copy,
//! so PrintWindow (and a screen-region BitBlt through a window DC) can return
//! a stale frame while the display itself keeps updating.
//!
//! DXGI Desktop Duplication captures the real display image regardless of how
//! the owning process presents. This module captures the monitor output that
//! a fullscreen window covers; `capture.rs` uses it as a fast path and keeps
//! the PrintWindow / WGC / BitBlt ladder as a fallback for every error case
//! reported here (rotated output, protected/exclusive content, no single
//! monitor fully covered by the window, duplication unavailable, ...).
//!
//! The cursor is not part of the duplicated frame unless a process draws it,
//! matching the GDI screen-capture path. Only identity-rotation outputs are
//! handled; rotated displays fall back to the existing capture ladder.
//!
//! The first frame handed out after `DuplicateOutput` is an unpopulated
//! placeholder (no present time, all-zero pixels), so `acquire_frame` skips
//! frames until the display has actually presented. A desktop that does not
//! present within the budget falls back to the existing capture ladder as
//! well, so a frozen screenshot is never returned from here.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION_IDENTITY,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
    DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
};

/// Timeout passed to each `AcquireNextFrame` call. A static desktop can make
/// the call time out; we retry until the total budget below is spent.
const ACQUIRE_STEP_TIMEOUT_MS: u32 = 250;

/// Total budget for delivering one duplicated frame. The first frame after
/// `DuplicateOutput` normally arrives immediately, so this is generous.
const ACQUIRE_TOTAL_TIMEOUT_MS: u64 = 1_000;

/// One monitor-sized desktop frame captured through Desktop Duplication.
pub(crate) struct CapturedOutput {
    /// Tightly packed, top-down 32-bit BGRA pixels (`width * 4` bytes/row).
    pub(crate) bgra: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Screen rectangle (physical pixels) the frame was captured from.
    pub(crate) desktop_rect: RECT,
}

/// Captures the current display image of the single monitor that `window_rect`
/// fully covers, returning the pixels plus that monitor's screen rectangle.
///
/// A fullscreen flip-model Direct3D window covers its monitor exactly (an
/// exclusive-mode window may overhang it slightly), which is the shape used to
/// route those windows away from the stale PrintWindow path. Returns an error
/// — and the caller falls back to the normal capture ladder — when the
/// rectangle does not fully cover exactly one monitor, when the monitor is
/// rotated, or when duplication is unavailable (already duplicated, protected
/// content, remote session, ...).
pub(crate) fn capture_output_covered_by(window_rect: RECT) -> Result<CapturedOutput> {
    unsafe { capture_output_covered_by_impl(window_rect) }
}

unsafe fn capture_output_covered_by_impl(window_rect: RECT) -> Result<CapturedOutput> {
    if !rect_has_area(window_rect) {
        bail!("DXGI duplication: window rectangle is empty");
    }

    let factory: IDXGIFactory1 =
        CreateDXGIFactory1().context("DXGI duplication: CreateDXGIFactory1 failed")?;

    let mut adapter_index = 0;
    loop {
        let adapter = match factory.EnumAdapters1(adapter_index) {
            Ok(adapter) => adapter,
            // DXGI_ERROR_NOT_FOUND: no more adapters.
            Err(_) => break,
        };
        adapter_index += 1;

        let mut output_index = 0;
        loop {
            let output = match adapter.EnumOutputs(output_index) {
                Ok(output) => output,
                // DXGI_ERROR_NOT_FOUND: no more outputs on this adapter.
                Err(_) => break,
            };
            output_index += 1;

            let output_desc = output
                .GetDesc()
                .context("DXGI duplication: IDXGIOutput::GetDesc failed")?;
            if !output_desc.AttachedToDesktop.as_bool()
                || !rect_covers(window_rect, output_desc.DesktopCoordinates)
            {
                continue;
            }

            // Desktop Duplication requires the D3D11 device and the output to
            // belong to the same adapter, so create the device on this one.
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                Some(&*adapter),
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE(std::ptr::null_mut()),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("DXGI duplication: D3D11CreateDevice failed")?;
            let device =
                device.context("DXGI duplication: D3D11CreateDevice returned no device")?;
            let context =
                context.context("DXGI duplication: D3D11CreateDevice returned no context")?;

            let output1: IDXGIOutput1 = output
                .cast()
                .context("DXGI duplication: output does not support IDXGIOutput1")?;
            let duplication = output1.DuplicateOutput(&device).context(
                "DXGI duplication: DuplicateOutput failed (output already duplicated, \
                 protected content, or a remote session)",
            )?;
            if duplication.GetDesc().Rotation.0 != DXGI_MODE_ROTATION_IDENTITY.0 {
                bail!("DXGI duplication: rotated outputs are not supported");
            }

            let (_frame, texture, texture_desc) = acquire_frame(&duplication)?;
            if texture_desc.Format.0 != DXGI_FORMAT_B8G8R8A8_UNORM.0 {
                bail!(
                    "DXGI duplication: unexpected desktop pixel format {:?}",
                    texture_desc.Format
                );
            }

            let staging = create_staging_texture(&device, &texture_desc)?;
            context.CopyResource(&staging, &texture);
            let bgra = read_mapped_texture(&context, &staging, &texture_desc)?;

            return Ok(CapturedOutput {
                bgra,
                width: texture_desc.Width,
                height: texture_desc.Height,
                desktop_rect: output_desc.DesktopCoordinates,
            });
        }
    }

    bail!("DXGI duplication: no monitor is fully covered by the requested window rectangle")
}

/// Holds one `AcquireNextFrame` lease so every early return releases the
/// frame back to the output instead of leaking the duplication.
struct AcquiredFrame<'a> {
    duplication: &'a IDXGIOutputDuplication,
}

impl Drop for AcquiredFrame<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.duplication.ReleaseFrame() };
    }
}

/// Acquires one desktop frame and returns the texture plus its description.
///
/// Retries `AcquireNextFrame` on `DXGI_ERROR_WAIT_TIMEOUT` (no new frame yet)
/// until the total budget runs out. Any other error — access lost, protected
/// content, mode change — is surfaced so the caller can fall back.
unsafe fn acquire_frame(
    duplication: &IDXGIOutputDuplication,
) -> Result<(AcquiredFrame<'_>, ID3D11Texture2D, D3D11_TEXTURE2D_DESC)> {
    let deadline = Instant::now() + Duration::from_millis(ACQUIRE_TOTAL_TIMEOUT_MS);
    loop {
        let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        match duplication.AcquireNextFrame(ACQUIRE_STEP_TIMEOUT_MS, &mut frame_info, &mut resource)
        {
            Ok(()) => {
                // A freshly created duplication delivers an initial
                // placeholder frame: no present time, no accumulated updates,
                // and an unpainted (all-zero) desktop image. Skip it and wait
                // for the first real present instead of returning a blank
                // screenshot; a desktop that never presents falls through to
                // the existing capture ladder.
                if frame_info.LastPresentTime == 0 {
                    drop(AcquiredFrame { duplication });
                    if Instant::now() >= deadline {
                        bail!(
                            "DXGI duplication: no presented desktop frame within \
                             {ACQUIRE_TOTAL_TIMEOUT_MS} ms"
                        );
                    }
                    continue;
                }
                let resource = resource
                    .context("DXGI duplication: AcquireNextFrame returned no desktop resource")?;
                let texture: ID3D11Texture2D = resource
                    .cast()
                    .context("DXGI duplication: desktop resource is not a 2D texture")?;
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                texture.GetDesc(&mut desc);
                if desc.Width == 0 || desc.Height == 0 {
                    bail!("DXGI duplication: acquired an empty desktop texture");
                }
                return Ok((AcquiredFrame { duplication }, texture, desc));
            }
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => {
                if Instant::now() >= deadline {
                    bail!(
                        "DXGI duplication: AcquireNextFrame timed out after \
                         {ACQUIRE_TOTAL_TIMEOUT_MS} ms"
                    );
                }
            }
            Err(error) => {
                bail!("DXGI duplication: AcquireNextFrame failed: {error}");
            }
        }
    }
}

/// Creates a CPU-readable staging texture matching the duplicated frame.
unsafe fn create_staging_texture(
    device: &ID3D11Device,
    texture_desc: &D3D11_TEXTURE2D_DESC,
) -> Result<ID3D11Texture2D> {
    let mut staging_desc = *texture_desc;
    staging_desc.MipLevels = 1;
    staging_desc.ArraySize = 1;
    staging_desc.Usage = D3D11_USAGE_STAGING;
    staging_desc.BindFlags = 0;
    staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
    staging_desc.MiscFlags = 0;

    let mut staging: Option<ID3D11Texture2D> = None;
    device
        .CreateTexture2D(&staging_desc, None, Some(&mut staging))
        .context("DXGI duplication: CreateTexture2D(staging) failed")?;
    staging.context("DXGI duplication: CreateTexture2D returned no texture")
}

/// Maps a staging texture and copies the BGRA rows into a tightly packed
/// buffer (the mapped row pitch may be padded and can exceed `width * 4`).
unsafe fn read_mapped_texture(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    texture_desc: &D3D11_TEXTURE2D_DESC,
) -> Result<Vec<u8>> {
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    context
        .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .context("DXGI duplication: ID3D11DeviceContext::Map(staging) failed")?;

    let width = texture_desc.Width as usize;
    let height = texture_desc.Height as usize;
    let stride = mapped.RowPitch as usize;
    let src_base = mapped.pData as *const u8;
    let mut bgra = vec![0u8; width * height * 4];
    for row in 0..height {
        std::ptr::copy_nonoverlapping(
            src_base.add(row * stride),
            bgra.as_mut_ptr().add(row * width * 4),
            width * 4,
        );
    }
    context.Unmap(staging, 0);
    Ok(bgra)
}

/// True when `rect` has a positive width and height.
fn rect_has_area(rect: RECT) -> bool {
    rect.right > rect.left && rect.bottom > rect.top
}

/// True when `outer` fully contains `inner` (and `inner` has area).
fn rect_covers(outer: RECT, inner: RECT) -> bool {
    rect_has_area(inner)
        && outer.left <= inner.left
        && outer.top <= inner.top
        && outer.right >= inner.right
        && outer.bottom >= inner.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn identical_rect_is_covered() {
        let monitor = rect(0, 0, 2048, 1152);
        assert!(rect_covers(monitor, monitor));
    }

    #[test]
    fn monitor_rect_is_covered_by_slightly_larger_window() {
        // Exclusive fullscreen windows may overhang the monitor by a pixel.
        let window = rect(-1, -1, 2049, 1153);
        assert!(rect_covers(window, rect(0, 0, 2048, 1152)));
    }

    #[test]
    fn partial_overlap_is_not_covered() {
        // A window that only covers part of a monitor must not take the
        // duplication path — its pixels would not map 1:1 to the window.
        let window = rect(0, 0, 1920, 1080);
        assert!(!rect_covers(window, rect(0, 0, 2048, 1152)));
    }

    #[test]
    fn monitor_on_secondary_display_is_covered() {
        let monitor = rect(-2560, 0, 0, 1440);
        let window = rect(-2560, 0, 0, 1440);
        assert!(rect_covers(window, monitor));
    }

    #[test]
    fn empty_rects_are_rejected() {
        assert!(!rect_has_area(rect(0, 0, 0, 100)));
        assert!(!rect_covers(rect(0, 0, 100, 100), rect(10, 10, 10, 20)));
    }
}
