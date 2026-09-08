use crate::{
    CreateSignedServiceUrlRequest, HttpHeader, HttpRequest, Sandbox, SdkError, SignedServiceUrl,
    client::CyclopsClient,
    routes::{
        signed_service_url_collection, signed_service_url_item, signed_service_url_list,
        validate_signed_service_url_claim,
    },
    services::resolve_service_name,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use url::Url;

const MIN_EXPIRATION_SECONDS: u32 = 60;
const MAX_EXPIRATION_SECONDS: u32 = 86_400;
const MAX_LABEL_BYTES: usize = 120;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateSignedServiceUrlBody<'a> {
    claim: &'a str,
    sandbox: &'a str,
    service: &'a str,
    logical_service: &'a str,
    label: Option<&'a str>,
    expires_in_seconds: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignedServiceUrlResponse {
    id: String,
    namespace: String,
    claim: String,
    sandbox: String,
    logical_service: String,
    label: Option<String>,
    url: String,
    created_at: String,
    expires_at: String,
    revoked_at: Option<String>,
}

impl From<SignedServiceUrlResponse> for SignedServiceUrl {
    fn from(response: SignedServiceUrlResponse) -> Self {
        Self {
            id: response.id,
            namespace: response.namespace,
            claim: response.claim,
            sandbox: response.sandbox,
            service: response.logical_service,
            label: response.label,
            url: response.url,
            created_at: response.created_at,
            expires_at: response.expires_at,
            revoked_at: response.revoked_at,
        }
    }
}

#[uniffi::export]
impl CyclopsClient {
    pub async fn create_signed_service_url(
        self: Arc<Self>,
        request: CreateSignedServiceUrlRequest,
    ) -> Result<SignedServiceUrl, SdkError> {
        let service_name = resolve_service_name(&request.sandbox, &request.service)?;
        let label = validate_create_request(&request)?;
        validate_signed_service_url_claim(&request.sandbox.claim)?;
        let url = signed_service_url_collection(self.base_url(), &request.sandbox.namespace)?;
        let body = serde_json::to_vec(&CreateSignedServiceUrlBody {
            claim: &request.sandbox.claim,
            sandbox: &request.sandbox.name,
            service: &service_name,
            logical_service: &request.service,
            label: label.as_deref(),
            expires_in_seconds: request.expires_in_seconds,
        })
        .map_err(|error| SdkError::Body {
            reason: error.to_string(),
        })?;

        let response: SignedServiceUrlResponse = self
            .send_json_crud(
                "create signed service URL",
                json_request("POST", url, Some(body)),
                &[201],
            )
            .await
            .map_err(map_signed_service_url_error)?;
        Ok(response.into())
    }

    pub async fn list_signed_service_urls(
        self: Arc<Self>,
        sandbox: Sandbox,
    ) -> Result<Vec<SignedServiceUrl>, SdkError> {
        let url = signed_service_url_list(self.base_url(), &sandbox.namespace, &sandbox.claim)?;
        let responses: Vec<SignedServiceUrlResponse> = self
            .send_json_crud(
                "list signed service URLs",
                json_request("GET", url, None),
                &[200],
            )
            .await
            .map_err(map_signed_service_url_error)?;
        Ok(responses.into_iter().map(Into::into).collect())
    }

    pub async fn revoke_signed_service_url(
        self: Arc<Self>,
        signed_service_url: SignedServiceUrl,
    ) -> Result<(), SdkError> {
        let url = signed_service_url_item(
            self.base_url(),
            &signed_service_url.namespace,
            &signed_service_url.id,
        )?;
        self.send_unit_crud(
            "revoke signed service URL",
            json_request("DELETE", url, None),
            &[204],
        )
        .await
        .map_err(map_signed_service_url_error)
    }
}

fn validate_create_request(
    request: &CreateSignedServiceUrlRequest,
) -> Result<Option<String>, SdkError> {
    if !(MIN_EXPIRATION_SECONDS..=MAX_EXPIRATION_SECONDS).contains(&request.expires_in_seconds) {
        return Err(SdkError::InvalidResourceName {
            field: "expires_in_seconds".into(),
            value: request.expires_in_seconds.to_string(),
            reason: format!(
                "must be between {MIN_EXPIRATION_SECONDS} and {MAX_EXPIRATION_SECONDS} seconds"
            ),
        });
    }

    let Some(label) = request.label.as_deref() else {
        return Ok(None);
    };
    let label = label.trim();
    if label.is_empty() {
        return Ok(None);
    }
    if label.len() > MAX_LABEL_BYTES {
        return Err(SdkError::InvalidResourceName {
            field: "label".into(),
            value: label.into(),
            reason: format!("must be at most {MAX_LABEL_BYTES} UTF-8 bytes"),
        });
    }
    Ok(Some(label.into()))
}

fn json_request(method: &str, url: Url, body: Option<Vec<u8>>) -> HttpRequest {
    HttpRequest {
        method: method.into(),
        url: url.into(),
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
        body,
        timeout_secs: None,
    }
}

fn map_signed_service_url_error(error: SdkError) -> SdkError {
    match error {
        SdkError::Status { status: 503, .. } => SdkError::SignedServiceUrlsUnavailable,
        other => other,
    }
}
