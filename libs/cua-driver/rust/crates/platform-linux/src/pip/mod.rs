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

use pip_preview::{PipBackend, PipConfig};

pub fn start(_cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
    Err(anyhow::anyhow!(
        "PiP preview is not yet implemented on Linux — track \
         trycua/cua follow-up issue for status"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_reports_unsupported() {
        let error = start(&PipConfig::default()).err().expect("unsupported PiP");
        assert_eq!(
            error.to_string(),
            "PiP preview is not yet implemented on Linux — track trycua/cua follow-up issue for status"
        );
    }
}
