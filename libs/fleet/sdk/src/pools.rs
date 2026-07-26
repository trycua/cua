use crate::{
    CreatePoolRequest, CyclopsClient, HttpHeader, HttpRequest, HttpResponse, Pool,
    ResourceMetadata, SdkError, routes,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::Arc;
use url::Url;

const JSON_CONTENT_TYPE: &str = "application/json";

#[derive(Deserialize)]
struct ResourceList<T> {
    items: Vec<T>,
}

enum NamespaceOwnership {
    Created,
    Existing,
}

#[uniffi::export]
impl CyclopsClient {
    pub async fn create_pool(
        self: Arc<Self>,
        request: CreatePoolRequest,
    ) -> Result<Pool, SdkError> {
        let namespace_collection_url = routes::namespace_collection(self.base_url())?;
        let namespace_item_url = routes::namespace_item(self.base_url(), &request.namespace)?;
        let collection_url = routes::pool_collection(self.base_url(), &request.namespace)?;
        let pool = Pool {
            api_version: "cua.ai/v1".into(),
            kind: "OSGymWorkspacePool".into(),
            metadata: ResourceMetadata {
                namespace: request.namespace.clone(),
                name: request.namespace,
                labels: None,
            },
            spec: request.spec,
            status: None,
        };
        self.ensure_lifecycle_pool_identity(&pool)?;
        routes::pool_item(
            self.base_url(),
            &pool.metadata.namespace,
            &pool.metadata.name,
        )?;

        let _lifecycle_guard = self
            .namespace_lifecycle_guard(&pool.metadata.namespace)
            .await;
        let namespace_created = matches!(
            self.ensure_namespace(&namespace_collection_url, &pool.metadata.namespace)
                .await?,
            NamespaceOwnership::Created
        );
        let result = self
            .send_json(
                "create pool",
                json_request("POST", collection_url, Some(to_json(&pool)?)),
                &[200, 201, 202],
            )
            .await;

        match result {
            Ok(pool) => Ok(pool),
            Err(error) => {
                if namespace_created {
                    let _ = self.delete_namespace(&namespace_item_url).await;
                }
                Err(error)
            }
        }
    }

    pub async fn list_pools(self: Arc<Self>, namespace: String) -> Result<Vec<Pool>, SdkError> {
        let collection_url = routes::pool_collection(self.base_url(), &namespace)?;
        let list: ResourceList<Pool> = self
            .send_json(
                "list pools",
                json_request("GET", collection_url, None),
                &[200],
            )
            .await?;
        Ok(list.items)
    }

    pub async fn get_pool(self: Arc<Self>, pool: Pool) -> Result<Pool, SdkError> {
        let item_url = self.pool_item_url(&pool)?;
        self.send_json("get pool", json_request("GET", item_url, None), &[200])
            .await
    }

    pub async fn update_pool(self: Arc<Self>, pool: Pool) -> Result<Pool, SdkError> {
        let item_url = self.pool_item_url(&pool)?;
        let body = to_json(&pool)?;
        self.send_json(
            "update pool",
            json_request("PUT", item_url, Some(body)),
            &[200],
        )
        .await
    }

    pub async fn delete_pool(self: Arc<Self>, pool: Pool) -> Result<(), SdkError> {
        self.ensure_lifecycle_pool_identity(&pool)?;
        let item_url = self.pool_item_url(&pool)?;
        let namespace_url = routes::namespace_item(self.base_url(), &pool.metadata.namespace)?;
        let _lifecycle_guard = self
            .namespace_lifecycle_guard(&pool.metadata.namespace)
            .await;
        self.send_unit(
            "delete pool",
            json_request("DELETE", item_url, None),
            &[200, 202, 204, 404],
        )
        .await?;
        self.delete_namespace(&namespace_url).await
    }
}

impl CyclopsClient {
    async fn ensure_namespace(
        &self,
        namespace_url: &Url,
        namespace: &str,
    ) -> Result<NamespaceOwnership, SdkError> {
        let response = self
            .send_allowed(
                "create namespace",
                json_request(
                    "POST",
                    namespace_url.clone(),
                    Some(to_json(&serde_json::json!({ "name": namespace }))?),
                ),
                &[201, 409],
            )
            .await?;
        match response.status {
            201 => Ok(NamespaceOwnership::Created),
            409 => Ok(NamespaceOwnership::Existing),
            status => Err(SdkError::status("create namespace", status, &response.body)),
        }
    }

    async fn delete_namespace(&self, namespace_url: &Url) -> Result<(), SdkError> {
        self.send_unit(
            "delete namespace",
            json_request("DELETE", namespace_url.clone(), None),
            &[200, 202, 204, 404],
        )
        .await
    }

    fn ensure_lifecycle_pool_identity(&self, pool: &Pool) -> Result<(), SdkError> {
        if pool.metadata.namespace != pool.metadata.name {
            return Err(SdkError::Configuration {
                reason:
                    "pool metadata namespace and name must match for namespace lifecycle operations"
                        .into(),
            });
        }
        Ok(())
    }

    fn pool_item_url(&self, pool: &Pool) -> Result<Url, SdkError> {
        routes::pool_item(
            self.base_url(),
            &pool.metadata.namespace,
            &pool.metadata.name,
        )
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        operation: &str,
        request: HttpRequest,
        allowed: &[u16],
    ) -> Result<T, SdkError> {
        let response = self.send_allowed(operation, request, allowed).await?;
        serde_json::from_slice(&response.body).map_err(|error| SdkError::Body {
            reason: error.to_string(),
        })
    }

    async fn send_unit(
        &self,
        operation: &str,
        request: HttpRequest,
        allowed: &[u16],
    ) -> Result<(), SdkError> {
        self.send_allowed(operation, request, allowed).await?;
        Ok(())
    }

    async fn send_allowed(
        &self,
        operation: &str,
        request: HttpRequest,
        allowed: &[u16],
    ) -> Result<HttpResponse, SdkError> {
        match self.execute_authenticated(request).await {
            Ok(response) if allowed.contains(&response.status) => Ok(response),
            Ok(response) => Err(SdkError::status(operation, response.status, &response.body)),
            Err(SdkError::Status { status, body, .. }) if allowed.contains(&status) => {
                Ok(HttpResponse {
                    status,
                    headers: Vec::new(),
                    body: body.into_bytes(),
                })
            }
            Err(SdkError::Status { status, body, .. }) => Err(SdkError::Status {
                operation: operation.into(),
                status,
                body,
            }),
            Err(error) => Err(error),
        }
    }
}

fn json_request(method: &str, url: Url, body: Option<Vec<u8>>) -> HttpRequest {
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
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>, SdkError> {
    serde_json::to_vec(value).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })
}
