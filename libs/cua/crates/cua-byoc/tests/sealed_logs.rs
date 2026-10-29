// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A sealed cloud token never reaches the logs: sealed, opened in memory
//! and re-sealed under the pool's next key generation with every log line
//! captured at TRACE. Its own test binary, so the process-wide subscriber
//! sees every line (a scoped one misses lines when tests run in parallel).
#![cfg(feature = "federation")]

mod sealed_support;

use std::sync::{Arc, Mutex};

use cua_byoc::federation::{IssuerClient, NewConnection};
use cua_byoc::model::Auth;
use cua_byoc::sealed::PoolSealer;
use cua_byoc::{Clouds, Connection, Secret, SecretSource, Store};
use sealed_support::{FakeIssuer, FakePool};

const TOKEN: &str = "hcloud-LOGS-token-0123456789";

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn the_token_never_reaches_the_logs() {
    let buf = Buf::default();
    let w = buf.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || w.clone())
            .finish(),
    )
    .unwrap();

    let home = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::new(home.path()));
    let issuer = Arc::new(FakeIssuer::default());
    let pool = FakePool::new();
    let keys = pool.join("laptop");
    let clouds = Clouds::new(home.path(), vec![], store.clone())
        .with_issuer(issuer.clone() as Arc<dyn IssuerClient>)
        .with_sealer(keys.clone() as Arc<dyn PoolSealer>);
    let tokens = clouds.sealed_tokens();

    // Sealed and put on the account's connection.
    let server = issuer
        .create_connection(
            &NewConnection {
                cloud: "hetzner".into(),
                identifiers: [("project".to_string(), "default".to_string())].into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let server = tokens
        .put(&server, &Secret::new(TOKEN), None)
        .await
        .unwrap();
    assert_eq!(server.sealed_token.as_ref().unwrap().generation, 1);
    store
        .connect(Connection {
            provider: "hetzner".into(),
            region: "fsn1".into(),
            auth: Auth::Sealed {
                conn_id: server.id.clone(),
            },
            ..Default::default()
        })
        .unwrap();

    // A device leaves: the next use opens it and re-seals it.
    pool.revoke("desktop");
    let opened = SecretSource::secret(tokens.as_ref(), "hetzner")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opened.expose(), TOKEN);
    assert_eq!(
        issuer
            .only("hetzner")
            .sealed_token
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    tracing::info!(token = ?opened, "a secret logged by mistake is redacted");

    let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("re-sealed the cloud token") && logs.contains("Secret(redacted)"),
        "the log was captured: {logs}"
    );
    assert!(!logs.contains(TOKEN), "{logs}");
    for out in [
        format!("{opened:?}"),
        format!("{tokens:?}"),
        format!("{:?}", issuer.conns.lock().unwrap()),
    ] {
        assert!(!out.contains(TOKEN), "{out}");
    }
}
