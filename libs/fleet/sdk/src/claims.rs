use crate::{
    Claim, CreateClaimRequest, CyclopsClient, HttpHeader, HttpRequest, HttpResponse, Pool,
    ResourceMetadata, Sandbox, SdkError, Template, routes,
};
use cyclops_sdk_schema::{ClaimSpec, DEFAULT_CLAIM_BIND_DEADLINE_SECONDS};
#[cfg(not(target_arch = "wasm32"))]
use futures_timer::Delay;
#[cfg(target_arch = "wasm32")]
use gloo_timers::future::TimeoutFuture;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
use url::Url;

const JSON_CONTENT_TYPE: &str = "application/json";

#[derive(Deserialize)]
struct ResourceList<T> {
    items: Vec<T>,
}

async fn wait_for_claim_poll_interval(interval_ms: u64) {
    #[cfg(target_arch = "wasm32")]
    TimeoutFuture::new(interval_ms.min(u64::from(u32::MAX)) as u32).await;

    #[cfg(not(target_arch = "wasm32"))]
    Delay::new(Duration::from_millis(interval_ms)).await;
}

#[uniffi::export]
impl CyclopsClient {
    pub async fn create_claim(
        self: Arc<Self>,
        request: CreateClaimRequest,
    ) -> Result<Claim, SdkError> {
        self.ensure_claim_pool_identity(&request.pool)?;
        let pool = request.pool;
        // Default the template ref from the warm pool's own spec — never from
        // a naming convention. A hand-built ref that names a nonexistent
        // template makes the bind queue lookup miss forever and the claim
        // times out with no useful error (the hermes-cua-pool incident).
        let mut spec = request.spec.unwrap_or_else(|| ClaimSpec {
            sandbox_template_ref: pool.spec.sandbox_template_ref.clone(),
            warmpool: None,
            bind_deadline: Some(DEFAULT_CLAIM_BIND_DEADLINE_SECONDS),
            ttl_seconds_after_created: None,
            lifecycle: None,
        });
        if spec.bind_deadline.is_none() {
            spec.bind_deadline = Some(DEFAULT_CLAIM_BIND_DEADLINE_SECONDS);
        }
        if spec.sandbox_template_ref.name.is_empty() {
            return Err(SdkError::Configuration {
                reason: "sandbox template name must not be empty".into(),
            });
        }
        let name = match request.name {
            Some(name) => {
                routes::validate_dns_label_for("claim name", &name)?;
                name
            }
            None => claim_name()?,
        };

        let claim = Claim {
            api_version: "osgym.cua.ai/v1alpha1".into(),
            kind: "OSGymSandboxClaim".into(),
            metadata: ResourceMetadata {
                namespace: pool.metadata.namespace.clone(),
                name,
                labels: None,
                creation_timestamp: None,
            },
            spec,
            status: None,
        };
        let collection_url = routes::claim_collection(self.base_url(), &claim.metadata.namespace)?;
        send_json(
            self.as_ref(),
            "create claim",
            json_request("POST", collection_url, Some(to_json(&claim)?)),
            &[200, 201, 202],
        )
        .await
    }

    pub async fn list_claims(self: Arc<Self>, namespace: String) -> Result<Vec<Claim>, SdkError> {
        let collection_url = routes::claim_collection(self.base_url(), &namespace)?;
        let list: ResourceList<Claim> = send_json(
            self.as_ref(),
            "list claims",
            json_request("GET", collection_url, None),
            &[200],
        )
        .await?;
        Ok(list.items)
    }

    pub async fn get_claim(self: Arc<Self>, claim: Claim) -> Result<Claim, SdkError> {
        let item_url = self.claim_item_url(&claim)?;
        send_json(
            self.as_ref(),
            "get claim",
            json_request("GET", item_url, None),
            &[200],
        )
        .await
    }

    pub async fn delete_claim(self: Arc<Self>, claim: Claim) -> Result<(), SdkError> {
        let item_url = self.claim_item_url(&claim)?;
        send_unit(
            self.as_ref(),
            "delete claim",
            json_request("DELETE", item_url, None),
            &[200, 202, 204, 404],
        )
        .await
    }

    /// Push the claim's `spec.lifecycle.shutdownTime` forward. That absolute
    /// expiry is the only liveness input the pool operator's claim reaper
    /// honors, so a holder that outlives its current lease must renew before
    /// the deadline passes or the bound sandbox is deleted underneath it.
    /// Deliberately narrower than a claim update: nothing else on the claim
    /// can be mutated through the SDK.
    pub async fn renew_claim(
        self: Arc<Self>,
        claim: Claim,
        shutdown_time: String,
    ) -> Result<Claim, SdkError> {
        if shutdown_time.trim().is_empty() {
            return Err(SdkError::Configuration {
                reason: "shutdown time must not be empty".into(),
            });
        }
        let item_url = self.claim_item_url(&claim)?;
        let body = to_json(&serde_json::json!({
            "spec": { "lifecycle": { "shutdownTime": shutdown_time } }
        }))?;
        send_json(
            self.as_ref(),
            "renew claim",
            merge_patch_request(item_url, Some(body)),
            &[200],
        )
        .await
    }

    pub async fn wait_claim(self: Arc<Self>, claim: Claim) -> Result<Sandbox, SdkError> {
        self.claim_item_url(&claim)?;
        for attempt in 0..self.claim_poll_limit() {
            let current = Arc::clone(&self).get_claim(claim.clone()).await?;
            let status = current.status.as_ref();
            let phase = status
                .and_then(|status| status.phase.as_deref())
                .unwrap_or("Pending");
            if phase == "Bound" {
                let sandbox = status
                    .and_then(|status| status.sandbox.as_ref())
                    .and_then(|sandbox| sandbox.name.as_deref())
                    .ok_or_else(|| SdkError::Body {
                        reason: "bound claim omitted status.sandbox.name".into(),
                    })?;
                let services = service_names(&self.template_for_claim(&current).await?);
                return Ok(Sandbox {
                    namespace: current.metadata.namespace,
                    claim: current.metadata.name,
                    name: sandbox.into(),
                    services,
                });
            }
            if is_terminal_claim_phase(phase) {
                return Err(SdkError::ClaimFailed {
                    phase: phase.into(),
                    status: serialized_status(status)?,
                });
            }
            if attempt + 1 < self.claim_poll_limit() {
                wait_for_claim_poll_interval(self.claim_poll_interval_ms()).await;
            }
        }
        Err(SdkError::ClaimTimeout)
    }
}

impl CyclopsClient {
    fn ensure_claim_pool_identity(&self, pool: &Pool) -> Result<(), SdkError> {
        routes::validate_dns_label_for("namespace", &pool.metadata.namespace)?;
        routes::validate_dns_label_for("pool name", &pool.metadata.name)?;
        if pool.metadata.namespace != pool.metadata.name {
            return Err(SdkError::Configuration {
                reason:
                    "pool metadata namespace and name must match for claim lifecycle operations"
                        .into(),
            });
        }
        Ok(())
    }

    fn claim_item_url(&self, claim: &Claim) -> Result<Url, SdkError> {
        routes::claim_item(
            self.base_url(),
            &claim.metadata.namespace,
            &claim.metadata.name,
        )
    }

    async fn template_for_claim(&self, claim: &Claim) -> Result<Template, SdkError> {
        let template = &claim.spec.sandbox_template_ref.name;
        if template.is_empty() {
            return Err(SdkError::Body {
                reason: "claim omitted spec.sandboxTemplateRef.name".into(),
            });
        }
        let item_url = routes::template_item(self.base_url(), &claim.metadata.namespace, template)?;
        send_json(
            self,
            "get claim template",
            json_request("GET", item_url, None),
            &[200],
        )
        .await
    }
}

// Random petnames instead of a sequence: a per-client counter restarts at 1
// for every fresh client, so retries and concurrent leases all proposed
// claim-1 and collided with whatever the previous lease still held.
fn claim_name() -> Result<String, SdkError> {
    petname::petname(2, "-")
        .map(|name| format!("claim-{name}"))
        .ok_or_else(|| SdkError::Configuration {
            reason: "claim name generation returned no words".into(),
        })
}

fn service_names(template: &Template) -> Vec<String> {
    let mut names: Vec<String> = template
        .spec
        .vm_template
        .services
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|service| service.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn is_terminal_claim_phase(phase: &str) -> bool {
    matches!(phase, "Failed" | "Error" | "Expired")
}

fn serialized_status<T: Serialize>(status: Option<&T>) -> Result<String, SdkError> {
    serde_json::to_string(&status).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })
}

fn merge_patch_request(url: Url, body: Option<Vec<u8>>) -> HttpRequest {
    let mut request = json_request("PATCH", url, body);
    request.headers[1].value = "application/merge-patch+json".into();
    request
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
        timeout_secs: None,
    }
}

fn to_json<T: Serialize>(value: &T) -> Result<Vec<u8>, SdkError> {
    serde_json::to_vec(value).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })
}

async fn send_json<T: DeserializeOwned>(
    client: &CyclopsClient,
    operation: &str,
    request: HttpRequest,
    allowed: &[u16],
) -> Result<T, SdkError> {
    let response = send_allowed(client, operation, request, allowed).await?;
    serde_json::from_slice(&response.body).map_err(|error| SdkError::Body {
        reason: error.to_string(),
    })
}

async fn send_unit(
    client: &CyclopsClient,
    operation: &str,
    request: HttpRequest,
    allowed: &[u16],
) -> Result<(), SdkError> {
    send_allowed(client, operation, request, allowed).await?;
    Ok(())
}

async fn send_allowed(
    client: &CyclopsClient,
    operation: &str,
    request: HttpRequest,
    allowed: &[u16],
) -> Result<HttpResponse, SdkError> {
    match client.execute_authenticated(request).await {
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
