//! `cua-vmm`: local sandbox runtimes for the cua SDK.
//!
//! One [`Runtime`] trait over five backends:
//!
//! | backend | guests | module |
//! |---|---|---|
//! | Lume (`lume serve`, Apple Virtualization.framework) | macOS, Linux arm64 | [`lume`] |
//! | QEMU (`qemu-system-*`, hvf/kvm/whpx/tcg) | Linux, Windows | [`qemu`] |
//! | OCI containers via the Docker Engine API (gVisor `runsc` preferred) | Linux rootfs | [`container`] |
//! | Firecracker microVMs (Linux/KVM, unprivileged via `pasta`) | Linux rootfs | `microvm` |
//! | Apple Containerization microVMs (`container` CLI, macOS 26) | Linux rootfs | [`containerization`] |
//!
//! Sandboxes are **daemon-agnostic**: readiness is "the backend reports the
//! instance running" plus optional user [`Probe`]s. Nothing here waits for
//! computer-server, cua-spacesd or any other in-guest agent. Guest access
//! that needs no agent is exposed directly: published ports, VNC, QMP, the
//! serial log, SSH, [`GuestExec`] (container exec / SSH / `lume ssh`) and
//! VNC screenshots (`vnc::capture_png`).
//!
//! Local runtimes need **zero pre-setup**: [`auto`] picks a backend for an
//! image and [`auto::doctor`] reports what is available, what is missing and
//! what the SDK would provision (e.g. installing `runsc` into a Colima VM,
//! starting `lume serve`).

pub mod auto;
pub mod bulk;
pub mod cleanup;
pub mod cloudinit;
#[cfg(feature = "container")]
pub mod container;
#[cfg(feature = "containerization")]
pub mod containerization;
pub mod density;
pub mod disk;
pub mod egress;
pub mod elf;
pub mod error;
pub mod exec;
pub mod filetable;
pub mod gpu;
pub mod host;
pub mod hostvms;
pub mod identity_gate;
pub mod inject;
pub mod kinds;
pub mod local_network;
#[cfg(feature = "lume")]
pub mod lume;
pub mod managed;
#[cfg(feature = "microvm")]
pub mod microvm;
pub mod preflight;
pub mod probe;
pub mod progress;
#[cfg(feature = "qemu")]
pub mod qemu;
pub mod runtime;
pub mod screen;
pub mod storage;
pub mod types;
#[cfg(feature = "vnc")]
pub mod vnc;

pub use error::{Result, VmmError};
pub use exec::{ExecChunk, ExecOutput, ExecRequest, GuestExec, PrefixExec, SshExec};
pub use runtime::{
    CheckpointExport, DiskResolver, ExportFile, GuestIdentity, OciRunConfig, RootfsLayers,
    RootfsResolver, Runtime,
};
pub use types::*;
