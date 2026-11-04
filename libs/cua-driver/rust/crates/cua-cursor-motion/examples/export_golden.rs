//! Write the golden trajectories shared with `@trycua/cursor-motion`:
//!
//! ```bash
//! cargo run -p cua-cursor-motion --features serde --example export_golden \
//!   > crates/cua-cursor-motion/fixtures/golden.json
//! ```

#[path = "../tests/common/golden_cases.rs"]
mod golden_cases;

fn main() {
    println!(
        "{}",
        serde_json::to_string(&golden_cases::golden()).unwrap()
    );
}
