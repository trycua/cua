//! gRPC-Web through a buffering proxy: the Fleet gateway (nginx) delivers a
//! unary response's data frame and trailers frame in ONE chunk, which
//! tonic-web 0.14's client decoder alone mis-reads as "missing grpc-status
//! trailer". This fake proxy answers exactly that way.

use std::time::Duration;

use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serves `n` requests on one listener, each answered with a single HTTP/1.1
/// chunk holding an empty-message data frame plus the trailers frame.
async fn coalescing_server(n: usize) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut paths = Vec::new();
        let (mut sock, _) = listener.accept().await.unwrap();
        for _ in 0..n {
            // Read one request (headers + chunked body ending in 0\r\n\r\n), bounded.
            let mut req = Vec::new();
            let mut b = [0u8; 4096];
            while !req.windows(5).any(|w| w == b"0\r\n\r\n") {
                let k = tokio::time::timeout(Duration::from_secs(10), sock.read(&mut b))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(k > 0 && req.len() < 1 << 20, "request too large or closed");
                req.extend_from_slice(&b[..k]);
            }
            let line = String::from_utf8_lossy(&req)
                .lines()
                .next()
                .unwrap()
                .to_string();
            paths.push(line);
            let mut payload = vec![0u8, 0, 0, 0, 0]; // data frame, empty message
            let trailer = b"grpc-status:0\r\n";
            payload.push(0x80);
            payload.extend_from_slice(&(trailer.len() as u32).to_be_bytes());
            payload.extend_from_slice(trailer);
            let mut resp = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/grpc-web+proto\r\ntransfer-encoding: chunked\r\n\r\n{:X}\r\n",
                payload.len()
            )
            .into_bytes();
            resp.extend_from_slice(&payload);
            resp.extend_from_slice(b"\r\n0\r\n\r\n");
            sock.write_all(&resp).await.unwrap();
        }
        paths
    });
    (format!("http://{addr}/api/svc/ns/sbx-env"), task)
}

#[tokio::test]
async fn unary_call_survives_data_and_trailers_in_one_chunk() {
    let (url, server) = coalescing_server(2).await;
    let options = ConnectOptions::parse(&url)
        .unwrap()
        .transport(TransportPreference::GrpcWeb)
        .probe(false)
        .token("t");
    let client = SpacesdClient::connect(options).await.unwrap();
    for _ in 0..2 {
        client
            .system()
            .health(cua_spacesd_client::pb::HealthRequest {})
            .await
            .expect("health through a coalescing proxy");
    }
    let paths = server.await.unwrap();
    assert!(
        paths
            .iter()
            .all(|p| p.starts_with("POST /api/svc/ns/sbx-env/cua.env.v1.SystemService/Health ")),
        "{paths:?}"
    );
}
