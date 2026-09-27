use crate::{
    HttpHeader, HttpRequest, HttpResponse, Sandbox, SdkError, ServiceStreamTarget,
    client::CyclopsClient,
    routes::{service_url, service_websocket_url, validate_dns_label_for},
};
use std::{collections::HashSet, sync::Arc};

const AUTH_HEADER_NAME: &str = "authorization";

#[uniffi::export]
impl CyclopsClient {
    pub async fn service_request(
        self: Arc<Self>,
        sandbox: Sandbox,
        service: String,
        path: String,
        request: HttpRequest,
    ) -> Result<HttpResponse, SdkError> {
        let service_name = resolve_service_name(&sandbox, &service)?;
        let url = service_url(self.base_url(), &sandbox.namespace, &service_name, &path)?;
        let request = HttpRequest {
            method: request.method,
            url: url.to_string(),
            headers: fleet_headers(filtered_headers(request.headers), &sandbox.claim),
            body: request.body,
            timeout_secs: request.timeout_secs,
            max_response_bytes: request.max_response_bytes,
        };
        self.execute_authenticated_service(request).await
    }

    /// Where a native client opens its own WebSocket to a sandbox service:
    /// the gateway's `/api/svc` proxy forwards the HTTP upgrade, so the
    /// returned `ws(s)://` URL plus the returned bearer header are all a
    /// Rust or Swift caller needs to dial the socket directly.
    /// `service_request` stays the path for unary requests.
    pub async fn service_websocket_url(
        self: Arc<Self>,
        sandbox: Sandbox,
        service: String,
        path: String,
    ) -> Result<ServiceStreamTarget, SdkError> {
        let service_name = resolve_service_name(&sandbox, &service)?;
        let url = service_websocket_url(self.base_url(), &sandbox.namespace, &service_name, &path)?;
        let token = self.bearer_token(false).await?;
        Ok(ServiceStreamTarget {
            url: url.into(),
            auth_header_name: AUTH_HEADER_NAME.into(),
            auth_header_value: format!("Bearer {token}"),
        })
    }
}

fn fleet_headers(mut headers: Vec<HttpHeader>, claim: &str) -> Vec<HttpHeader> {
    // Correlation is the exact claim returned in Sandbox; it is never inferred.
    if !claim.is_empty()
        && claim.len() <= 128
        && claim
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~-".contains(&b))
    {
        headers.push(HttpHeader {
            name: "X-Cua-Fleet-Claim".into(),
            value: claim.into(),
        });
    }
    headers
}

pub(crate) fn resolve_service_name(sandbox: &Sandbox, service: &str) -> Result<String, SdkError> {
    if !sandbox.services.iter().any(|known| known == service) {
        let mut available = sandbox.services.clone();
        available.sort_unstable();
        available.dedup();
        return Err(SdkError::UnknownService {
            requested: service.into(),
            available,
        });
    }

    validate_dns_label_for("sandbox", &sandbox.name)?;
    validate_dns_label_for("service", service)?;

    let service_name = format!("{}-{service}", sandbox.name);
    validate_dns_label_for("service", &service_name)?;
    Ok(service_name)
}

fn filtered_headers(headers: Vec<HttpHeader>) -> Vec<HttpHeader> {
    let mut blocked: HashSet<String> = [
        "connection",
        "host",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "authorization",
        "x-cua-fleet-claim",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();

    for header in &headers {
        if header.name.eq_ignore_ascii_case("connection") {
            for token in header.value.split(',') {
                let token = token.trim();
                if !token.is_empty() {
                    blocked.insert(token.to_ascii_lowercase());
                }
            }
        }
    }

    headers
        .into_iter()
        .filter(|header| !blocked.contains(&header.name.to_ascii_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::filtered_headers;
    use crate::{
        CyclopsClient, CyclopsTokenProviderConfiguration, HttpClient, HttpError, HttpHeader,
        HttpRequest, HttpResponse, Sandbox, SdkError,
    };
    use std::sync::Arc;

    struct NoNetworkHttpClient;

    #[async_trait::async_trait]
    impl HttpClient for NoNetworkHttpClient {
        async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
            Err(HttpError::Transport {
                reason: format!("unexpected network request to {}", request.url),
            })
        }
    }

    fn client() -> Arc<CyclopsClient> {
        CyclopsClient::connect_with_access_token(
            CyclopsTokenProviderConfiguration {
                base_url: "https://cyclops.example".into(),
                pool_poll_interval_ms: 1,
                pool_poll_limit: 1,
                claim_poll_interval_ms: 1,
                claim_poll_limit: 1,
            },
            "test-token".into(),
            Arc::new(NoNetworkHttpClient),
        )
        .unwrap()
    }

    fn sandbox() -> Sandbox {
        Sandbox {
            namespace: "example-pool".into(),
            claim: "claim-example".into(),
            name: "sandbox-1".into(),
            services: vec!["vnc".into(), "rcdp".into()],
        }
    }

    #[tokio::test]
    async fn websocket_target_carries_wss_url_and_bearer_header() {
        let target = client()
            .service_websocket_url(sandbox(), "vnc".into(), "/websockify".into())
            .await
            .unwrap();

        assert_eq!(
            target.url,
            "wss://cyclops.example/api/svc/example-pool/sandbox-1-vnc/websockify"
        );
        assert_eq!(target.auth_header_name, "authorization");
        assert_eq!(target.auth_header_value, "Bearer test-token");
    }

    #[tokio::test]
    async fn websocket_target_rejects_unknown_services() {
        let error = client()
            .service_websocket_url(sandbox(), "missing".into(), "/".into())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            SdkError::UnknownService { requested, available }
                if requested == "missing" && available == ["rcdp", "vnc"]
        ));
    }

    #[tokio::test]
    async fn websocket_target_rejects_traversal_paths() {
        let error = client()
            .service_websocket_url(sandbox(), "vnc".into(), "/../admin".into())
            .await
            .unwrap_err();

        assert!(matches!(error, SdkError::InvalidServicePath { .. }));
    }

    #[tokio::test]
    async fn access_token_returns_static_bearer_even_when_forced() {
        assert_eq!(client().access_token(false).await.unwrap(), "test-token");
        assert_eq!(client().access_token(true).await.unwrap(), "test-token");
    }

    #[test]
    fn removes_connection_nominated_headers_case_insensitively() {
        let headers = filtered_headers(vec![
            HttpHeader {
                name: "Connection".into(),
                value: "X-Remove".into(),
            },
            HttpHeader {
                name: "x-remove".into(),
                value: "no".into(),
            },
            HttpHeader {
                name: "x-keep".into(),
                value: "yes".into(),
            },
        ]);

        assert_eq!(
            headers,
            vec![HttpHeader {
                name: "x-keep".into(),
                value: "yes".into(),
            }]
        );
    }
}

#[cfg(test)]
mod service_resolution_tests {
    use super::resolve_service_name;
    use crate::{Sandbox, SdkError};

    fn sandbox(name: &str, services: &[&str]) -> Sandbox {
        Sandbox {
            namespace: "tenant-a".into(),
            claim: "claim-a".into(),
            name: name.into(),
            services: services.iter().map(|service| (*service).into()).collect(),
        }
    }

    #[test]
    fn resolves_logical_service_to_kubernetes_service_name() {
        assert_eq!(
            resolve_service_name(&sandbox("sandbox-a", &["mcp"]), "mcp").unwrap(),
            "sandbox-a-mcp"
        );
    }

    #[test]
    fn unknown_service_lists_sorted_deduplicated_services() {
        let error = resolve_service_name(&sandbox("sandbox-a", &["vnc", "mcp", "vnc"]), "browser")
            .unwrap_err();

        assert!(matches!(
            error,
            SdkError::UnknownService { requested, available }
                if requested == "browser" && available == vec!["mcp", "vnc"]
        ));
    }

    #[test]
    fn rejects_invalid_derived_kubernetes_service_name() {
        let error = resolve_service_name(&sandbox(&"a".repeat(61), &["mcp"]), "mcp").unwrap_err();

        assert!(matches!(error, SdkError::InvalidResourceName { .. }));
    }
}
