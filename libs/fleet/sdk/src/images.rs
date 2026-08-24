use crate::{CyclopsClient, HttpHeader, HttpRequest, PreservedJson, SdkError, routes};
use serde::Deserialize;
use std::sync::Arc;

const JSON_CONTENT_TYPE: &str = "application/json";

#[derive(Deserialize)]
struct ImageList {
    items: Vec<PreservedJson>,
}

#[uniffi::export]
impl CyclopsClient {
    pub async fn get_image(
        self: Arc<Self>,
        namespace: String,
        name: String,
    ) -> Result<Arc<PreservedJson>, SdkError> {
        let image = self
            .send_json_crud(
                "get image",
                json_request(
                    "GET",
                    routes::image_item(self.base_url(), &namespace, &name)?,
                    None,
                ),
                &[200],
            )
            .await?;
        Ok(Arc::new(image))
    }

    pub async fn create_image(
        self: Arc<Self>,
        namespace: String,
        manifest: Arc<PreservedJson>,
    ) -> Result<Arc<PreservedJson>, SdkError> {
        let collection_url = routes::image_collection(self.base_url(), &namespace)?;
        validate_manifest_namespace(&namespace, &manifest)?;
        let image = self
            .send_json_crud(
                "create image",
                json_request(
                    "POST",
                    collection_url,
                    Some(serde_json::to_vec(manifest.as_ref()).map_err(|error| {
                        SdkError::Body {
                            reason: error.to_string(),
                        }
                    })?),
                ),
                &[201],
            )
            .await?;
        Ok(Arc::new(image))
    }

    pub async fn delete_image(
        self: Arc<Self>,
        namespace: String,
        name: String,
    ) -> Result<(), SdkError> {
        self.send_unit_crud(
            "delete image",
            json_request(
                "DELETE",
                routes::image_item(self.base_url(), &namespace, &name)?,
                None,
            ),
            &[200, 202, 204, 404],
        )
        .await
    }

    pub async fn list_images(
        self: Arc<Self>,
        namespace: String,
    ) -> Result<Vec<Arc<PreservedJson>>, SdkError> {
        let images: ImageList = self
            .send_json_crud(
                "list images",
                json_request(
                    "GET",
                    routes::image_collection(self.base_url(), &namespace)?,
                    None,
                ),
                &[200],
            )
            .await?;
        Ok(images.items.into_iter().map(Arc::new).collect())
    }
}

fn validate_manifest_namespace(namespace: &str, manifest: &PreservedJson) -> Result<(), SdkError> {
    let reason = match manifest.as_value().pointer("/metadata/namespace") {
        Some(serde_json::Value::String(value)) if value == namespace => return Ok(()),
        Some(serde_json::Value::String(_)) => {
            "image metadata.namespace must match the requested namespace"
        }
        Some(_) => "image metadata.namespace must be a string",
        None => "image metadata.namespace is required",
    };
    Err(SdkError::Configuration {
        reason: reason.into(),
    })
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
