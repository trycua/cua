// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Drive read and write throughput and latency, per backend.
//!
//! `cargo run --release -p cua-volume --example bench` measures the local
//! backend; with the `s3` feature and `CUA_DRIVE_S3_TEST_ENDPOINT` set
//! (see `tests/run-minio.sh --bench`) it measures MinIO too. Results go to
//! `$CUA_DRIVE_BENCH_OUT` (default `./drive-bench`) as `drive.json` and
//! `drive.md`.

use std::sync::Arc;
use std::time::Instant;

use cua_volume::backend::{Backend, Condition};

struct Row {
    backend: String,
    size: usize,
    n: usize,
    op: &'static str,
    p50_ms: f64,
    p95_ms: f64,
    mb_s: f64,
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i]
}

async fn measure(name: &str, b: &dyn Backend, size: usize, n: usize, out: &mut Vec<Row>) {
    let payload: Vec<u8> = (0..size).map(|i| (i * 31 % 251) as u8).collect();
    let prefix = format!("bench-{:08x}/", rand::random::<u32>());
    let mut w = vec![];
    let t0 = Instant::now();
    for i in 0..n {
        let t = Instant::now();
        b.put(&format!("{prefix}{i}"), payload.clone(), Condition::None)
            .await
            .expect("put");
        w.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let wt = t0.elapsed().as_secs_f64();
    let mut r = vec![];
    let t0 = Instant::now();
    for i in 0..n {
        let t = Instant::now();
        let (bytes, _) = b.get(&format!("{prefix}{i}"), None).await.expect("get");
        assert_eq!(bytes.len(), size);
        r.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    let rt = t0.elapsed().as_secs_f64();
    let mb = (size * n) as f64 / 1_048_576.0;
    for (op, lat, secs) in [("write", &mut w, wt), ("read", &mut r, rt)] {
        out.push(Row {
            backend: name.into(),
            size,
            n,
            op,
            p50_ms: pct(lat, 0.5),
            p95_ms: pct(lat, 0.95),
            mb_s: mb / secs,
        });
    }
    for i in 0..n {
        let _ = b.delete(&format!("{prefix}{i}"), Condition::None).await;
    }
}

fn human(size: usize) -> String {
    if size >= 1 << 20 {
        format!("{} MiB", size >> 20)
    } else {
        format!("{} KiB", size >> 10)
    }
}

#[tokio::main]
async fn main() {
    let mut rows = vec![];
    let dir = tempfile::tempdir().unwrap();
    #[cfg_attr(not(feature = "s3"), allow(unused_mut))]
    let mut backends: Vec<(String, Arc<dyn Backend>)> = vec![(
        "fs".into(),
        Arc::new(cua_volume::fs::FsBackend::new(dir.path())),
    )];
    #[cfg(feature = "s3")]
    if let Ok(endpoint) = std::env::var("CUA_DRIVE_S3_TEST_ENDPOINT") {
        use cua_volume::s3::*;
        let b = S3Backend::new(
            S3Config {
                endpoint: Some(endpoint),
                region: "us-east-1".into(),
                bucket: "cua-volume-bench".into(),
                root: String::new(),
                path_style: true,
            },
            Arc::new(StaticCredentials(S3Credentials {
                access_key_id: std::env::var("CUA_DRIVE_S3_TEST_ACCESS_KEY").unwrap(),
                secret_access_key: std::env::var("CUA_DRIVE_S3_TEST_SECRET_KEY").unwrap(),
                session_token: None,
                expires_ms: None,
            })),
        )
        .unwrap();
        let _ = b
            .client()
            .create_bucket()
            .bucket("cua-volume-bench")
            .send()
            .await;
        b.client()
            .put_bucket_versioning()
            .bucket("cua-volume-bench")
            .versioning_configuration(
                aws_sdk_s3::types::VersioningConfiguration::builder()
                    .status(aws_sdk_s3::types::BucketVersioningStatus::Enabled)
                    .build(),
            )
            .send()
            .await
            .unwrap();
        backends.push(("s3 (MinIO, Docker, loopback)".into(), Arc::new(b)));
    }
    for (name, b) in &backends {
        measure(name, b.as_ref(), 4 << 10, 200, &mut rows).await;
        measure(name, b.as_ref(), 16 << 20, 8, &mut rows).await;
    }
    let out = std::path::PathBuf::from(
        std::env::var("CUA_DRIVE_BENCH_OUT").unwrap_or_else(|_| "drive-bench".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let json: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({"backend": r.backend, "object_bytes": r.size, "objects": r.n,
                "op": r.op, "p50_ms": r.p50_ms, "p95_ms": r.p95_ms, "mb_per_s": r.mb_s})
        })
        .collect();
    std::fs::write(
        out.join("drive.json"),
        serde_json::to_string_pretty(&json).unwrap(),
    )
    .unwrap();
    let mut md = String::from(
        "| Backend | Object | Op | Objects | p50 ms | p95 ms | MB/s |\n|---|---|---|---|---|---|---|\n",
    );
    for r in &rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {:.2} | {:.2} | {:.1} |\n",
            r.backend,
            human(r.size),
            r.op,
            r.n,
            r.p50_ms,
            r.p95_ms,
            r.mb_s
        ));
    }
    std::fs::write(out.join("drive.md"), &md).unwrap();
    print!("{md}");
}
