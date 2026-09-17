//! Protocol-only skeleton for the optional `cua-perception` worker.
//!
//! Version 1 uses a 4-byte unsigned big-endian payload length followed by that
//! many bytes of UTF-8 JSON. Each request frame produces exactly one response
//! frame. Payloads are capped before allocation. This crate has no screen,
//! input, browser, or network integration; parsing recognizes only the
//! synthetic fixture embedded below until a separately reviewed inference
//! runtime is added.
//!
//! Framing errors are terminal because the byte stream cannot be safely
//! resynchronized; the worker responds once and exits. Valid frames containing
//! malformed JSON or invalid request schemas receive structured errors and the
//! worker continues with the next frame. The fixture response is not the public
//! visual-region contract: public typed regions belong to the separate
//! `cua-driver-contract` pull request.

use std::io::{self, Cursor, Read, Write};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{ImageError, ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const PROTOCOL_VERSION: &str = "cua-perception/1";
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_IMAGE_DIMENSION: u32 = 16_384;
pub const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
const MAX_DECODE_ALLOC_BYTES: u64 = MAX_IMAGE_PIXELS * 8;

const FIXTURE_PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const FIXTURE_SHA256: &str = "431ced6916a2a21a156e38701afe55bbd7f88969fbbfc56d7fe099d47f265460";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    Empty,
    Oversized { declared: usize, maximum: usize },
    TruncatedPrefix { received: usize },
    TruncatedPayload { declared: usize, received: usize },
    Io(String),
}

impl FrameError {
    fn code(&self) -> &'static str {
        match self {
            Self::Empty
            | Self::Oversized { .. }
            | Self::TruncatedPrefix { .. }
            | Self::TruncatedPayload { .. } => "invalid_frame",
            Self::Io(_) => "io_error",
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Empty => "frame payload must not be empty".to_owned(),
            Self::Oversized { declared, maximum } => {
                format!("frame declares {declared} bytes; maximum is {maximum}")
            }
            Self::TruncatedPrefix { received } => {
                format!("frame length prefix is incomplete: received {received} of 4 bytes")
            }
            Self::TruncatedPayload { declared, received } => {
                format!("frame declares {declared} bytes but only {received} were received")
            }
            Self::Io(message) => message.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol: String,
    request_id: String,
    #[serde(flatten)]
    command: Command,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum Command {
    Health(EmptyParams),
    SelfTest(EmptyParams),
    Parse(ParseParams),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyParams {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParseParams {
    capture_id: String,
    image: ImageInput,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageInput {
    media_type: String,
    width: u32,
    height: u32,
    byte_length: u64,
    data_base64: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Response {
    protocol: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
    #[serde(flatten)]
    outcome: Outcome,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Outcome {
    Ok { result: Value },
    Error { error: ProtocolError },
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ProtocolError {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl Response {
    fn ok(request_id: String, result: Value) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            request_id: Some(request_id),
            outcome: Outcome::Ok { result },
        }
    }

    fn error(
        request_id: Option<String>,
        code: &'static str,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            request_id,
            outcome: Outcome::Error {
                error: ProtocolError {
                    code,
                    message: message.into(),
                    details,
                },
            },
        }
    }

    pub fn from_frame_error(error: &FrameError) -> Self {
        Self::error(None, error.code(), error.message(), None)
    }
}

pub fn read_frame(reader: &mut impl Read) -> Result<Option<Vec<u8>>, FrameError> {
    let mut prefix = [0_u8; 4];
    let mut prefix_read = 0;
    while prefix_read < prefix.len() {
        match reader.read(&mut prefix[prefix_read..]) {
            Ok(0) if prefix_read == 0 => return Ok(None),
            Ok(0) => {
                return Err(FrameError::TruncatedPrefix {
                    received: prefix_read,
                })
            }
            Ok(count) => prefix_read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameError::Io(error.to_string())),
        }
    }

    let declared = u32::from_be_bytes(prefix) as usize;
    if declared == 0 {
        return Err(FrameError::Empty);
    }
    if declared > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized {
            declared,
            maximum: MAX_FRAME_BYTES,
        });
    }

    let mut payload = vec![0_u8; declared];
    let mut received = 0;
    while received < declared {
        match reader.read(&mut payload[received..]) {
            Ok(0) => return Err(FrameError::TruncatedPayload { declared, received }),
            Ok(count) => received += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameError::Io(error.to_string())),
        }
    }
    Ok(Some(payload))
}

pub fn write_frame(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "frame contains {} bytes; maximum is {MAX_FRAME_BYTES}",
                payload.len()
            ),
        ));
    }
    let length = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame exceeds u32 length"))?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(payload)?;
    writer.flush()
}

pub fn handle_payload(payload: &[u8]) -> Response {
    let value: Value = match serde_json::from_slice(payload) {
        Ok(value) => value,
        Err(error) => {
            return Response::error(
                None,
                "invalid_json",
                "request payload is not valid JSON",
                Some(json!({ "reason": error.to_string() })),
            )
        }
    };
    let request_id = value
        .get("request_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let request: Request = match serde_json::from_value(value) {
        Ok(request) => request,
        Err(error) => {
            return Response::error(
                request_id,
                "invalid_request",
                "request does not match the v1 schema",
                Some(json!({ "reason": error.to_string() })),
            )
        }
    };

    if request.protocol != PROTOCOL_VERSION {
        return Response::error(
            Some(request.request_id),
            "incompatible_protocol",
            format!("unsupported protocol version: {}", request.protocol),
            Some(json!({ "supported": [PROTOCOL_VERSION] })),
        );
    }
    if request.request_id.is_empty() || request.request_id.len() > 128 {
        return Response::error(
            Some(request.request_id),
            "invalid_request",
            "request_id must contain 1 to 128 bytes",
            None,
        );
    }

    match request.command {
        Command::Health(_) => Response::ok(
            request.request_id,
            json!({
                "ready": true,
                "protocol": PROTOCOL_VERSION,
                "runtime": "fixture_only",
                "capabilities": ["health", "self_test", "parse_fixture"]
            }),
        ),
        Command::SelfTest(_) => match run_self_test() {
            Ok(result) => Response::ok(request.request_id, result),
            Err(error) => Response::error(
                Some(request.request_id),
                "self_test_failed",
                error.message,
                error.details,
            ),
        },
        Command::Parse(params) => match parse_fixture(params) {
            Ok(result) => Response::ok(request.request_id, result),
            Err(error) => Response::error(
                Some(request.request_id),
                error.code,
                error.message,
                error.details,
            ),
        },
    }
}

struct WorkerError {
    code: &'static str,
    message: String,
    details: Option<Value>,
}

impl WorkerError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

fn parse_fixture(params: ParseParams) -> Result<Value, WorkerError> {
    if params.capture_id.is_empty() || params.capture_id.len() > 256 {
        return Err(WorkerError::new(
            "invalid_request",
            "capture_id must contain 1 to 256 bytes",
        ));
    }
    let bytes = validate_image(&params.image)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != FIXTURE_SHA256 {
        return Err(WorkerError::new(
            "unsupported_fixture",
            "the protocol skeleton accepts only its synthetic test fixture",
        )
        .with_details(json!({ "sha256": digest })));
    }

    Ok(fixture_result(&params.capture_id))
}

fn validate_image(image: &ImageInput) -> Result<Vec<u8>, WorkerError> {
    if image.media_type != "image/png" {
        return Err(WorkerError::new(
            "invalid_image",
            "media_type must be image/png in protocol v1",
        ));
    }
    if image.width == 0
        || image.height == 0
        || image.width > MAX_IMAGE_DIMENSION
        || image.height > MAX_IMAGE_DIMENSION
        || u64::from(image.width) * u64::from(image.height) > MAX_IMAGE_PIXELS
    {
        return Err(
            WorkerError::new("invalid_image", "image dimensions exceed protocol limits")
                .with_details(json!({
                    "width": image.width,
                    "height": image.height,
                    "max_dimension": MAX_IMAGE_DIMENSION,
                    "max_pixels": MAX_IMAGE_PIXELS
                })),
        );
    }
    if image.byte_length == 0 || image.byte_length > MAX_IMAGE_BYTES {
        return Err(
            WorkerError::new("invalid_image", "image byte_length exceeds protocol limits")
                .with_details(json!({
                    "byte_length": image.byte_length,
                    "max_bytes": MAX_IMAGE_BYTES
                })),
        );
    }
    let byte_length = usize::try_from(image.byte_length).map_err(|_| {
        WorkerError::new(
            "invalid_image",
            "image byte_length cannot be represented on this platform",
        )
    })?;
    let expected_base64_length = byte_length.div_ceil(3) * 4;
    if image.data_base64.len() != expected_base64_length {
        return Err(WorkerError::new(
            "invalid_image",
            "data_base64 length is inconsistent with byte_length",
        )
        .with_details(json!({
            "declared_byte_length": image.byte_length,
            "expected_base64_length": expected_base64_length,
            "actual_base64_length": image.data_base64.len()
        })));
    }

    let bytes = BASE64.decode(&image.data_base64).map_err(|error| {
        WorkerError::new("invalid_image", "data_base64 is not valid base64")
            .with_details(json!({ "reason": error.to_string() }))
    })?;
    if bytes.len() != byte_length {
        return Err(WorkerError::new(
            "invalid_image",
            "decoded image length does not match byte_length",
        )
        .with_details(json!({
            "declared": byte_length,
            "actual": bytes.len()
        })));
    }

    let actual = png_reader(&bytes)
        .into_dimensions()
        .map_err(map_image_error)?;
    if u64::from(actual.0) * u64::from(actual.1) > MAX_IMAGE_PIXELS {
        return Err(WorkerError::new(
            "invalid_image",
            "decoded PNG dimensions exceed image resource limits",
        )
        .with_details(json!({
            "actual": { "width": actual.0, "height": actual.1 },
            "max_pixels": MAX_IMAGE_PIXELS
        })));
    }
    if actual != (image.width, image.height) {
        return Err(WorkerError::new(
            "invalid_image",
            "decoded PNG dimensions do not match metadata",
        )
        .with_details(json!({
            "declared": { "width": image.width, "height": image.height },
            "actual": { "width": actual.0, "height": actual.1 }
        })));
    }
    png_reader(&bytes).decode().map_err(map_image_error)?;
    Ok(bytes)
}

fn png_reader(bytes: &[u8]) -> ImageReader<Cursor<&[u8]>> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    reader.limits(limits);
    reader
}

fn map_image_error(error: ImageError) -> WorkerError {
    match error {
        ImageError::Limits(_) => {
            WorkerError::new("invalid_image", "decoded PNG exceeds image resource limits")
                .with_details(json!({ "reason": error.to_string() }))
        }
        _ => WorkerError::new("invalid_image", "image bytes are not a valid PNG")
            .with_details(json!({ "reason": error.to_string() })),
    }
}

fn fixture_result(capture_id: &str) -> Value {
    json!({
        "capture_id": capture_id,
        "image": {
            "sha256": FIXTURE_SHA256,
            "width": 1,
            "height": 1
        },
        "coordinate_space": "image_pixels",
        "regions": [
            {
                "id": "fixture-text-1",
                "kind": "text",
                "bounds": { "x": 0, "y": 0, "width": 1, "height": 1 },
                "text": "fixture",
                "confidence": 1.0
            }
        ],
        "runtime": "fixture_only"
    })
}

fn run_self_test() -> Result<Value, WorkerError> {
    let bytes = BASE64
        .decode(FIXTURE_PNG_BASE64)
        .map_err(|error| WorkerError::new("self_test_failed", error.to_string()))?;
    let result = parse_fixture(ParseParams {
        capture_id: "self-test".to_owned(),
        image: ImageInput {
            media_type: "image/png".to_owned(),
            width: 1,
            height: 1,
            byte_length: bytes.len() as u64,
            data_base64: FIXTURE_PNG_BASE64.to_owned(),
        },
    })?;
    if result != fixture_result("self-test") {
        return Err(WorkerError::new(
            "self_test_failed",
            "fixture parser produced an unexpected result",
        ));
    }
    Ok(json!({
        "passed": true,
        "checks": ["fixture_decode", "image_validation", "known_answer_fixture"],
        "fixture_sha256": FIXTURE_SHA256
    }))
}
