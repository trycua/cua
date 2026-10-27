//! The Connect protocol (https://connectrpc.com/docs/protocol), server side,
//! with the JSON codec: unary calls (`application/json`), server streams and
//! client streams (`application/connect+json` envelopes). Our own
//! implementation of the published protocol; no generated code.

use axum::body::Body;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures_util::{Stream, StreamExt};
use http::{HeaderMap, Response, StatusCode, header};
use http_body_util::BodyExt;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::time::Duration;

/// Connect error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Canceled,
    Unknown,
    InvalidArgument,
    DeadlineExceeded,
    NotFound,
    AlreadyExists,
    PermissionDenied,
    ResourceExhausted,
    FailedPrecondition,
    Aborted,
    OutOfRange,
    Unimplemented,
    Internal,
    Unavailable,
    DataLoss,
    Unauthenticated,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Canceled => "canceled",
            Code::Unknown => "unknown",
            Code::InvalidArgument => "invalid_argument",
            Code::DeadlineExceeded => "deadline_exceeded",
            Code::NotFound => "not_found",
            Code::AlreadyExists => "already_exists",
            Code::PermissionDenied => "permission_denied",
            Code::ResourceExhausted => "resource_exhausted",
            Code::FailedPrecondition => "failed_precondition",
            Code::Aborted => "aborted",
            Code::OutOfRange => "out_of_range",
            Code::Unimplemented => "unimplemented",
            Code::Internal => "internal",
            Code::Unavailable => "unavailable",
            Code::DataLoss => "data_loss",
            Code::Unauthenticated => "unauthenticated",
        }
    }

    /// The HTTP status of a unary error response (Connect spec table).
    pub fn http_status(self) -> StatusCode {
        let n = match self {
            Code::Canceled => 499,
            Code::Unknown | Code::Internal | Code::DataLoss => 500,
            Code::InvalidArgument | Code::FailedPrecondition | Code::OutOfRange => 400,
            Code::DeadlineExceeded => 504,
            Code::NotFound => 404,
            Code::AlreadyExists | Code::Aborted => 409,
            Code::PermissionDenied => 403,
            Code::ResourceExhausted => 429,
            Code::Unimplemented => 501,
            Code::Unavailable => 503,
            Code::Unauthenticated => 401,
        };
        StatusCode::from_u16(n).expect("valid status")
    }

    pub fn from_tonic(code: tonic::Code) -> Code {
        use tonic::Code as T;
        match code {
            T::Ok | T::Unknown => Code::Unknown,
            T::Cancelled => Code::Canceled,
            T::InvalidArgument => Code::InvalidArgument,
            T::DeadlineExceeded => Code::DeadlineExceeded,
            T::NotFound => Code::NotFound,
            T::AlreadyExists => Code::AlreadyExists,
            T::PermissionDenied => Code::PermissionDenied,
            T::ResourceExhausted => Code::ResourceExhausted,
            T::FailedPrecondition => Code::FailedPrecondition,
            T::Aborted => Code::Aborted,
            T::OutOfRange => Code::OutOfRange,
            T::Unimplemented => Code::Unimplemented,
            T::Internal => Code::Internal,
            T::Unavailable => Code::Unavailable,
            T::DataLoss => Code::DataLoss,
            T::Unauthenticated => Code::Unauthenticated,
        }
    }
}

/// An RPC error: a Connect code and a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: Code,
    pub message: String,
}

impl RpcError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(Code::InvalidArgument, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(Code::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(Code::Internal, message)
    }

    pub fn to_json(&self) -> Value {
        json!({"code": self.code.as_str(), "message": self.message})
    }

    /// A unary error response.
    pub fn into_response(self) -> Response<Body> {
        Response::builder()
            .status(self.code.http_status())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(self.to_json().to_string()))
            .expect("valid response")
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl From<tonic::Status> for RpcError {
    fn from(s: tonic::Status) -> Self {
        RpcError::new(Code::from_tonic(s.code()), s.message().to_string())
    }
}

/// Envelope flag: compressed message.
const FLAG_COMPRESSED: u8 = 0x01;
/// Envelope flag: end of stream.
const FLAG_END: u8 = 0x02;
/// Largest envelope we accept (64 MiB): bounds memory per message.
pub const MAX_MESSAGE: usize = 64 << 20;

/// Frames one envelope.
pub fn envelope(flags: u8, payload: &[u8]) -> Bytes {
    let mut b = BytesMut::with_capacity(5 + payload.len());
    b.put_u8(flags);
    b.put_u32(payload.len() as u32);
    b.put_slice(payload);
    b.freeze()
}

/// Whether the request is a streaming (enveloped) Connect call.
pub fn is_streaming(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/connect+"))
}

/// The `Connect-Timeout-Ms` deadline, when set.
pub fn timeout(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get("connect-timeout-ms")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
}

tokio::task_local! {
    /// The Go name of the request message being decoded (`process.SendSignalRequest`).
    static REQUEST_TYPE: String;
}

/// Runs `f` with the request message's Go type name, which decode errors
/// of a JSON request quote the way connect-go's `protojson` does.
pub async fn with_request_type<F: std::future::Future>(name: String, f: F) -> F::Output {
    REQUEST_TYPE.scope(name, f).await
}

/// protojson's text for a number that is not a uint32, with the line and
/// column of the value: `unmarshal message: unmarshal into *process.StartRequest:
/// proto: (line 1:19): invalid value for uint32 field pid: -5`.
fn uint32_error(bytes: &[u8]) -> Option<String> {
    const FIELDS: [&str; 4] = ["pid", "cols", "rows", "depth"];
    fn find(v: &serde_json::Value) -> Option<(String, String)> {
        let o = v.as_object()?;
        for (k, x) in o {
            if FIELDS.contains(&k.as_str())
                && let serde_json::Value::Number(n) = x
                && n.as_u64().and_then(|n| u32::try_from(n).ok()).is_none()
            {
                return Some((k.clone(), n.to_string()));
            }
            if let Some(found) = find(x) {
                return Some(found);
            }
        }
        None
    }
    let ty = REQUEST_TYPE.try_with(Clone::clone).ok()?;
    let text = std::str::from_utf8(bytes).ok()?;
    let (field, shown) = find(&serde_json::from_str(text).ok()?)?;
    let key = text.find(&format!("\"{field}\""))?;
    let rest = &text[key + field.len() + 2..];
    let colon = rest.find(':')?;
    let value_at = key + field.len() + 2 + colon + 1;
    let value_at = value_at + text[value_at..].len() - text[value_at..].trim_start().len();
    let before = &text[..value_at];
    let line = before.matches('\n').count() + 1;
    let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    Some(format!(
        "unmarshal message: unmarshal into *{ty}: proto: (line {line}:{col}): invalid value for uint32 field {field}: {shown}"
    ))
}

pub(crate) fn decode_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, RpcError> {
    let bytes = if bytes.is_empty() {
        b"{}" as &[u8]
    } else {
        bytes
    };
    serde_json::from_slice(bytes).map_err(|e| {
        RpcError::invalid(uint32_error(bytes).unwrap_or_else(|| format!("invalid request: {e}")))
    })
}

/// Reads the request message of a unary or server-streaming call.
pub async fn read_request<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Body,
) -> Result<T, RpcError> {
    if headers
        .get("connect-content-encoding")
        .or_else(|| headers.get(header::CONTENT_ENCODING))
        .and_then(|v| v.to_str().ok())
        .is_some_and(|e| e != "identity")
    {
        return Err(RpcError::new(
            Code::Unimplemented,
            "compressed requests are not supported",
        ));
    }
    let bytes = http_body_util::Limited::new(body, MAX_MESSAGE)
        .collect()
        .await
        .map_err(|e| RpcError::invalid(format!("reading request: {e}")))?
        .to_bytes();
    if !is_streaming(headers) {
        return decode_json(&bytes);
    }
    let mut buf = bytes;
    let (flags, payload) =
        take_envelope(&mut buf)?.ok_or_else(|| RpcError::invalid("missing request message"))?;
    if flags & FLAG_COMPRESSED != 0 {
        return Err(RpcError::new(
            Code::Unimplemented,
            "compressed messages are not supported",
        ));
    }
    decode_json(&payload)
}

/// Splits one envelope off the front of `buf`, if complete.
pub(crate) fn take_envelope(buf: &mut Bytes) -> Result<Option<(u8, Bytes)>, RpcError> {
    if buf.len() < 5 {
        return Ok(None);
    }
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    if len > MAX_MESSAGE {
        return Err(RpcError::new(
            Code::ResourceExhausted,
            format!("message of {len} bytes exceeds the {MAX_MESSAGE} byte limit"),
        ));
    }
    if buf.len() < 5 + len {
        return Ok(None);
    }
    let flags = buf[0];
    buf.advance(5);
    Ok(Some((flags, buf.split_to(len))))
}

/// The messages of a client stream, decoded as they arrive.
pub fn client_stream<T: DeserializeOwned + Send + 'static>(
    body: Body,
) -> impl Stream<Item = Result<T, RpcError>> + Send + 'static {
    async_stream(body)
}

fn async_stream<T: DeserializeOwned + Send + 'static>(
    body: Body,
) -> impl Stream<Item = Result<T, RpcError>> + Send + 'static {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<T, RpcError>>(16);
    tokio::spawn(async move {
        let mut data = body.into_data_stream();
        let mut buf = Bytes::new();
        loop {
            loop {
                match take_envelope(&mut buf) {
                    Ok(Some((flags, payload))) => {
                        if flags & FLAG_END != 0 {
                            return;
                        }
                        let item = if flags & FLAG_COMPRESSED != 0 {
                            Err(RpcError::new(
                                Code::Unimplemented,
                                "compressed messages are not supported",
                            ))
                        } else {
                            decode_json(&payload)
                        };
                        let stop = item.is_err();
                        if tx.send(item).await.is_err() || stop {
                            return;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                }
            }
            match data.next().await {
                Some(Ok(chunk)) => {
                    let mut joined = BytesMut::with_capacity(buf.len() + chunk.len());
                    joined.extend_from_slice(&buf);
                    joined.extend_from_slice(&chunk);
                    buf = joined.freeze();
                }
                Some(Err(e)) => {
                    let _ = tx
                        .send(Err(RpcError::new(Code::Canceled, e.to_string())))
                        .await;
                    return;
                }
                None => return,
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(rx)
}

/// A unary response.
pub fn unary(result: Result<Value, RpcError>) -> Response<Body> {
    match result {
        Ok(v) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .expect("valid response"),
        Err(e) => e.into_response(),
    }
}

/// A streaming response (server stream, or a client stream's single reply):
/// each `Ok` is one message; the first `Err` ends the stream with that
/// error; the end-of-stream envelope is always written.
pub fn streaming<S>(messages: S) -> Response<Body>
where
    S: Stream<Item = Result<Value, RpcError>> + Send + 'static,
{
    let frames = futures_util::stream::unfold(
        (Box::pin(messages), false),
        |(mut messages, done)| async move {
            if done {
                return None;
            }
            match messages.next().await {
                Some(Ok(v)) => Some((
                    Ok::<_, std::convert::Infallible>(envelope(0, v.to_string().as_bytes())),
                    (messages, false),
                )),
                Some(Err(e)) => Some((
                    Ok(envelope(
                        FLAG_END,
                        json!({"error": e.to_json()}).to_string().as_bytes(),
                    )),
                    (messages, true),
                )),
                None => Some((Ok(envelope(FLAG_END, b"{}")), (messages, true))),
            }
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/connect+json")
        .body(Body::from_stream(frames))
        .expect("valid response")
}

/// A streaming response that fails before any message.
pub fn streaming_error(e: RpcError) -> Response<Body> {
    streaming(futures_util::stream::iter([Err(e)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trip() {
        let mut b = envelope(0, br#"{"a":1}"#);
        let (flags, payload) = take_envelope(&mut b).unwrap().unwrap();
        assert_eq!(flags, 0);
        assert_eq!(&payload[..], br#"{"a":1}"#);
        assert!(b.is_empty());
    }

    #[test]
    fn partial_envelope_waits() {
        let full = envelope(0, b"{}");
        let mut part = full.slice(0..4);
        assert!(take_envelope(&mut part).unwrap().is_none());
    }

    #[test]
    fn oversized_envelope_is_rejected() {
        let mut b = Bytes::from_static(&[0, 0xff, 0xff, 0xff, 0xff]);
        assert_eq!(
            take_envelope(&mut b).unwrap_err().code,
            Code::ResourceExhausted
        );
    }

    #[test]
    fn error_statuses_follow_the_spec() {
        assert_eq!(Code::NotFound.http_status().as_u16(), 404);
        assert_eq!(Code::Unavailable.http_status().as_u16(), 503);
        assert_eq!(Code::DeadlineExceeded.http_status().as_u16(), 504);
        assert_eq!(Code::AlreadyExists.http_status().as_u16(), 409);
        assert_eq!(Code::from_tonic(tonic::Code::NotFound), Code::NotFound);
    }

    #[tokio::test]
    async fn streaming_writes_end_of_stream() {
        let resp = streaming(futures_util::stream::iter([
            Ok(json!({"x": 1})),
            Err(RpcError::not_found("gone")),
        ]));
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let mut b = body;
        let (f1, p1) = take_envelope(&mut b).unwrap().unwrap();
        assert_eq!((f1, &p1[..]), (0, &br#"{"x":1}"#[..]));
        let (f2, p2) = take_envelope(&mut b).unwrap().unwrap();
        assert_eq!(f2, FLAG_END);
        let v: Value = serde_json::from_slice(&p2).unwrap();
        assert_eq!(v["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn client_stream_decodes_split_chunks() {
        let a = envelope(0, br#"{"n":1}"#);
        let b = envelope(0, br#"{"n":2}"#);
        let mut all = BytesMut::new();
        all.extend_from_slice(&a);
        all.extend_from_slice(&b);
        let all = all.freeze();
        let chunks: Vec<Result<Bytes, std::io::Error>> =
            vec![Ok(all.slice(0..3)), Ok(all.slice(3..9)), Ok(all.slice(9..))];
        let body = Body::from_stream(futures_util::stream::iter(chunks));
        let got: Vec<Value> = client_stream::<Value>(body)
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(got, vec![json!({"n":1}), json!({"n":2})]);
    }
}
