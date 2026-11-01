// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Same bundled + native trust policy as the SDK's reqwest auth/discovery clients.
//!
//! The roots are loaded once per process and shared ([`root_store`],
//! [`client_config`]): reading the native store is expensive on macOS
//! (`SecTrustSettingsCopyCertificates` parses the trust settings of every
//! root), and every relay connection used to read it again, a quarter of a
//! core at idle with the app open (F53). A load older than
//! [`REFRESH_AFTER`] is replaced in the background, so a CA the user adds or
//! removes applies within minutes; a change of `SSL_CERT_FILE` or
//! `SSL_CERT_DIR` applies to the next connection.
use crate::{Error, Result};
use rustls::{ClientConfig, RootCertStore};
use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// How long one load of the roots is used before it is read again.
pub(crate) const REFRESH_AFTER: Duration = Duration::from_secs(10 * 60);

/// What selects the native input (rustls-native-certs reads these).
type NativeInput = (Option<OsString>, Option<OsString>);

fn native_input() -> NativeInput {
    (
        std::env::var_os("SSL_CERT_FILE"),
        std::env::var_os("SSL_CERT_DIR"),
    )
}

struct Loaded {
    at: Instant,
    input: NativeInput,
    roots: Arc<RootCertStore>,
    config: Arc<ClientConfig>,
}

static LOADED: RwLock<Option<Loaded>> = RwLock::new(None);
static REFRESHING: AtomicBool = AtomicBool::new(false);
/// Loads of the native store so far (the regression guard counts them).
static NATIVE_LOADS: AtomicUsize = AtomicUsize::new(0);

/// How many times this process read the native root store.
#[cfg(test)]
pub(crate) fn native_loads() -> usize {
    NATIVE_LOADS.load(Ordering::Relaxed)
}

fn load_roots() -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // SSL_CERT_FILE / SSL_CERT_DIR, when present, select the native input.
    // Match reqwest: tolerate unavailable/empty native stores and partial parse
    // failures, but fail when every returned native certificate is invalid DER.
    // Never include native loader errors (which may contain private paths).
    NATIVE_LOADS.fetch_add(1, Ordering::Relaxed);
    let native = rustls_native_certs::load_native_certs();
    let (valid, invalid) = roots.add_parsable_certificates(native.certs);
    if valid == 0 && invalid > 0 {
        return Err(Error::Transport(
            "zero valid certificates found in native root store".into(),
        ));
    }
    Ok(roots)
}

fn load(input: NativeInput) -> Result<Loaded> {
    let roots = Arc::new(load_roots()?);
    let config = Arc::new(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Transport(e.to_string()))?
            .with_root_certificates(roots.clone())
            .with_no_client_auth(),
    );
    Ok(Loaded {
        at: Instant::now(),
        input,
        roots,
        config,
    })
}

/// Reads the roots again on a thread of its own (one at a time); the
/// current ones serve meanwhile, and stay when the new read fails.
fn refresh_in_background() {
    if REFRESHING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("cua-tls-roots".into())
        .spawn(|| {
            let input = native_input();
            let next = load(input);
            let mut slot = LOADED.write().unwrap_or_else(|e| e.into_inner());
            match next {
                Ok(l) => *slot = Some(l),
                Err(e) => {
                    tracing::warn!(error = %e, "TLS roots not reloaded; keeping the current ones");
                    if let Some(l) = slot.as_mut() {
                        l.at = Instant::now();
                    }
                }
            }
            REFRESHING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        REFRESHING.store(false, Ordering::Release);
    }
}

fn current() -> Result<(Arc<RootCertStore>, Arc<ClientConfig>)> {
    let input = native_input();
    {
        let slot = LOADED.read().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = slot.as_ref()
            && l.input == input
        {
            if l.at.elapsed() >= REFRESH_AFTER {
                refresh_in_background();
            }
            return Ok((l.roots.clone(), l.config.clone()));
        }
    }
    // First use, or the native input changed: read it now, so this
    // connection already uses it.
    let l = load(input)?;
    let out = (l.roots.clone(), l.config.clone());
    *LOADED.write().unwrap_or_else(|e| e.into_inner()) = Some(l);
    Ok(out)
}

/// The bundled roots plus the native ones (shared; see the module docs).
pub(crate) fn root_store() -> Result<RootCertStore> {
    Ok(current()?.0.as_ref().clone())
}

/// A client config over [`root_store`] (shared; cheap to clone).
pub(crate) fn client_config() -> Result<ClientConfig> {
    Ok(current()?.1.as_ref().clone())
}

#[cfg(test)]
mod tests {
    /// Runs `test` in a child process of this test binary (the cache and
    /// its counter are process-wide; other tests share this process).
    fn in_child(test: &str, env: &[(&str, &std::ffi::OsStr)]) {
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args(["--exact", test, "--test-threads", "1"])
            .env("CUA_TEST_TLS_CHILD", "1")
            .env_remove("SSL_CERT_DIR")
            .env_remove("SSL_CERT_FILE");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let output = cmd.output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn child() -> bool {
        std::env::var_os("CUA_TEST_TLS_CHILD").is_some()
    }

    /// F53: one read of the native store serves every connection.
    #[test]
    fn many_connections_read_the_native_store_once() {
        if !child() {
            return in_child(
                "tls::tests::many_connections_read_the_native_store_once",
                &[],
            );
        }
        let before = super::native_loads();
        for _ in 0..200 {
            super::client_config().unwrap();
            super::root_store().unwrap();
        }
        assert_eq!(super::native_loads() - before, 1);
    }

    /// A changed `SSL_CERT_FILE` applies to the next connection, once.
    #[test]
    fn a_changed_native_input_is_read_again_once() {
        if !child() {
            return in_child("tls::tests::a_changed_native_input_is_read_again_once", &[]);
        }
        let before = super::native_loads();
        super::client_config().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "").unwrap();
        // SAFETY: this child runs this one test on one thread.
        unsafe { std::env::set_var("SSL_CERT_FILE", &empty) };
        for _ in 0..20 {
            let roots = super::root_store().unwrap();
            assert_eq!(roots.roots, webpki_roots::TLS_SERVER_ROOTS);
        }
        assert_eq!(super::native_loads() - before, 2);
    }

    /// The compromised eMudhra intermediates (CUA-1948) are still refused
    /// with the shared roots: the distrust is the verifier's, whatever
    /// anchors it is given.
    #[test]
    fn the_shared_roots_keep_the_emudhra_distrust() {
        use rustls_pki_types::pem::PemObject;
        use rustls_pki_types::{CertificateDer, UnixTime};
        let json: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/emudhra/impacted-intermediates.json"
        ))
        .unwrap();
        let roots = super::root_store().unwrap();
        // Twice: the second is the cached path.
        let roots_again = super::root_store().unwrap();
        assert_eq!(roots.roots, roots_again.roots);
        let now = UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_792_454_400));
        let certs = json.as_array().unwrap();
        assert_eq!(certs.len(), 16);
        for c in certs {
            let der =
                CertificateDer::from_pem_slice(c["pem"].as_str().unwrap().as_bytes()).unwrap();
            let ee = webpki::EndEntityCert::try_from(&der).unwrap();
            let err = ee
                .verify_for_usage(
                    webpki::ALL_VERIFICATION_ALGS,
                    &roots_again.roots,
                    &[],
                    now,
                    webpki::KeyUsage::server_auth(),
                    None,
                    None,
                )
                .map(|_| ())
                .expect_err(c["name"].as_str().unwrap());
            assert_eq!(format!("{err:?}"), "CertDistrusted", "{}", c["name"]);
        }
    }

    #[test]
    fn empty_native_source_retains_bundled_roots() {
        const CHILD: &str = "CUA_TEST_BUNDLED_ROOTS_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let roots = super::root_store().unwrap();
            assert_eq!(roots.roots, webpki_roots::TLS_SERVER_ROOTS);
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "").unwrap();
        for path in [empty, dir.path().join("missing.pem")] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tls::tests::empty_native_source_retains_bundled_roots",
                ])
                .env(CHILD, "1")
                .env("SSL_CERT_FILE", path)
                .env_remove("SSL_CERT_DIR")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
