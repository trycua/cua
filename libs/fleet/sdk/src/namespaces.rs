use crate::{CyclopsClient, HttpHeader, HttpRequest, Namespace, SdkError, routes};
use serde::Serialize;
use std::sync::Arc;

const JSON_CONTENT_TYPE: &str = "application/json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NamespaceOwnership {
    Created,
    Existing,
}

#[uniffi::export]
impl CyclopsClient {
    pub async fn list_namespaces(self: Arc<Self>) -> Result<Vec<Namespace>, SdkError> {
        self.send_json_crud(
            "list namespaces",
            json_request("GET", routes::namespace_collection(self.base_url())?, None),
            &[200],
        )
        .await
    }

    pub async fn create_namespace(self: Arc<Self>, name: String) -> Result<Namespace, SdkError> {
        routes::namespace_item(self.base_url(), &name)?;
        self.send_json_crud(
            "create namespace",
            json_request(
                "POST",
                routes::namespace_collection(self.base_url())?,
                Some(to_json(&serde_json::json!({ "name": name }))?),
            ),
            &[201],
        )
        .await
    }

    pub async fn get_namespace(self: Arc<Self>, name: String) -> Result<Namespace, SdkError> {
        self.send_json_crud(
            "get namespace",
            json_request("GET", routes::namespace_item(self.base_url(), &name)?, None),
            &[200],
        )
        .await
    }

    pub async fn delete_namespace(self: Arc<Self>, name: String) -> Result<(), SdkError> {
        self.send_unit_crud(
            "delete namespace",
            json_request(
                "DELETE",
                routes::namespace_item(self.base_url(), &name)?,
                None,
            ),
            &[200, 202, 204, 404],
        )
        .await
    }
}

impl CyclopsClient {
    pub(crate) async fn create_namespace_if_missing(
        &self,
        name: &str,
    ) -> Result<NamespaceOwnership, SdkError> {
        let response = self
            .send_allowed(
                "create namespace",
                json_request(
                    "POST",
                    routes::namespace_collection(self.base_url())?,
                    Some(to_json(&serde_json::json!({ "name": name }))?),
                ),
                &[201, 409],
            )
            .await?;
        Ok(if response.status == 201 {
            NamespaceOwnership::Created
        } else {
            NamespaceOwnership::Existing
        })
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

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>, SdkError> {
    serde_json::to_vec(value).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })
}
