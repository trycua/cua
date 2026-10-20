//! Regression test for the compromised eMudhra intermediate CAs (reported 2026-10-17).
//!
//! Eight eMudhra intermediate CA private keys leaked; each exists as a cert
//! issued by "emSign Root TLS/CS CA - G3" and a cross-sign from the Comodo
//! "AAA Certificate Services" root. We do not have the keys, so we cannot mint
//! a leaf they sign. Instead we test at the level the upstream fix works on:
//! rustls-webpki >= 0.103.16 rejects ANY certificate path that contains one of
//! the eight intermediate SPKIs (`Error::CertDistrusted`), and the check runs
//! before issuer lookup and signature checks. So presenting the real
//! intermediate certificates to the verifier must yield `CertDistrusted`.
//! On rustls-webpki <= 0.103.15 the same input yields `UnknownIssuer` or
//! `CaUsedAsEndEntity`, so this test fails there.
//!
//! The root-store test covers webpki-roots >= 1.0.10, which dropped
//! "emSign ECC Root CA - G3".

use rustls_pki_types::{CertificateDer, TrustAnchor, UnixTime};
use serde_json::Value;
use std::time::Duration;
use webpki::{anchor_from_trusted_cert, EndEntityCert, KeyUsage, ALL_VERIFICATION_ALGS};

const INTERMEDIATES: &str = include_str!("fixtures/emudhra/impacted-intermediates.json");
const ROOTS: &str = include_str!("fixtures/emudhra/roots.json");

fn pem_to_der(pem: &str) -> CertificateDer<'static> {
    use rustls_pki_types::pem::PemObject;
    CertificateDer::from_pem_slice(pem.as_bytes()).expect("valid PEM")
}

fn certs(json: &str) -> Vec<(String, CertificateDer<'static>)> {
    let v: Value = serde_json::from_str(json).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                format!("{} <- {}", c["name"].as_str().unwrap(), c["issued_by"].as_str().unwrap_or("")),
                pem_to_der(c["pem"].as_str().unwrap()),
            )
        })
        .collect()
}

/// 2026-10-20, inside the validity window of every fixture certificate.
fn now() -> UnixTime {
    UnixTime::since_unix_epoch(Duration::from_secs(1_792_454_400))
}

fn verify(cert: &CertificateDer<'_>, anchors: &[TrustAnchor<'_>]) -> Result<(), String> {
    let ee = EndEntityCert::try_from(cert).map_err(|e| format!("{e:?}"))?;
    ee.verify_for_usage(ALL_VERIFICATION_ALGS, anchors, &[], now(), KeyUsage::server_auth(), None, None)
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

#[test]
fn fixtures_cover_all_eight_intermediates() {
    let list = certs(INTERMEDIATES);
    // 8 distinct keys, each as the emSign-issued cert and the AAA cross-sign.
    assert_eq!(list.len(), 16);
    let v: Value = serde_json::from_str(INTERMEDIATES).unwrap();
    let mut spkis: Vec<_> = v.as_array().unwrap().iter().map(|c| c["spki_sha256"].as_str().unwrap()).collect();
    spkis.sort();
    spkis.dedup();
    assert_eq!(spkis.len(), 8);
}

#[test]
fn webpki_rejects_every_compromised_intermediate_with_webpki_roots_1x() {
    let anchors: Vec<TrustAnchor<'static>> = webpki_roots::TLS_SERVER_ROOTS.to_vec();
    for (name, der) in certs(INTERMEDIATES) {
        let err = verify(&der, &anchors).expect_err(&name);
        assert_eq!(err, "CertDistrusted", "{name}: expected the SPKI distrust error, got {err} (rustls-webpki < 0.103.16?)");
    }
}

#[test]
fn webpki_rejects_every_compromised_intermediate_with_webpki_roots_026() {
    // tokio-tungstenite still pulls webpki-roots 0.26.x, which keeps the
    // emSign ECC root; the verifier-side SPKI block is what protects that path.
    let anchors: Vec<TrustAnchor<'static>> = webpki_roots_026::TLS_SERVER_ROOTS.to_vec();
    for (name, der) in certs(INTERMEDIATES) {
        let err = verify(&der, &anchors).expect_err(&name);
        assert_eq!(err, "CertDistrusted", "{name}: expected the SPKI distrust error, got {err}");
    }
}

#[test]
fn webpki_roots_1x_dropped_the_emsign_ecc_root() {
    let v: Value = serde_json::from_str(ROOTS).unwrap();
    let ecc = v
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "emSign ECC Root CA - G3")
        .expect("fixture has the emSign ECC root");
    let der = pem_to_der(ecc["pem"].as_str().unwrap());
    let want = anchor_from_trusted_cert(&der).unwrap();
    assert!(
        !webpki_roots::TLS_SERVER_ROOTS
            .iter()
            .any(|a| a.subject_public_key_info.as_ref() == want.subject_public_key_info.as_ref()),
        "webpki-roots still trusts emSign ECC Root CA - G3 (need >= 1.0.10)"
    );
}
