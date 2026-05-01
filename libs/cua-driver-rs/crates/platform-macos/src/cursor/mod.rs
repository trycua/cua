//! Agent cursor overlay — transparent click-through window.
//!
//! Public surface:
//! - `overlay::init(cfg)` — called from `register_tools_with_cursor`
//! - `overlay::run_on_main_thread()` — called from `main()` on the main thread
//! - `overlay::send_command(cmd)` — called from tool implementations

pub mod state;
pub mod overlay;

// Re-export the legacy per-instance cursor state (used by tools for multi-cursor tracking).
pub use state::{CursorRegistry, CursorState};
// Note: `state::CursorConfig` (the old runtime config) is intentionally not re-exported
// at this level to avoid conflicting with `cursor_overlay::CursorConfig` (the CLI/shared config).
