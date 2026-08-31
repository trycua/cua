//! Windows picture-in-picture preview stub.
//!
//! The Win32 implementation is a follow-up to the experimental macOS
//! drop. The plan is a layered, no-activate `WS_EX_NOACTIVATE |
//! WS_EX_TOPMOST` HWND with a child `STATIC` showing the bitmap of
//! the latest screenshot, fed via `SetWindowPos` for the never-key
//! behavior the spec requires. Tracking issue is linked in the docs.
//!
//! Until that lands, `start()` returns a clear error so `main.rs`
//! can log "PiP unavailable on this platform" and continue without
//! the window.

use pip_preview::{PipBackend, PipBackendFactory, PipConfig};

pub struct WindowsPipBackendFactory;

impl PipBackendFactory for WindowsPipBackendFactory {
    fn start(&self, _cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
        Err(anyhow::anyhow!(
            "PiP preview is not yet implemented on Windows — track \
             trycua/cua follow-up issue for status"
        ))
    }
}
