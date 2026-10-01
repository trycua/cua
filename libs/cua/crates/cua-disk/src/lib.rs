//! `cua-disk`: everything the cua SDK writes to disk, how much it takes, and
//! how it is cleaned up.
//!
//! * [`Layout`]: every location under the cua home (`$CUA_HOME`, else
//!   `~/.cua`) plus the few that live elsewhere (Lume's VM store, the
//!   container engine, cua-bench runs, cua-spacesd logs).
//! * [`scan`]: per-item accounting ([`Report`]): the image cache, VM and
//!   container sandboxes, SDK-labelled Docker images and volumes, Lume VMs
//!   the SDK created, build outputs, logs and data. Each item says whether it
//!   is cache (evictable), what references it, and when it was last used.
//! * [`gc`]: the cache budget and least-recently-used eviction of
//!   unreferenced cache entries. It never removes a sandbox (running,
//!   stopped or named), anything a sandbox references, or a Docker object
//!   without the `ai.cua.managed` label (images the SDK pulled are recorded
//!   in a ledger instead, see `cua_vmm::container::ledger`).
//! * [`logs`]: size caps for the host logs other processes write.
//!
//! Free-space checks before pulls, builds and VM creates live one layer
//! down, in `cua_vmm::disk` (every writer can call them).

pub mod docker;
pub mod gc;
pub mod layout;
pub mod logs;
pub mod lume;
pub mod qcow2;
pub mod scan;
pub mod scratch;

pub use cua_vmm::disk::{Budget, CacheConfig, InsufficientDisk, Space, format_size, parse_size};
pub use gc::{GcOptions, GcReport, Removed, auto_gc, collect, plan};
pub use layout::Layout;
pub use scan::{Category, Item, Report, Scanner, Target};
