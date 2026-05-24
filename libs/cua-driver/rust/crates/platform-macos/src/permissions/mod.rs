//! macOS TCC permission checks + first-launch CLI gate.
//!
//! Two layers:
//!   - [`status`] — low-level booleans for Accessibility / Screen Recording.
//!   - [`gate`]   — startup-time interactive flow that walks the user through
//!                  granting the missing permissions before `serve` binds.
//!
//! The gate is a Rust port of Swift's `PermissionsGate` (SwiftUI panel).
//! Two presentation surfaces:
//!   - native NSPanel (`panel`, Phase 1+) — used when the daemon is launched
//!     from the bundled `.app` and the env-var opt-out is not set;
//!   - terminal banner (`gate::wait_for_grants`) — fallback for bare-binary
//!     invocations, headless environments, CI, and explicit opt-outs.
//!
//! Mirrors `libs/cua-driver/Sources/CuaDriverCore/Permissions/`:
//!   - `Permissions.swift`      → `permissions::status`
//!   - `PermissionsGate.swift`  → `permissions::gate` + `permissions::panel`

pub mod gate;
pub mod status;

#[cfg(target_os = "macos")]
pub mod panel;

pub use status::{PermissionsStatus, current_status};
pub use gate::{GateOpts, MissingPermission, run_if_needed};
