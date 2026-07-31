//! Trusted, session-owned native resources for managed host integrations.
//!
//! These records are accepted only by trusted SDK constructors. They are not
//! registered as tools and never appear in MCP schemas. The validated policy
//! rewrites path-free resource identifiers immediately before dispatch.

use cua_driver_core::protocol::{Content, ToolResult};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, uniffi::Record)]
pub struct TrustedResourceFile {
    pub resource_id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, uniffi::Record)]
pub struct TrustedResourceDirectory {
    pub resource_id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, uniffi::Record)]
pub struct TrustedSessionResources {
    pub recording_root: Option<String>,
    pub browser_download_root: Option<String>,
    pub upload_files: Vec<TrustedResourceFile>,
    pub replay_directories: Vec<TrustedResourceDirectory>,
    pub ffmpeg_path: Option<String>,
    pub browser_existing_profile_approved: bool,
}

fn is_indirect(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions and other reparse points are not always reported as
        // symlinks. Treat every reparse point as indirect for host resources.
        return metadata.file_attributes() & 0x400 != 0;
    }
    #[allow(unreachable_code)]
    false
}

#[derive(Debug)]
struct ValidatedFile {
    path: PathBuf,
    handle: Arc<File>,
    identity: same_file::Handle,
}

impl ValidatedFile {
    fn open(resource_id: &str, raw: &str, executable: bool) -> Result<Self, String> {
        let path = validate_absolute_path(raw, "file")?;
        let direct = std::fs::symlink_metadata(&path)
            .map_err(|_| format!("trusted resource {resource_id} does not exist"))?;
        if is_indirect(&direct) || !direct.is_file() {
            return Err(format!(
                "trusted resource {resource_id} must be a direct regular file"
            ));
        }
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            if direct.permissions().mode() & 0o111 == 0 {
                return Err(format!(
                    "trusted helper {resource_id} is not an executable file"
                ));
            }
        }
        let canonical = std::fs::canonicalize(&path)
            .map_err(|_| format!("trusted resource {resource_id} cannot be canonicalized"))?;
        let handle = File::open(&canonical)
            .map_err(|_| format!("trusted resource {resource_id} cannot be opened"))?;
        let identity = same_file::Handle::from_file(
            handle
                .try_clone()
                .map_err(|_| format!("trusted resource {resource_id} cannot be retained"))?,
        )
        .map_err(|_| format!("trusted resource {resource_id} has no stable file identity"))?;
        Ok(Self {
            path: canonical,
            handle: Arc::new(handle),
            identity,
        })
    }

    fn revalidate(&self, resource_id: &str) -> Result<&Path, String> {
        let direct = std::fs::symlink_metadata(&self.path)
            .map_err(|_| format!("trusted resource {resource_id} is no longer available"))?;
        if is_indirect(&direct) || !direct.is_file() {
            return Err(format!(
                "trusted resource {resource_id} changed type before use"
            ));
        }
        let current_path = std::fs::canonicalize(&self.path)
            .map_err(|_| format!("trusted resource {resource_id} cannot be revalidated"))?;
        let current_identity = same_file::Handle::from_path(&self.path)
            .map_err(|_| format!("trusted resource {resource_id} cannot be revalidated"))?;
        let retained_identity = same_file::Handle::from_file(
            self.handle
                .try_clone()
                .map_err(|_| format!("trusted resource {resource_id} handle is unavailable"))?,
        )
        .map_err(|_| format!("trusted resource {resource_id} handle is unavailable"))?;
        if current_path != self.path
            || current_identity != self.identity
            || retained_identity != self.identity
        {
            return Err(format!(
                "trusted resource {resource_id} was replaced before use"
            ));
        }
        Ok(&self.path)
    }
}

#[derive(Debug)]
struct ValidatedDirectory {
    path: PathBuf,
    identity: same_file::Handle,
}

impl ValidatedDirectory {
    fn open(resource_id: &str, raw: &str) -> Result<Self, String> {
        let path = validate_absolute_path(raw, "directory")?;
        let direct = std::fs::symlink_metadata(&path)
            .map_err(|_| format!("trusted resource {resource_id} does not exist"))?;
        if is_indirect(&direct) || !direct.is_dir() {
            return Err(format!(
                "trusted resource {resource_id} must be a direct directory"
            ));
        }
        let path = std::fs::canonicalize(path)
            .map_err(|_| format!("trusted resource {resource_id} cannot be canonicalized"))?;
        let identity = same_file::Handle::from_path(&path)
            .map_err(|_| format!("trusted resource {resource_id} has no stable file identity"))?;
        Ok(Self { path, identity })
    }

    fn revalidate(&self, resource_id: &str) -> Result<&Path, String> {
        let direct = std::fs::symlink_metadata(&self.path)
            .map_err(|_| format!("trusted resource {resource_id} is no longer available"))?;
        if is_indirect(&direct) || !direct.is_dir() {
            return Err(format!(
                "trusted resource {resource_id} changed type before use"
            ));
        }
        let current = std::fs::canonicalize(&self.path)
            .map_err(|_| format!("trusted resource {resource_id} cannot be revalidated"))?;
        let current_identity = same_file::Handle::from_path(&self.path)
            .map_err(|_| format!("trusted resource {resource_id} cannot be revalidated"))?;
        if current != self.path || current_identity != self.identity {
            return Err(format!(
                "trusted resource {resource_id} was replaced before use"
            ));
        }
        Ok(&self.path)
    }
}

fn validate_absolute_path(raw: &str, kind: &str) -> Result<PathBuf, String> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(format!("trusted {kind} path must be non-empty"));
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return Err(format!("trusted {kind} path must be absolute"));
    }
    Ok(path)
}

fn validate_resource_id(resource_id: &str) -> Result<(), String> {
    if resource_id.is_empty()
        || resource_id.len() > 128
        || matches!(resource_id, "." | "..")
        || !resource_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(
            "trusted resource ids must use 1-128 ASCII letters, digits, '.', '_', or '-'".into(),
        );
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct ValidatedTrustedSessionResources {
    recording_root: Option<ValidatedDirectory>,
    browser_download_root: Option<ValidatedDirectory>,
    upload_files: HashMap<String, ValidatedFile>,
    replay_directories: HashMap<String, ValidatedDirectory>,
    ffmpeg: Option<ValidatedFile>,
    browser_existing_profile_approved: bool,
}

impl ValidatedTrustedSessionResources {
    pub(crate) fn validate(resources: TrustedSessionResources) -> Result<Arc<Self>, String> {
        let recording_root = resources
            .recording_root
            .as_deref()
            .map(|path| ValidatedDirectory::open("recording-root", path))
            .transpose()?;
        let browser_download_root = resources
            .browser_download_root
            .as_deref()
            .map(|path| ValidatedDirectory::open("browser-download-root", path))
            .transpose()?;

        let mut ids = HashSet::new();
        let mut upload_files = HashMap::new();
        if resources.upload_files.len() > 32 {
            return Err("a trusted session may bind at most 32 upload files".into());
        }
        for resource in resources.upload_files {
            validate_resource_id(&resource.resource_id)?;
            if !ids.insert(resource.resource_id.clone()) {
                return Err(format!(
                    "duplicate trusted resource id {}",
                    resource.resource_id
                ));
            }
            upload_files.insert(
                resource.resource_id.clone(),
                ValidatedFile::open(&resource.resource_id, &resource.path, false)?,
            );
        }

        let mut replay_directories = HashMap::new();
        if resources.replay_directories.len() > 64 {
            return Err("a trusted session may bind at most 64 replay artifacts".into());
        }
        for resource in resources.replay_directories {
            validate_resource_id(&resource.resource_id)?;
            if !ids.insert(resource.resource_id.clone()) {
                return Err(format!(
                    "duplicate trusted resource id {}",
                    resource.resource_id
                ));
            }
            replay_directories.insert(
                resource.resource_id.clone(),
                ValidatedDirectory::open(&resource.resource_id, &resource.path)?,
            );
        }

        let ffmpeg = resources
            .ffmpeg_path
            .as_deref()
            .map(|path| ValidatedFile::open("ffmpeg", path, true))
            .transpose()?;

        Ok(Arc::new(Self {
            recording_root,
            browser_download_root,
            upload_files,
            replay_directories,
            ffmpeg,
            browser_existing_profile_approved: resources.browser_existing_profile_approved,
        }))
    }

    pub(crate) fn apply(&self, tool_name: &str, args: &mut Value) -> Result<(), String> {
        let arguments = args
            .as_object_mut()
            .ok_or_else(|| "managed session actions require an object argument".to_owned())?;
        match tool_name {
            "start_recording" => {
                let root = self
                    .recording_root
                    .as_ref()
                    .ok_or_else(|| "trusted recording root is not configured".to_owned())?
                    .revalidate("recording-root")?;
                arguments.insert(
                    "output_dir".into(),
                    Value::String(root.to_string_lossy().into_owned()),
                );
                if let Some(ffmpeg) = &self.ffmpeg {
                    arguments.insert(
                        "_cua_trusted_ffmpeg_path".into(),
                        Value::String(ffmpeg.revalidate("ffmpeg")?.to_string_lossy().into_owned()),
                    );
                }
            }
            "browser_download" => {
                let root = self
                    .browser_download_root
                    .as_ref()
                    .ok_or_else(|| "trusted browser download root is not configured".to_owned())?
                    .revalidate("browser-download-root")?;
                arguments.insert(
                    "destination_root".into(),
                    Value::String(root.to_string_lossy().into_owned()),
                );
                arguments.insert(
                    "_cua_browser_download_mcp_host_approved".into(),
                    Value::Bool(true),
                );
            }
            "browser_prepare" if self.browser_existing_profile_approved => {
                if arguments
                    .get("strategy")
                    .and_then(Value::as_object)
                    .and_then(|strategy| strategy.get("kind"))
                    .and_then(Value::as_str)
                    == Some("existing_profile")
                {
                    arguments.insert(
                        "_cua_browser_prepare_trusted_host_existing_profile_approved".into(),
                        Value::Bool(true),
                    );
                } else {
                    arguments.insert(
                        "_cua_browser_prepare_mcp_host_approved".into(),
                        Value::Bool(true),
                    );
                }
            }
            "browser_prepare" => {
                if arguments
                    .get("strategy")
                    .and_then(Value::as_object)
                    .and_then(|strategy| strategy.get("kind"))
                    .and_then(Value::as_str)
                    != Some("existing_profile")
                {
                    arguments.insert(
                        "_cua_browser_prepare_mcp_host_approved".into(),
                        Value::Bool(true),
                    );
                }
            }
            "browser_set_input_files" => {
                let ids = arguments
                    .get("files")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "managed upload requires resource ids in files".to_owned())?;
                if ids.len() > 32 {
                    return Err("a managed upload may contain at most 32 resource ids".into());
                }
                let mut paths = Vec::with_capacity(ids.len());
                for id in ids {
                    let id = id
                        .as_str()
                        .ok_or_else(|| "managed upload resource ids must be strings".to_owned())?;
                    let file = self
                        .upload_files
                        .get(id)
                        .ok_or_else(|| format!("upload resource id {id} is not approved"))?;
                    paths.push(Value::String(
                        file.revalidate(id)?.to_string_lossy().into_owned(),
                    ));
                }
                arguments.insert("files".into(), Value::Array(paths));
            }
            "replay_trajectory" => {
                let id = arguments
                    .get("dir")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "managed replay requires an opaque artifact id".to_owned())?;
                let directory = self
                    .replay_directories
                    .get(id)
                    .ok_or_else(|| format!("replay resource id {id} is not approved"))?;
                arguments.insert(
                    "dir".into(),
                    Value::String(directory.revalidate(id)?.to_string_lossy().into_owned()),
                );
            }
            "get_desktop_state" | "get_window_state" => {
                // Managed providers return screenshots inline. A model-selected
                // output path must never turn an observation into an arbitrary
                // host filesystem write.
                arguments.remove("screenshot_out_file");
            }
            "click" => {
                // The click debugger is an optional local-development output.
                // Keep it unavailable when host-owned resource binding is
                // active rather than accepting a model-selected path.
                arguments.remove("debug_image_out");
            }
            "launch_app" => {
                return Err(
                    "application launch is unavailable in a managed resource session".into(),
                );
            }
            "install_ffmpeg" => {
                return Err(
                    "dependency installation is unavailable in a managed resource session".into(),
                );
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn status(&self) -> Value {
        serde_json::json!({
            "schema_version": "1",
            "managed": true,
            "recording_root_ready": self.recording_root
                .as_ref()
                .is_some_and(|root| root.revalidate("recording-root").is_ok()),
            "browser_download_root_ready": self.browser_download_root
                .as_ref()
                .is_some_and(|root| root.revalidate("browser-download-root").is_ok()),
            "upload_file_count": self.upload_files
                .iter()
                .filter(|(id, file)| file.revalidate(id).is_ok())
                .count(),
            "replay_artifact_count": self.replay_directories
                .iter()
                .filter(|(id, directory)| directory.revalidate(id).is_ok())
                .count(),
            "file_output_suppression_ready": true,
            "ffmpeg_helper_ready": self.ffmpeg.as_ref().is_some_and(|helper| helper.revalidate("ffmpeg").is_ok()),
            "browser_existing_profile_approved": self.browser_existing_profile_approved,
        })
    }

    pub(crate) fn augment_result(&self, tool_name: &str, result: &mut ToolResult) {
        if matches!(tool_name, "get_session_state" | "health_report") {
            let structured = result
                .structured_content
                .get_or_insert_with(|| Value::Object(Map::new()));
            if let Some(object) = structured.as_object_mut() {
                object.insert("trusted_resources".into(), self.status());
            }
            redact_path_fields(result.structured_content.as_mut());
        }
        if matches!(
            tool_name,
            "start_recording" | "stop_recording" | "get_recording_state"
        ) {
            redact_path_fields(result.structured_content.as_mut());
            let failed = result.is_error == Some(true);
            for content in &mut result.content {
                if let Content::Text { text, .. } = content {
                    *text = match (tool_name, failed) {
                        ("start_recording", false) => {
                            "Recording started in the host-managed session root.".into()
                        }
                        ("stop_recording", false) => {
                            "Recording stopped in the host-managed session root.".into()
                        }
                        ("get_recording_state", false) => {
                            "Recording state returned for the host-managed session.".into()
                        }
                        _ => "Recording action failed in the host-managed session.".into(),
                    };
                }
            }
        }
        if tool_name == "replay_trajectory" {
            redact_replay_result(result);
        }
        if matches!(tool_name, "list_apps" | "launch_app") {
            redact_application_paths(result.structured_content.as_mut());
        }
    }
}

impl cua_driver_core::tool::TrustedArgumentPolicy for ValidatedTrustedSessionResources {
    fn apply(&self, tool_name: &str, args: &mut Value) -> Result<(), String> {
        ValidatedTrustedSessionResources::apply(self, tool_name, args)
    }
}

fn redact_replay_result(result: &mut ToolResult) {
    let is_authorization_refusal = result
        .structured_content
        .as_ref()
        .and_then(|structured| structured.pointer("/refusal/code"))
        .is_some();
    redact_path_fields(result.structured_content.as_mut());
    let mut attempted = 0;
    let mut succeeded = 0;
    let mut failed = u64::from(result.is_error == Some(true));
    if let Some(structured) = result.structured_content.as_mut() {
        if let Some(turns) = structured.get_mut("turns").and_then(Value::as_array_mut) {
            for turn in turns {
                if let Some(turn) = turn.as_object_mut() {
                    if turn.contains_key("result_summary") {
                        turn.insert(
                            "result_summary".into(),
                            Value::String("[host-managed result]".into()),
                        );
                    }
                    if turn.contains_key("parse_error") {
                        turn.insert(
                            "parse_error".into(),
                            Value::String("[host-managed parse error]".into()),
                        );
                    }
                }
            }
        }
        if let Some(failure) = structured
            .get_mut("first_failure")
            .and_then(Value::as_object_mut)
        {
            if failure.contains_key("error") {
                failure.insert("error".into(), Value::String("[host-managed error]".into()));
            }
        }
        attempted = structured
            .get("attempted")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        succeeded = structured
            .get("succeeded")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        failed = structured
            .get("failed")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    }
    if !is_authorization_refusal {
        for content in &mut result.content {
            if let Content::Text { text, .. } = content {
                *text = format!(
                    "Replay completed for a host-managed artifact: attempted={attempted} succeeded={succeeded} failed={failed}"
                );
            }
        }
    }
}

fn redact_application_paths(value: Option<&mut Value>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "path" | "launch_path" | "exe_path") {
                    if !value.is_null() {
                        *value = Value::String("[host-managed]".into());
                    }
                } else {
                    redact_application_paths(Some(value));
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_application_paths(Some(value));
            }
        }
        _ => {}
    }
}

fn redact_path_fields(value: Option<&mut Value>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "output_dir"
                        | "path"
                        | "directory"
                        | "last_video_path"
                        | "absolute_path"
                        | "executable_path"
                        | "screenshot_file_path"
                        | "file_path"
                        | "destination_root"
                ) {
                    *value = Value::String("[host-managed]".into());
                } else if key == "last_error" && !value.is_null() {
                    *value = Value::String("[host-managed error]".into());
                } else {
                    redact_path_fields(Some(value));
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact_path_fields(Some(value));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture() -> (tempfile::TempDir, TrustedSessionResources) {
        let root = tempfile::tempdir().unwrap();
        let recordings = root.path().join("recordings");
        let downloads = root.path().join("downloads");
        let replay = root.path().join("replay");
        std::fs::create_dir_all(&recordings).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&replay).unwrap();
        let upload = root.path().join("upload.txt");
        File::create(&upload)
            .unwrap()
            .write_all(b"approved")
            .unwrap();
        (
            root,
            TrustedSessionResources {
                recording_root: Some(recordings.to_string_lossy().into_owned()),
                browser_download_root: Some(downloads.to_string_lossy().into_owned()),
                upload_files: vec![TrustedResourceFile {
                    resource_id: "upload-1".into(),
                    path: upload.to_string_lossy().into_owned(),
                }],
                replay_directories: vec![TrustedResourceDirectory {
                    resource_id: "replay-1".into(),
                    path: replay.to_string_lossy().into_owned(),
                }],
                ffmpeg_path: None,
                browser_existing_profile_approved: false,
            },
        )
    }

    #[test]
    fn managed_paths_are_injected_and_opaque_ids_are_resolved() {
        let (_root, resources) = fixture();
        let validated = ValidatedTrustedSessionResources::validate(resources).unwrap();
        let mut recording = serde_json::json!({"output_dir": "/model/chosen"});
        validated.apply("start_recording", &mut recording).unwrap();
        assert_ne!(recording["output_dir"], "/model/chosen");

        let mut upload = serde_json::json!({"files": ["upload-1"]});
        validated
            .apply("browser_set_input_files", &mut upload)
            .unwrap();
        assert!(Path::new(upload["files"][0].as_str().unwrap()).is_absolute());

        let mut replay = serde_json::json!({"dir": "replay-1"});
        validated.apply("replay_trajectory", &mut replay).unwrap();
        assert!(Path::new(replay["dir"].as_str().unwrap()).is_absolute());

        let mut screenshot =
            serde_json::json!({"screenshot_out_file": "/model/chosen/capture.png"});
        validated
            .apply("get_window_state", &mut screenshot)
            .unwrap();
        assert!(screenshot.get("screenshot_out_file").is_none());

        let mut click = serde_json::json!({"debug_image_out": "/model/chosen/click.png"});
        validated.apply("click", &mut click).unwrap();
        assert!(click.get("debug_image_out").is_none());

        let mut launch = serde_json::json!({
            "name": "Chrome",
            "urls": ["file:///model/chosen/file"],
            "additional_arguments": ["--user-data-dir=/model/chosen"]
        });
        assert!(validated
            .apply("launch_app", &mut launch)
            .unwrap_err()
            .contains("unavailable"));

        assert!(validated
            .apply("install_ffmpeg", &mut serde_json::json!({"confirm": true}))
            .unwrap_err()
            .contains("unavailable"));
    }

    #[test]
    fn browser_existing_profile_approval_is_host_injected_only_when_bound() {
        let (_root, resources) = fixture();
        let validated = ValidatedTrustedSessionResources::validate(resources).unwrap();
        let mut unapproved = serde_json::json!({
            "strategy": {"kind": "existing_profile"},
            "_cua_browser_prepare_trusted_host_existing_profile_approved": true
        });
        cua_driver_core::tool_args::sanitize_reserved_args(&mut unapproved);
        validated.apply("browser_prepare", &mut unapproved).unwrap();
        assert!(unapproved
            .get("_cua_browser_prepare_trusted_host_existing_profile_approved")
            .is_none());

        let (_root, mut resources) = fixture();
        resources.browser_existing_profile_approved = true;
        let validated = ValidatedTrustedSessionResources::validate(resources).unwrap();
        let mut approved = serde_json::json!({"strategy": {"kind": "existing_profile"}});
        validated.apply("browser_prepare", &mut approved).unwrap();
        assert_eq!(
            approved["_cua_browser_prepare_trusted_host_existing_profile_approved"],
            Value::Bool(true)
        );

        let mut isolated = serde_json::json!({
            "profile": {"mode": "isolated_new"},
            "allow_launch": true
        });
        validated.apply("browser_prepare", &mut isolated).unwrap();
        assert_eq!(
            isolated["_cua_browser_prepare_mcp_host_approved"],
            Value::Bool(true)
        );
    }

    #[test]
    fn unknown_ids_relative_paths_and_links_fail_closed() {
        let (root, resources) = fixture();
        let validated = ValidatedTrustedSessionResources::validate(resources).unwrap();
        let mut unknown = serde_json::json!({"files": ["not-approved"]});
        assert!(validated
            .apply("browser_set_input_files", &mut unknown)
            .is_err());

        assert!(
            ValidatedTrustedSessionResources::validate(TrustedSessionResources {
                recording_root: Some("relative".into()),
                browser_download_root: None,
                upload_files: Vec::new(),
                replay_directories: Vec::new(),
                ffmpeg_path: None,
                browser_existing_profile_approved: false,
            })
            .is_err()
        );

        #[cfg(unix)]
        {
            let target = root.path().join("target.txt");
            File::create(&target).unwrap();
            let link = root.path().join("link.txt");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(
                ValidatedTrustedSessionResources::validate(TrustedSessionResources {
                    recording_root: None,
                    browser_download_root: None,
                    upload_files: vec![TrustedResourceFile {
                        resource_id: "linked".into(),
                        path: link.to_string_lossy().into_owned(),
                    }],
                    replay_directories: Vec::new(),
                    ffmpeg_path: None,
                    browser_existing_profile_approved: false,
                })
                .is_err()
            );
        }
    }

    #[test]
    fn helper_paths_are_paired() {
        let (_root, mut resources) = fixture();
        resources.ffmpeg_path = Some("/missing/ffmpeg".into());
        assert!(ValidatedTrustedSessionResources::validate(resources).is_err());
    }

    #[test]
    fn ids_and_resource_counts_are_bounded() {
        let (root, mut resources) = fixture();
        resources.upload_files[0].resource_id = "../escape".into();
        assert!(ValidatedTrustedSessionResources::validate(resources).is_err());

        let mut files = Vec::new();
        for index in 0..33 {
            let path = root.path().join(format!("upload-{index}.txt"));
            File::create(&path).unwrap();
            files.push(TrustedResourceFile {
                resource_id: format!("upload-{index}"),
                path: path.to_string_lossy().into_owned(),
            });
        }
        assert!(
            ValidatedTrustedSessionResources::validate(TrustedSessionResources {
                recording_root: None,
                browser_download_root: None,
                upload_files: files,
                replay_directories: Vec::new(),
                ffmpeg_path: None,
                browser_existing_profile_approved: false,
            })
            .unwrap_err()
            .contains("at most 32")
        );
    }

    #[test]
    fn recording_results_preserve_failure_state_and_redact_native_paths() {
        let (_root, resources) = fixture();
        let validated = ValidatedTrustedSessionResources::validate(resources).unwrap();
        let mut failed = ToolResult::error("failed at /private/recordings");
        failed.structured_content = Some(serde_json::json!({
            "output_dir": "/private/recordings",
            "last_video_path": "/private/recordings/recording.mp4",
            "last_error": "helper /private/bin/ffmpeg failed"
        }));

        validated.augment_result("start_recording", &mut failed);

        assert_eq!(failed.is_error, Some(true));
        assert_eq!(
            failed.structured_content.as_ref().unwrap()["output_dir"],
            "[host-managed]"
        );
        assert_eq!(
            failed.structured_content.as_ref().unwrap()["last_video_path"],
            "[host-managed]"
        );
        assert_eq!(
            failed.structured_content.as_ref().unwrap()["last_error"],
            "[host-managed error]"
        );
        assert!(matches!(
            failed.content.as_slice(),
            [Content::Text { text, .. }]
                if text == "Recording action failed in the host-managed session."
        ));
    }

    #[test]
    fn replay_and_application_results_do_not_disclose_native_paths_or_nested_errors() {
        let (root, resources) = fixture();
        let validated = ValidatedTrustedSessionResources::validate(resources).unwrap();
        let private_path = root.path().join("replay").to_string_lossy().into_owned();
        let mut replay = ToolResult::text(format!("replay {private_path}: failed"));
        replay.structured_content = Some(serde_json::json!({
            "directory": private_path,
            "attempted": 1,
            "succeeded": 0,
            "failed": 1,
            "turns": [{
                "turn": "turn-00001",
                "tool": "install_ffmpeg",
                "ok": false,
                "result_summary": "failed at /private/bin/ffmpeg"
            }],
            "first_failure": {
                "turn": "turn-00001",
                "tool": "install_ffmpeg",
                "error": "failed at /private/bin/ffmpeg"
            }
        }));
        validated.augment_result("replay_trajectory", &mut replay);
        let encoded = serde_json::to_string(&replay).unwrap();
        assert!(!encoded.contains("/private/"));
        assert!(!encoded.contains(root.path().to_string_lossy().as_ref()));
        assert!(encoded.contains("[host-managed result]"));
        assert!(encoded.contains("[host-managed error]"));

        let mut replay_error = ToolResult::error("failed at /private/replay/trajectory.jsonl");
        validated.augment_result("replay_trajectory", &mut replay_error);
        let encoded = serde_json::to_string(&replay_error).unwrap();
        assert!(!encoded.contains("/private/"));
        assert!(encoded.contains("host-managed artifact"));
        assert!(!encoded.contains(root.path().to_string_lossy().as_ref()));

        let mut replay_refusal = ToolResult::error("Permission denied: replay id is not approved");
        replay_refusal.structured_content = Some(serde_json::json!({
            "refusal": {"code": "permission_denied"}
        }));
        validated.augment_result("replay_trajectory", &mut replay_refusal);
        assert!(matches!(
            replay_refusal.content.as_slice(),
            [Content::Text { text, .. }]
                if text == "Permission denied: replay id is not approved"
        ));

        let mut apps = ToolResult::text("apps");
        apps.structured_content = Some(serde_json::json!({
            "apps": [{
                "name": "Example",
                "path": "/private/Applications/Example",
                "nested": {"launch_path": "/private/bin/example"}
            }]
        }));
        validated.augment_result("list_apps", &mut apps);
        let encoded = serde_json::to_string(&apps).unwrap();
        assert!(!encoded.contains("/private/"));
    }

    #[cfg(unix)]
    #[test]
    fn file_and_directory_replacement_races_fail_closed() {
        let (root, resources) = fixture();
        let upload_path = PathBuf::from(&resources.upload_files[0].path);
        let recording_path = PathBuf::from(resources.recording_root.as_ref().unwrap());
        let validated = ValidatedTrustedSessionResources::validate(resources).unwrap();

        let moved_upload = root.path().join("old-upload.txt");
        std::fs::rename(&upload_path, &moved_upload).unwrap();
        File::create(&upload_path).unwrap();
        let mut upload = serde_json::json!({"files": ["upload-1"]});
        assert!(validated
            .apply("browser_set_input_files", &mut upload)
            .unwrap_err()
            .contains("replaced before use"));

        let moved_recording = root.path().join("old-recordings");
        std::fs::rename(&recording_path, &moved_recording).unwrap();
        std::fs::create_dir(&recording_path).unwrap();
        let mut recording = serde_json::json!({});
        assert!(validated
            .apply("start_recording", &mut recording)
            .unwrap_err()
            .contains("replaced before use"));
    }
}
