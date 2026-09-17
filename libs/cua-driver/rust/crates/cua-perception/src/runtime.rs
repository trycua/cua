use std::{fs, path::Path, sync::Mutex};

use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use ort::{
    session::{builder::GraphOptimizationLevel, Session},
    value::Tensor,
};
use serde_json::{json, Value};
use thiserror::Error;

use crate::{
    manifest::{DictionaryFormat, ValidatedManifest},
    postprocess::{
        ctc_decode, decode_detector, decode_ocr_probability_map, Detection, OcrPostprocess, Rect,
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
        )?;
        let ocr_detector = load_session(&validated.ocr_detector_path, "ocr_detector", threads)?;
        verify_io(
            &ocr_detector,
            "ocr_detector",
            &validated.manifest.ocr.detector.input_name,
            &validated.manifest.ocr.detector.output_name,
        )?;
        let ocr_recognizer =
            load_session(&validated.ocr_recognizer_path, "ocr_recognizer", threads)?;
        verify_io(
            &ocr_recognizer,
            "ocr_recognizer",
            &validated.manifest.ocr.recognizer.input_name,
            &validated.manifest.ocr.recognizer.output_name,
        )?;
        let dictionary_source = fs::read_to_string(&validated.dictionary_path)
            .map_err(|error| InferenceError::Dictionary(error.to_string()))?;
        let dictionary =
            parse_dictionary(&dictionary_source, validated.manifest.ocr.dictionary_format)?;
        if dictionary.is_empty() {
            return Err(InferenceError::Dictionary(
                "dictionary contains no symbols".to_owned(),
            ));
        }

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
            }
        })
    }

    pub fn parse(&self, image: &DynamicImage) -> Result<Vec<InferenceRegion>, InferenceError> {
        let detector_manifest = &self.validated.manifest.detector;
        let tensor = detector_tensor(
            image,
            detector_manifest.input_width,
            detector_manifest.input_height,
        );
        let (shape, output) = run_tensor(
            &self.detector,
            "detector",
            &detector_manifest.input_name,
            &detector_manifest.output_name,
            tensor.clone(),
        )?;
        let icons = decode_detector(
            &output,
            &shape,
            detector_manifest.output_layout,
            tensor.letterbox,
            detector_manifest.confidence_threshold,
            detector_manifest.iou_threshold,
        )
        .map_err(|reason| InferenceError::Output {
            model: "detector",
            reason,
        })?;

        let ocr_detector_manifest = &self.validated.manifest.ocr.detector;
        let tensor = ocr_detector_tensor(image, ocr_detector_manifest.input_width);
        let (shape, output) = run_tensor(
            &self.ocr_detector,
            "ocr_detector",
            &ocr_detector_manifest.input_name,
            &ocr_detector_manifest.output_name,
            tensor.clone(),
        )?;
        let text_boxes = decode_ocr_probability_map(
            &output,
            &shape,
            tensor.letterbox,
            OcrPostprocess {
                pixel_threshold: ocr_detector_manifest.pixel_threshold,
                box_threshold: ocr_detector_manifest.box_threshold,
                unclip_ratio: ocr_detector_manifest.unclip_ratio,
                minimum_area: ocr_detector_manifest.minimum_area,
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
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(32, 32, Rgb([255, 255, 255])));
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
        image: &DynamicImage,
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
        let crop = image.crop_imm(x1, y1, x2 - x1, y2 - y1);
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
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        DictionaryFormat::PaddleInferenceYaml => {
            let marker = "  character_dict:\n";
            let block = source
                .split_once(marker)
                .map(|(_, block)| block)
                .ok_or_else(|| {
                    InferenceError::Dictionary("missing PostProcess.character_dict".to_owned())
                })?;
            let mut values = block
                .lines()
                .take_while(|line| line.starts_with("  - "))
                .map(|line| parse_yaml_scalar(&line[4..]))
                .collect::<Result<Vec<_>, _>>()?;
            // Paddle's CTCLabelDecode enables a trailing space class for this
            // model family even though the YAML list contains only symbols.
            values.push(" ".to_owned());
            values
        }
    };
    if values.is_empty() {
        return Err(InferenceError::Dictionary(
            "dictionary contains no symbols".to_owned(),
        ));
    }
    Ok(values)
}

fn parse_yaml_scalar(value: &str) -> Result<String, InferenceError> {
    let value = value.trim();
    if value.starts_with('"') && value.ends_with('"') {
        return serde_json::from_str(value)
            .map_err(|error| InferenceError::Dictionary(error.to_string()));
    }
    if value.starts_with('\'') && value.ends_with('\'') {
        return Ok(value[1..value.len() - 1].replace("''", "'"));
    }
    Ok(value.to_owned())
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

fn verify_io(
    session: &Session,
    model: &'static str,
    input_name: &str,
    output_name: &str,
) -> Result<(), InferenceError> {
    if !session.inputs.iter().any(|input| input.name == input_name) {
        return Err(InferenceError::MissingInput {
            model,
            name: input_name.to_owned(),
        });
    }
    if !session
        .outputs
        .iter()
        .any(|output| output.name == output_name)
    {
        return Err(InferenceError::MissingOutput {
            model,
            name: output_name.to_owned(),
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
