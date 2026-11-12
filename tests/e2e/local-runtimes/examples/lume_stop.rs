//! Debug helper: `cargo run --example lume_stop -- <vm>` (only for cua-e2e-* VMs).
#[tokio::main]
async fn main() {
    let name = std::env::args().nth(1).unwrap();
    assert!(name.starts_with("cua-e2e-"), "only test VMs");
    let r = reqwest_stop(&name).await;
    println!("{r:?}");
}
async fn reqwest_stop(name: &str) -> Result<(), String> {
    let c = cua_vmm::lume::LumeClient::new("http://127.0.0.1:7777");
    c.stop(name).await.map_err(|e| format!("{e:?}"))
}
