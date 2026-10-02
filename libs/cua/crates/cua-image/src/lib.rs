//! `cua-image`: OCI images for the cua SDK.
//!
//! * [`registry`] — pull/push via `oci-client` with credentials from env,
//!   docker config and credential helpers (anonymous otherwise), rate-limit
//!   retries, streaming verified blob downloads/uploads, multi-arch indexes.
//! * [`cache`] — content-addressed local cache at `~/.cua/images`.
//! * [`containerdisk`] — KubeVirt containerDisk (`FROM scratch` +
//!   `/disk/disk.img`) extract/pack, and a [`cua_vmm::DiskResolver`] so the
//!   QEMU backend boots `ImageSource::Oci` refs.
//! * [`detect`] — classify a reference (containerDisk, rootfs, Lume, other macOS).
//! * [`resolve`] — the one resolver: reference + backend + arch to a pinned
//!   variant (`ResolvedImage`), used by every SDK, the daemon and cua-fleet.
//! * [`canonical`] — `ghcr.io/trycua/{linux,windows,macos}`, their tiers
//!   (`-slim`, full, `-xcode`), Omarchy and their aliases.
//! * [`catalog`] — the shared image catalog (`libs/images/sandbox-images.json`).
//! * [`rootfs`] — gVisor/container rootfs tar pack/unpack (with whiteouts).
//! * [`media_types`] — Lume/Tart/agoda/QEMU/OCI media types and detection.
//! * [`spec`] — the `images.cua.ai/v1alpha1` Image resource (schemars).
//! * [`builder`] — local builder: boot under `cua-vmm`, apply layers through a
//!   [`cua_vmm::GuestExec`], pack as containerDisk and/or rootfs, push.

pub mod auth;
pub mod builder;
pub mod cache;
pub mod canonical;
pub mod catalog;
pub mod containerdisk;
pub mod detect;
pub mod digest;
pub mod error;
pub mod layout;
pub mod manifest;
pub mod media_types;
pub mod publish;
pub mod registry;
pub mod resolve;
pub mod rootfs;
pub mod spec;
pub mod testing;

pub use cache::ImageCache;
pub use containerdisk::ContainerDiskResolver;
pub use cua_vmm::{GuestExec, RegistryCredentials, normalize_registry, registry_of};
pub use error::{ImageError, Result};
pub use layout::{PackedImage, push_multiarch};
pub use registry::{ManifestSource, RegistryClient};
pub use resolve::{Backend, ResolvedImage, Variant, resolve};
pub use spec::ImageResource;
