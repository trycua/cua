//! The managed **cua-runtime** VM: a container engine the SDK boots itself
//! when the host has none.
//!
//! # Design
//!
//! Goal (plan §1.1): run gVisor rootfs images with zero pre-setup on hosts
//! with no Docker-compatible engine (a bare Mac, a CI box without Docker).
//!
//! * **Image.** A small Linux VM image, `ghcr.io/trycua/cua-runtime:<ver>`,
//!   published as a multi-arch containerDisk (`FROM scratch` + `/disk/disk.img`,
//!   amd64 + arm64). Debian genericcloud base + `containerd`, `runsc`
//!   (registered as the default runtime), `dockerd` for the Docker Engine API,
//!   and nothing else. Built by the `cua-image` local builder from an
//!   `images.cua.ai/v1alpha1` spec, so it dogfoods the builder.
//! * **Boot.** macOS Apple Silicon: the [`crate::lume`] backend (VZ, raw disk,
//!   cloud-init seed via `--mount`), falling back to [`crate::qemu`] with HVF.
//!   Linux: [`crate::qemu`] with KVM. One instance per user, named
//!   `cua-runtime`, state under `~/.cua/vmm/<backend>/cua-runtime`.
//! * **API socket.** dockerd listens on a unix socket inside the guest. The
//!   host reaches it through an SSH-forwarded unix socket
//!   (`ssh -L ~/.cua/runtime/docker.sock:/run/docker.sock`) authenticated with
//!   a per-install key injected by cloud-init. The forward is owned by the SDK
//!   (or `cua daemon`) and is only readable by the user. A TCP hostfwd is
//!   deliberately *not* used: it would expose an unauthenticated root API to
//!   every local user.
//! * **Use.** [`crate::container::ContainerRuntime`] connects with
//!   `ContainerConfig { endpoint: Some("unix://~/.cua/runtime/docker.sock") }`,
//!   so containers, exec, checkpoints and port publishing work unchanged.
//!   Published container ports are bound inside the VM; the runtime adds
//!   matching SSH `-L` forwards (or QEMU `hostfwd`s) per sandbox.
//! * **Lifecycle.** Started lazily on first container request, stopped by
//!   `cua runtime stop` or after an idle timeout by the daemon. Upgrades: a new
//!   image version boots a fresh instance; images are re-pulled by the engine.
//! * **doctor.** Reports `managed` as provisionable whenever QEMU or Lume is
//!   ready, with the pull size and first-boot estimate.
//!
//! # Status
//!
//! Not implemented yet: selecting a container backend on a host without an
//! engine returns an actionable error from [`crate::auto::select`]. The pieces
//! it needs (VM boot with cloud-init seeds, SSH exec, container backend over an
//! arbitrary endpoint) exist and are exercised by the e2e suite.

/// One-line status for `doctor`.
pub const STATUS: &str = "managed cua-runtime VM (containerd + runsc) not implemented yet; install Colima/Docker for containers";

/// Fixed instance name of the managed runtime VM.
pub const INSTANCE_NAME: &str = "cua-runtime";
