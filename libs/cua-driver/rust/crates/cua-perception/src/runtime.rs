use std::{
    ffi::{c_void, CStr},
    fs,
    os::raw::c_char,
    path::Path,
    sync::Mutex,
};

use image::{Rgb, RgbImage};
use libloading::Library;
use ort::{
    session::{builder::GraphOptimizationLevel, Session},
    tensor::TensorElementType,
    value::Tensor,
    value::ValueType,
};
use serde_json::{json, Value};
use thiserror::Error;

use crate::{
    manifest::{DictionaryFormat, ValidatedManifest},
    postprocess::{
        ctc_decode, decode_detector, decode_ocr_probability_map, Detection, DetectorPostprocess,
        OcrPostprocess, Rect,
    },
    preprocess::{detector_tensor, ocr_detector_tensor, ocr_recognizer_tensor, ImageTensor},
};

#[derive(Debug, Clone, PartialEq)]
pub struct InferenceRegion {
    pub kind: &'static str,
    pub bounds: Rect,
    pub text: Option<String>,
    pub confidence: f32,
    pub class_id: Option<usize>,
}

#[derive(Debug, Error)]
pub enum InferenceError {
    #[error("ONNX Runtime initialization failed: {0}")]
    Runtime(String),
    #[error("model {model} is missing input {name}")]
    MissingInput { model: &'static str, name: String },
    #[error("model {model} is missing output {name}")]
    MissingOutput { model: &'static str, name: String },
    #[error("model {model} inference failed: {reason}")]
    Run { model: &'static str, reason: String },
    #[error("model {model} output is invalid: {reason}")]
    Output { model: &'static str, reason: String },
    #[error("OCR dictionary could not be read: {0}")]
    Dictionary(String),
}

pub struct OnnxBackend {
    validated: ValidatedManifest,
    detector: Mutex<Session>,
    ocr_detector: Mutex<Session>,
    ocr_recognizer: Mutex<Session>,
    dictionary: Vec<String>,
}

impl OnnxBackend {
    pub fn load(validated: ValidatedManifest) -> Result<Self, InferenceError> {
        let runtime_version = runtime_version(&validated.runtime_library_path)?;
        verify_runtime_version(&validated.manifest.onnx_runtime.version, &runtime_version)?;
        ort::init_from(validated.runtime_library_path.to_string_lossy())
            .with_name("cua-perception")
            .with_telemetry(false)
            .commit()
            .map_err(|error| InferenceError::Runtime(error.to_string()))?;

        let threads = validated.manifest.onnx_runtime.intra_threads;
        let detector = load_session(&validated.detector_path, "detector", threads)?;
        verify_io(
            &detector,
            "detector",
            &validated.manifest.detector.input_name,
            &validated.manifest.detector.output_name,
            &[
                1,
                3,
                validated.manifest.detector.input_height as i64,
                validated.manifest.detector.input_width as i64,
            ],
            3,
        )?;
        let ocr_detector = load_session(&validated.ocr_detector_path, "ocr_detector", threads)?;
        verify_io(
            &ocr_detector,
            "ocr_detector",
            &validated.manifest.ocr.detector.input_name,
            &validated.manifest.ocr.detector.output_name,
            &[1, 3, -1, -1],
            4,
        )?;
        let ocr_recognizer =
            load_session(&validated.ocr_recognizer_path, "ocr_recognizer", threads)?;
        let dictionary_source = fs::read_to_string(&validated.dictionary_path)
            .map_err(|error| InferenceError::Dictionary(error.to_string()))?;
        let dictionary =
            parse_dictionary(&dictionary_source, validated.manifest.ocr.dictionary_format)?;
        verify_recognizer_io(&ocr_recognizer, &validated, dictionary.len())?;

        Ok(Self {
            validated,
            detector: Mutex::new(detector),
            ocr_detector: Mutex::new(ocr_detector),
            ocr_recognizer: Mutex::new(ocr_recognizer),
            dictionary,
        })
    }

    pub fn identity(&self) -> Value {
        json!({
            "backend": "onnx_runtime_cpu",
            "model": {
                "name": self.validated.manifest.identity.name,
                "version": self.validated.manifest.identity.version,
                "source_revision": self.validated.manifest.identity.source_revision,
                "manifest_sha256": self.validated.manifest_sha256,
            },
            "onnx_runtime": {
                "version": self.validated.manifest.onnx_runtime.version,
                "library_sha256": self.validated.manifest.onnx_runtime.library_sha256,
                "intra_threads": self.validated.manifest.onnx_runtime.intra_threads,
            },
            "text_geometry": "axis_aligned_bounds",
            "limitations": ["rotated_text_is_returned_as_an_axis_aligned_bound"]
        })
    }

    pub fn parse(&self, image: &RgbImage) -> Result<Vec<InferenceRegion>, InferenceError> {
        let detector_manifest = &self.validated.manifest.detector;
        let tensor = detector_tensor(
            image,
            detector_manifest.input_width,
            detector_manifest.input_height,
        );
        let detector_transform = tensor.letterbox;
        let (shape, output) = run_tensor(
            &self.detector,
            "detector",
            &detector_manifest.input_name,
            &detector_manifest.output_name,
            tensor,
        )?;
        let icons = decode_detector(
            &output,
            &shape,
            detector_manifest.output_layout,
            detector_transform,
            DetectorPostprocess {
                confidence_threshold: detector_manifest.confidence_threshold,
                iou_threshold: detector_manifest.iou_threshold,
                max_candidates: detector_manifest.max_candidates,
                max_detections: detector_manifest.max_detections,
            },
        )
        .map_err(|reason| InferenceError::Output {
            model: "detector",
            reason,
        })?;

        let ocr_detector_manifest = &self.validated.manifest.ocr.detector;
        let tensor = ocr_detector_tensor(image, ocr_detector_manifest.input_width);
        let ocr_transform = tensor.letterbox;
        let (shape, output) = run_tensor(
            &self.ocr_detector,
            "ocr_detector",
            &ocr_detector_manifest.input_name,
            &ocr_detector_manifest.output_name,
            tensor,
        )?;
        let text_boxes = decode_ocr_probability_map(
            &output,
            &shape,
            ocr_transform,
            OcrPostprocess {
                pixel_threshold: ocr_detector_manifest.pixel_threshold,
                box_threshold: ocr_detector_manifest.box_threshold,
                unclip_ratio: ocr_detector_manifest.unclip_ratio,
                minimum_area: ocr_detector_manifest.minimum_area,
                minimum_side: ocr_detector_manifest.minimum_side,
                max_candidates: ocr_detector_manifest.max_candidates,
            },
        )
        .map_err(|reason| InferenceError::Output {
            model: "ocr_detector",
            reason,
        })?;

        let mut regions = icons
            .into_iter()
            .map(|detection| InferenceRegion {
                kind: "icon",
                bounds: detection.bounds,
                text: None,
                confidence: detection.score,
                class_id: Some(detection.class_id),
            })
            .collect::<Vec<_>>();
        for detection in text_boxes {
            if let Some(region) = self.recognize(image, detection)? {
                regions.push(region);
            }
        }
        regions.sort_by(|left, right| {
            left.bounds
                .y1
                .total_cmp(&right.bounds.y1)
                .then(left.bounds.x1.total_cmp(&right.bounds.x1))
                .then(left.kind.cmp(right.kind))
        });
        Ok(regions)
    }

    pub fn self_test(&self) -> Result<Value, InferenceError> {
        let image = RgbImage::from_pixel(32, 32, Rgb([255, 255, 255]));
        let regions = self.parse(&image)?;
        Ok(json!({
            "passed": true,
            "checks": ["manifest_hashes", "onnx_session_io", "synthetic_inference"],
            "synthetic_region_count": regions.len(),
            "identity": self.identity()
        }))
    }

    fn recognize(
        &self,
        image: &RgbImage,
        detection: Detection,
    ) -> Result<Option<InferenceRegion>, InferenceError> {
        let (image_width, image_height) = image.dimensions();
        let x1 = detection.bounds.x1.floor().max(0.0) as u32;
        let y1 = detection.bounds.y1.floor().max(0.0) as u32;
        let x2 = detection.bounds.x2.ceil().min(image_width as f32) as u32;
        let y2 = detection.bounds.y2.ceil().min(image_height as f32) as u32;
        if x2 <= x1 || y2 <= y1 {
            return Ok(None);
        }
        let crop = image::imageops::crop_imm(image, x1, y1, x2 - x1, y2 - y1).to_image();
        let manifest = &self.validated.manifest.ocr.recognizer;
        let tensor = ocr_recognizer_tensor(&crop, manifest.input_width, manifest.input_height);
        let (shape, output) = run_tensor(
            &self.ocr_recognizer,
            "ocr_recognizer",
            &manifest.input_name,
            &manifest.output_name,
            tensor,
        )?;
        let (text, recognition_confidence) =
            ctc_decode(&output, &shape, &self.dictionary, manifest.blank_index).map_err(
                |reason| InferenceError::Output {
                    model: "ocr_recognizer",
                    reason,
                },
            )?;
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(InferenceRegion {
            kind: "text",
            bounds: detection.bounds,
            text: Some(text),
            confidence: (detection.score * recognition_confidence).clamp(0.0, 1.0),
            class_id: None,
        }))
    }
}

fn parse_dictionary(source: &str, format: DictionaryFormat) -> Result<Vec<String>, InferenceError> {
    let values = match format {
        DictionaryFormat::PlainLines => source
            .lines()
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        DictionaryFormat::PaddleInferenceYaml => {
            let yaml: serde_yaml::Value = serde_yaml::from_str(source)
                .map_err(|error| InferenceError::Dictionary(error.to_string()))?;
            let sequence = yaml
                .get("PostProcess")
                .and_then(|value| value.get("character_dict"))
                .and_then(serde_yaml::Value::as_sequence)
                .ok_or_else(|| {
                    InferenceError::Dictionary(
                        "missing or invalid PostProcess.character_dict".to_owned(),
                    )
                })?;
            let mut values = sequence
                .iter()
                .map(|value| {
                    value.as_str().map(str::to_owned).ok_or_else(|| {
                        InferenceError::Dictionary(
                            "character_dict entries must be strings".to_owned(),
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            // Paddle's CTCLabelDecode enables a trailing space class for this
            // model family even though the YAML list contains only symbols.
            values.push(" ".to_owned());
            values
        }
    };
    if values.is_empty() || values.iter().any(String::is_empty) {
        return Err(InferenceError::Dictionary(
            "dictionary contains no symbols".to_owned(),
        ));
    }
    Ok(values)
}

fn load_session(
    path: &Path,
    model: &'static str,
    intra_threads: usize,
) -> Result<Session, InferenceError> {
    Session::builder()
        .and_then(|builder| builder.with_optimization_level(GraphOptimizationLevel::Level3))
        .and_then(|builder| builder.with_intra_threads(intra_threads))
        .and_then(|builder| builder.with_memory_pattern(false))
        .and_then(|builder| builder.commit_from_file(path))
        .map_err(|error| InferenceError::Run {
            model,
            reason: error.to_string(),
        })
}

#[repr(C)]
struct OrtApiBase {
    get_api: unsafe extern "C" fn(u32) -> *const c_void,
    get_version_string: unsafe extern "C" fn() -> *const c_char,
}

fn runtime_version(path: &Path) -> Result<String, InferenceError> {
    // SAFETY: the library was hash-verified before this call. OrtGetApiBase is
    // the stable ONNX Runtime C ABI entry point, and the library remains loaded
    // while its version string is read into an owned Rust string.
    unsafe {
        let library =
            Library::new(path).map_err(|error| InferenceError::Runtime(error.to_string()))?;
        let get_api_base: libloading::Symbol<'_, unsafe extern "C" fn() -> *const OrtApiBase> =
            library
                .get(b"OrtGetApiBase\0")
                .map_err(|error| InferenceError::Runtime(error.to_string()))?;
        let api_base = get_api_base();
        if api_base.is_null() {
            return Err(InferenceError::Runtime(
                "OrtGetApiBase returned null".to_owned(),
            ));
        }
        let version = ((*api_base).get_version_string)();
        if version.is_null() {
            return Err(InferenceError::Runtime(
                "GetVersionString returned null".to_owned(),
            ));
        }
        CStr::from_ptr(version)
            .to_str()
            .map(str::to_owned)
            .map_err(|error| InferenceError::Runtime(error.to_string()))
    }
}

fn verify_runtime_version(declared: &str, actual: &str) -> Result<(), InferenceError> {
    if declared == actual {
        Ok(())
    } else {
        Err(InferenceError::Runtime(format!(
            "manifest declares ONNX Runtime {declared}, but library reports {actual}"
        )))
    }
}

fn verify_io(
    session: &Session,
    model: &'static str,
    input_name: &str,
    output_name: &str,
    expected_input_shape: &[i64],
    expected_output_rank: usize,
) -> Result<(), InferenceError> {
    let input = session
        .inputs
        .iter()
        .find(|input| input.name == input_name)
        .ok_or_else(|| InferenceError::MissingInput {
            model,
            name: input_name.to_owned(),
        })?;
    verify_tensor_type(
        model,
        "input",
        &input.input_type,
        expected_input_shape.len(),
    )?;
    verify_shape(model, "input", &input.input_type, expected_input_shape)?;
    let output = session
        .outputs
        .iter()
        .find(|output| output.name == output_name)
        .ok_or_else(|| InferenceError::MissingOutput {
            model,
            name: output_name.to_owned(),
        })?;
    verify_tensor_type(model, "output", &output.output_type, expected_output_rank)?;
    Ok(())
}

fn verify_recognizer_io(
    session: &Session,
    validated: &ValidatedManifest,
    dictionary_len: usize,
) -> Result<(), InferenceError> {
    let manifest = &validated.manifest.ocr.recognizer;
    verify_io(
        session,
        "ocr_recognizer",
        &manifest.input_name,
        &manifest.output_name,
        &[1, 3, manifest.input_height as i64, -1],
        3,
    )?;
    let output = session
        .outputs
        .iter()
        .find(|output| output.name == manifest.output_name)
        .expect("verify_io proved the output exists");
    if let ValueType::Tensor { shape, .. } = &output.output_type {
        let classes = shape.last().copied().unwrap_or(-1);
        let expected = (dictionary_len + 1) as i64;
        if classes != expected {
            return Err(InferenceError::Output {
                model: "ocr_recognizer",
                reason: format!(
                    "output has {classes} classes; dictionary plus blank requires {expected}"
                ),
            });
        }
    }
    Ok(())
}

fn verify_tensor_type(
    model: &'static str,
    route: &str,
    value_type: &ValueType,
    expected_rank: usize,
) -> Result<(), InferenceError> {
    match value_type {
        ValueType::Tensor { ty, shape, .. }
            if *ty == TensorElementType::Float32 && shape.len() == expected_rank =>
        {
            Ok(())
        }
        other => Err(InferenceError::Output {
            model,
            reason: format!("{route} must be a rank-{expected_rank} float32 tensor, got {other:?}"),
        }),
    }
}

fn verify_shape(
    model: &'static str,
    route: &str,
    value_type: &ValueType,
    expected: &[i64],
) -> Result<(), InferenceError> {
    let ValueType::Tensor { shape, .. } = value_type else {
        unreachable!("verify_tensor_type already checked the value type")
    };
    if shape.len() != expected.len()
        || shape
            .iter()
            .zip(expected)
            .any(|(actual, expected)| *expected >= 0 && *actual >= 0 && actual != expected)
    {
        return Err(InferenceError::Output {
            model,
            reason: format!("{route} shape {shape:?} does not match {expected:?}"),
        });
    }
    Ok(())
}

fn run_tensor(
    session: &Mutex<Session>,
    model: &'static str,
    input_name: &str,
    output_name: &str,
    tensor: ImageTensor,
) -> Result<(Vec<i64>, Vec<f32>), InferenceError> {
    let input =
        Tensor::from_array((tensor.shape, tensor.data.into_boxed_slice())).map_err(|error| {
            InferenceError::Run {
                model,
                reason: error.to_string(),
            }
        })?;
    let mut session = session.lock().map_err(|_| InferenceError::Run {
        model,
        reason: "model session lock was poisoned".to_owned(),
    })?;
    let outputs = session
        .run(vec![(input_name.to_owned(), input)])
        .map_err(|error| InferenceError::Run {
            model,
            reason: error.to_string(),
        })?;
    let output = outputs
        .get(output_name)
        .ok_or_else(|| InferenceError::MissingOutput {
            model,
            name: output_name.to_owned(),
        })?;
    let (shape, data) =
        output
            .try_extract_tensor::<f32>()
            .map_err(|error| InferenceError::Output {
                model,
                reason: error.to_string(),
            })?;
    Ok((shape.to_vec(), data.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_dictionary_preserves_spaces() {
        assert_eq!(
            parse_dictionary("a\r\n \n b \n", DictionaryFormat::PlainLines).unwrap(),
            ["a", " ", " b "]
        );
    }

    #[test]
    fn paddle_yaml_dictionary_parses_quoted_symbols_and_rejects_bad_entries() {
        let parsed = parse_dictionary(
            "PostProcess:\n  character_dict: [\"#\", 'a b', \" \" ]\n",
            DictionaryFormat::PaddleInferenceYaml,
        )
        .unwrap();
        assert_eq!(parsed, ["#", "a b", " ", " "]);

        for invalid in [
            "PostProcess:\n  character_dict: [a, 7]\n",
            "PostProcess:\n  character_dict: [\"\"]\n",
            "PostProcess: {}\n",
        ] {
            assert!(parse_dictionary(invalid, DictionaryFormat::PaddleInferenceYaml).is_err());
        }
    }

    #[test]
    fn runtime_version_must_match_manifest_exactly() {
        verify_runtime_version("1.26.0", "1.26.0").unwrap();
        assert!(verify_runtime_version("1.25.0", "1.26.0").is_err());
    }
}
