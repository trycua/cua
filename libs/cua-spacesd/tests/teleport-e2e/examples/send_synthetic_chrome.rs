// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Teleport a SYNTHETIC Chrome profile into a real, running cua-spacesd (a Lume
//! Space), the way the Cua Spaces app does, for the guest-side manual/E2E
//! proof of `fix(teleport): Chrome Safe Storage without keychain prompts`.
//!
//! Never reads the host's Chrome, profile or keychain: the source is a
//! throwaway directory holding a fake Chrome profile whose cookie store has
//! one plaintext test cookie. The receiver (in the Space) re-encrypts it under
//! the Space's own Safe Storage key, exactly as it does for a Keyvault or live
//! teleport (all three deliver the same bundle to the same receiver).
//!
//! ```text
//! cargo run --example send_synthetic_chrome -- http://192.168.64.206:3211 <token> [host] [name] [value]
//! ```

use std::sync::Arc;

use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};
use cua_teleport::providers::chrome::ChromeProvider;
use cua_teleport::{
    AppRef, ExportRegistry, Platform, Selection, SendOptions, Teleporter, TransferScope,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (url, token) = match (args.first(), args.get(1)) {
        (Some(u), Some(t)) => (u.clone(), t.clone()),
        _ => {
            eprintln!("usage: send_synthetic_chrome <spacesd-url> <token> [host] [name] [value]");
            std::process::exit(2);
        }
    };
    let host_key = args.get(2).cloned().unwrap_or_else(|| "localhost".into());
    let name = args.get(3).cloned().unwrap_or_else(|| "cua_session".into());
    let value = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| "alice-signed-in".into());

    let src = tempfile::tempdir().unwrap();
    let profile = src.path().join(".config/google-chrome/Default");
    std::fs::create_dir_all(profile.join("Network")).unwrap();
    let conn = rusqlite::Connection::open(profile.join("Network/Cookies")).unwrap();
    conn.execute_batch(
        "CREATE TABLE cookies (creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL,
         top_frame_site_key TEXT NOT NULL DEFAULT '', name TEXT NOT NULL,
         value TEXT NOT NULL, encrypted_value BLOB NOT NULL DEFAULT x'',
         path TEXT NOT NULL, expires_utc INTEGER NOT NULL, is_secure INTEGER NOT NULL,
         is_httponly INTEGER NOT NULL, last_access_utc INTEGER NOT NULL DEFAULT 0,
         has_expires INTEGER NOT NULL DEFAULT 1, is_persistent INTEGER NOT NULL DEFAULT 1,
         priority INTEGER NOT NULL DEFAULT 1, samesite INTEGER NOT NULL DEFAULT -1,
         source_scheme INTEGER NOT NULL DEFAULT 0, source_port INTEGER NOT NULL DEFAULT -1,
         UNIQUE (host_key, top_frame_site_key, name, path));",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO cookies (creation_utc, host_key, name, value, path, expires_utc, is_secure, is_httponly)
         VALUES (13300000000000000, ?1, ?2, ?3, '/', 13600000000000000, 0, 0)",
        rusqlite::params![host_key, name, value],
    )
    .unwrap();
    drop(conn);
    std::fs::write(profile.join("Preferences"), b"{}").unwrap();

    let sender_host = Arc::new(cua_teleport::FakeHost::new().with_home(src.path()));
    let mut registry = ExportRegistry::new();
    registry.register(Box::new(
        ChromeProvider::new()
            .without_devtools()
            .with_host(sender_host),
    ));
    let teleporter = Teleporter::with_registry(registry).options(SendOptions::default());

    let transport = if std::env::var_os("GRPC_WEB").is_some() {
        TransportPreference::GrpcWeb
    } else {
        TransportPreference::Native
    };
    let env = SpacesdClient::connect(
        ConnectOptions::parse(&url)
            .unwrap()
            .token(token)
            .transport(transport),
    )
    .await
    .expect("connect to the Space's cua-spacesd");
    let chrome = AppRef {
        app_id: "google-chrome".into(),
        display_name: "Google Chrome".into(),
        platform: Platform::Linux,
    };
    let started = std::time::Instant::now();
    match teleporter
        .send(
            &env,
            &chrome,
            TransferScope::FullProfile,
            Selection::All,
            Arc::new(|_: &cua_teleport::ApprovalRequest<'_>| true),
        )
        .await
    {
        Ok(outcome) => println!("TELEPORT OK in {:?}: {outcome:?}", started.elapsed()),
        Err(e) => {
            println!("TELEPORT FAILED after {:?}: {e}", started.elapsed());
            std::process::exit(1);
        }
    }
}
