//! The session marker: a non-secret `session.json` next to the session that
//! says one is stored, so implicit callers never read the OS credential
//! vault to find out. One test (it sets process environment); the vault is
//! the `test-keychain:` stand-in under a temp dir, never the real one.

use cua_auth::{Credentials, Store, may_have_session, read_session_marker};

fn creds(token: &str) -> Credentials {
    Credentials {
        access_token: token.into(),
        refresh_token: Some(format!("{token}-refresh")),
        expires_at: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
        token_type: "Bearer".into(),
        scope: None,
        id_token: None,
    }
}

fn reads(dir: &std::path::Path) -> u64 {
    std::fs::read_to_string(dir.join("reads"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[test]
fn marker_follows_the_stored_session_without_reading_the_vault() {
    let home = tempfile::tempdir().unwrap();
    let vault = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("CUA_HOME", home.path());
        std::env::set_var(
            "CUA_CREDENTIAL_STORE",
            format!("test-keychain:{}", vault.path().display()),
        );
        std::env::remove_var("CUA_FLEET_SESSION");
    }
    let store = Store::from_env();
    assert_eq!(store.kind(), "keychain");

    // No marker: nothing says a session exists, and the vault is not read.
    assert!(!may_have_session());
    assert_eq!(reads(vault.path()), 0, "the vault was probed");

    // Login stores the session and writes the marker (no token material).
    store.save(&creds("cua-e2e-secret-token")).unwrap();
    let marker_path = home.path().join(cua_auth::SESSION_MARKER);
    let raw = std::fs::read_to_string(&marker_path).unwrap();
    assert!(!raw.contains("cua-e2e-secret-token"), "{raw}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&marker_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(read_session_marker().unwrap().store, "keychain");
    assert!(may_have_session());
    assert_eq!(
        reads(vault.path()),
        0,
        "checking the marker never reads the vault"
    );
    unsafe { std::env::set_var("CUA_FLEET_SESSION", "0") };
    assert!(!may_have_session(), "CUA_FLEET_SESSION=0 says no");
    unsafe { std::env::remove_var("CUA_FLEET_SESSION") };

    // A session stored before markers existed: the next explicit read
    // (cua auth status, a cloud call) writes it.
    std::fs::remove_file(&marker_path).unwrap();
    assert!(!may_have_session());
    assert!(store.load().unwrap().is_some());
    assert_eq!(reads(vault.path()), 1);
    assert!(may_have_session(), "marker restored by the read");

    // Logout removes it.
    assert!(store.clear().unwrap());
    assert!(!marker_path.exists());
    assert!(!may_have_session());

    // A marker whose session another client removed goes on the next read.
    store.save(&creds("t2")).unwrap();
    std::fs::remove_file(vault.path().join("vault.json")).unwrap();
    assert!(store.load().unwrap().is_none());
    assert!(!marker_path.exists());

    // The file store needs no marker to be found.
    let file_home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("CUA_HOME", file_home.path());
        std::env::set_var("CUA_CREDENTIAL_STORE", "file");
    }
    assert!(!may_have_session());
    Store::from_env().save(&creds("t3")).unwrap();
    std::fs::remove_file(file_home.path().join(cua_auth::SESSION_MARKER)).unwrap();
    assert!(may_have_session());
}
