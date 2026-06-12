//! Linux picture-in-picture preview stub.
//!
//! The Linux implementation is a follow-up to the experimental macOS
//! drop. Plan: GTK4 always-on-top utility window under X11/XWayland,
//! with a Wayland-native fallback via `wlr-layer-shell` where
//! supported. Tracking issue linked in the docs.
//!
//! Until that lands, `start()` returns a clear error so `main.rs`
//! can log "PiP unavailable on this platform" and continue without
//! the window.

use pip_preview::{PipBackend, PipBackendFactory, PipConfig};

pub struct LinuxPipBackendFactory;

impl PipBackendFactory for LinuxPipBackendFactory {
    fn start(&self, _cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
        Err(anyhow::anyhow!(
            "PiP preview is not yet implemented on Linux — track \
             trycua/cua follow-up issue for status"
        ))
    }
}
