use crate::{CyclopsClient, HttpHeader, HttpRequest, SdkError, routes};
use serde::{Deserialize, Serialize};
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
        timeout_secs: None,
    }
}
