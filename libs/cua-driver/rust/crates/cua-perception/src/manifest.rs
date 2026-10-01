use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelManifest {
    pub schema_version: u32,
    pub identity: ModelIdentity,
    pub onnx_runtime: RuntimeIdentity,
    pub detector: DetectorManifest,
    pub ocr: OcrManifest,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelIdentity {
    pub name: String,
    pub version: String,
    pub source_url: String,
    pub source_revision: String,
    pub license: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeIdentity {
    pub version: String,
    pub target: String,
    pub library_sha256: String,
    pub intra_threads: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DetectorManifest {
    pub model: Artifact,
    pub input_name: String,
    pub output_name: String,
    pub input_width: u32,
    pub input_height: u32,
    pub confidence_threshold: f32,
    pub iou_threshold: f32,
    pub output_layout: DetectorOutputLayout,
    pub max_candidates: usize,
    pub max_detections: usize,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DetectorOutputLayout {
    YoloV8CxcywhClassScores,
    XyxyScoreClass,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OcrManifest {
    pub detector: OcrDetectorManifest,
    pub recognizer: OcrRecognizerManifest,
    pub dictionary: Artifact,
    pub dictionary_format: DictionaryFormat,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DictionaryFormat {
    PlainLines,
    PaddleInferenceYaml,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OcrDetectorManifest {
    pub model: Artifact,
    pub input_name: String,
    pub output_name: String,
    pub input_width: u32,
    pub input_height: u32,
    pub pixel_threshold: f32,
    pub box_threshold: f32,
    pub unclip_ratio: f32,
    pub minimum_area: u32,
    pub minimum_side: u32,
    pub max_candidates: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OcrRecognizerManifest {
    pub model: Artifact,
    pub input_name: String,
    pub output_name: String,
    pub input_width: u32,
    pub input_height: u32,
    pub blank_index: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct ValidatedManifest {
    pub manifest: ModelManifest,
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    pub detector_path: PathBuf,
    pub ocr_detector_path: PathBuf,
    pub ocr_recognizer_path: PathBuf,
    pub dictionary_path: PathBuf,
    pub runtime_library_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("failed to read {path}: {reason}")]
    Read { path: PathBuf, reason: String },
    #[error("model manifest is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported model manifest schema {actual}; expected {expected}")]
    Schema { actual: u32, expected: u32 },
    #[error("manifest field {0} must not be empty")]
    EmptyField(&'static str),
    #[error("manifest field {0} is outside its valid range")]
    InvalidValue(&'static str),
    #[error("artifact path must be a relative path without '..': {0}")]
    UnsafePath(PathBuf),
    #[error("artifact is missing or not a regular file: {0}")]
    MissingArtifact(PathBuf),
    #[error("sha256 mismatch for {path}: expected {expected}, got {actual}")]
    HashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
}

impl ValidatedManifest {
    pub fn load(manifest_path: &Path, runtime_library_path: &Path) -> Result<Self, ManifestError> {
        let manifest_bytes = read_bytes(manifest_path)?;
        let manifest: ModelManifest = serde_json::from_slice(&manifest_bytes)?;
        manifest.validate_fields()?;
        let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));

        let detector_path = validate_artifact(root, &manifest.detector.model)?;
        let ocr_detector_path = validate_artifact(root, &manifest.ocr.detector.model)?;
        let ocr_recognizer_path = validate_artifact(root, &manifest.ocr.recognizer.model)?;
        let dictionary_path = validate_artifact(root, &manifest.ocr.dictionary)?;
        validate_absolute_artifact(runtime_library_path, &manifest.onnx_runtime.library_sha256)?;

        Ok(Self {
            manifest,
            manifest_path: manifest_path.to_path_buf(),
            manifest_sha256: digest(&manifest_bytes),
            detector_path,
            ocr_detector_path,
            ocr_recognizer_path,
            dictionary_path,
            runtime_library_path: runtime_library_path.to_path_buf(),
        })
    }
}

impl ModelManifest {
    fn validate_fields(&self) -> Result<(), ManifestError> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::Schema {
                actual: self.schema_version,
                expected: MANIFEST_SCHEMA_VERSION,
            });
        }
        for (name, value) in [
            ("identity.name", self.identity.name.as_str()),
            ("identity.version", self.identity.version.as_str()),
            ("identity.source_url", self.identity.source_url.as_str()),
            (
                "identity.source_revision",
                self.identity.source_revision.as_str(),
            ),
            ("identity.license", self.identity.license.as_str()),
            ("onnx_runtime.version", self.onnx_runtime.version.as_str()),
            ("onnx_runtime.target", self.onnx_runtime.target.as_str()),
            ("detector.input_name", self.detector.input_name.as_str()),
            ("detector.output_name", self.detector.output_name.as_str()),
            (
                "ocr.detector.input_name",
                self.ocr.detector.input_name.as_str(),
            ),
            (
                "ocr.detector.output_name",
                self.ocr.detector.output_name.as_str(),
            ),
            (
                "ocr.recognizer.input_name",
                self.ocr.recognizer.input_name.as_str(),
            ),
            (
                "ocr.recognizer.output_name",
                self.ocr.recognizer.output_name.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(ManifestError::EmptyField(name));
            }
        }
        for (name, width, height) in [
            (
                "detector input dimensions",
                self.detector.input_width,
                self.detector.input_height,
            ),
            (
                "OCR detector input dimensions",
                self.ocr.detector.input_width,
                self.ocr.detector.input_height,
            ),
            (
                "OCR recognizer input dimensions",
                self.ocr.recognizer.input_width,
                self.ocr.recognizer.input_height,
            ),
        ] {
            if width == 0 || height == 0 || width > 4096 || height > 4096 {
                return Err(ManifestError::InvalidValue(name));
            }
        }
        if !(0.0..=1.0).contains(&self.detector.confidence_threshold)
            || !(0.0..=1.0).contains(&self.detector.iou_threshold)
            || !(0.0..=1.0).contains(&self.ocr.detector.pixel_threshold)
            || !(0.0..=1.0).contains(&self.ocr.detector.box_threshold)
            || self.ocr.detector.unclip_ratio < 1.0
            || self.ocr.detector.minimum_area == 0
            || self.ocr.detector.minimum_side == 0
            || self.ocr.detector.max_candidates == 0
            || self.detector.max_candidates == 0
            || self.detector.max_detections == 0
            || self.detector.max_detections > self.detector.max_candidates
        {
            return Err(ManifestError::InvalidValue("postprocessing thresholds"));
        }
        validate_sha256(
            "onnx_runtime.library_sha256",
            &self.onnx_runtime.library_sha256,
        )?;
        if self.onnx_runtime.intra_threads == 0 || self.onnx_runtime.intra_threads > 64 {
            return Err(ManifestError::InvalidValue("onnx_runtime.intra_threads"));
        }
        let actual_target = current_target();
        if self.onnx_runtime.target != actual_target {
            return Err(ManifestError::InvalidValue("onnx_runtime.target"));
        }
        for artifact in [
            &self.detector.model,
            &self.ocr.detector.model,
            &self.ocr.recognizer.model,
            &self.ocr.dictionary,
        ] {
            validate_sha256("artifact.sha256", &artifact.sha256)?;
        }
        Ok(())
    }
}

fn current_target() -> String {
    let arch = std::env::consts::ARCH;
    let suffix = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        "linux" => "unknown-linux-gnu",
        other => other,
    };
    format!("{arch}-{suffix}")
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), ManifestError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::InvalidValue(field));
    }
    Ok(())
}

fn validate_artifact(root: &Path, artifact: &Artifact) -> Result<PathBuf, ManifestError> {
    if artifact.path.is_absolute()
        || artifact
            .path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(ManifestError::UnsafePath(artifact.path.clone()));
    }
    let path = root.join(&artifact.path);
    validate_absolute_artifact(&path, &artifact.sha256)?;
    Ok(path)
}

fn validate_absolute_artifact(path: &Path, expected: &str) -> Result<(), ManifestError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ManifestError::MissingArtifact(path.into()))?;
    if !metadata.file_type().is_file() || metadata.len() == 0 {
        return Err(ManifestError::MissingArtifact(path.into()));
    }
    let actual = digest(&read_bytes(path)?);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(ManifestError::HashMismatch {
            path: path.into(),
            expected: expected.to_ascii_lowercase(),
            actual,
        });
    }
    Ok(())
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, ManifestError> {
    fs::read(path).map_err(|error| ManifestError::Read {
        path: path.into(),
        reason: error.to_string(),
    })
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "cua-perception-manifest-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn manifest_loads_only_when_every_hash_matches() {
        let directory = TestDirectory::new();
        for name in ["detector.onnx", "ocr-det.onnx", "ocr-rec.onnx"] {
            fs::write(directory.0.join(name), name.as_bytes()).unwrap();
        }
        fs::write(directory.0.join("dictionary.txt"), b"a\nb\n").unwrap();
        fs::write(directory.0.join("runtime.dylib"), b"runtime").unwrap();
        let artifact = |name: &str| {
            json!({
                "path": name,
                "sha256": digest(&fs::read(directory.0.join(name)).unwrap())
            })
        };
        let mut manifest = json!({
            "schema_version": 1,
            "identity": {
                "name": "test", "version": "1", "source_url": "https://example.invalid",
                "source_revision": "abc", "license": "test-only"
            },
            "onnx_runtime": {
                "version": "1", "target": current_target(),
                "library_sha256": digest(b"runtime"), "intra_threads": 1
            },
            "detector": {
                "model": artifact("detector.onnx"), "input_name": "images", "output_name": "output0",
                "input_width": 32, "input_height": 32, "confidence_threshold": 0.3,
                "iou_threshold": 0.1, "output_layout": "yolo_v8_cxcywh_class_scores",
                "max_candidates": 100, "max_detections": 10
            },
            "ocr": {
                "detector": {
                    "model": artifact("ocr-det.onnx"), "input_name": "x", "output_name": "out",
                    "input_width": 32, "input_height": 32, "pixel_threshold": 0.3,
                    "box_threshold": 0.6, "unclip_ratio": 1.5, "minimum_area": 1,
                    "minimum_side": 1, "max_candidates": 10
                },
                "recognizer": {
                    "model": artifact("ocr-rec.onnx"), "input_name": "x", "output_name": "out",
                    "input_width": 32, "input_height": 32, "blank_index": 0
                },
                "dictionary": artifact("dictionary.txt"),
                "dictionary_format": "plain_lines"
            }
        });
        let manifest_path = directory.0.join("manifest.json");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        let validated =
            ValidatedManifest::load(&manifest_path, &directory.0.join("runtime.dylib")).unwrap();
        assert_eq!(validated.manifest.identity.name, "test");

        manifest["onnx_runtime"]["target"] = json!("wrong-target");
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(matches!(
            ValidatedManifest::load(&manifest_path, &directory.0.join("runtime.dylib")),
            Err(ManifestError::InvalidValue("onnx_runtime.target"))
        ));
        manifest["onnx_runtime"]["target"] = json!(current_target());
        fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();

        fs::write(directory.0.join("detector.onnx"), b"tampered").unwrap();
        assert!(matches!(
            ValidatedManifest::load(&manifest_path, &directory.0.join("runtime.dylib")),
            Err(ManifestError::HashMismatch { .. })
        ));
    }

    #[test]
    fn artifact_paths_cannot_escape_the_manifest_directory() {
        let artifact = Artifact {
            path: PathBuf::from("../model.onnx"),
            sha256: "0".repeat(64),
        };
        assert!(matches!(
            validate_artifact(Path::new("/tmp/models"), &artifact),
            Err(ManifestError::UnsafePath(_))
        ));
    }
}
