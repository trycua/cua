#![allow(dead_code)]

use async_trait::async_trait;
use cyclops_sdk::{HttpClient, HttpError, HttpRequest, HttpResponse};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::{Mutex, Notify};

pub struct ScriptedHttpClient {
    requests: Mutex<Vec<HttpRequest>>,
    responses: Mutex<VecDeque<QueuedResponse>>,
}

enum QueuedResponse {
    Immediate(Result<HttpResponse, HttpError>),
    Blocking {
        response: Result<HttpResponse, HttpError>,
        started: Arc<Notify>,
        release: Arc<Notify>,
    },
}

pub struct BlockedResponse {
    started: Arc<Notify>,
    release: Arc<Notify>,
}

impl ScriptedHttpClient {
    pub fn new(responses: impl IntoIterator<Item = Result<HttpResponse, HttpError>>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(
                responses
                    .into_iter()
                    .map(QueuedResponse::Immediate)
                    .collect(),
            ),
        }
    }

    pub async fn enqueue(&self, response: Result<HttpResponse, HttpError>) {
        self.responses
            .lock()
            .await
            .push_back(QueuedResponse::Immediate(response));
    }

    pub async fn enqueue_blocking(
        &self,
        response: Result<HttpResponse, HttpError>,
    ) -> BlockedResponse {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        self.responses
            .lock()
            .await
            .push_back(QueuedResponse::Blocking {
                response,
                started: Arc::clone(&started),
                release: Arc::clone(&release),
            });
        BlockedResponse { started, release }
    }

    pub async fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().await.clone()
    }

    pub async fn request_count(&self) -> usize {
        self.requests.lock().await.len()
    }

    pub async fn authenticated_requests(&self) -> Vec<HttpRequest> {
        self.requests().await.into_iter().skip(1).collect()
    }
}

impl BlockedResponse {
    pub async fn wait_until_started(&self) {
        self.started.notified().await;
    }

    pub fn release(&self) {
        self.release.notify_one();
    }
}

#[async_trait]
impl HttpClient for ScriptedHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        self.requests.lock().await.push(request);
        let response = {
            self.responses
                .lock()
                .await
                .pop_front()
                .expect("scripted HTTP client received an unexpected request")
        };
        match response {
            QueuedResponse::Immediate(response) => response,
            QueuedResponse::Blocking {
                response,
                started,
                release,
            } => {
                started.notify_one();
                release.notified().await;
                response
            }
        }
    }
}
