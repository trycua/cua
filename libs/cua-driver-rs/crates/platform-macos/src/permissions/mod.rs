//! macOS TCC permission checks + first-launch CLI gate.
//!
//! Two layers:
//!   - [`status`] — low-level booleans for Accessibility / Screen Recording.
//!   - [`gate`]   — startup-time interactive flow that walks the user through
//!                  granting the missing permissions before `serve` binds.
//!
//! The gate is a Rust port of Swift's `PermissionsGate` (SwiftUI panel),
//! re-implemented as a terminal-only flow so the Rust port does not pull in
//! a GUI framework.  See `gate.rs` for the rationale + UX shape.
//!
//! Mirrors `libs/cua-driver/Sources/CuaDriverCore/Permissions/`:
//!   - `Permissions.swift`      → `permissions::status`
//!   - `PermissionsGate.swift`  → `permissions::gate` (CLI-only)

pub mod gate;
pub mod status;

pub use status::{PermissionsStatus, current_status};
pub use gate::{GateOpts, MissingPermission, run_if_needed};
