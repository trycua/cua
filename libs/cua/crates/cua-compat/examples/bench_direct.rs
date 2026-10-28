//! The direct Cua side of the compat overhead guard: the same operations the
//! gateway's E2B clients perform (create a Space, run a command, write and
//! read a file), done straight through the Cua SDK and the Space's
//! cua-spacesd, with no gateway and no provider protocol in between.
//!
//!     bench_direct [--on local] [--kind vm] [--image IMG] [--cpus 2] [--memory-mb 2048]
//!                  [--n 100] [--mib 64]
//!
//! Prints one JSON object (milliseconds; MiB/s).

use cua_compat::backend::{
    Backend, CreateSpec, CuaBackend, Identity, Placement, Source, SpaceDefaults,
};
use cua_spacesd_client::{Command, pb};
use futures_util::StreamExt;
use std::time::Instant;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = arg(&args, "--n")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let mib: usize = arg(&args, "--mib")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let defaults = SpaceDefaults {
        on: arg(&args, "--on").unwrap_or_else(|| "local".into()),
        kind: arg(&args, "--kind").unwrap_or_else(|| "vm".into()),
        image: arg(&args, "--image").unwrap_or_default(),
        cpus: arg(&args, "--cpus").and_then(|v| v.parse().ok()),
        memory_mb: arg(&args, "--memory-mb").and_then(|v| v.parse().ok()),
        overlays: vec![],
    };
    let backend = CuaBackend::new(defaults);
    let who = Identity::default();
    let name = format!("e2b-bench-{}", std::process::id());
    let t0 = Instant::now();
    backend
        .create(
            &who,
            CreateSpec {
                name: name.clone(),
                source: Source::Image(None),
                env: Default::default(),
                internet: true,
                place: Placement::default(),
            },
        )
        .await
        .expect("create");
    let c = backend.spacesd(&who, &name).await.expect("spacesd");
    let create_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let out = run(&backend, &who, &name, n, mib, &c).await;
    let _ = backend.delete(&who, &name).await;
    let mut out = out;
    out["create_ms"] = serde_json::json!(create_ms);
    println!("{out}");
}

async fn run(
    _backend: &CuaBackend,
    _who: &Identity,
    _name: &str,
    n: usize,
    mib: usize,
    c: &cua_spacesd_client::SpacesdClient,
) -> serde_json::Value {
    // Command round trip.
    let mut ts = Vec::new();
    for _ in 0..(n + 5) {
        let t = Instant::now();
        let o = c
            .run(Command::new("/bin/echo").arg("hi"))
            .await
            .expect("run");
        assert!(o.success());
        ts.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    ts.drain(..5);
    ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let cmd_p50 = ts[ts.len() / 2];
    // File write and read, streamed.
    let chunk = vec![0x5au8; 1 << 20];
    let path = "/tmp/bench-direct.bin";
    let t = Instant::now();
    let (tx, rx) = tokio::sync::mpsc::channel::<pb::WriteFileRequest>(4);
    tx.send(pb::WriteFileRequest {
        message: Some(pb::write_file_request::Message::Header(
            pb::WriteFileHeader {
                path: path.into(),
                mode: pb::WriteMode::Overwrite as i32,
                permissions: 0o644,
                create_parents: true,
                expected_size: 0,
                expected_sha256: String::new(),
                ..Default::default()
            },
        )),
    })
    .await
    .unwrap();
    let mut fs = c.filesystem();
    let call = tokio::spawn(async move {
        fs.write_file(tokio_stream::wrappers::ReceiverStream::new(rx))
            .await
            .map(|r| r.into_inner())
    });
    for _ in 0..mib {
        tx.send(pb::WriteFileRequest {
            message: Some(pb::write_file_request::Message::Data(chunk.clone())),
        })
        .await
        .unwrap();
    }
    drop(tx);
    call.await.unwrap().expect("write");
    let write_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let mut stream = c
        .filesystem()
        .read_file(pb::ReadFileRequest {
            path: path.into(),
            offset: 0,
            length: 0,
            chunk_size: 0,
            compute_sha256: false,
            ..Default::default()
        })
        .await
        .expect("read")
        .into_inner();
    let mut got = 0usize;
    while let Some(m) = stream.next().await {
        if let Some(pb::read_file_response::Message::Chunk(d)) = m.expect("chunk").message {
            got += d.data.len();
        }
    }
    let read_s = t.elapsed().as_secs_f64();
    assert_eq!(got, mib << 20);
    serde_json::json!({
        "command_p50_ms": cmd_p50,
        "write_mib_s": mib as f64 / write_s,
        "read_mib_s": mib as f64 / read_s,
    })
}
