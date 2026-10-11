// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! GPU options a sandbox can be created with, per runtime.
//!
//! Every runtime says what it offers on this host ([`GpuSupport`]): the
//! options, each with whether it works here and why not. A runtime with
//! nothing to offer has no options and says why ([`GpuSupport::reason`]),
//! so a UI hides its GPU row and a CLI can explain a refused `--gpu`.
//!
//! | runtime | option | what it does |
//! |---|---|---|
//! | Lume (macOS guests) | [`PARAVIRTUAL`] | the unrestricted feature level of Apple's paravirtualized GPU, applied before each start |
//! | QEMU (Linux host) | [`VIRGL`] | `virtio-gpu-gl-pci` with a headless EGL display on the host's render node |
//! | containers (Linux host) | [`NVIDIA`] | every NVIDIA GPU through the engine's `nvidia` runtime |
//! | cloud and contrib providers | their GPU types | a GPU machine type, gated by quota and budget |

use serde::{Deserialize, Serialize};

/// Lume: Apple's paravirtualized GPU at its unrestricted device feature
/// level (`com.apple.gpusw.ParavirtualizedGraphics`
/// `ForceUnrestrictedDeviceFeatureLevel`).
pub const PARAVIRTUAL: &str = "paravirtual";
/// QEMU: virgl (`virtio-gpu-gl-pci`, `-display egl-headless`).
pub const VIRGL: &str = "virgl";
/// Containers: NVIDIA GPUs through the engine's `nvidia` runtime.
pub const NVIDIA: &str = "nvidia";

/// The Lume GPU guide (published docs route of
/// `docs/content/docs/lume/guides/gpu-passthrough.mdx`).
pub const LUME_GPU_DOCS: &str = "https://cua.ai/docs/lume/guides/gpu-passthrough";

/// One GPU option.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuOption {
    /// What to pass as `gpu` ([`PARAVIRTUAL`], [`VIRGL`], [`NVIDIA`], a
    /// cloud GPU type).
    pub id: String,
    /// How a person names it ("GPU acceleration").
    pub label: String,
    /// Experimental (a UI says so next to the label).
    #[serde(default)]
    pub experimental: bool,
    /// Whether it works on this host (and account) now.
    pub supported: bool,
    /// Why not, one short line (empty when supported).
    #[serde(default)]
    pub reason: String,
    /// A page that explains it.
    #[serde(default)]
    pub learn_more: Option<String>,
    /// Estimated cost per hour while it runs (cloud GPU types).
    #[serde(default)]
    pub usd_per_hour: Option<f64>,
}

/// What one runtime offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GpuSupport {
    /// The runtime (`lume`, `qemu`, `container`, `gvisor`, `fleet`, a
    /// contrib or cloud provider's name).
    pub runtime: String,
    /// Its options (empty: none).
    #[serde(default)]
    pub options: Vec<GpuOption>,
    /// Why it has none (empty when it has some).
    #[serde(default)]
    pub reason: String,
}

impl GpuSupport {
    /// A runtime with no GPU option, and why.
    pub fn none(runtime: &str, reason: impl Into<String>) -> Self {
        Self {
            runtime: runtime.into(),
            options: vec![],
            reason: reason.into(),
        }
    }

    /// The option `requested` names: `""` or `auto` picks the first one
    /// that works here. The error says why nothing fits.
    pub fn pick(&self, requested: &str) -> std::result::Result<&GpuOption, String> {
        let requested = requested.trim();
        let any = requested.is_empty() || requested == "auto" || requested == "on";
        if self.options.is_empty() {
            return Err(format!(
                "{} has no GPU option{}",
                self.runtime,
                if self.reason.is_empty() {
                    String::new()
                } else {
                    format!(": {}", self.reason)
                }
            ));
        }
        let found = if any {
            self.options
                .iter()
                .find(|o| o.supported)
                .or(self.options.first())
        } else {
            self.options.iter().find(|o| o.id == requested)
        };
        match found {
            Some(o) if o.supported => Ok(o),
            Some(o) => Err(format!("{} is not available here: {}", o.label, o.reason)),
            None => Err(format!(
                "{} has no GPU option {requested:?} (it offers {})",
                self.runtime,
                self.options
                    .iter()
                    .map(|o| o.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

/// Lume's option on this host: Apple silicon only (the paravirtualized GPU
/// of Apple's Virtualization framework); `lume_found` says whether Lume is
/// installed.
pub fn lume_support(lume_found: bool) -> GpuSupport {
    lume_support_for(std::env::consts::OS, std::env::consts::ARCH, lume_found)
}

fn lume_support_for(os: &str, arch: &str, lume_found: bool) -> GpuSupport {
    if os != "macos" {
        return GpuSupport::none("lume", "Lume runs on macOS only");
    }
    let reason = if arch != "aarch64" {
        "Needs a Mac with Apple silicon"
    } else if !lume_found {
        "Needs Lume"
    } else {
        ""
    };
    GpuSupport {
        runtime: "lume".into(),
        options: vec![GpuOption {
            id: PARAVIRTUAL.into(),
            label: "GPU acceleration".into(),
            experimental: true,
            supported: reason.is_empty(),
            reason: reason.into(),
            learn_more: Some(LUME_GPU_DOCS.into()),
            usd_per_hour: None,
        }],
        reason: String::new(),
    }
}

/// What a QEMU on this host needs for virgl.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirglHost {
    /// `virtio-gpu-gl-pci` is in `-device help`.
    pub gl_device: bool,
    /// `egl-headless` is in `-display help`.
    pub egl_headless: bool,
    /// A DRM render node (`/dev/dri/renderD128`).
    pub render_node: Option<String>,
}

/// QEMU's option from what the host has. macOS QEMU builds have no
/// headless GL display, so there is none there.
pub fn qemu_support(os: &str, host: &VirglHost) -> GpuSupport {
    if os != "linux" {
        return GpuSupport::none(
            "qemu",
            "QEMU on this system has no headless OpenGL display for virgl",
        );
    }
    let reason = if !host.gl_device {
        "This QEMU was built without virglrenderer (no virtio-gpu-gl-pci)"
    } else if !host.egl_headless {
        "This QEMU has no egl-headless display"
    } else if host.render_node.is_none() {
        "No GPU render node (/dev/dri/renderD*)"
    } else {
        ""
    };
    GpuSupport {
        runtime: "qemu".into(),
        options: vec![GpuOption {
            id: VIRGL.into(),
            label: "GPU acceleration (virgl)".into(),
            experimental: true,
            supported: reason.is_empty(),
            reason: reason.into(),
            learn_more: None,
            usd_per_hour: None,
        }],
        reason: String::new(),
    }
}

/// The container engine's option: NVIDIA GPUs when it has the `nvidia`
/// runtime. `os` is the host's: Docker on macOS and Windows runs Linux in a
/// VM that sees no GPU.
pub fn container_support(os: &str, runtime: &str, nvidia_runtime: bool) -> GpuSupport {
    if os != "linux" {
        return GpuSupport::none(
            runtime,
            "Containers here run in a Linux VM with no GPU access",
        );
    }
    if !nvidia_runtime {
        return GpuSupport::none(runtime, "The container engine has no nvidia runtime");
    }
    let gvisor = runtime == "gvisor";
    GpuSupport {
        runtime: runtime.into(),
        options: vec![GpuOption {
            id: NVIDIA.into(),
            label: "NVIDIA GPUs".into(),
            experimental: false,
            supported: !gvisor,
            reason: if gvisor {
                "gVisor containers get no GPU; choose runc".into()
            } else {
                String::new()
            },
            learn_more: None,
            usd_per_hour: None,
        }],
        reason: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lume_offers_gpu_acceleration_on_apple_silicon_only() {
        let s = lume_support_for("macos", "aarch64", true);
        let o = s.pick("").unwrap();
        assert_eq!((o.id.as_str(), o.experimental), (PARAVIRTUAL, true));
        assert_eq!(o.learn_more.as_deref(), Some(LUME_GPU_DOCS));
        let intel = lume_support_for("macos", "x86_64", true);
        assert_eq!(intel.options[0].reason, "Needs a Mac with Apple silicon");
        assert_eq!(
            intel.pick("paravirtual").unwrap_err(),
            "GPU acceleration is not available here: Needs a Mac with Apple silicon"
        );
        assert_eq!(
            lume_support_for("macos", "aarch64", false).options[0].reason,
            "Needs Lume"
        );
        assert!(
            lume_support_for("linux", "aarch64", true)
                .options
                .is_empty()
        );
    }

    #[test]
    fn qemu_needs_every_virgl_piece_on_linux() {
        let all = VirglHost {
            gl_device: true,
            egl_headless: true,
            render_node: Some("/dev/dri/renderD128".into()),
        };
        assert!(qemu_support("linux", &all).pick("virgl").is_ok());
        assert!(qemu_support("macos", &all).options.is_empty());
        let no_node = VirglHost {
            render_node: None,
            ..all.clone()
        };
        assert_eq!(
            qemu_support("linux", &no_node).options[0].reason,
            "No GPU render node (/dev/dri/renderD*)"
        );
        let no_gl = VirglHost {
            gl_device: false,
            ..all
        };
        assert!(!qemu_support("linux", &no_gl).options[0].supported);
    }

    #[test]
    fn containers_get_nvidia_gpus_on_linux_with_runc() {
        assert!(
            container_support("linux", "container", true)
                .pick("nvidia")
                .is_ok()
        );
        let err = container_support("macos", "container", true)
            .pick("")
            .unwrap_err();
        assert!(err.contains("Linux VM with no GPU access"), "{err}");
        assert!(!container_support("linux", "gvisor", true).options[0].supported);
        assert!(
            container_support("linux", "container", false)
                .options
                .is_empty()
        );
        let e = container_support("linux", "container", true)
            .pick("a100")
            .unwrap_err();
        assert!(e.contains("offers nvidia"), "{e}");
    }
}
