use crate::{CyclopsClient, HttpHeader, HttpRequest, SdkError, routes};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};

const JSON_CONTENT_TYPE: &str = "application/json";

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ImageUploadFileRequest {
    pub digest: String,
    pub size_bytes: u64,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ImageUploadRequest {
    pub namespace: String,
    pub files: Vec<ImageUploadFileRequest>,
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct PresignedPut {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ImageUploadInstruction {
    pub digest: String,
    pub size_bytes: u64,
    pub reference: String,
    pub upload: Option<PresignedPut>,
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ImageUploadResponse {
    pub files: Vec<ImageUploadInstruction>,
}

#[uniffi::export]
impl CyclopsClient {
    /// Hash and upload one file, or reuse a matching existing object.
    /// Returns only the bound digest, size, and tenant reference, never a signed URL.
    /// This does not create an Image or attest to object versioning/encryption.
    pub async fn upload_image_file(
        self: Arc<Self>,
        namespace: String,
        name: String,
        contents: Vec<u8>,
    ) -> Result<ImageUploadInstruction, SdkError> {
        let checksum = Sha256::digest(&contents);
        let digest = format!("sha256:{checksum:x}");
        let size_bytes = contents.len() as u64;
        let tenant_hash = Sha256::digest(namespace.as_bytes());
        let tenant_hex = format!("{tenant_hash:x}");
        let reference = format!(
            "uploads/tenant-{}/{}",
            &tenant_hex[..32],
            URL_SAFE_NO_PAD.encode(Sha256::digest(digest.as_bytes()))
        );
        let mut response = Arc::clone(&self)
            .presign_image_uploads(ImageUploadRequest {
                namespace,
                files: vec![ImageUploadFileRequest {
                    digest: digest.clone(),
                    size_bytes,
                    name,
                }],
            })
            .await
            .map_err(redact_presign_error)?;
        if response.files.len() != 1 {
            return Err(invalid_instruction());
        }
        let mut instruction = response.files.remove(0);
        if instruction.digest != digest
            || instruction.size_bytes != size_bytes
            || instruction.reference != reference
        {
            return Err(invalid_instruction());
        }
        if let Some(upload) = instruction.upload.take() {
            validate_upload(&upload, size_bytes, &STANDARD.encode(checksum))?;
            self.send_image_upload(HttpRequest {
                method: upload.method,
                url: upload.url,
                headers: upload
                    .headers
                    .into_iter()
                    .map(|(name, value)| HttpHeader { name, value })
                    .collect(),
                body: Some(contents),
                timeout_secs: Some(300),
                max_response_bytes: Some(4096),
            })
            .await?;
        }
        Ok(instruction)
    }

    pub async fn presign_image_uploads(
        self: Arc<Self>,
        request: ImageUploadRequest,
    ) -> Result<ImageUploadResponse, SdkError> {
        let body = serde_json::to_vec(&request).map_err(|error| SdkError::Body {
            reason: error.to_string(),
        })?;
        self.send_json_crud(
            "presign image uploads",
            json_request(
                "POST",
                routes::image_uploads_presign(self.base_url())?,
                Some(body),
            ),
            &[200],
        )
        .await
    }
}

fn json_request(method: &str, url: url::Url, body: Option<Vec<u8>>) -> HttpRequest {
    HttpRequest {
        method: method.into(),
        url: url.into(),
        headers: vec![
            HttpHeader {
                name: "accept".into(),
                value: JSON_CONTENT_TYPE.into(),
            },
            HttpHeader {
                name: "content-type".into(),
                value: JSON_CONTENT_TYPE.into(),
            },
        ],
        body,
        timeout_secs: Some(30),
        max_response_bytes: Some(1024 * 1024),
    }
}

fn invalid_instruction() -> SdkError {
    SdkError::Body {
        reason: "invalid image upload instruction".into(),
    }
}

fn validate_upload(upload: &PresignedPut, size_bytes: u64, checksum: &str) -> Result<(), SdkError> {
    let url = url::Url::parse(&upload.url).map_err(|_| invalid_instruction())?;
    let authority = upload
        .url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
    if authority.is_none_or(|authority| authority.is_empty() || authority.contains('@'))
        || upload
            .url
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte == b'\\')
        || upload.method != "PUT"
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_instruction());
    }
    let mut names = std::collections::HashSet::new();
    for (name, value) in &upload.headers {
        let lower = name.to_ascii_lowercase();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
            || value
                .bytes()
                .any(|byte| (byte < 32 && byte != b'\t') || byte == 127)
            || !names.insert(lower.clone())
            || matches!(
                lower.as_str(),
                "authorization" | "proxy-authorization" | "cookie" | "transfer-encoding"
            )
        {
            return Err(invalid_instruction());
        }
        validate_signed_value(&lower, value, size_bytes, checksum)?;
    }
    for (name, value) in url.query_pairs() {
        validate_signed_value(&name.to_ascii_lowercase(), &value, size_bytes, checksum)?;
    }
    Ok(())
}

fn validate_signed_value(
    name: &str,
    value: &str,
    size_bytes: u64,
    checksum: &str,
) -> Result<(), SdkError> {
    if (name == "content-length"
        && (value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || value.parse::<u64>().ok() != Some(size_bytes)))
        || (name == "x-amz-checksum-sha256" && value != checksum)
    {
        return Err(invalid_instruction());
    }
    Ok(())
}

fn redact_presign_error(mut error: SdkError) -> SdkError {
    match &mut error {
        SdkError::Status {
            operation, body, ..
        } => {
            *operation = "presign image uploads".into();
            body.clear();
        }
        SdkError::Configuration { reason }
        | SdkError::Transport { reason }
        | SdkError::Token { reason }
        | SdkError::Body { reason } => {
            *reason = "image upload presign failed".into();
        }
        SdkError::InvalidResourceName {
            field,
            value,
            reason,
        } => {
            field.clear();
            value.clear();
            *reason = "image upload presign failed".into();
        }
        SdkError::UnknownService {
            requested,
            available,
        } => {
            requested.clear();
            available.clear();
        }
        SdkError::InvalidServicePath { path } => path.clear(),
        SdkError::ClaimFailed { phase, status } => {
            phase.clear();
            status.clear();
        }
        SdkError::PoolAccessDenied {
            operation,
            namespace,
            body,
            ..
        } => {
            *operation = "presign image uploads".into();
            namespace.clear();
            body.clear();
        }
        SdkError::SignedServiceUrlsUnavailable | SdkError::ClaimTimeout => {}
    }
    error
}
