//! The control-plane handlers for volumes, dicts, queues and snapshots
//! (the trait impl in `api.rs` delegates here).

use crate::modal::api::{Api, secs};
use crate::modal::db::{DictRec, QueueRec, now_s};
use crate::modal::pb::api as pb;
use crate::modal::sandbox::{SbState, status as gs};
use crate::modal::volumes::{BLOCK_SIZE, rel_path};
use crate::modal::{Caller, errors, ids};
use tonic::{Request, Response, Status};

type Resp<T> = Result<Response<T>, Status>;

const QUEUE_PARTITION_LIMIT: usize = 5000;

fn caller<T>(r: &Request<T>) -> Result<Caller, Status> {
    r.extensions()
        .get::<Caller>()
        .cloned()
        .ok_or_else(|| Status::unauthenticated("Token missing. Could not authenticate client."))
}

fn env_name(s: &str) -> String {
    if s.is_empty() {
        "main".into()
    } else {
        s.to_string()
    }
}

fn creation(t: f64) -> pb::CreationInfo {
    pb::CreationInfo {
        created_at: t,
        created_by: "cua".into(),
    }
}

fn vol_meta(v: &crate::modal::db::VolumeRec) -> pb::VolumeMetadata {
    pb::VolumeMetadata {
        version: pb::VolumeFsVersion::V1 as i32,
        name: v.name.clone().unwrap_or_default(),
        creation_info: Some(creation(v.created_at)),
    }
}

impl Api {
    // ---- volumes ----------------------------------------------------------

    pub(super) async fn h_volume_get_or_create(
        &self,
        r: Request<pb::VolumeGetOrCreateRequest>,
    ) -> Resp<pb::VolumeGetOrCreateResponse> {
        use pb::ObjectCreationType as T;
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        if q.version == pb::VolumeFsVersion::V2 as i32 {
            return Err(errors::invalid(
                "Volumes on Cua Volume are version 1: the version 2 block protocol is not served (omit version=)",
            ));
        }
        let kind = T::try_from(q.object_creation_type).unwrap_or(T::Unspecified);
        let env = env_name(&q.environment_name);
        let named = !q.deployment_name.is_empty();
        if named && !matches!(kind, T::Ephemeral | T::AnonymousOwnedByApp) {
            let existing = m.db.read(|d| {
                d.volumes
                    .values()
                    .find(|v| {
                        v.key_id == c.key_id()
                            && v.name.as_deref() == Some(&q.deployment_name)
                            && v.environment == env
                    })
                    .cloned()
            });
            match (existing, kind) {
                (Some(_), T::CreateFailIfExists) => {
                    return Err(errors::exists(format!(
                        "Volume '{}' already exists",
                        q.deployment_name
                    )));
                }
                (Some(v), T::CreateOverwriteIfExists) => {
                    m.vol_purge(&c, &v).await;
                    m.init_volume(&c, &v).await?;
                    return Ok(Response::new(pb::VolumeGetOrCreateResponse {
                        volume_id: v.id.clone(),
                        version: pb::VolumeFsVersion::V1 as i32,
                        metadata: Some(vol_meta(&v)),
                    }));
                }
                (Some(v), _) => {
                    return Ok(Response::new(pb::VolumeGetOrCreateResponse {
                        volume_id: v.id.clone(),
                        version: pb::VolumeFsVersion::V1 as i32,
                        metadata: Some(vol_meta(&v)),
                    }));
                }
                (None, T::CreateIfMissing | T::CreateFailIfExists | T::CreateOverwriteIfExists) => {
                }
                (None, _) => {
                    return Err(errors::not_found(format!(
                        "Volume '{}' not found in environment '{}'",
                        q.deployment_name, env
                    )));
                }
            }
        }
        let v = crate::modal::Modal::new_volume_rec(
            &c,
            named.then(|| q.deployment_name.clone()),
            &env,
            1,
        );
        m.init_volume(&c, &v).await?;
        m.db.write(|d| d.volumes.insert(v.id.clone(), v.clone()));
        Ok(Response::new(pb::VolumeGetOrCreateResponse {
            volume_id: v.id.clone(),
            version: pb::VolumeFsVersion::V1 as i32,
            metadata: Some(vol_meta(&v)),
        }))
    }

    pub(super) async fn h_volume_get_by_id(
        &self,
        r: Request<pb::VolumeGetByIdRequest>,
    ) -> Resp<pb::VolumeGetByIdResponse> {
        let c = caller(&r)?;
        let v = self.0.volume_of(&c, &r.into_inner().volume_id)?;
        Ok(Response::new(pb::VolumeGetByIdResponse {
            volume_id: v.id.clone(),
            metadata: Some(vol_meta(&v)),
        }))
    }

    pub(super) async fn h_volume_list(
        &self,
        r: Request<pb::VolumeListRequest>,
    ) -> Resp<pb::VolumeListResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let (max, before) = q
            .pagination
            .map(|p| (p.max_objects, p.created_before))
            .unwrap_or((0, 0.0));
        let mut v: Vec<_> = self.0.db.read(|d| {
            d.volumes
                .values()
                .filter(|v| v.key_id == c.key_id() && v.name.is_some())
                .filter(|v| before == 0.0 || v.created_at < before)
                .cloned()
                .collect()
        });
        v.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        if max > 0 {
            v.truncate(max as usize);
        }
        Ok(Response::new(pb::VolumeListResponse {
            items: v
                .iter()
                .map(|v| pb::VolumeListItem {
                    label: v.name.clone().unwrap_or_default(),
                    volume_id: v.id.clone(),
                    created_at: v.created_at,
                    metadata: Some(vol_meta(v)),
                })
                .collect(),
            environment_name: q.environment_name,
        }))
    }

    pub(super) async fn h_volume_delete(&self, r: Request<pb::VolumeDeleteRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let v = self.0.volume_of(&c, &r.into_inner().volume_id)?;
        let mounted = self.0.db.read(|d| {
            d.sandboxes
                .values()
                .any(|s| s.running() && s.def().volume_mounts.iter().any(|m| m.volume_id == v.id))
        });
        if mounted {
            return Err(errors::conflict(format!(
                "Volume {} is mounted in a running Sandbox",
                v.id
            )));
        }
        self.0.vol_purge(&c, &v).await;
        self.0.db.write(|d| d.volumes.remove(&v.id));
        Ok(Response::new(()))
    }

    pub(super) async fn h_volume_rename(&self, r: Request<pb::VolumeRenameRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let v = self.0.volume_of(&c, &q.volume_id)?;
        let taken = self.0.db.read(|d| {
            d.volumes.values().any(|x| {
                x.key_id == c.key_id()
                    && x.id != v.id
                    && x.name.as_deref() == Some(&q.name)
                    && x.environment == v.environment
            })
        });
        if taken {
            return Err(errors::exists(format!(
                "Volume '{}' already exists",
                q.name
            )));
        }
        // The folder keeps its name; the volume's name is what changes.
        self.0.db.write(|d| {
            if let Some(x) = d.volumes.get_mut(&v.id) {
                x.name = Some(q.name.clone());
            }
        });
        Ok(Response::new(()))
    }

    pub(super) async fn h_volume_commit(
        &self,
        r: Request<pb::VolumeCommitRequest>,
    ) -> Resp<pb::VolumeCommitResponse> {
        let c = caller(&r)?;
        let v = self.0.volume_of(&c, &r.into_inner().volume_id)?;
        self.0.db.write(|d| {
            if let Some(x) = d.volumes.get_mut(&v.id) {
                x.commits += 1;
            }
        });
        // Cua Volume is consistent at once: there is nothing to reload.
        Ok(Response::new(pb::VolumeCommitResponse {
            skip_reload: true,
        }))
    }

    pub(super) async fn h_volume_reload(&self, r: Request<pb::VolumeReloadRequest>) -> Resp<()> {
        let c = caller(&r)?;
        self.0.volume_of(&c, &r.into_inner().volume_id)?;
        Ok(Response::new(()))
    }

    pub(super) async fn h_volume_heartbeat(
        &self,
        r: Request<pb::VolumeHeartbeatRequest>,
    ) -> Resp<()> {
        let c = caller(&r)?;
        let v = self.0.volume_of(&c, &r.into_inner().volume_id)?;
        self.0.db.write(|d| {
            if let Some(x) = d.volumes.get_mut(&v.id) {
                x.heartbeat_at = now_s();
            }
        });
        Ok(Response::new(()))
    }

    pub(super) async fn h_volume_list_files(
        &self,
        r: Request<pb::VolumeListFilesRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::VolumeListFilesResponse>> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let v = self.0.volume_of(&c, &q.volume_id)?;
        let entries = self
            .0
            .vol_list(&c, &v, &q.path, q.recursive, q.max_entries)
            .await?;
        let batches: Vec<Result<pb::VolumeListFilesResponse, Status>> = entries
            .chunks(1000)
            .map(|c| {
                Ok(pb::VolumeListFilesResponse {
                    entries: c.to_vec(),
                })
            })
            .collect();
        Ok(Response::new(Box::pin(futures_util::stream::iter(batches))))
    }

    pub(super) async fn h_volume_list_files2(
        &self,
        r: Request<pb::VolumeListFiles2Request>,
    ) -> Resp<tonic::codegen::BoxStream<pb::VolumeListFiles2Response>> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let v = self.0.volume_of(&c, &q.volume_id)?;
        let entries = self
            .0
            .vol_list(&c, &v, &q.path, q.recursive, q.max_entries)
            .await?;
        let batches: Vec<Result<pb::VolumeListFiles2Response, Status>> = entries
            .chunks(1000)
            .map(|c| {
                Ok(pb::VolumeListFiles2Response {
                    entries: c.to_vec(),
                })
            })
            .collect();
        Ok(Response::new(Box::pin(futures_util::stream::iter(batches))))
    }

    pub(super) async fn h_volume_get_file(
        &self,
        r: Request<pb::VolumeGetFileRequest>,
    ) -> Resp<pb::VolumeGetFileResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let v = self.0.volume_of(&c, &q.volume_id)?;
        let rel = rel_path(&q.path)?;
        let data = self.0.vol_read(&c, &v, &rel).await?;
        let size = data.len() as u64;
        let start = q.start.min(size);
        let len = if q.len == 0 {
            size - start
        } else {
            q.len.min(size - start)
        };
        let slice = data[start as usize..(start + len) as usize].to_vec();
        Ok(Response::new(pb::VolumeGetFileResponse {
            data_oneof: Some(pb::volume_get_file_response::DataOneof::Data(slice.into())),
            size,
            start,
            len,
        }))
    }

    pub(super) async fn h_volume_get_file2(
        &self,
        r: Request<pb::VolumeGetFile2Request>,
    ) -> Resp<pb::VolumeGetFile2Response> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let v = m.volume_of(&c, &q.volume_id)?;
        let rel = rel_path(&q.path)?;
        let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        let entries = m
            .vol_list(&c, &v, parent, false, None)
            .await
            .unwrap_or_default();
        let Some(e) = entries
            .iter()
            .find(|e| e.path == rel && e.r#type == pb::file_entry::FileType::File as i32)
        else {
            let _ = name;
            return Err(errors::not_found(format!(
                "No such file or directory: {}",
                q.path
            )));
        };
        let size = e.size;
        let start = q.start.min(size);
        let len = if q.len == 0 {
            size - start
        } else {
            q.len.min(size - start)
        };
        let mut urls = Vec::new();
        if len > 0 {
            let first = start / BLOCK_SIZE;
            let last = (start + len - 1) / BLOCK_SIZE;
            for b in first..=last {
                let lo = (b * BLOCK_SIZE).max(start);
                let hi = ((b + 1) * BLOCK_SIZE).min(start + len);
                let tok = m.signer.mint_with(
                    "vol",
                    &v.id,
                    600,
                    serde_json::json!({"path": rel, "lo": lo, "hi": hi, "key": c.key_id()}),
                );
                urls.push(format!("{}/_cua/vol/{tok}", m.cfg.api_url));
            }
        }
        Ok(Response::new(pb::VolumeGetFile2Response {
            get_urls: urls,
            size,
            start,
            len,
        }))
    }

    pub(super) async fn h_volume_put_files(
        &self,
        r: Request<pb::VolumePutFilesRequest>,
    ) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let v = m.volume_of(&c, &q.volume_id)?;
        // Check everything before writing anything.
        let mut jobs = Vec::new();
        for f in &q.files {
            let rel = rel_path(&f.filename)?;
            if rel.is_empty() {
                return Err(errors::invalid("a file needs a path"));
            }
            if q.disallow_overwrite_existing_files && m.vol_exists(&c, &v, &rel).await {
                return Err(errors::exists(format!("File exists: {}", f.filename)));
            }
            let data = m.blobs.get_sha(&f.sha256_hex).ok_or_else(|| {
                errors::not_found(format!(
                    "File {} (sha256 {}) has not been uploaded",
                    f.filename, f.sha256_hex
                ))
            })?;
            jobs.push((rel, data));
        }
        for (rel, data) in jobs {
            m.backend
                .volume_write(&c.identity(), &crate::modal::Modal::vol_key(&v, &rel), data)
                .await
                .map_err(errors::from_backend)?;
        }
        Ok(Response::new(()))
    }

    pub(super) async fn h_volume_remove_file(
        &self,
        c: Caller,
        volume_id: &str,
        path: &str,
        recursive: bool,
    ) -> Resp<()> {
        let v = self.0.volume_of(&c, volume_id)?;
        let rel = rel_path(path)?;
        self.0.vol_remove(&c, &v, &rel, recursive).await?;
        Ok(Response::new(()))
    }

    pub(super) async fn h_volume_copy_files(
        &self,
        c: Caller,
        volume_id: &str,
        srcs: &[String],
        dst: &str,
        recursive: bool,
    ) -> Resp<()> {
        let v = self.0.volume_of(&c, volume_id)?;
        self.0.vol_copy(&c, &v, srcs, dst, recursive).await?;
        Ok(Response::new(()))
    }

    // ---- dicts ------------------------------------------------------------

    fn dict_of(&self, c: &Caller, id: &str) -> Result<DictRec, Status> {
        self.0
            .db
            .read(|d| d.dicts.get(id).filter(|x| x.key_id == c.key_id()).cloned())
            .ok_or_else(|| errors::not_found(format!("Dict {id} not found")))
    }

    pub(super) async fn h_dict_get_or_create(
        &self,
        r: Request<pb::DictGetOrCreateRequest>,
    ) -> Resp<pb::DictGetOrCreateResponse> {
        use pb::ObjectCreationType as T;
        let c = caller(&r)?;
        let q = r.into_inner();
        let kind = T::try_from(q.object_creation_type).unwrap_or(T::Unspecified);
        let env = env_name(&q.environment_name);
        let named = !q.deployment_name.is_empty();
        let rec = self.0.db.write(|d| {
            if named && !matches!(kind, T::Ephemeral | T::AnonymousOwnedByApp) {
                let existing = d
                    .dicts
                    .values()
                    .find(|x| {
                        x.key_id == c.key_id()
                            && x.name.as_deref() == Some(&q.deployment_name)
                            && x.environment == env
                    })
                    .cloned();
                match (existing, kind) {
                    (Some(_), T::CreateFailIfExists) => {
                        return Err(errors::exists(format!(
                            "Dict '{}' already exists",
                            q.deployment_name
                        )));
                    }
                    (Some(mut x), T::CreateOverwriteIfExists) => {
                        x.items = q
                            .data
                            .iter()
                            .map(|e| (hex::encode(&e.key), hex::encode(&e.value)))
                            .collect();
                        d.dicts.insert(x.id.clone(), x.clone());
                        return Ok(x);
                    }
                    (Some(x), _) => return Ok(x),
                    (
                        None,
                        T::CreateIfMissing | T::CreateFailIfExists | T::CreateOverwriteIfExists,
                    ) => {}
                    (None, _) => {
                        return Err(errors::not_found(format!(
                            "Dict '{}' not found in environment '{}'",
                            q.deployment_name, env
                        )));
                    }
                }
            }
            let x = DictRec {
                id: ids::new_id("di"),
                key_id: c.key_id(),
                name: named.then(|| q.deployment_name.clone()),
                environment: env.clone(),
                created_at: now_s(),
                app_id: None,
                items: q
                    .data
                    .iter()
                    .map(|e| (hex::encode(&e.key), hex::encode(&e.value)))
                    .collect(),
                heartbeat_at: now_s(),
                ephemeral: kind == T::Ephemeral,
            };
            d.dicts.insert(x.id.clone(), x.clone());
            Ok(x)
        })?;
        Ok(Response::new(pb::DictGetOrCreateResponse {
            dict_id: rec.id.clone(),
            metadata: Some(pb::DictMetadata {
                name: rec.name.clone().unwrap_or_default(),
                creation_info: Some(creation(rec.created_at)),
            }),
        }))
    }

    pub(super) async fn h_dict_get_by_id(
        &self,
        r: Request<pb::DictGetByIdRequest>,
    ) -> Resp<pb::DictGetByIdResponse> {
        let c = caller(&r)?;
        let x = self.dict_of(&c, &r.into_inner().dict_id)?;
        Ok(Response::new(pb::DictGetByIdResponse {
            dict_id: x.id.clone(),
            metadata: Some(pb::DictMetadata {
                name: x.name.clone().unwrap_or_default(),
                creation_info: Some(creation(x.created_at)),
            }),
        }))
    }

    pub(super) async fn h_dict_update(
        &self,
        r: Request<pb::DictUpdateRequest>,
    ) -> Resp<pb::DictUpdateResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.dict_of(&c, &q.dict_id)?;
        let created = self.0.db.write(|d| {
            let Some(x) = d.dicts.get_mut(&q.dict_id) else {
                return false;
            };
            let mut created = false;
            for e in &q.updates {
                let (k, v) = (hex::encode(&e.key), hex::encode(&e.value));
                match x.items.iter_mut().find(|(ek, _)| *ek == k) {
                    Some(slot) => {
                        if !q.if_not_exists {
                            slot.1 = v;
                        }
                    }
                    None => {
                        x.items.push((k, v));
                        created = true;
                    }
                }
            }
            created
        });
        Ok(Response::new(pb::DictUpdateResponse { created }))
    }

    pub(super) async fn h_dict_get(
        &self,
        r: Request<pb::DictGetRequest>,
    ) -> Resp<pb::DictGetResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let x = self.dict_of(&c, &q.dict_id)?;
        let k = hex::encode(&q.key);
        Ok(Response::new(
            match x.items.iter().find(|(ek, _)| *ek == k) {
                Some((_, v)) => pb::DictGetResponse {
                    found: true,
                    value: hex::decode(v).ok().map(Into::into),
                },
                None => pb::DictGetResponse {
                    found: false,
                    value: None,
                },
            },
        ))
    }

    pub(super) async fn h_dict_contains(
        &self,
        r: Request<pb::DictContainsRequest>,
    ) -> Resp<pb::DictContainsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let x = self.dict_of(&c, &q.dict_id)?;
        let k = hex::encode(&q.key);
        Ok(Response::new(pb::DictContainsResponse {
            found: x.items.iter().any(|(ek, _)| *ek == k),
        }))
    }

    pub(super) async fn h_dict_len(
        &self,
        r: Request<pb::DictLenRequest>,
    ) -> Resp<pb::DictLenResponse> {
        let c = caller(&r)?;
        let x = self.dict_of(&c, &r.into_inner().dict_id)?;
        Ok(Response::new(pb::DictLenResponse {
            len: x.items.len() as i32,
        }))
    }

    pub(super) async fn h_dict_pop(
        &self,
        r: Request<pb::DictPopRequest>,
    ) -> Resp<pb::DictPopResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.dict_of(&c, &q.dict_id)?;
        let k = hex::encode(&q.key);
        let got = self.0.db.write(|d| {
            let x = d.dicts.get_mut(&q.dict_id)?;
            let i = x.items.iter().position(|(ek, _)| *ek == k)?;
            Some(x.items.remove(i).1)
        });
        Ok(Response::new(match got {
            Some(v) => pb::DictPopResponse {
                found: true,
                value: hex::decode(v).ok().map(Into::into),
            },
            None => pb::DictPopResponse {
                found: false,
                value: None,
            },
        }))
    }

    pub(super) async fn h_dict_clear(&self, r: Request<pb::DictClearRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().dict_id;
        self.dict_of(&c, &id)?;
        self.0.db.write(|d| {
            if let Some(x) = d.dicts.get_mut(&id) {
                x.items.clear();
            }
        });
        Ok(Response::new(()))
    }

    pub(super) async fn h_dict_contents(
        &self,
        r: Request<pb::DictContentsRequest>,
    ) -> Resp<tonic::codegen::BoxStream<pb::DictEntry>> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let x = self.dict_of(&c, &q.dict_id)?;
        let out: Vec<Result<pb::DictEntry, Status>> = x
            .items
            .iter()
            .map(|(k, v)| {
                Ok(pb::DictEntry {
                    key: if q.keys {
                        hex::decode(k).unwrap_or_default().into()
                    } else {
                        Default::default()
                    },
                    value: if q.values {
                        hex::decode(v).unwrap_or_default().into()
                    } else {
                        Default::default()
                    },
                })
            })
            .collect();
        Ok(Response::new(Box::pin(futures_util::stream::iter(out))))
    }

    pub(super) async fn h_dict_delete(&self, r: Request<pb::DictDeleteRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().dict_id;
        self.dict_of(&c, &id)?;
        self.0.db.write(|d| d.dicts.remove(&id));
        Ok(Response::new(()))
    }

    pub(super) async fn h_dict_list(
        &self,
        r: Request<pb::DictListRequest>,
    ) -> Resp<pb::DictListResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let (max, before) = q
            .pagination
            .map(|p| (p.max_objects, p.created_before))
            .unwrap_or((0, 0.0));
        let mut v: Vec<DictRec> = self.0.db.read(|d| {
            d.dicts
                .values()
                .filter(|x| x.key_id == c.key_id() && x.name.is_some())
                .filter(|x| before == 0.0 || x.created_at < before)
                .cloned()
                .collect()
        });
        v.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        if max > 0 {
            v.truncate(max as usize);
        }
        Ok(Response::new(pb::DictListResponse {
            dicts: v
                .iter()
                .map(|x| pb::dict_list_response::DictInfo {
                    name: x.name.clone().unwrap_or_default(),
                    created_at: x.created_at,
                    dict_id: x.id.clone(),
                    metadata: Some(pb::DictMetadata {
                        name: x.name.clone().unwrap_or_default(),
                        creation_info: Some(creation(x.created_at)),
                    }),
                })
                .collect(),
            environment_name: q.environment_name,
        }))
    }

    pub(super) async fn h_dict_heartbeat(&self, r: Request<pb::DictHeartbeatRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().dict_id;
        self.dict_of(&c, &id)?;
        self.0.db.write(|d| {
            if let Some(x) = d.dicts.get_mut(&id) {
                x.heartbeat_at = now_s();
            }
        });
        Ok(Response::new(()))
    }

    // ---- queues -----------------------------------------------------------

    fn queue_of(&self, c: &Caller, id: &str) -> Result<QueueRec, Status> {
        self.0
            .db
            .read(|d| d.queues.get(id).filter(|x| x.key_id == c.key_id()).cloned())
            .ok_or_else(|| errors::not_found(format!("Queue {id} not found")))
    }

    pub(super) async fn h_queue_get_or_create(
        &self,
        r: Request<pb::QueueGetOrCreateRequest>,
    ) -> Resp<pb::QueueGetOrCreateResponse> {
        use pb::ObjectCreationType as T;
        let c = caller(&r)?;
        let q = r.into_inner();
        let kind = T::try_from(q.object_creation_type).unwrap_or(T::Unspecified);
        let env = env_name(&q.environment_name);
        let named = !q.deployment_name.is_empty();
        let rec = self.0.db.write(|d| {
            if named && !matches!(kind, T::Ephemeral | T::AnonymousOwnedByApp) {
                let existing = d
                    .queues
                    .values()
                    .find(|x| {
                        x.key_id == c.key_id()
                            && x.name.as_deref() == Some(&q.deployment_name)
                            && x.environment == env
                    })
                    .cloned();
                match (existing, kind) {
                    (Some(_), T::CreateFailIfExists) => {
                        return Err(errors::exists(format!(
                            "Queue '{}' already exists",
                            q.deployment_name
                        )));
                    }
                    (Some(x), _) => return Ok(x),
                    (
                        None,
                        T::CreateIfMissing | T::CreateFailIfExists | T::CreateOverwriteIfExists,
                    ) => {}
                    (None, _) => {
                        return Err(errors::not_found(format!(
                            "Queue '{}' not found in environment '{}'",
                            q.deployment_name, env
                        )));
                    }
                }
            }
            let x = QueueRec {
                id: ids::new_id("qu"),
                key_id: c.key_id(),
                name: named.then(|| q.deployment_name.clone()),
                environment: env.clone(),
                created_at: now_s(),
                app_id: None,
                partitions: Default::default(),
                next_entry: 1,
                heartbeat_at: now_s(),
                ephemeral: kind == T::Ephemeral,
            };
            d.queues.insert(x.id.clone(), x.clone());
            Ok(x)
        })?;
        Ok(Response::new(pb::QueueGetOrCreateResponse {
            queue_id: rec.id.clone(),
            metadata: Some(pb::QueueMetadata {
                name: rec.name.clone().unwrap_or_default(),
                creation_info: Some(creation(rec.created_at)),
            }),
        }))
    }

    pub(super) async fn h_queue_get_by_id(
        &self,
        r: Request<pb::QueueGetByIdRequest>,
    ) -> Resp<pb::QueueGetByIdResponse> {
        let c = caller(&r)?;
        let x = self.queue_of(&c, &r.into_inner().queue_id)?;
        Ok(Response::new(pb::QueueGetByIdResponse {
            queue_id: x.id.clone(),
            metadata: Some(pb::QueueMetadata {
                name: x.name.clone().unwrap_or_default(),
                creation_info: Some(creation(x.created_at)),
            }),
        }))
    }

    pub(super) async fn h_queue_put(&self, r: Request<pb::QueuePutRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.queue_of(&c, &q.queue_id)?;
        let part = hex::encode(&q.partition_key);
        let full = self.0.db.write(|d| {
            let Some(x) = d.queues.get_mut(&q.queue_id) else {
                return false;
            };
            let len = x.partitions.get(&part).map_or(0, Vec::len);
            if len + q.values.len() > QUEUE_PARTITION_LIMIT {
                return true;
            }
            for v in &q.values {
                let id = x.next_entry;
                x.next_entry += 1;
                x.partitions
                    .entry(part.clone())
                    .or_default()
                    .push((id, hex::encode(v)));
            }
            false
        });
        if full {
            return Err(errors::exhausted(format!(
                "Queue partition is full ({QUEUE_PARTITION_LIMIT} items)"
            )));
        }
        Ok(Response::new(()))
    }

    pub(super) async fn h_queue_get(
        &self,
        r: Request<pb::QueueGetRequest>,
    ) -> Resp<pb::QueueGetResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.queue_of(&c, &q.queue_id)?;
        let part = hex::encode(&q.partition_key);
        let deadline = tokio::time::Instant::now() + secs(q.timeout);
        let n = q.n_values.max(1) as usize;
        loop {
            let got = self.0.db.write(|d| {
                let x = d.queues.get_mut(&q.queue_id)?;
                let p = x.partitions.get_mut(&part)?;
                if p.is_empty() {
                    return None;
                }
                let take = n.min(p.len());
                Some(p.drain(..take).map(|(_, v)| v).collect::<Vec<_>>())
            });
            if let Some(vals) = got {
                return Ok(Response::new(pb::QueueGetResponse {
                    values: vals
                        .iter()
                        .filter_map(|v| hex::decode(v).ok())
                        .map(Into::into)
                        .collect(),
                }));
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(Response::new(pb::QueueGetResponse { values: vec![] }));
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub(super) async fn h_queue_next_items(
        &self,
        r: Request<pb::QueueNextItemsRequest>,
    ) -> Resp<pb::QueueNextItemsResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.queue_of(&c, &q.queue_id)?;
        let part = hex::encode(&q.partition_key);
        let after: u64 = q.last_entry_id.parse().unwrap_or(0);
        let deadline = tokio::time::Instant::now() + secs(q.item_poll_timeout);
        loop {
            let items = self.0.db.read(|d| {
                d.queues
                    .get(&q.queue_id)
                    .and_then(|x| x.partitions.get(&part))
                    .map(|p| {
                        p.iter()
                            .filter(|(id, _)| *id > after)
                            .map(|(id, v)| pb::QueueItem {
                                value: hex::decode(v).unwrap_or_default().into(),
                                entry_id: id.to_string(),
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            });
            if !items.is_empty() || tokio::time::Instant::now() >= deadline {
                return Ok(Response::new(pb::QueueNextItemsResponse { items }));
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub(super) async fn h_queue_len(
        &self,
        r: Request<pb::QueueLenRequest>,
    ) -> Resp<pb::QueueLenResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let x = self.queue_of(&c, &q.queue_id)?;
        let len = if q.total {
            x.partitions.values().map(Vec::len).sum::<usize>()
        } else {
            x.partitions
                .get(&hex::encode(&q.partition_key))
                .map_or(0, Vec::len)
        };
        Ok(Response::new(pb::QueueLenResponse { len: len as i32 }))
    }

    pub(super) async fn h_queue_clear(&self, r: Request<pb::QueueClearRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let q = r.into_inner();
        self.queue_of(&c, &q.queue_id)?;
        self.0.db.write(|d| {
            if let Some(x) = d.queues.get_mut(&q.queue_id) {
                if q.all_partitions {
                    x.partitions.clear();
                } else {
                    x.partitions.remove(&hex::encode(&q.partition_key));
                }
            }
        });
        Ok(Response::new(()))
    }

    pub(super) async fn h_queue_delete(&self, r: Request<pb::QueueDeleteRequest>) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().queue_id;
        self.queue_of(&c, &id)?;
        self.0.db.write(|d| d.queues.remove(&id));
        Ok(Response::new(()))
    }

    pub(super) async fn h_queue_list(
        &self,
        r: Request<pb::QueueListRequest>,
    ) -> Resp<pb::QueueListResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let (max, before) = q
            .pagination
            .map(|p| (p.max_objects, p.created_before))
            .unwrap_or((0, 0.0));
        let mut v: Vec<QueueRec> = self.0.db.read(|d| {
            d.queues
                .values()
                .filter(|x| x.key_id == c.key_id() && x.name.is_some())
                .filter(|x| before == 0.0 || x.created_at < before)
                .cloned()
                .collect()
        });
        v.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
        if max > 0 {
            v.truncate(max as usize);
        }
        Ok(Response::new(pb::QueueListResponse {
            queues: v
                .iter()
                .map(|x| pb::queue_list_response::QueueInfo {
                    name: x.name.clone().unwrap_or_default(),
                    created_at: x.created_at,
                    num_partitions: x.partitions.len() as i32,
                    total_size: x.partitions.values().map(Vec::len).sum::<usize>() as i32,
                    queue_id: x.id.clone(),
                    metadata: Some(pb::QueueMetadata {
                        name: x.name.clone().unwrap_or_default(),
                        creation_info: Some(creation(x.created_at)),
                    }),
                })
                .collect(),
            environment_name: q.environment_name,
        }))
    }

    pub(super) async fn h_queue_heartbeat(
        &self,
        r: Request<pb::QueueHeartbeatRequest>,
    ) -> Resp<()> {
        let c = caller(&r)?;
        let id = r.into_inner().queue_id;
        self.queue_of(&c, &id)?;
        self.0.db.write(|d| {
            if let Some(x) = d.queues.get_mut(&id) {
                x.heartbeat_at = now_s();
            }
        });
        Ok(Response::new(()))
    }

    // ---- snapshots ----------------------------------------------------------

    pub(super) async fn h_sandbox_snapshot(
        &self,
        r: Request<pb::SandboxSnapshotRequest>,
    ) -> Resp<pb::SandboxSnapshotResponse> {
        let c = caller(&r)?;
        let rec = self.0.sandbox_of(&c, &r.into_inner().sandbox_id)?;
        if rec.state != SbState::Running {
            return Err(errors::conflict("the sandbox is not running"));
        }
        let id = self.0.snapshot_memory(&rec, "").await?;
        Ok(Response::new(pb::SandboxSnapshotResponse {
            snapshot_id: id,
        }))
    }

    pub(super) async fn h_sandbox_snapshot_wait(
        &self,
        r: Request<pb::SandboxSnapshotWaitRequest>,
    ) -> Resp<pb::SandboxSnapshotWaitResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().snapshot_id;
        let ok = self
            .0
            .db
            .read(|d| d.snapshots.get(&id).is_some_and(|s| s.key_id == c.key_id()));
        if !ok {
            return Err(errors::not_found(format!("Snapshot {id} not found")));
        }
        // Snapshots are complete when SandboxSnapshot returns.
        Ok(Response::new(pb::SandboxSnapshotWaitResponse {
            result: Some(pb::GenericResult {
                status: gs::SUCCESS,
                ..Default::default()
            }),
        }))
    }

    pub(super) async fn h_sandbox_snapshot_get(
        &self,
        r: Request<pb::SandboxSnapshotGetRequest>,
    ) -> Resp<pb::SandboxSnapshotGetResponse> {
        let c = caller(&r)?;
        let id = r.into_inner().snapshot_id;
        let snap = self
            .0
            .db
            .read(|d| {
                d.snapshots
                    .get(&id)
                    .filter(|s| s.key_id == c.key_id())
                    .cloned()
            })
            .ok_or_else(|| errors::not_found(format!("Snapshot {id} not found")))?;
        Ok(Response::new(pb::SandboxSnapshotGetResponse {
            snapshot_id: id,
            handle_metadata: Some(pb::SandboxSnapshotHandleMetadata {
                is_v2: !crate::modal::ids::is_id(&snap.sandbox_id, "sb"),
            }),
        }))
    }

    pub(super) async fn h_sandbox_restore(
        &self,
        r: Request<pb::SandboxRestoreRequest>,
    ) -> Resp<pb::SandboxRestoreResponse> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let rec = self
            .0
            .restore_sandbox(
                &c,
                &q.snapshot_id,
                q.sandbox_name_override_type,
                &q.sandbox_name_override,
                false,
            )
            .await?;
        Ok(Response::new(pb::SandboxRestoreResponse {
            sandbox_id: rec.id,
        }))
    }

    pub(super) async fn h_sandbox_restore_v2(
        &self,
        r: Request<pb::SandboxRestoreV2Request>,
    ) -> Resp<pb::SandboxRestoreV2Response> {
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let rec = m
            .restore_sandbox(
                &c,
                &q.snapshot_id,
                q.sandbox_name_override_type,
                &q.sandbox_name_override,
                true,
            )
            .await?;
        let guard = crate::modal::sandbox::CancelGuard::new(m.clone(), rec.id.clone());
        let done = m
            .wait_scheduled(&rec.id, std::time::Duration::from_secs(30 * 60))
            .await
            .ok_or_else(|| errors::not_found("sandbox vanished"))?;
        guard.disarm();
        if done.state != SbState::Running {
            return Err(errors::conflict(
                done.error
                    .unwrap_or_else(|| "the sandbox did not start".into()),
            ));
        }
        Ok(Response::new(pb::SandboxRestoreV2Response {
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
        }))
    }

    pub(super) async fn h_sandbox_snapshot_fs(
        &self,
        r: Request<pb::SandboxSnapshotFsRequest>,
    ) -> Resp<pb::SandboxSnapshotFsResponse> {
        let c = caller(&r)?;
        let rec = self.0.sandbox_of(&c, &r.into_inner().sandbox_id)?;
        let image_id = self.0.snapshot_filesystem(&rec).await?;
        let meta = self
            .0
            .image(&image_id)
            .and_then(|i| i.built)
            .map(|b| crate::modal::api::image_meta(&b));
        Ok(Response::new(pb::SandboxSnapshotFsResponse {
            image_id,
            result: Some(pb::GenericResult {
                status: gs::SUCCESS,
                ..Default::default()
            }),
            image_metadata: meta,
        }))
    }

    pub(super) async fn h_sandbox_get_exit_snapshot(
        &self,
        r: Request<pb::SandboxGetExitSnapshotRequest>,
    ) -> Resp<pb::SandboxGetExitSnapshotResponse> {
        use pb::sandbox_get_exit_snapshot_response as o;
        let c = caller(&r)?;
        let q = r.into_inner();
        let m = &self.0;
        let rec = m.sandbox_of(&c, &q.sandbox_id)?;
        let enabled = rec
            .def()
            .experimental_options_v2
            .get("enable_exit_snapshot")
            .is_some_and(|v| v.eq_ignore_ascii_case("true"));
        if !enabled {
            return Err(errors::invalid(
                "exit snapshot is not enabled for this Sandbox: create it with experimental_options={\"enable_exit_snapshot\": True}",
            ));
        }
        let deadline = tokio::time::Instant::now() + secs(q.timeout);
        let mut rx = m.sb_events(&q.sandbox_id).await;
        loop {
            let rec = m
                .sandbox(&q.sandbox_id)
                .ok_or_else(|| errors::not_found("sandbox vanished"))?;
            if let Some(img) = &rec.exit_snapshot {
                return Ok(Response::new(pb::SandboxGetExitSnapshotResponse {
                    outcome: Some(o::Outcome::Success(o::Success {
                        image_id: img.clone(),
                    })),
                }));
            }
            if let Some(e) = &rec.exit_snapshot_error {
                return Ok(Response::new(pb::SandboxGetExitSnapshotResponse {
                    outcome: Some(o::Outcome::Error(o::Error {
                        error_code: o::ErrorCode::Internal as i32,
                        message: e.clone(),
                    })),
                }));
            }
            if tokio::time::timeout_at(deadline, rx.changed())
                .await
                .is_err()
            {
                return Ok(Response::new(pb::SandboxGetExitSnapshotResponse {
                    outcome: Some(o::Outcome::Pending(o::Pending {})),
                }));
            }
        }
    }
}
