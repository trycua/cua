use crate::{
    CreateUserApiKeyRequest, CyclopsClient, HttpHeader, HttpRequest, NewUserApiKey, SdkError,
    UserApiKey, routes,
};
use serde::Deserialize;
use std::sync::Arc;

const JSON_CONTENT_TYPE: &str = "application/json";

#[derive(Deserialize)]
struct ListUserApiKeysResponse {
    keys: Vec<UserApiKey>,
}

#[uniffi::export]
impl CyclopsClient {
    pub async fn list_user_api_keys(self: Arc<Self>) -> Result<Vec<UserApiKey>, SdkError> {
        let response: ListUserApiKeysResponse = self
            .send_json_crud(
                "list user API keys",
                json_request("GET", routes::user_key_collection(self.base_url())?, None),
                &[200],
            )
            .await?;
        Ok(response.keys)
    }

    pub async fn create_user_api_key(
        self: Arc<Self>,
        request: CreateUserApiKeyRequest,
    ) -> Result<NewUserApiKey, SdkError> {
        let body = serde_json::to_vec(&request).map_err(|error| SdkError::Body {
            reason: error.to_string(),
        })?;
        self.send_json_crud(
            "create user API key",
            json_request(
                "POST",
                routes::user_key_collection(self.base_url())?,
                Some(body),
            ),
            &[201],
        )
        .await
    }

    pub async fn delete_user_api_key(self: Arc<Self>, id: String) -> Result<(), SdkError> {
        if id.is_empty() {
            return Err(SdkError::Configuration {
                reason: "user API key id must not be empty".into(),
            });
        }
        self.send_unit_crud(
            "delete user API key",
            json_request("DELETE", routes::user_key_item(self.base_url(), &id)?, None),
            &[204],
        )
        .await
    }
}

fn json_request(method: &str, url: url::Url, body: Option<Vec<u8>>) -> HttpRequest {
    let mut headers = vec![HttpHeader {
        name: "accept".into(),
        value: JSON_CONTENT_TYPE.into(),
    }];
    if body.is_some() {
        headers.push(HttpHeader {
            name: "content-type".into(),
            value: JSON_CONTENT_TYPE.into(),
        });
    }
    HttpRequest {
        method: method.into(),
        url: url.into(),
        headers,
        body,
    }
}
