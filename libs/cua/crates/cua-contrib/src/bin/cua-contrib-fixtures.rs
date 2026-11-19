//! Loopback fixtures for contrib-provider tests outside Rust (the cua-bench
//! and docs lanes): a schema mock of the Daytona API whose sandboxes' port
//! 3211 is a mock cua-spacesd (in-memory files, simulated processes).
//! Prints one JSON line with the endpoints, then serves until stdin closes.
//! Nothing touches the host beyond two loopback listeners.

use cua_contrib::testing::MockDaytona;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use tokio::io::AsyncReadExt;

/// The fixture's Daytona key (not a secret: only this mock accepts it).
const KEY: &str = "dtn_fixture_key";

#[tokio::main]
async fn main() {
    let spacesd = MockServer::start(MockAuth::default()).await;
    let daytona = MockDaytona::start(KEY).await;
    daytona.route_port(3211, &spacesd.url());
    println!(
        "{}",
        serde_json::json!({
            "daytona_api": daytona.url(),
            "daytona_key": KEY,
            "spacesd": spacesd.url(),
        })
    );
    // Serve until the parent closes stdin (bounded reads, no busy loop).
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 256];
    loop {
        match stdin.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let live = daytona.sandboxes();
    if !live.is_empty() {
        eprintln!("cua-contrib-fixtures: sandboxes left at exit: {live:?}");
    }
}
