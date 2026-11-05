//! Windows images built on this computer from an evaluation ISO.
//!
//! `windows` / `windows:2022` is the published containerDisk. The other
//! Windows versions (`windows:11`, `windows:2025`, ...) have no published
//! image: the first Space made from one installs Windows unattended from an
//! ISO (yours, or Microsoft's evaluation media), seals the result as a base
//! disk under `$CUA_HOME/images/windows/<version>-<arch>/`, and every later
//! Space is a clone of it. See `docs/windows-images.md`.
//!
//! * [`table`] - the version table and alias normalisation.
//! * [`iso`] - ISO resolution: your path, then the cache, then a download.
//! * [`unattend`] - the answer file, the in-guest setup script, the data ISO.
//! * [`build`] - the build plan, cache layout and orchestration.

pub mod build;
pub mod iso;
pub mod joliet;
pub mod table;
pub mod unattend;

pub use table::{WindowsArch, WindowsVersion, lookup, normalize_version, versions};
