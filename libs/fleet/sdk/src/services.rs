use crate::{
    HttpHeader, HttpRequest, HttpResponse, Sandbox, SdkError, client::CyclopsClient,
    routes::service_url,
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
        if !sandbox.services.iter().any(|known| known == &service) {
            let mut available = sandbox.services.clone();
            available.sort_unstable();
            available.dedup();
            return Err(SdkError::UnknownService {
                requested: service,
                available,
            });
        }

        let url = service_url(self.base_url(), &sandbox, &service, &path)?;
        let request = HttpRequest {
            method: request.method,
            url: url.to_string(),
            headers: fleet_headers(filtered_headers(request.headers), &sandbox.claim),
            body: request.body,
            timeout_secs: request.timeout_secs,
        };
        self.execute_authenticated_service(request).await
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
