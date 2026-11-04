//! The gateway's local TLS: a CA made for this machine and a wildcard leaf
//! for `*.<domain>`, so `https://<port>-<id>.<domain>` (what E2B's
//! `get_host` gives callers) verifies.
//!
//! - The CA is name-constrained to the gateway domain: it cannot vouch for
//!   any other name, even if its key leaked.
//! - Keys are written 0600, certificates 0644, under `dir`.
//! - `bundle.pem` is the OS roots plus our CA, for `SSL_CERT_FILE`: callers
//!   keep trusting public sites (an RL trainer, model APIs) while trusting ours.

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose,
    GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use time::{Duration, OffsetDateTime};

/// The CA's common name (also how its old copies are dropped from bundles).
pub const CA_NAME: &str = "Cua compat local CA";

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("tls: {0}")]
    Io(#[from] std::io::Error),
    #[error("tls: certificate: {0}")]
    Cert(#[from] rcgen::Error),
    #[error("tls: {0}")]
    Rustls(#[from] rustls::Error),
    #[error("tls: {0}")]
    Other(String),
}

/// The files one gateway domain uses.
#[derive(Debug, Clone)]
pub struct TlsFiles {
    pub dir: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Meta {
    domain: String,
    not_after_unix: i64,
}

impl TlsFiles {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
    pub fn ca_cert(&self) -> PathBuf {
        self.dir.join("ca.pem")
    }
    pub fn ca_key(&self) -> PathBuf {
        self.dir.join("ca.key")
    }
    pub fn leaf_cert(&self) -> PathBuf {
        self.dir.join("leaf.pem")
    }
    pub fn leaf_key(&self) -> PathBuf {
        self.dir.join("leaf.key")
    }
    /// OS roots + our CA: point `SSL_CERT_FILE` here.
    pub fn bundle(&self) -> PathBuf {
        self.dir.join("bundle.pem")
    }
    fn meta(&self) -> PathBuf {
        self.dir.join("meta.json")
    }
}

/// The host part of a domain that may carry a port (`e2b.localhost:49443`).
pub fn domain_host(domain: &str) -> &str {
    domain.rsplit_once(':').map_or(domain, |(h, p)| {
        if p.chars().all(|c| c.is_ascii_digit()) {
            h
        } else {
            domain
        }
    })
}

/// Makes sure a CA and a leaf for `*.<domain>` exist and are current (made
/// for this domain, more than 30 days from expiry), creating them when
/// not; rewrites the bundle. Returns the rustls server config.
pub fn ensure(files: &TlsFiles, domain: &str) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let host = domain_host(domain).to_ascii_lowercase();
    std::fs::create_dir_all(&files.dir)?;
    restrict_dir(&files.dir)?;
    let current = std::fs::read(files.meta())
        .ok()
        .and_then(|b| serde_json::from_slice::<Meta>(&b).ok())
        .is_some_and(|m| {
            m.domain == host
                && m.not_after_unix
                    > (OffsetDateTime::now_utc() + Duration::days(30)).unix_timestamp()
        })
        && [
            files.ca_cert(),
            files.ca_key(),
            files.leaf_cert(),
            files.leaf_key(),
        ]
        .iter()
        .all(|p| p.exists());
    if !current {
        generate(files, &host)?;
    }
    write_bundle(files)?;
    server_config(files)
}

fn generate(files: &TlsFiles, host: &str) -> Result<(), TlsError> {
    let now = OffsetDateTime::now_utc();
    let ca_key = KeyPair::generate()?;
    let mut ca = CertificateParams::new(Vec::<String>::new())?;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, CA_NAME);
    dn.push(DnType::OrganizationName, "Cua");
    ca.distinguished_name = dn;
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca.name_constraints = Some(NameConstraints {
        permitted_subtrees: vec![GeneralSubtree::DnsName(host.to_string())],
        excluded_subtrees: vec![],
    });
    ca.not_before = now - Duration::days(1);
    ca.not_after = now + Duration::days(825);
    let ca_cert = ca.self_signed(&ca_key)?;

    let leaf_key = KeyPair::generate()?;
    let mut leaf = CertificateParams::new(vec![host.to_string(), format!("*.{host}")])?;
    // Python 3.13+ verifies strictly (VERIFY_X509_STRICT): the leaf needs
    // an authority key identifier.
    leaf.use_authority_key_identifier_extension = true;
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, host);
    leaf.distinguished_name = dn;
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf.not_before = now - Duration::days(1);
    leaf.not_after = now + Duration::days(397);
    let issuer = Issuer::new(ca, &ca_key);
    let leaf_cert = leaf.signed_by(&leaf_key, &issuer)?;

    write_file(&files.ca_key(), ca_key.serialize_pem().as_bytes(), 0o600)?;
    write_file(&files.ca_cert(), ca_cert.pem().as_bytes(), 0o644)?;
    write_file(
        &files.leaf_key(),
        leaf_key.serialize_pem().as_bytes(),
        0o600,
    )?;
    write_file(&files.leaf_cert(), leaf_cert.pem().as_bytes(), 0o644)?;
    let meta = Meta {
        domain: host.to_string(),
        not_after_unix: (now + Duration::days(397)).unix_timestamp(),
    };
    write_file(
        &files.meta(),
        &serde_json::to_vec(&meta).expect("meta"),
        0o644,
    )?;
    Ok(())
}

/// Writes `bundle.pem`: the OS trust roots (without any earlier copy of our
/// CA) followed by our CA.
pub fn write_bundle(files: &TlsFiles) -> Result<(), TlsError> {
    let ca_pem = std::fs::read_to_string(files.ca_cert())?;
    let roots = rustls_native_certs::load_native_certs();
    let mut out = String::new();
    for cert in roots.certs {
        if contains(cert.as_ref(), CA_NAME.as_bytes()) {
            continue;
        }
        out.push_str(&pem_encode("CERTIFICATE", cert.as_ref()));
    }
    out.push_str(&ca_pem);
    write_file(&files.bundle(), out.as_bytes(), 0o644)?;
    Ok(())
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn pem_encode(label: &str, der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut s = format!("-----BEGIN {label}-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        s.push_str(std::str::from_utf8(chunk).expect("ascii"));
        s.push('\n');
    }
    s.push_str(&format!("-----END {label}-----\n"));
    s
}

/// The rustls config serving the leaf, ALPN `http/1.1` (the router pipes
/// bytes to a guest server whose protocol it does not know).
pub fn server_config(files: &TlsFiles) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(files.leaf_cert())
        .map_err(|e| TlsError::Other(e.to_string()))?
        .collect::<Result<_, _>>()
        .map_err(|e| TlsError::Other(e.to_string()))?;
    let key = PrivateKeyDer::from_pem_file(files.leaf_key())
        .map_err(|e| TlsError::Other(e.to_string()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

pub(crate) fn write_file(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(mode);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    std::fs::rename(&tmp, path)
}

pub(crate) fn restrict_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_host_strips_port() {
        assert_eq!(domain_host("e2b.localhost:49443"), "e2b.localhost");
        assert_eq!(domain_host("e2b.localhost"), "e2b.localhost");
    }

    #[test]
    fn generates_restricted_keys_and_a_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let files = TlsFiles::new(dir.path().join("tls"));
        let cfg = ensure(&files, "e2b.localhost:49443").unwrap();
        assert_eq!(cfg.alpn_protocols, vec![b"http/1.1".to_vec()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for k in [files.ca_key(), files.leaf_key()] {
                let mode = std::fs::metadata(&k).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{}", k.display());
            }
        }
        let bundle = std::fs::read_to_string(files.bundle()).unwrap();
        let ca = std::fs::read_to_string(files.ca_cert()).unwrap();
        assert!(bundle.ends_with(&ca));
        // A second ensure keeps the same CA.
        ensure(&files, "e2b.localhost:49443").unwrap();
        assert_eq!(std::fs::read_to_string(files.ca_cert()).unwrap(), ca);
        // A different domain gets a new CA.
        ensure(&files, "other.localhost").unwrap();
        assert_ne!(std::fs::read_to_string(files.ca_cert()).unwrap(), ca);
    }

    #[test]
    fn leaf_verifies_for_wildcard_hosts_only() {
        use rustls::client::danger::ServerCertVerifier;
        let dir = tempfile::tempdir().unwrap();
        let files = TlsFiles::new(dir.path());
        ensure(&files, "e2b.localhost").unwrap();
        let ca = CertificateDer::from_pem_file(files.ca_cert()).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier =
            rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .unwrap();
        let leaf: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(files.leaf_cert())
            .unwrap()
            .map(|c| c.unwrap())
            .collect();
        let now = rustls::pki_types::UnixTime::now();
        let ok = |name: &str| {
            verifier
                .verify_server_cert(
                    &leaf[0],
                    &[],
                    &rustls::pki_types::ServerName::try_from(name.to_string()).unwrap(),
                    &[],
                    now,
                )
                .is_ok()
        };
        assert!(ok("8080-abc123.e2b.localhost"));
        assert!(!ok("example.com"));
        assert!(!ok("a.b.e2b.localhost"));
    }
}
