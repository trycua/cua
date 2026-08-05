use crate::{CyclopsClient, HttpHeader, HttpRequest, Namespace, SdkError, routes};
use std::sync::Arc;

const JSON_CONTENT_TYPE: &str = "application/json";

#[uniffi::export]
impl CyclopsClient {
    pub async fn list_namespaces(self: Arc<Self>) -> Result<Vec<Namespace>, SdkError> {
        self.send_json_crud(
            "list namespaces",
            json_request("GET", routes::namespace_collection(self.base_url())?),
            &[200],
        )
        .await
    }
}

fn json_request(method: &str, url: url::Url) -> HttpRequest {
    HttpRequest {
        method: method.into(),
        url: url.into(),
        headers: vec![HttpHeader {
            name: "accept".into(),
            value: JSON_CONTENT_TYPE.into(),
        }],
        body: None,
    }
}
