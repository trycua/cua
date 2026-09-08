use crate::{
    HttpHeader, HttpRequest, HttpResponse, Sandbox, SdkError,
    client::CyclopsClient,
    routes::{service_url, validate_dns_label_for},
};
use std::{collections::HashSet, sync::Arc};

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
            headers: filtered_headers(request.headers),
            body: request.body,
            timeout_secs: request.timeout_secs,
        };
        self.execute_authenticated_service(request).await
    }
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
    use crate::HttpHeader;

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
