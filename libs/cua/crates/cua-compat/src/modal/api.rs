//! `modal.client.ModalClient`: the control plane.

use crate::modal::db::{AppRec, SecretRec, now_s};
use crate::modal::image::{ImageRec, Status as ImgStatus};
use crate::modal::pb::api::{self as pb, modal_client_server::ModalClient};
use crate::modal::sandbox::{SbState, status as gs};
use crate::modal::{Caller, Modal, errors, ids};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tonic::{Request, Response, Status};

pub struct Api(pub Arc<Modal>);

type Resp<T> = Result<Response<T>, Status>;

fn caller<T>(r: &Request<T>) -> Result<Caller, Status> {
    r.extensions()
        .get::<Caller>()
        .cloned()
        .ok_or_else(|| Status::unauthenticated("Token missing. Could not authenticate client."))
}

pub fn secs(d: f32) -> Duration {
    Duration::from_secs_f32(d.clamp(0.0, 600.0))
}

const BUILDER_VERSION: &str = "2025.06";

fn lifecycle(a: &AppRec) -> pb::AppLifecycle {
    pb::AppLifecycle {
        app_state: if a.stopped {
            pb::AppState::Stopped as i32
        } else if a.ephemeral {
            pb::AppState::Ephemeral as i32
        } else {
            pb::AppState::Deployed as i32
        },
        created_at: a.created_at,
        deployed_at: if a.ephemeral { 0.0 } else { a.created_at },
        ..Default::default()
    }
}

fn app_meta(a: &AppRec) -> pb::AppHandleMetadata {
    pb::AppHandleMetadata {
        description: a.description.clone(),
        app_id: a.id.clone(),
        environment_name: a.environment.clone(),
        lifecycle: Some(lifecycle(a)),
        ..Default::default()
    }
}

fn creation(t: f64) -> pb::CreationInfo {
    pb::CreationInfo {
        created_at: t,
        created_by: "cua".into(),
    }
}

#[tonic::async_trait]
impl ModalClient for Api {
    async fn client_hello(&self, _r: Request<()>) -> Resp<pb::ClientHelloResponse> {
        Ok(Response::new(pb::ClientHelloResponse {
            warning: String::new(),
            image_builder_version: BUILDER_VERSION.into(),
            server_warnings: vec![],
        }))
    }

    async fn auth_token_get(
        &self,
        r: Request<pb::AuthTokenGetRequest>,
    ) -> Resp<pb::AuthTokenGetResponse> {
        let c = caller(&r)?;
        Ok(Response::new(pb::AuthTokenGetResponse {
            token: self.0.signer.mint("api", &c.key_id(), self.0.cfg.jwt_ttl_s),
        }))
    }

    // ---- apps -----------------------------------------------------------

    async fn app_get_or_create(
        &self,
        r: Request<pb::AppGetOrCreateRequest>,
    ) -> Resp<pb::AppGetOrCreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let create = q.object_creation_type == pb::ObjectCreationType::CreateIfMissing as i32
            || q.object_creation_type == pb::ObjectCreationType::CreateOverwriteIfExists as i32;
        let env = if q.environment_name.is_empty() {
            "main".into()
        } else {
            q.environment_name.clone()
        };
        let m = &self.0;
        let rec = m.db.write(|d| {
            if let Some(a) = d.apps.values().find(|a| {
                a.key_id == c.key_id()
                    && a.name.as_deref() == Some(&q.app_name)
                    && a.environment == env
                    && !a.stopped
                    && !a.ephemeral
            }) {
                return Some(a.clone());
            }
            if !create {
                return None;
            }
            let a = AppRec {
                id: ids::new_id("ap"),
                key_id: c.key_id(),
                name: Some(q.app_name.clone()),
                environment: env.clone(),
                description: q.app_name.clone(),
                created_at: now_s(),
                ephemeral: false,
                stopped: false,
                tags: Default::default(),
                heartbeat_at: now_s(),
                function_ids: Default::default(),
                class_ids: Default::default(),
                deployed_at: 0.0,
                deployment_version: 0,
            };
            d.apps.insert(a.id.clone(), a.clone());
            Some(a)
        });
        match rec {
            Some(a) => Ok(Response::new(pb::AppGetOrCreateResponse {
                app_id: a.id.clone(),
                handle_metadata: Some(app_meta(&a)),
            })),
            None => Err(errors::not_found(format!(
                "App '{}' not found in environment '{}'",
                q.app_name, env
            ))),
        }
    }

    async fn app_create(&self, r: Request<pb::AppCreateRequest>) -> Resp<pb::AppCreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let a = AppRec {
            id: ids::new_id("ap"),
            key_id: c.key_id(),
            name: None,
            environment: if q.environment_name.is_empty() {
                "main".into()
            } else {
                q.environment_name
            },
            description: q.description,
            created_at: now_s(),
            ephemeral: true,
            stopped: false,
            tags: q.tags.into_iter().collect(),
            heartbeat_at: now_s(),
            function_ids: Default::default(),
            class_ids: Default::default(),
            deployed_at: 0.0,
            deployment_version: 0,
        };
        self.0.db.write(|d| d.apps.insert(a.id.clone(), a.clone()));
        Ok(Response::new(pb::AppCreateResponse {
            app_id: a.id.clone(),
            app_page_url: format!("{}/apps/{}", self.0.cfg.api_url, a.id),
            app_logs_url: String::new(),
        }))
    }

    async fn app_heartbeat(&self, r: Request<pb::AppHeartbeatRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().app_id;
        let ok = self.0.db.write(|d| match d.apps.get_mut(&id) {
            Some(a) if a.key_id == c.key_id() => {
                a.heartbeat_at = now_s();
                true
            }
            _ => false,
        });
        if ok {
            Ok(Response::new(()))
        } else {
            Err(errors::not_found(format!("App {id} not found")))
        }
    }

    async fn app_client_disconnect(&self, r: Request<pb::AppClientDisconnectRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().app_id;
        self.stop_app(&c, &id, false).await;
        Ok(Response::new(()))
    }

    async fn app_stop(&self, r: Request<pb::AppStopRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().app_id;
        self.stop_app(&c, &id, true).await;
        Ok(Response::new(()))
    }

    async fn app_get_lifecycle(
        &self,
        r: Request<pb::AppGetLifecycleRequest>,
    ) -> Resp<pb::AppGetLifecycleResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().app_id;
        let a = self
            .0
            .db
            .read(|d| d.apps.get(&id).filter(|a| a.key_id == c.key_id()).cloned())
            .ok_or_else(|| errors::not_found(format!("App {id} not found")))?;
        Ok(Response::new(pb::AppGetLifecycleResponse {
            lifecycle: Some(lifecycle(&a)),
        }))
    }

    async fn app_get_info(
        &self,
        r: Request<pb::AppGetInfoRequest>,
    ) -> Resp<pb::AppGetInfoResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().app_id;
        let a = self
            .0
            .db
            .read(|d| d.apps.get(&id).filter(|a| a.key_id == c.key_id()).cloned())
            .ok_or_else(|| errors::not_found(format!("App {id} not found")))?;
        Ok(Response::new(pb::AppGetInfoResponse {
            info: Some(app_meta(&a)),
            ..Default::default()
        }))
    }

    async fn app_set_tags(&self, r: Request<pb::AppSetTagsRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let ok = self.0.db.write(|d| match d.apps.get_mut(&q.app_id) {
            Some(a) if a.key_id == c.key_id() => {
                a.tags = q.tags.into_iter().collect();
                true
            }
            _ => false,
        });
        if ok {
            Ok(Response::new(()))
        } else {
            Err(errors::not_found(format!("App {} not found", q.app_id)))
        }
    }

    async fn app_get_tags(
        &self,
        r: Request<pb::AppGetTagsRequest>,
    ) -> Resp<pb::AppGetTagsResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().app_id;
        let a = self
            .0
            .db
            .read(|d| d.apps.get(&id).filter(|a| a.key_id == c.key_id()).cloned())
            .ok_or_else(|| errors::not_found(format!("App {id} not found")))?;
        Ok(Response::new(pb::AppGetTagsResponse {
            tags: a.tags.into_iter().collect(),
        }))
    }

    // ---- environments / workspace ---------------------------------------

    async fn environment_get_or_create(
        &self,
        r: Request<pb::EnvironmentGetOrCreateRequest>,
    ) -> Resp<pb::EnvironmentGetOrCreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let name = if q.deployment_name.is_empty() {
            "main".to_string()
        } else {
            q.deployment_name
        };
        if name != "main" {
            let key = format!("{}/{name}", c.key_id());
            let create = q.object_creation_type == pb::ObjectCreationType::CreateIfMissing as i32;
            let found = self.0.db.write(|d| {
                if d.environments.contains_key(&key) {
                    return true;
                }
                if create {
                    d.environments.insert(
                        key.clone(),
                        crate::modal::db::EnvRec {
                            name: name.clone(),
                            key_id: c.key_id(),
                            created_at: now_s(),
                        },
                    );
                    return true;
                }
                false
            });
            if !found {
                return Err(errors::not_found(format!("Environment '{name}' not found")));
            }
        }
        Ok(Response::new(pb::EnvironmentGetOrCreateResponse {
            environment_id: format!("en-{}", name),
            metadata: Some(pb::EnvironmentMetadata {
                name,
                settings: Some(pb::EnvironmentSettings {
                    image_builder_version: BUILDER_VERSION.into(),
                    webhook_suffix: "cua".into(),
                    ..Default::default()
                }),
                environment_type: 0,
            }),
        }))
    }

    async fn workspace_name_lookup(
        &self,
        _r: Request<()>,
    ) -> Resp<pb::WorkspaceNameLookupResponse> {
        Ok(Response::new(pb::WorkspaceNameLookupResponse {
            username: "cua".into(),
            ..Default::default()
        }))
    }

    // ---- secrets --------------------------------------------------------

    async fn secret_get_or_create(
        &self,
        r: Request<pb::SecretGetOrCreateRequest>,
    ) -> Resp<pb::SecretGetOrCreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        use pb::ObjectCreationType as T;
        let kind = T::try_from(q.object_creation_type).unwrap_or(T::Unspecified);
        let env_name = if q.environment_name.is_empty() {
            "main".to_string()
        } else {
            q.environment_name.clone()
        };
        let named = !q.deployment_name.is_empty();
        let m = &self.0;
        let res: Result<SecretRec, Status> = m.db.write(|d| {
            if named {
                let existing = d
                    .secrets
                    .values()
                    .find(|s| {
                        s.key_id == c.key_id()
                            && s.name.as_deref() == Some(&q.deployment_name)
                            && s.environment == env_name
                    })
                    .cloned();
                match (existing, kind) {
                    (Some(_), T::CreateFailIfExists) => {
                        return Err(errors::exists(format!(
                            "Secret '{}' already exists",
                            q.deployment_name
                        )));
                    }
                    (Some(mut s), T::CreateOverwriteIfExists) => {
                        s.env = q.env_dict.clone().into_iter().collect();
                        d.secrets.insert(s.id.clone(), s.clone());
                        return Ok(s);
                    }
                    (Some(s), _) => {
                        for k in &q.required_keys {
                            if !s.env.contains_key(k) {
                                return Err(errors::not_found(format!(
                                    "Secret is missing key(s): {k}"
                                )));
                            }
                        }
                        return Ok(s);
                    }
                    (
                        None,
                        T::CreateIfMissing | T::CreateFailIfExists | T::CreateOverwriteIfExists,
                    ) => {}
                    (None, _) => {
                        return Err(errors::not_found(format!(
                            "Secret '{}' not found in environment '{}'",
                            q.deployment_name, env_name
                        )));
                    }
                }
            }
            let s = SecretRec {
                id: ids::new_id("st"),
                key_id: c.key_id(),
                name: named.then(|| q.deployment_name.clone()),
                environment: env_name.clone(),
                created_at: now_s(),
                app_id: (!q.app_id.is_empty()).then(|| q.app_id.clone()),
                env: q.env_dict.clone().into_iter().collect(),
            };
            d.secrets.insert(s.id.clone(), s.clone());
            Ok(s)
        });
        let s = res?;
        Ok(Response::new(pb::SecretGetOrCreateResponse {
            secret_id: s.id.clone(),
            metadata: Some(secret_meta(&s)),
        }))
    }

    async fn secret_get_info(
        &self,
        r: Request<pb::SecretGetInfoRequest>,
    ) -> Resp<pb::SecretGetInfoResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().secret_id;
        let s = self.secret_of(&c, &id)?;
        Ok(Response::new(pb::SecretGetInfoResponse {
            metadata: Some(secret_meta(&s)),
        }))
    }

    async fn secret_list(&self, r: Request<pb::SecretListRequest>) -> Resp<pb::SecretListResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let (max, before) = q
            .pagination
            .map(|p| (p.max_objects, p.created_before))
            .unwrap_or((0, 0.0));
        let mut v: Vec<SecretRec> = self.0.db.read(|d| {
            d.secrets
                .values()
                .filter(|s| s.key_id == c.key_id() && s.name.is_some())
                .filter(|s| before == 0.0 || s.created_at < before)
                .cloned()
                .collect()
        });
        v.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        if max > 0 {
            v.truncate(max as usize);
        }
        Ok(Response::new(pb::SecretListResponse {
            items: v
                .iter()
                .map(|s| pb::SecretListItem {
                    label: s.name.clone().unwrap_or_default(),
                    created_at: s.created_at,
                    last_used_at: 0.0,
                    environment_name: s.environment.clone(),
                    secret_id: s.id.clone(),
                    metadata: Some(secret_meta(s)),
                })
                .collect(),
            environment_name: q.environment_name,
        }))
    }

    async fn secret_delete(&self, r: Request<pb::SecretDeleteRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().secret_id;
        self.secret_of(&c, &id)?;
        self.0.db.write(|d| d.secrets.remove(&id));
        Ok(Response::new(()))
    }

    async fn secret_update(&self, r: Request<pb::SecretUpdateRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.secret_of(&c, &q.secret_id)?;
        validate_env_keys(
            q.updates
                .iter()
                .filter(|u| u.value.is_some())
                .map(|u| &u.key),
        )?;
        self.0.db.write(|d| {
            if let Some(s) = d.secrets.get_mut(&q.secret_id) {
                for u in &q.updates {
                    match &u.value {
                        Some(v) => {
                            s.env.insert(u.key.clone(), v.clone());
                        }
                        None => {
                            s.env.remove(&u.key);
                        }
                    }
                }
            }
        });
        Ok(Response::new(()))
    }

    // ---- blobs and mounts -------------------------------------------------

    async fn blob_create(&self, r: Request<pb::BlobCreateRequest>) -> Resp<pb::BlobCreateResponse> {
        caller(&r)?;
        let id = ids::new_id("bl");
        let url = format!("{}/_cua/blob/{id}", self.0.cfg.api_url);
        // The 1.6 client reads the lists (one provider, no fallback); older
        // clients read the single URL.
        Ok(Response::new(pb::BlobCreateResponse {
            blob_id: id.clone(),
            blob_ids: vec![id],
            upload_type_oneof: Some(pb::blob_create_response::UploadTypeOneof::UploadUrl(
                url.clone(),
            )),
            upload_types_oneof: Some(pb::blob_create_response::UploadTypesOneof::UploadUrls(
                pb::UploadUrlList { items: vec![url] },
            )),
        }))
    }

    async fn blob_get(&self, r: Request<pb::BlobGetRequest>) -> Resp<pb::BlobGetResponse> {
        caller(&r)?;
        let id = r.into_inner().blob_id;
        if !self.0.blobs.has_id(&id) {
            return Err(errors::not_found(format!("Blob {id} not found")));
        }
        Ok(Response::new(pb::BlobGetResponse {
            download_url: format!("{}/_cua/blob/{id}", self.0.cfg.api_url),
        }))
    }

    async fn mount_put_file(
        &self,
        r: Request<pb::MountPutFileRequest>,
    ) -> Resp<pb::MountPutFileResponse> {
        caller(&r)?;
        let q = r.into_inner();
        let ts = Some(prost_types::Timestamp {
            seconds: now_s() as i64,
            nanos: 0,
        });
        if q.data_oneof.is_none() {
            // Existence check.
            return Ok(Response::new(pb::MountPutFileResponse {
                exists: self.0.blobs.has_sha(&q.sha256_hex),
                checked_at: ts,
            }));
        }
        let data = match q.data_oneof {
            Some(pb::mount_put_file_request::DataOneof::Data(d)) => d.to_vec(),
            Some(pb::mount_put_file_request::DataOneof::DataBlobId(b)) => {
                self.0
                    .blobs
                    .get_id(&b)
                    .ok_or_else(|| errors::not_found(format!("Blob {b} not found")))?
            }
            None => unreachable!(),
        };
        self.0
            .blobs
            .put_sha(Some(&q.sha256_hex), &data)
            .map_err(errors::invalid)?;
        Ok(Response::new(pb::MountPutFileResponse {
            exists: true,
            checked_at: ts,
        }))
    }

    async fn mount_batched_check_existence(
        &self,
        r: Request<pb::MountBatchedCheckExistenceRequest>,
    ) -> Resp<pb::MountBatchedCheckExistenceResponse> {
        caller(&r)?;
        let q = r.into_inner();
        Ok(Response::new(pb::MountBatchedCheckExistenceResponse {
            missing_sha256_hex_hashes: q
                .sha256_hex_hashes
                .into_iter()
                .filter(|h| !self.0.blobs.has_sha(h))
                .collect(),
            checked_at: Some(prost_types::Timestamp {
                seconds: now_s() as i64,
                nanos: 0,
            }),
        }))
    }

    async fn mount_get_or_create(
        &self,
        r: Request<pb::MountGetOrCreateRequest>,
    ) -> Resp<pb::MountGetOrCreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.mount_get_or_create_impl(&c, q)
    }

    // ---- images ---------------------------------------------------------

    async fn image_get_or_create(
        &self,
        r: Request<pb::ImageGetOrCreateRequest>,
    ) -> Resp<pb::ImageGetOrCreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let img = q.image.unwrap_or_default();
        let has_function = img.build_function.is_some() || !q.build_function_id.is_empty();
        if img.dockerfile_commands.is_empty() && !has_function {
            return Err(errors::invalid("an image needs Dockerfile commands"));
        }
        if has_function {
            let ok = m.db.read(|d| {
                d.functions
                    .get(&q.build_function_id)
                    .is_some_and(|f| f.key_id == c.key_id() && f.created)
            });
            if !ok {
                return Err(errors::not_found(format!(
                    "Build function {} not found",
                    q.build_function_id
                )));
            }
            if img.base_images.len() != 1 {
                return Err(errors::invalid(
                    "a build function runs on exactly one base image",
                ));
            }
        }
        for (_, id) in img
            .base_images
            .iter()
            .map(|b| (b.docker_tag.clone(), b.image_id.clone()))
        {
            let ok =
                m.db.read(|d| d.images.get(&id).is_some_and(|i| i.key_id == c.key_id()));
            if !ok {
                return Err(errors::not_found(format!("Base image {id} not found")));
            }
        }
        let spec = m
            .spec_of(&img, &q.builder_version, &q.build_function_id)
            .map_err(errors::invalid)?;
        let hash = m.hash_of(&c.key_id(), &spec);
        let force = q.force_build || q.ignore_cache;
        let existing = m.db.read(|d| {
            d.images
                .values()
                .find(|i| i.key_id == c.key_id() && i.hash == hash)
                .cloned()
        });
        let rec = match existing {
            Some(rec) if !force && !matches!(rec.status, ImgStatus::Failed(_)) => rec,
            other => {
                let id = other.map(|o| o.id).unwrap_or_else(|| ids::new_id("im"));
                let rec = ImageRec {
                    id: id.clone(),
                    key_id: c.key_id(),
                    created_at: now_s(),
                    spec,
                    hash,
                    status: ImgStatus::Pending,
                    built: None,
                    tags: Default::default(),
                    expires_at: None,
                };
                m.db.write(|d| d.images.insert(id.clone(), rec.clone()));
                // A fresh log for a fresh build.
                m.live.build_logs.lock().await.remove(&id);
                let me = m.clone();
                tokio::spawn(async move {
                    let _ = me.ensure_image(id).await;
                });
                rec
            }
        };
        let (result, metadata) = match (&rec.status, &rec.built) {
            (ImgStatus::Built, Some(b)) => (
                Some(pb::GenericResult {
                    status: gs::SUCCESS,
                    ..Default::default()
                }),
                Some(image_meta(b)),
            ),
            _ => (None, None),
        };
        Ok(Response::new(pb::ImageGetOrCreateResponse {
            image_id: rec.id,
            result,
            metadata,
        }))
    }

    async fn image_join_streaming(
        &self,
        r: Request<pb::ImageJoinStreamingRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::ImageJoinStreamingResponse>> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = self.0.clone();
        let rec = m
            .image(&q.image_id)
            .filter(|i| i.key_id == c.key_id())
            .ok_or_else(|| errors::not_found(format!("Image {} not found", q.image_id)))?;
        let log = m.build_log(&rec.id).await;
        let from: usize = q.last_entry_id.parse().unwrap_or(0);
        let hold = secs(q.timeout.max(1.0));
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let id = rec.id.clone();
        tokio::spawn(async move {
            let mut next = from;
            let mut sub = log.subscribe();
            let deadline = tokio::time::Instant::now() + hold;
            loop {
                let lines = log.lines_from(next);
                let outcome = log.outcome();
                let done =
                    outcome.is_some() || m.image(&id).is_some_and(|i| i.status == ImgStatus::Built);
                if !lines.is_empty() || done {
                    next += lines.len();
                    let mut resp = pb::ImageJoinStreamingResponse {
                        entry_id: next.to_string(),
                        task_logs: lines
                            .iter()
                            .map(|l| pb::TaskLogs {
                                data: format!("{l}\n"),
                                file_descriptor: pb::FileDescriptor::Stdout as i32,
                                timestamp: now_s(),
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    };
                    if done {
                        let rec = m.image(&id);
                        match (&outcome, rec.as_ref().and_then(|r| r.built.as_ref())) {
                            (Some(Err(e)), _) => {
                                resp.result = Some(pb::GenericResult {
                                    status: gs::FAILURE,
                                    exception: e.clone(),
                                    ..Default::default()
                                });
                            }
                            (_, Some(b)) => {
                                resp.result = Some(pb::GenericResult {
                                    status: gs::SUCCESS,
                                    ..Default::default()
                                });
                                resp.metadata = Some(image_meta(b));
                            }
                            _ => {
                                resp.result = Some(pb::GenericResult {
                                    status: gs::FAILURE,
                                    exception: "the image build was lost".into(),
                                    ..Default::default()
                                });
                            }
                        }
                        resp.eof = true;
                    }
                    if tx.send(Ok(resp)).await.is_err() || done {
                        return;
                    }
                    continue;
                }
                if tokio::time::timeout_at(deadline, sub.changed())
                    .await
                    .is_err()
                {
                    // Nothing new within the hold: the client asks again.
                    let _ = tx
                        .send(Ok(pb::ImageJoinStreamingResponse {
                            entry_id: next.to_string(),
                            ..Default::default()
                        }))
                        .await;
                    return;
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    async fn image_from_id(
        &self,
        r: Request<pb::ImageFromIdRequest>,
    ) -> Resp<pb::ImageFromIdResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().image_id;
        let rec = self
            .0
            .image(&id)
            .filter(|i| i.key_id == c.key_id())
            .ok_or_else(|| errors::not_found(format!("Image {id} not found")))?;
        Ok(Response::new(pb::ImageFromIdResponse {
            image_id: id,
            metadata: rec.built.as_ref().map(image_meta),
        }))
    }

    async fn image_build_chain_get(
        &self,
        r: Request<pb::ImageBuildChainGetRequest>,
    ) -> Resp<pb::ImageBuildChainGetResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().image_id;
        let m = &self.0;
        // The layer and the layers it is built on, base first.
        let mut chain = Vec::new();
        let mut cur = Some(id.clone());
        while let Some(i) = cur.take() {
            let rec = m
                .image(&i)
                .filter(|x| x.key_id == c.key_id())
                .ok_or_else(|| errors::not_found(format!("Image {i} not found")))?;
            cur = rec.spec.base_images.first().map(|(_, b)| b.clone());
            chain.push(rec);
            if chain.len() > 64 {
                break;
            }
        }
        chain.reverse();
        let spans = m.build_spans.lock().expect("build spans").clone();
        let ts = |ns: u64| prost_types::Timestamp {
            seconds: (ns / 1_000_000_000) as i64,
            nanos: (ns % 1_000_000_000) as i32,
        };
        let steps = chain
            .into_iter()
            .map(|rec| {
                let (start, end) = spans.get(&rec.id).copied().unwrap_or_else(|| {
                    let t = (rec.created_at * 1e9) as u64;
                    (t, t)
                });
                pb::ImageBuildStep {
                    image_id: rec.id.clone(),
                    started_at: Some(ts(start)),
                    finished_at: Some(ts(end.max(start) + 2_000)),
                    builder_app_id: crate::modal::image::build_app_id(&rec.id),
                    builder_task_id: crate::modal::image::build_task_id(&rec.id),
                }
            })
            .collect();
        Ok(Response::new(pb::ImageBuildChainGetResponse {
            build_steps: steps,
        }))
    }

    async fn image_publish(
        &self,
        r: Request<pb::ImagePublishRequest>,
    ) -> Resp<pb::ImagePublishResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        if q.tag.is_empty() {
            return Err(errors::invalid("an image name is required"));
        }
        self.0
            .image(&q.image_id)
            .filter(|i| i.key_id == c.key_id())
            .ok_or_else(|| errors::not_found(format!("Image {} not found", q.image_id)))?;
        // `allow_public` publishes beyond the workspace; there is one
        // workspace here.
        self.0.db.write(|d| {
            d.image_tags
                .insert(format!("{}/{}", c.key_id(), q.tag), q.image_id.clone())
        });
        Ok(Response::new(pb::ImagePublishResponse::default()))
    }

    async fn image_get_by_tag(
        &self,
        r: Request<pb::ImageGetByTagRequest>,
    ) -> Resp<pb::ImageGetByTagResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let id = self
            .0
            .db
            .read(|d| {
                d.image_tags
                    .get(&format!("{}/{}", c.key_id(), q.tag))
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("Image '{}' not found", q.tag)))?;
        Ok(Response::new(pb::ImageGetByTagResponse { image_id: id }))
    }

    async fn image_delete(&self, r: Request<pb::ImageDeleteRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().image_id;
        let rec = self
            .0
            .image(&id)
            .filter(|i| i.key_id == c.key_id())
            .ok_or_else(|| errors::not_found(format!("Image {id} not found")))?;
        let in_use = self.0.db.read(|d| {
            d.sandboxes
                .values()
                .any(|s| s.image_id == id && s.running())
                || d.images
                    .values()
                    .any(|i| i.id != id && i.spec.base_images.iter().any(|(_, b)| b == &id))
        });
        if in_use {
            return Err(errors::conflict(format!("Image {id} is in use")));
        }
        if let Some(cp) = rec.built.and_then(|b| b.checkpoint) {
            let who = c.identity();
            // A checkpoint other images share stays.
            let shared = self.0.db.read(|d| {
                d.images.values().any(|i| {
                    i.id != id && i.built.as_ref().and_then(|b| b.checkpoint.as_ref()) == Some(&cp)
                })
            });
            if !shared {
                let _ = self.0.backend.delete_checkpoint(&who, &cp).await;
            }
        }
        self.0.db.write(|d| {
            d.image_tags.retain(|_, v| *v != id);
            d.images.remove(&id)
        });
        Ok(Response::new(()))
    }

    // ---- sandboxes --------------------------------------------------------

    async fn sandbox_create(
        &self,
        r: Request<pb::SandboxCreateRequest>,
    ) -> Resp<pb::SandboxCreateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let def = q.definition.unwrap_or_default();
        let app =
            m.db.read(|d| {
                d.apps
                    .get(&q.app_id)
                    .filter(|a| a.key_id == c.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("App {} not found", q.app_id)))?;
        if app.stopped {
            return Err(errors::conflict(format!("App {} is stopped", q.app_id)));
        }
        m.check_sandbox_definition(&c, &def)?;
        let tags = q
            .tags
            .into_iter()
            .map(|t| (t.tag_name, t.tag_value))
            .collect();
        let rec = m
            .create_sandbox(&c, &q.app_id, def, tags, false, BTreeMap::new())
            .await?;
        Ok(Response::new(pb::SandboxCreateResponse {
            sandbox_id: rec.id.clone(),
            metadata: Some(rec.handle_metadata()),
        }))
    }

    async fn sandbox_get_task_id(
        &self,
        r: Request<pb::SandboxGetTaskIdRequest>,
    ) -> Resp<pb::SandboxGetTaskIdResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        m.sandbox_of(&c, &q.sandbox_id)?;
        // A short hold keeps the client's polling quick without a busy loop.
        let hold = if q.wait_until_ready {
            secs(q.timeout.unwrap_or(55.0))
        } else {
            Duration::from_millis(750)
        };
        let rec = m
            .wait_scheduled(&q.sandbox_id, hold)
            .await
            .ok_or_else(|| errors::not_found(format!("Sandbox {} not found", q.sandbox_id)))?;
        Ok(Response::new(match rec.state {
            SbState::Scheduling => pb::SandboxGetTaskIdResponse::default(),
            SbState::Running => pb::SandboxGetTaskIdResponse {
                task_id: Some(rec.task_id.clone()),
                task_result: None,
            },
            SbState::Finished => {
                // A sandbox that never started has a result and no task.
                let started = rec.started_at > 0.0;
                pb::SandboxGetTaskIdResponse {
                    task_id: started.then(|| rec.task_id.clone()),
                    task_result: rec.result.as_ref().map(|x| {
                        let mut p = x.to_pb();
                        if let Some(e) = &rec.error {
                            p.exception = e.clone();
                        }
                        p
                    }),
                }
            }
        }))
    }

    async fn sandbox_wait(
        &self,
        r: Request<pb::SandboxWaitRequest>,
    ) -> Resp<pb::SandboxWaitResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        m.sandbox_of(&c, &q.sandbox_id)?;
        let rec = m
            .wait_finished(&q.sandbox_id, secs(q.timeout))
            .await
            .ok_or_else(|| errors::not_found(format!("Sandbox {} not found", q.sandbox_id)))?;
        Ok(Response::new(pb::SandboxWaitResponse {
            result: rec.result.as_ref().map(|x| x.to_pb()),
            metadata: Some(rec.handle_metadata()),
        }))
    }

    async fn sandbox_terminate(
        &self,
        r: Request<pb::SandboxTerminateRequest>,
    ) -> Resp<pb::SandboxTerminateResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().sandbox_id;
        let m = &self.0;
        let rec = m.sandbox_of(&c, &id)?;
        let existing = rec.result.as_ref().map(|x| x.to_pb());
        if rec.running() {
            m.terminate(&id).await;
        }
        Ok(Response::new(pb::SandboxTerminateResponse {
            existing_result: existing,
        }))
    }

    async fn sandbox_get_tunnels(
        &self,
        r: Request<pb::SandboxGetTunnelsRequest>,
    ) -> Resp<pb::SandboxGetTunnelsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        m.sandbox_of(&c, &q.sandbox_id)?;
        let rec = m
            .wait_scheduled(&q.sandbox_id, secs(q.timeout.min(50.0)))
            .await
            .ok_or_else(|| errors::not_found(format!("Sandbox {} not found", q.sandbox_id)))?;
        if rec.state == SbState::Scheduling {
            return Ok(Response::new(pb::SandboxGetTunnelsResponse {
                result: Some(pb::GenericResult {
                    status: gs::TIMEOUT,
                    ..Default::default()
                }),
                tunnels: vec![],
            }));
        }
        Ok(Response::new(pb::SandboxGetTunnelsResponse {
            result: Some(pb::GenericResult {
                status: gs::SUCCESS,
                ..Default::default()
            }),
            tunnels: rec
                .tunnels
                .iter()
                .map(crate::modal::sandbox::tunnel_pb)
                .collect(),
        }))
    }

    async fn sandbox_list(
        &self,
        r: Request<pb::SandboxListRequest>,
    ) -> Resp<pb::SandboxListResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let want: Vec<(String, String)> = q
            .tags
            .into_iter()
            .map(|t| (t.tag_name, t.tag_value))
            .collect();
        let mut v: Vec<_> = self.0.db.read(|d| {
            d.sandboxes
                .values()
                .filter(|s| s.key_id == c.key_id())
                .filter(|s| q.include_finished || s.running())
                .filter(|s| q.app_id.is_empty() || s.app_id == q.app_id)
                .filter(|s| q.before_timestamp == 0.0 || s.created_at < q.before_timestamp)
                .filter(|s| want.iter().all(|(k, val)| s.tags.get(k) == Some(val)))
                .cloned()
                .collect()
        });
        v.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        v.truncate(100);
        Ok(Response::new(pb::SandboxListResponse {
            sandboxes: v.iter().map(|s| s.info()).collect(),
        }))
    }

    async fn sandbox_tags_set(&self, r: Request<pb::SandboxTagsSetRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.0.sandbox_of(&c, &q.sandbox_id)?;
        self.0.update_sandbox(&q.sandbox_id, |s| {
            s.tags = q
                .tags
                .into_iter()
                .map(|t| (t.tag_name, t.tag_value))
                .collect();
        });
        Ok(Response::new(()))
    }

    async fn sandbox_tags_get(
        &self,
        r: Request<pb::SandboxTagsGetRequest>,
    ) -> Resp<pb::SandboxTagsGetResponse> {
        let c = caller(&r)?;
        let rec = self.0.sandbox_of(&c, &r.into_inner().sandbox_id)?;
        Ok(Response::new(pb::SandboxTagsGetResponse {
            tags: rec
                .tags
                .into_iter()
                .map(|(k, v)| pb::SandboxTag {
                    tag_name: k,
                    tag_value: v,
                })
                .collect(),
        }))
    }

    async fn sandbox_get_from_name(
        &self,
        r: Request<pb::SandboxGetFromNameRequest>,
    ) -> Resp<pb::SandboxGetFromNameResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let env = if q.environment_name.is_empty() {
            "main".to_string()
        } else {
            q.environment_name
        };
        let found = m.db.read(|d| {
            let apps: Vec<&AppRec> = d
                .apps
                .values()
                .filter(|a| {
                    a.key_id == c.key_id()
                        && a.name.as_deref() == Some(&q.app_name)
                        && a.environment == env
                })
                .collect();
            let mut cands: Vec<_> = d
                .sandboxes
                .values()
                .filter(|s| {
                    s.key_id == c.key_id()
                        && s.name.as_deref() == Some(&q.sandbox_name)
                        && apps.iter().any(|a| a.id == s.app_id)
                })
                .cloned()
                .collect();
            cands.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
            cands
                .iter()
                .find(|s| s.running())
                .or_else(|| {
                    if q.include_terminated {
                        cands.first()
                    } else {
                        None
                    }
                })
                .cloned()
        });
        match found {
            Some(s) => Ok(Response::new(pb::SandboxGetFromNameResponse {
                sandbox_id: s.id.clone(),
                metadata: Some(s.handle_metadata()),
            })),
            None => Err(errors::not_found(format!(
                "Sandbox with name '{}' not found in app '{}'",
                q.sandbox_name, q.app_name
            ))),
        }
    }

    async fn sandbox_get_logs(
        &self,
        r: Request<pb::SandboxGetLogsRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::TaskLogsBatch>> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let rec = self.0.sandbox_of(&c, &q.sandbox_id)?;
        let fd = if q.file_descriptor == pb::FileDescriptor::Stderr as i32 {
            crate::modal::exec::STDERR
        } else {
            crate::modal::exec::STDOUT
        };
        Ok(Response::new(
            self.0
                .sandbox_log_stream(rec, fd, q.last_entry_id, secs(q.timeout.max(1.0)))
                .await,
        ))
    }

    async fn sandbox_stdin_write(
        &self,
        r: Request<pb::SandboxStdinWriteRequest>,
    ) -> Resp<pb::SandboxStdinWriteResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let rec = self.0.sandbox_of(&c, &q.sandbox_id)?;
        let entry = self.0.live.entrypoints.lock().await.get(&rec.id).cloned();
        let Some(s) = entry else {
            return Err(errors::conflict(
                "Sandbox has no running entrypoint to write to",
            ));
        };
        self.0.touch(&rec.id).await;
        // The client numbers its writes from 1 (`index`); the offsets are ours.
        let offset = s.stdin_status().await.0;
        s.write_stdin(offset, &q.input, q.eof)
            .await
            .map_err(|e| errors::conflict(format!("{e:?}")))?;
        Ok(Response::new(pb::SandboxStdinWriteResponse {}))
    }

    async fn task_get_command_router_access(
        &self,
        r: Request<pb::TaskGetCommandRouterAccessRequest>,
    ) -> Resp<pb::TaskGetCommandRouterAccessResponse> {
        let c = caller(&r)?;
        let task = r.into_inner().task_id;
        let m = &self.0;
        let rec =
            m.db.read(|d| {
                d.sandboxes
                    .values()
                    .find(|s| s.task_id == task && s.key_id == c.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("Task {task} not found")))?;
        if rec.state == SbState::Finished {
            return Err(errors::conflict(format!(
                "Task {task} has already finished"
            )));
        }
        Ok(Response::new(pb::TaskGetCommandRouterAccessResponse {
            jwt: m.signer.mint("tcr", &task, m.cfg.jwt_ttl_s),
            url: m.router_url(&c),
        }))
    }

    // ---- V2 sandboxes: the default backend of client 1.6.2 -------------------
    //
    // The V2 RPCs carry the same messages as V1 and add the client's auth
    // token (`x-modal-auth-token`, from `AuthTokenGet`); creation returns the
    // task and the command router's address at once.

    async fn sandbox_create_v2(
        &self,
        r: Request<pb::SandboxCreateV2Request>,
    ) -> Resp<pb::SandboxCreateV2Response> {
        self.v2_auth(&r)?;
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let def = q.definition.unwrap_or_default();
        let app =
            m.db.read(|d| {
                d.apps
                    .get(&q.app_id)
                    .filter(|a| a.key_id == c.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("App {} not found", q.app_id)))?;
        if app.stopped {
            return Err(errors::conflict(format!("App {} is stopped", q.app_id)));
        }
        m.check_sandbox_definition(&c, &def)?;
        // Secrets given with the request: inline environments and ids.
        let mut env = BTreeMap::new();
        if let Some(e) = &q.ephemeral_secrets {
            env.extend(e.contents.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        for src in &q.secret_sources {
            match &src.source {
                Some(pb::secret_source::Source::SecretId(id)) => {
                    let sec =
                        m.db.read(|d| {
                            d.secrets
                                .get(id)
                                .filter(|x| x.key_id == c.key_id())
                                .cloned()
                        })
                        .ok_or_else(|| errors::not_found(format!("Secret {id} not found")))?;
                    env.extend(sec.env);
                }
                Some(pb::secret_source::Source::Env(e)) => {
                    env.extend(e.contents.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                None => {}
            }
        }
        let tags = q
            .tags
            .into_iter()
            .map(|t| (t.tag_name, t.tag_value))
            .collect();
        let rec = m
            .create_sandbox(&c, &q.app_id, def, tags, true, env)
            .await?;
        // If the client goes away mid-create, the sandbox goes with it.
        let guard = crate::modal::sandbox::CancelGuard::new(m.clone(), rec.id.clone());
        let done = m
            .wait_scheduled(&rec.id, Duration::from_secs(30 * 60))
            .await
            .ok_or_else(|| errors::not_found("sandbox vanished"))?;
        guard.disarm();
        // A sandbox whose entrypoint already ended is still a created
        // sandbox (`wait` returns its exit code); only one that never
        // started (no start time) is an error.
        let started = done.started_at > 0.0;
        match done.state {
            SbState::Running | SbState::Finished if started => {
                Ok(Response::new(pb::SandboxCreateV2Response {
                    sandbox_id: done.id.clone(),
                    tunnels: done
                        .tunnels
                        .iter()
                        .map(crate::modal::sandbox::tunnel_pb)
                        .collect(),
                    task_id: done.task_id.clone(),
                    metadata: Some(done.handle_metadata()),
                    command_router_access: Some(pb::CommandRouterAccess {
                        jwt: m.signer.mint("tcr", &done.task_id, m.cfg.jwt_ttl_s),
                        url: m.router_url(&c),
                    }),
                    sandbox_token: m.signer.mint("sbx", &done.id, m.cfg.jwt_ttl_s * 24),
                }))
            }
            _ => Err(errors::conflict(
                done.error
                    .unwrap_or_else(|| "the sandbox did not start".into()),
            )),
        }
    }

    async fn sandbox_get_task_id_v2(
        &self,
        r: Request<pb::SandboxGetTaskIdRequest>,
    ) -> Resp<pb::SandboxGetTaskIdResponse> {
        self.v2_auth(&r)?;
        self.sandbox_get_task_id(r).await
    }

    async fn sandbox_wait_v2(
        &self,
        r: Request<pb::SandboxWaitRequest>,
    ) -> Resp<pb::SandboxWaitResponse> {
        self.v2_auth(&r)?;
        self.sandbox_wait(r).await
    }

    async fn sandbox_terminate_v2(
        &self,
        r: Request<pb::SandboxTerminateRequest>,
    ) -> Resp<pb::SandboxTerminateResponse> {
        self.v2_auth(&r)?;
        self.sandbox_terminate(r).await
    }

    async fn sandbox_create_connect_token(
        &self,
        r: Request<pb::SandboxCreateConnectTokenRequest>,
    ) -> Resp<pb::SandboxCreateConnectTokenResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let rec = m.sandbox_of(&c, &q.sandbox_id)?;
        if !rec.running() {
            return Err(errors::conflict(format!(
                "Sandbox {} is not running",
                q.sandbox_id
            )));
        }
        let port = q.port.unwrap_or(8080);
        if !(1..=65535).contains(&port) {
            return Err(errors::invalid("port must be between 1 and 65535"));
        }
        let (host, tls_port) = match m.cfg.sandbox_domain.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h.to_string(), p.to_string()),
            _ => (m.cfg.sandbox_domain.clone(), "443".to_string()),
        };
        let token = m.signer.mint_with(
            "conn",
            &rec.id,
            24 * 3600,
            serde_json::json!({"port": port, "meta": q.user_metadata}),
        );
        Ok(Response::new(pb::SandboxCreateConnectTokenResponse {
            url: format!(
                "https://{port}-{}.{host}:{tls_port}",
                rec.id.to_ascii_lowercase()
            ),
            token,
        }))
    }

    async fn sandbox_create_connect_token_v2(
        &self,
        r: Request<pb::SandboxCreateConnectTokenRequest>,
    ) -> Resp<pb::SandboxCreateConnectTokenResponse> {
        self.v2_auth(&r)?;
        self.sandbox_create_connect_token(r).await
    }

    async fn sandbox_get_tunnels_v2(
        &self,
        r: Request<pb::SandboxGetTunnelsRequest>,
    ) -> Resp<pb::SandboxGetTunnelsResponse> {
        self.v2_auth(&r)?;
        self.sandbox_get_tunnels(r).await
    }

    async fn sandbox_list_v2(
        &self,
        r: Request<pb::SandboxListRequest>,
    ) -> Resp<pb::SandboxListResponse> {
        self.v2_auth(&r)?;
        self.sandbox_list(r).await
    }

    async fn sandbox_tags_set_v2(&self, r: Request<pb::SandboxTagsSetRequest>) -> Resp<()> {
        self.v2_auth(&r)?;
        self.sandbox_tags_set(r).await
    }

    async fn sandbox_tags_get_v2(
        &self,
        r: Request<pb::SandboxTagsGetRequest>,
    ) -> Resp<pb::SandboxTagsGetResponse> {
        self.v2_auth(&r)?;
        self.sandbox_tags_get(r).await
    }

    async fn sandbox_get_from_name_v2(
        &self,
        r: Request<pb::SandboxGetFromNameRequest>,
    ) -> Resp<pb::SandboxGetFromNameResponse> {
        self.v2_auth(&r)?;
        self.sandbox_get_from_name(r).await
    }

    async fn sandbox_set_name(
        &self,
        r: Request<pb::SandboxSetNameRequest>,
    ) -> Resp<pb::SandboxSetNameResponse> {
        self.v2_auth(&r)?;
        let c = caller(&r)?;
        let q = r.into_inner();
        let rec = self.0.sandbox_of(&c, &q.sandbox_id)?;
        if !rec.running() {
            return Err(errors::conflict("the sandbox is no longer running"));
        }
        match &rec.name {
            Some(n) if n == &q.name => return Ok(Response::new(pb::SandboxSetNameResponse {})),
            Some(_) => return Err(errors::conflict("the sandbox already has a different name")),
            None => {}
        }
        let taken = self.0.db.read(|d| {
            d.sandboxes.values().any(|s| {
                s.key_id == c.key_id()
                    && s.app_id == rec.app_id
                    && s.running()
                    && s.name.as_deref() == Some(&q.name)
            })
        });
        if taken {
            return Err(errors::exists(format!(
                "Sandbox with name '{}' already exists in this app",
                q.name
            )));
        }
        self.0
            .update_sandbox(&rec.id, |s| s.name = Some(q.name.clone()));
        Ok(Response::new(pb::SandboxSetNameResponse {}))
    }

    async fn sandbox_get_command_router_access(
        &self,
        r: Request<pb::SandboxGetCommandRouterAccessRequest>,
    ) -> Resp<pb::SandboxGetCommandRouterAccessResponse> {
        self.v2_auth(&r)?;
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let rec = match q.target {
            Some(pb::sandbox_get_command_router_access_request::Target::SandboxId(id)) => {
                m.sandbox_of(&c, &id)?
            }
            Some(pb::sandbox_get_command_router_access_request::Target::TaskId(t)) => {
                m.db.read(|d| {
                    d.sandboxes
                        .values()
                        .find(|s| s.task_id == t && s.key_id == c.key_id())
                        .cloned()
                })
                .ok_or_else(|| errors::not_found(format!("Task {t} not found")))?
            }
            None => return Err(errors::invalid("a sandbox id or task id is required")),
        };
        if rec.state == SbState::Finished {
            return Err(errors::conflict(format!(
                "Sandbox {} has already finished",
                rec.id
            )));
        }
        Ok(Response::new(pb::SandboxGetCommandRouterAccessResponse {
            jwt: m.signer.mint("tcr", &rec.task_id, m.cfg.jwt_ttl_s),
            url: m.router_url(&c),
            worker_ipv4_address: String::new(),
        }))
    }

    // ---- volumes, dicts, queues, snapshots (see api_objects.rs) --------------

    async fn volume_get_or_create(
        &self,
        r: Request<pb::VolumeGetOrCreateRequest>,
    ) -> Resp<pb::VolumeGetOrCreateResponse> {
        self.h_volume_get_or_create(r).await
    }
    async fn volume_get_by_id(
        &self,
        r: Request<pb::VolumeGetByIdRequest>,
    ) -> Resp<pb::VolumeGetByIdResponse> {
        self.h_volume_get_by_id(r).await
    }
    async fn volume_list(&self, r: Request<pb::VolumeListRequest>) -> Resp<pb::VolumeListResponse> {
        self.h_volume_list(r).await
    }
    async fn volume_delete(&self, r: Request<pb::VolumeDeleteRequest>) -> Resp<()> {
        self.h_volume_delete(r).await
    }
    async fn volume_rename(&self, r: Request<pb::VolumeRenameRequest>) -> Resp<()> {
        self.h_volume_rename(r).await
    }
    async fn volume_commit(
        &self,
        r: Request<pb::VolumeCommitRequest>,
    ) -> Resp<pb::VolumeCommitResponse> {
        self.h_volume_commit(r).await
    }
    async fn volume_reload(&self, r: Request<pb::VolumeReloadRequest>) -> Resp<()> {
        self.h_volume_reload(r).await
    }
    async fn volume_heartbeat(&self, r: Request<pb::VolumeHeartbeatRequest>) -> Resp<()> {
        self.h_volume_heartbeat(r).await
    }
    async fn volume_list_files(
        &self,
        r: Request<pb::VolumeListFilesRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::VolumeListFilesResponse>> {
        self.h_volume_list_files(r).await
    }
    async fn volume_list_files2(
        &self,
        r: Request<pb::VolumeListFiles2Request>,
    ) -> Resp<tonic::codegen::BoxStream<pb::VolumeListFiles2Response>> {
        self.h_volume_list_files2(r).await
    }
    async fn volume_get_file(
        &self,
        r: Request<pb::VolumeGetFileRequest>,
    ) -> Resp<pb::VolumeGetFileResponse> {
        self.h_volume_get_file(r).await
    }
    async fn volume_get_file2(
        &self,
        r: Request<pb::VolumeGetFile2Request>,
    ) -> Resp<pb::VolumeGetFile2Response> {
        self.h_volume_get_file2(r).await
    }
    async fn volume_put_files(&self, r: Request<pb::VolumePutFilesRequest>) -> Resp<()> {
        self.h_volume_put_files(r).await
    }
    async fn volume_remove_file(&self, r: Request<pb::VolumeRemoveFileRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.h_volume_remove_file(c, &q.volume_id, &q.path, q.recursive)
            .await
    }
    async fn volume_remove_file2(&self, r: Request<pb::VolumeRemoveFile2Request>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.h_volume_remove_file(c, &q.volume_id, &q.path, q.recursive)
            .await
    }
    async fn volume_copy_files(&self, r: Request<pb::VolumeCopyFilesRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.h_volume_copy_files(c, &q.volume_id, &q.src_paths, &q.dst_path, q.recursive)
            .await
    }
    async fn volume_copy_files2(&self, r: Request<pb::VolumeCopyFiles2Request>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.h_volume_copy_files(c, &q.volume_id, &q.src_paths, &q.dst_path, q.recursive)
            .await
    }

    async fn dict_get_or_create(
        &self,
        r: Request<pb::DictGetOrCreateRequest>,
    ) -> Resp<pb::DictGetOrCreateResponse> {
        self.h_dict_get_or_create(r).await
    }
    async fn dict_get_by_id(
        &self,
        r: Request<pb::DictGetByIdRequest>,
    ) -> Resp<pb::DictGetByIdResponse> {
        self.h_dict_get_by_id(r).await
    }
    async fn dict_update(&self, r: Request<pb::DictUpdateRequest>) -> Resp<pb::DictUpdateResponse> {
        self.h_dict_update(r).await
    }
    async fn dict_get(&self, r: Request<pb::DictGetRequest>) -> Resp<pb::DictGetResponse> {
        self.h_dict_get(r).await
    }
    async fn dict_contains(
        &self,
        r: Request<pb::DictContainsRequest>,
    ) -> Resp<pb::DictContainsResponse> {
        self.h_dict_contains(r).await
    }
    async fn dict_len(&self, r: Request<pb::DictLenRequest>) -> Resp<pb::DictLenResponse> {
        self.h_dict_len(r).await
    }
    async fn dict_pop(&self, r: Request<pb::DictPopRequest>) -> Resp<pb::DictPopResponse> {
        self.h_dict_pop(r).await
    }
    async fn dict_clear(&self, r: Request<pb::DictClearRequest>) -> Resp<()> {
        self.h_dict_clear(r).await
    }
    async fn dict_contents(
        &self,
        r: Request<pb::DictContentsRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::DictEntry>> {
        self.h_dict_contents(r).await
    }
    async fn dict_delete(&self, r: Request<pb::DictDeleteRequest>) -> Resp<()> {
        self.h_dict_delete(r).await
    }
    async fn dict_list(&self, r: Request<pb::DictListRequest>) -> Resp<pb::DictListResponse> {
        self.h_dict_list(r).await
    }
    async fn dict_heartbeat(&self, r: Request<pb::DictHeartbeatRequest>) -> Resp<()> {
        self.h_dict_heartbeat(r).await
    }

    async fn queue_get_or_create(
        &self,
        r: Request<pb::QueueGetOrCreateRequest>,
    ) -> Resp<pb::QueueGetOrCreateResponse> {
        self.h_queue_get_or_create(r).await
    }
    async fn queue_get_by_id(
        &self,
        r: Request<pb::QueueGetByIdRequest>,
    ) -> Resp<pb::QueueGetByIdResponse> {
        self.h_queue_get_by_id(r).await
    }
    async fn queue_put(&self, r: Request<pb::QueuePutRequest>) -> Resp<()> {
        self.h_queue_put(r).await
    }
    async fn queue_get(&self, r: Request<pb::QueueGetRequest>) -> Resp<pb::QueueGetResponse> {
        self.h_queue_get(r).await
    }
    async fn queue_next_items(
        &self,
        r: Request<pb::QueueNextItemsRequest>,
    ) -> Resp<pb::QueueNextItemsResponse> {
        self.h_queue_next_items(r).await
    }
    async fn queue_len(&self, r: Request<pb::QueueLenRequest>) -> Resp<pb::QueueLenResponse> {
        self.h_queue_len(r).await
    }
    async fn queue_clear(&self, r: Request<pb::QueueClearRequest>) -> Resp<()> {
        self.h_queue_clear(r).await
    }
    async fn queue_delete(&self, r: Request<pb::QueueDeleteRequest>) -> Resp<()> {
        self.h_queue_delete(r).await
    }
    async fn queue_list(&self, r: Request<pb::QueueListRequest>) -> Resp<pb::QueueListResponse> {
        self.h_queue_list(r).await
    }
    async fn queue_heartbeat(&self, r: Request<pb::QueueHeartbeatRequest>) -> Resp<()> {
        self.h_queue_heartbeat(r).await
    }

    async fn sandbox_snapshot(
        &self,
        r: Request<pb::SandboxSnapshotRequest>,
    ) -> Resp<pb::SandboxSnapshotResponse> {
        self.h_sandbox_snapshot(r).await
    }
    async fn sandbox_snapshot_wait(
        &self,
        r: Request<pb::SandboxSnapshotWaitRequest>,
    ) -> Resp<pb::SandboxSnapshotWaitResponse> {
        self.h_sandbox_snapshot_wait(r).await
    }
    async fn sandbox_snapshot_get(
        &self,
        r: Request<pb::SandboxSnapshotGetRequest>,
    ) -> Resp<pb::SandboxSnapshotGetResponse> {
        self.h_sandbox_snapshot_get(r).await
    }
    async fn sandbox_restore(
        &self,
        r: Request<pb::SandboxRestoreRequest>,
    ) -> Resp<pb::SandboxRestoreResponse> {
        self.h_sandbox_restore(r).await
    }
    async fn sandbox_restore_v2(
        &self,
        r: Request<pb::SandboxRestoreV2Request>,
    ) -> Resp<pb::SandboxRestoreV2Response> {
        self.v2_auth(&r)?;
        self.h_sandbox_restore_v2(r).await
    }
    async fn sandbox_snapshot_fs(
        &self,
        r: Request<pb::SandboxSnapshotFsRequest>,
    ) -> Resp<pb::SandboxSnapshotFsResponse> {
        self.h_sandbox_snapshot_fs(r).await
    }
    async fn sandbox_get_exit_snapshot(
        &self,
        r: Request<pb::SandboxGetExitSnapshotRequest>,
    ) -> Resp<pb::SandboxGetExitSnapshotResponse> {
        self.h_sandbox_get_exit_snapshot(r).await
    }
    async fn sandbox_get_exit_snapshot_v2(
        &self,
        r: Request<pb::SandboxGetExitSnapshotRequest>,
    ) -> Resp<pb::SandboxGetExitSnapshotResponse> {
        self.v2_auth(&r)?;
        self.h_sandbox_get_exit_snapshot(r).await
    }

    // ---- environments, app listings, logs (see api_misc.rs) -------------------

    async fn app_list(&self, r: Request<pb::AppListRequest>) -> Resp<pb::AppListResponse> {
        self.h_app_list(r).await
    }
    async fn app_get_by_deployment_name(
        &self,
        r: Request<pb::AppGetByDeploymentNameRequest>,
    ) -> Resp<pb::AppGetByDeploymentNameResponse> {
        self.h_app_get_by_deployment_name(r).await
    }
    async fn environment_create(&self, r: Request<pb::EnvironmentCreateRequest>) -> Resp<()> {
        self.h_environment_create(r).await
    }
    async fn environment_list(&self, r: Request<()>) -> Resp<pb::EnvironmentListResponse> {
        self.h_environment_list(r).await
    }
    async fn environment_delete(&self, r: Request<pb::EnvironmentDeleteRequest>) -> Resp<()> {
        self.h_environment_delete(r).await
    }
    async fn environment_update(
        &self,
        r: Request<pb::EnvironmentUpdateRequest>,
    ) -> Resp<pb::EnvironmentListItem> {
        self.h_environment_update(r).await
    }
    async fn function_precreate(
        &self,
        r: Request<pb::FunctionPrecreateRequest>,
    ) -> Resp<pb::FunctionPrecreateResponse> {
        self.h_function_precreate(r).await
    }
    async fn function_create(
        &self,
        r: Request<pb::FunctionCreateRequest>,
    ) -> Resp<pb::FunctionCreateResponse> {
        self.h_function_create(r).await
    }
    async fn function_get(
        &self,
        r: Request<pb::FunctionGetRequest>,
    ) -> Resp<pb::FunctionGetResponse> {
        self.h_function_get(r).await
    }
    async fn function_get_by_id(
        &self,
        r: Request<pb::FunctionGetByIdRequest>,
    ) -> Resp<pb::FunctionGetByIdResponse> {
        self.h_function_get_by_id(r).await
    }
    async fn class_create(
        &self,
        r: Request<pb::ClassCreateRequest>,
    ) -> Resp<pb::ClassCreateResponse> {
        self.h_class_create(r).await
    }
    async fn class_get(&self, r: Request<pb::ClassGetRequest>) -> Resp<pb::ClassGetResponse> {
        self.h_class_get(r).await
    }
    async fn function_bind_params(
        &self,
        r: Request<pb::FunctionBindParamsRequest>,
    ) -> Resp<pb::FunctionBindParamsResponse> {
        self.h_function_bind_params(r).await
    }
    async fn app_publish(&self, r: Request<pb::AppPublishRequest>) -> Resp<pb::AppPublishResponse> {
        self.h_app_publish(r).await
    }
    async fn app_get_layout(
        &self,
        r: Request<pb::AppGetLayoutRequest>,
    ) -> Resp<pb::AppGetLayoutResponse> {
        self.h_app_get_layout(r).await
    }
    async fn app_get_objects(
        &self,
        r: Request<pb::AppGetObjectsRequest>,
    ) -> Resp<pb::AppGetObjectsResponse> {
        self.h_app_get_objects(r).await
    }
    async fn function_map(
        &self,
        r: Request<pb::FunctionMapRequest>,
    ) -> Resp<pb::FunctionMapResponse> {
        self.h_function_map(r).await
    }
    async fn function_put_inputs(
        &self,
        r: Request<pb::FunctionPutInputsRequest>,
    ) -> Resp<pb::FunctionPutInputsResponse> {
        self.h_function_put_inputs(r).await
    }
    async fn function_get_outputs(
        &self,
        r: Request<pb::FunctionGetOutputsRequest>,
    ) -> Resp<pb::FunctionGetOutputsResponse> {
        self.h_function_get_outputs(r).await
    }
    async fn function_call_from_id(
        &self,
        r: Request<pb::FunctionCallFromIdRequest>,
    ) -> Resp<pb::FunctionCallFromIdResponse> {
        self.h_function_call_from_id(r).await
    }
    async fn function_call_cancel(&self, r: Request<pb::FunctionCallCancelRequest>) -> Resp<()> {
        self.h_function_call_cancel(r).await
    }
    async fn function_retry_inputs(
        &self,
        r: Request<pb::FunctionRetryInputsRequest>,
    ) -> Resp<pb::FunctionRetryInputsResponse> {
        self.h_function_retry_inputs(r).await
    }
    async fn function_call_get_info(
        &self,
        r: Request<pb::FunctionCallGetInfoRequest>,
    ) -> Resp<pb::FunctionCallGetInfoResponse> {
        self.h_function_call_get_info(r).await
    }
    async fn function_get_current_stats(
        &self,
        r: Request<pb::FunctionGetCurrentStatsRequest>,
    ) -> Resp<pb::FunctionStats> {
        self.h_function_get_current_stats(r).await
    }
    async fn function_get_call_graph(
        &self,
        r: Request<pb::FunctionGetCallGraphRequest>,
    ) -> Resp<pb::FunctionGetCallGraphResponse> {
        self.h_function_get_call_graph(r).await
    }
    async fn function_get_scheduling_params(
        &self,
        r: Request<pb::FunctionGetSchedulingParamsRequest>,
    ) -> Resp<pb::FunctionGetSchedulingParamsResponse> {
        self.h_function_get_scheduling_params(r).await
    }
    async fn function_update_scheduling_params(
        &self,
        r: Request<pb::FunctionUpdateSchedulingParamsRequest>,
    ) -> Resp<pb::FunctionUpdateSchedulingParamsResponse> {
        self.h_function_update_scheduling_params(r).await
    }
    async fn task_list(&self, r: Request<pb::TaskListRequest>) -> Resp<pb::TaskListResponse> {
        self.h_task_list(r).await
    }
    async fn container_heartbeat(
        &self,
        r: Request<pb::ContainerHeartbeatRequest>,
    ) -> Resp<pb::ContainerHeartbeatResponse> {
        self.h_container_heartbeat(r).await
    }
    async fn function_get_inputs(
        &self,
        r: Request<pb::FunctionGetInputsRequest>,
    ) -> Resp<pb::FunctionGetInputsResponse> {
        self.h_function_get_inputs(r).await
    }
    async fn function_put_outputs(&self, r: Request<pb::FunctionPutOutputsRequest>) -> Resp<()> {
        self.h_function_put_outputs(r).await
    }
    async fn function_call_put_data_out(
        &self,
        r: Request<pb::FunctionCallPutDataRequest>,
    ) -> Resp<()> {
        self.h_function_call_put_data_out(r).await
    }
    async fn function_call_get_data_out(
        &self,
        r: Request<pb::FunctionCallGetDataRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::DataChunk>> {
        self.h_function_call_get_data_out(r).await
    }
    async fn container_stop(
        &self,
        r: Request<pb::ContainerStopRequest>,
    ) -> Resp<pb::ContainerStopResponse> {
        self.h_container_stop(r).await
    }
    async fn task_result(&self, r: Request<pb::TaskResultRequest>) -> Resp<()> {
        self.h_task_result(r).await
    }
    async fn function_get_dynamic_concurrency(
        &self,
        r: Request<pb::FunctionGetDynamicConcurrencyRequest>,
    ) -> Resp<pb::FunctionGetDynamicConcurrencyResponse> {
        self.h_function_get_dynamic_concurrency(r).await
    }
    async fn function_finish_inputs(
        &self,
        r: Request<pb::FunctionFinishInputsRequest>,
    ) -> Resp<()> {
        self.h_function_finish_inputs(r).await
    }
    async fn container_hello(&self, _r: Request<()>) -> Resp<()> {
        Ok(Response::new(()))
    }
    async fn app_fetch_logs(
        &self,
        r: Request<pb::AppFetchLogsRequest>,
    ) -> Resp<pb::AppFetchLogsResponse> {
        self.h_app_fetch_logs(r).await
    }
    async fn app_count_logs(
        &self,
        r: Request<pb::AppCountLogsRequest>,
    ) -> Resp<pb::AppCountLogsResponse> {
        self.h_app_count_logs(r).await
    }
    async fn app_get_logs(
        &self,
        r: Request<pb::AppGetLogsRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::TaskLogsBatch>> {
        self.h_app_get_logs(r).await
    }
    async fn token_info_get(
        &self,
        r: Request<pb::TokenInfoGetRequest>,
    ) -> Resp<pb::TokenInfoGetResponse> {
        self.h_token_info_get(r).await
    }
}

/// Secrets become environment variables: names are letters, digits and
/// underscores, not starting with a digit.
pub fn validate_env_keys<'a>(keys: impl IntoIterator<Item = &'a String>) -> Result<(), Status> {
    for k in keys {
        if k.is_empty() {
            return Err(errors::invalid("Secret key name cannot be empty"));
        }
        let mut cs = k.chars();
        let first_ok = cs
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if !first_ok || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(errors::invalid(format!(
                "Secret key name {k:?} is invalid for environment variables. Only letters, numbers, and underscores are allowed."
            )));
        }
    }
    Ok(())
}

fn secret_meta(s: &SecretRec) -> pb::SecretMetadata {
    pb::SecretMetadata {
        name: s.name.clone().unwrap_or_default(),
        creation_info: Some(creation(s.created_at)),
        keys: s.env.keys().cloned().collect(),
        environment_name: s.environment.clone(),
    }
}

pub fn image_meta(b: &crate::modal::image::Built) -> pb::ImageMetadata {
    pb::ImageMetadata {
        python_version_info: b.python.clone(),
        python_packages: Default::default(),
        workdir: b.workdir.clone(),
        libc_version_info: Some("glibc 2.39".into()),
        image_builder_version: Some(BUILDER_VERSION.into()),
    }
}

impl Api {
    /// The V2 RPCs carry the auth token `AuthTokenGet` issued.
    fn v2_auth<T>(&self, r: &Request<T>) -> Result<(), Status> {
        let token = r
            .metadata()
            .get("x-modal-auth-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        self.0
            .signer
            .verify(token, "api")
            .map(|_| ())
            .map_err(|why| Status::unauthenticated(format!("invalid auth token: {why}")))
    }

    fn secret_of(&self, c: &Caller, id: &str) -> Result<SecretRec, Status> {
        self.0
            .db
            .read(|d| {
                d.secrets
                    .get(id)
                    .filter(|s| s.key_id == c.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("Secret {id} not found")))
    }

    /// Stops an app and ends its sandboxes. A client disconnecting only
    /// stops an ephemeral app (`force`: an explicit stop of any app).
    async fn stop_app(&self, c: &Caller, id: &str, force: bool) {
        let stop = self.0.db.write(|d| match d.apps.get_mut(id) {
            Some(a) if a.key_id == c.key_id() && !a.stopped && (a.ephemeral || force) => {
                a.stopped = true;
                true
            }
            _ => false,
        });
        if stop {
            // An ephemeral app takes its sandboxes with it.
            let ids: Vec<String> = self.0.db.read(|d| {
                d.sandboxes
                    .values()
                    .filter(|s| s.app_id == id && s.running())
                    .map(|s| s.id.clone())
                    .collect()
            });
            for sb in ids {
                self.0.terminate(&sb).await;
            }
        }
    }

    fn mount_get_or_create_impl(
        &self,
        c: &Caller,
        q: pb::MountGetOrCreateRequest,
    ) -> Resp<pb::MountGetOrCreateResponse> {
        use crate::modal::db::{MountFileRec, MountRec};
        let m = &self.0;
        // Global mounts the client names (`python-build-standalone-*`) are
        // not files: recognised by name and handled by the image builder.
        let files: Vec<MountFileRec> = q
            .files
            .iter()
            .map(|f| MountFileRec {
                filename: f.filename.clone(),
                sha256: f.sha256_hex.clone(),
                size: f.size.unwrap_or(0),
                mode: f.mode,
            })
            .collect();
        for f in &files {
            if !m.blobs.has_sha(&f.sha256) {
                return Err(errors::not_found(format!(
                    "Mount file {} (sha256 {}) has not been uploaded",
                    f.filename, f.sha256
                )));
            }
        }
        let name = (!q.deployment_name.is_empty()).then(|| q.deployment_name.clone());
        let id = m.db.write(|d| {
            if let Some(n) = &name
                && let Some(x) = d
                    .mounts
                    .values()
                    .find(|x| x.key_id == c.key_id() && x.name.as_ref() == Some(n))
                && q.files.is_empty()
            {
                return x.id.clone();
            }
            let rec = MountRec {
                id: ids::new_id("mo"),
                key_id: c.key_id(),
                name: name.clone(),
                created_at: now_s(),
                files,
            };
            let id = rec.id.clone();
            d.mounts.insert(id.clone(), rec);
            id
        });
        use sha2::Digest;
        let checksum = m.db.read(|d| {
            let mut h = sha2::Sha256::new();
            if let Some(x) = d.mounts.get(&id) {
                for f in &x.files {
                    h.update(f.filename.as_bytes());
                    h.update(f.sha256.as_bytes());
                }
            }
            hex::encode(h.finalize())
        });
        Ok(Response::new(pb::MountGetOrCreateResponse {
            mount_id: id,
            handle_metadata: Some(pb::MountHandleMetadata {
                content_checksum_sha256_hex: checksum,
            }),
        }))
    }
}
