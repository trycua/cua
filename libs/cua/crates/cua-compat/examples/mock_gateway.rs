//! A gateway over mock Spaces (in-process mock cua-spacesd: simulated
//! processes, in-memory files; nothing runs on the host), for driving the
//! unmodified provider SDK in hermetic tests.
//!
//!     cargo run -p cua-compat --features testing --example mock_gateway -- \
//!         --dir /tmp/gw [--provider e2b|daytona] [--port 8080=127.0.0.1:18080]
//!
//! Prints one JSON line with the client environment, then serves.

use cua_compat::backend::mock::MockBackend;
use cua_compat::{Gateway, GatewayOptions, Provider};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut ports = Vec::new();
    let mut provider = Provider::E2b;
    let mut plan_limit = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = args.next(),
            "--provider" => {
                provider =
                    Provider::parse(&args.next().expect("--provider NAME")).expect("provider")
            }
            "--port" => {
                let spec = args.next().expect("--port GUEST=HOST:PORT");
                let (guest, addr) = spec.split_once('=').expect("--port GUEST=HOST:PORT");
                ports.push((guest.parse::<u16>().expect("guest port"), addr.to_string()));
            }
            // Every create answers the live-session limit (for SDK repros).
            "--plan-limit" => plan_limit = true,
            other => panic!("unknown argument {other}"),
        }
    }
    let backend = Arc::new(MockBackend::new());
    for (g, a) in ports {
        backend.ports.lock().await.insert(g, a);
    }
    if plan_limit {
        *backend.create_fails.lock().await = Some(cua_compat::backend::BackendError::RateLimited(
            "You're using 2 of 2 live sessions on the free plan. Close one or upgrade to Pro."
                .into(),
        ));
    }
    let mut opts = GatewayOptions::new(provider);
    opts.listen = "127.0.0.1:0".parse().unwrap();
    opts.host_port = 0;
    opts.dir = dir.map(Into::into);
    opts.create_default_user = false;
    let gw = Gateway::start(opts, Some(backend)).await.expect("gateway");
    let env: serde_json::Map<String, serde_json::Value> = gw
        .env()
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    println!("{}", serde_json::Value::Object(env));
    gw.wait().await;
}
