//! Run the markdown processor over a real recording directory.
//! Usage: cargo run -p cua-driver-core --example process_dir -- <dir>
use cua_driver_core::recording_markdown::{process, ProcessOptions};

fn main() {
    let dir = std::env::args().nth(1).expect("pass a recording dir");
    let res = process(std::path::Path::new(&dir), &ProcessOptions::default()).unwrap();
    println!("trajectory: {}", res.trajectory_md.display());
    println!("summary: {} turns, {} human, {} agent",
        res.summary.turn_count, res.summary.human_turns, res.summary.agent_turns);
}
