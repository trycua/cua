//! Errors for image operations.

pub type Result<T, E = ImageError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error("invalid image reference '{0}': {1}")]
    Reference(String, String),

    #[error("registry: {0}")]
    Registry(String),

    /// The registry has no such repository, tag or digest.
    #[error("registry: not found: {0}")]
    NotFound(String),

    /// The registry refused the credentials in use (or anonymous access).
    #[error("registry: Not authorized: {0}")]
    Unauthorized(String),

    /// The image has no variant the requested backend can run.
    #[error("{reference}: {reason}")]
    UnsupportedVariant {
        reference: String,
        /// Variants the reference offers (`rootfs`, `containerdisk`, ...), and
        /// `macos` for a darwin image in no Lume format.
        found: Vec<String>,
        /// The backend that asked.
        backend: String,
        reason: String,
    },

    /// A catalog image (`libs/images/sandbox-images.json`) that is not
    /// published yet: a constructor (`Image.omarchy()`, a tier) refuses it
    /// rather than fail later at pull time. An explicit reference still works.
    #[error(
        "{reference} is not published yet; to use it anyway, pass the reference explicitly (Image.from_registry(\"{reference}\") or `cua sb create {reference}`)"
    )]
    NotPublished { reference: String },

    #[error("no {wanted} manifest in image index (available: {available})")]
    PlatformNotFound { wanted: String, available: String },

    #[error("image {reference} is not a {expected}: {detail}")]
    WrongFormat {
        reference: String,
        expected: &'static str,
        detail: String,
    },

    #[error("digest mismatch for {what}: expected {expected}, got {actual}")]
    Digest {
        what: String,
        expected: String,
        actual: String,
    },

    #[error("invalid image spec: {0}")]
    Spec(String),

    #[error("build failed at {step}: {detail}")]
    Build { step: String, detail: String },

    #[error(transparent)]
    Vmm(#[from] cua_vmm::VmmError),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<oci_client::errors::OciDistributionError> for ImageError {
    fn from(e: oci_client::errors::OciDistributionError) -> Self {
        use oci_client::errors::{OciDistributionError as E, OciErrorCode as C};
        let msg = e.to_string();
        match &e {
            E::ImageManifestNotFoundError(_) => ImageError::NotFound(msg),
            E::UnauthorizedError { .. } | E::AuthenticationFailure(_) => {
                ImageError::Unauthorized(msg)
            }
            E::ServerError { code: 404, .. } => ImageError::NotFound(msg),
            E::ServerError {
                code: 401 | 403, ..
            } => ImageError::Unauthorized(msg),
            E::RegistryError { envelope, .. } => {
                let codes: Vec<&C> = envelope.errors.iter().map(|x| &x.code).collect();
                if codes
                    .iter()
                    .any(|c| matches!(c, C::ManifestUnknown | C::NameUnknown | C::NotFound))
                {
                    ImageError::NotFound(msg)
                } else if codes
                    .iter()
                    .any(|c| matches!(c, C::Unauthorized | C::Denied))
                {
                    ImageError::Unauthorized(msg)
                } else {
                    ImageError::Registry(msg)
                }
            }
            _ => ImageError::Registry(msg),
        }
    }
}

impl From<ImageError> for cua_vmm::VmmError {
    fn from(e: ImageError) -> Self {
        match e {
            ImageError::Vmm(v) => v,
            other => cua_vmm::VmmError::Other(other.to_string()),
        }
    }
}
