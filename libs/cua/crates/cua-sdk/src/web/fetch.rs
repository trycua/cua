//! A cyclops-sdk `HttpClient` over the global `fetch` (`globalThis.fetch`).
//!
//! cyclops-sdk's own browser transport calls `window.fetch`, so it fails
//! wherever there is no `window`: Node, Deno, Bun, web and service workers.
//! This one looks `fetch` up on the global object instead and keeps the
//! same contract: no redirects, no ambient credentials, only the supplied
//! headers and body, and `max_response_bytes` enforced while streaming.

use cyclops_sdk::{HttpClient, HttpError, HttpRequest, HttpResponse};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// `fetch` from the global object (browser page, worker or Node).
pub(crate) struct GlobalFetch;

fn err(e: JsValue) -> HttpError {
    let reason = e
        .as_string()
        .or_else(|| {
            e.dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
        })
        .unwrap_or_else(|| format!("fetch failed: {e:?}"));
    HttpError::Transport { reason }
}

fn transport(reason: impl Into<String>) -> HttpError {
    HttpError::Transport {
        reason: reason.into(),
    }
}

/// `AbortSignal.timeout(ms)` when the host has it (browsers, Node >= 17.3).
fn timeout_signal(secs: u64) -> Option<web_sys::AbortSignal> {
    let global = js_sys::global();
    let ctor = js_sys::Reflect::get(&global, &"AbortSignal".into()).ok()?;
    let f = js_sys::Reflect::get(&ctor, &"timeout".into())
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    let ms = JsValue::from_f64((secs.saturating_mul(1000)) as f64);
    f.call1(&ctor, &ms).ok()?.dyn_into().ok()
}

#[async_trait::async_trait(?Send)]
impl HttpClient for GlobalFetch {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let init = web_sys::RequestInit::new();
        init.set_method(&request.method);
        init.set_redirect(web_sys::RequestRedirect::Error);
        init.set_credentials(web_sys::RequestCredentials::Omit);
        if let Some(body) = &request.body {
            init.set_body(&js_sys::Uint8Array::from(body.as_slice()).into());
        }
        if let Some(secs) = request.timeout_secs
            && let Some(signal) = timeout_signal(secs)
        {
            init.set_signal(Some(&signal));
        }
        let req = web_sys::Request::new_with_str_and_init(&request.url, &init).map_err(err)?;
        let headers = req.headers();
        for h in &request.headers {
            headers.set(&h.name, &h.value).map_err(err)?;
        }

        let global = js_sys::global();
        let fetch = js_sys::Reflect::get(&global, &"fetch".into())
            .map_err(err)?
            .dyn_into::<js_sys::Function>()
            .map_err(|_| transport("globalThis.fetch is unavailable"))?;
        let promise = fetch
            .call1(&global, &req)
            .map_err(err)?
            .dyn_into::<js_sys::Promise>()
            .map_err(|_| transport("fetch did not return a Promise"))?;
        let response = JsFuture::from(promise)
            .await
            .map_err(err)?
            .dyn_into::<web_sys::Response>()
            .map_err(|_| transport("fetch did not resolve to a Response"))?;

        let mut body = Vec::new();
        if let Some(stream) = response.body() {
            let reader = stream
                .get_reader()
                .dyn_into::<web_sys::ReadableStreamDefaultReader>()
                .map_err(|_| transport("response body has no default reader"))?;
            loop {
                let chunk = JsFuture::from(reader.read()).await.map_err(err)?;
                let done = js_sys::Reflect::get(&chunk, &"done".into())
                    .map_err(err)?
                    .as_bool()
                    .unwrap_or(false);
                if done {
                    break;
                }
                let value = js_sys::Reflect::get(&chunk, &"value".into()).map_err(err)?;
                let bytes = js_sys::Uint8Array::new(&value);
                let next = body.len() as u64 + u64::from(bytes.length());
                if request.max_response_bytes.is_some_and(|max| next > max) {
                    let _ = JsFuture::from(reader.cancel()).await;
                    return Err(transport(format!(
                        "response body exceeds {} bytes",
                        request.max_response_bytes.unwrap_or_default()
                    )));
                }
                body.extend_from_slice(&bytes.to_vec());
            }
            reader.release_lock();
        }
        Ok(HttpResponse {
            status: response.status(),
            headers: vec![],
            body,
        })
    }
}
