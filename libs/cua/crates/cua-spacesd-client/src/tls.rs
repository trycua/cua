// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Same bundled + native trust policy as the SDK's reqwest auth/discovery clients.
use crate::{Error, Result};
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;

pub(crate) fn root_store() -> Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // SSL_CERT_FILE / SSL_CERT_DIR, when present, select the native input.
    // Match reqwest: tolerate unavailable/empty native stores and partial parse
    // failures, but fail when every returned native certificate is invalid DER.
    // Never include native loader errors (which may contain private paths).
    let native = rustls_native_certs::load_native_certs();
    let (valid, invalid) = roots.add_parsable_certificates(native.certs);
    if valid == 0 && invalid > 0 {
        return Err(Error::Transport(
            "zero valid certificates found in native root store".into(),
        ));
    }
    Ok(roots)
}

pub(crate) fn client_config() -> Result<ClientConfig> {
    Ok(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| Error::Transport(e.to_string()))?
            .with_root_certificates(root_store()?)
            .with_no_client_auth(),
    )
}

#[cfg(test)]
mod tests {
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
