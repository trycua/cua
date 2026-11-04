//! Tunnel frame codec: one frame per WebSocket binary message.
//!
//! ```text
//! [op:u8][stream:u32 BE][payload…]
//! ```

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Open {
        stream: u32,
        host: String,
        port: u16,
    },
    OpenOk {
        stream: u32,
    },
    OpenErr {
        stream: u32,
        reason: String,
    },
    Data {
        stream: u32,
        bytes: Vec<u8>,
    },
    Close {
        stream: u32,
    },
}

const OP_OPEN: u8 = 1;
const OP_OPEN_OK: u8 = 2;
const OP_OPEN_ERR: u8 = 3;
const OP_DATA: u8 = 4;
const OP_CLOSE: u8 = 5;

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Frame::Open { stream, host, port } => {
                out.push(OP_OPEN);
                out.extend_from_slice(&stream.to_be_bytes());
                out.extend_from_slice(&port.to_be_bytes());
                out.extend_from_slice(host.as_bytes());
            }
            Frame::OpenOk { stream } => {
                out.push(OP_OPEN_OK);
                out.extend_from_slice(&stream.to_be_bytes());
            }
            Frame::OpenErr { stream, reason } => {
                out.push(OP_OPEN_ERR);
                out.extend_from_slice(&stream.to_be_bytes());
                out.extend_from_slice(reason.as_bytes());
            }
            Frame::Data { stream, bytes } => {
                out.push(OP_DATA);
                out.extend_from_slice(&stream.to_be_bytes());
                out.extend_from_slice(bytes);
            }
            Frame::Close { stream } => {
                out.push(OP_CLOSE);
                out.extend_from_slice(&stream.to_be_bytes());
            }
        }
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Frame, &'static str> {
        if buf.len() < 5 {
            return Err("frame shorter than header");
        }
        let op = buf[0];
        let stream = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
        let payload = &buf[5..];
        match op {
            OP_OPEN => {
                if payload.len() < 2 {
                    return Err("OPEN missing port");
                }
                let port = u16::from_be_bytes([payload[0], payload[1]]);
                let host = std::str::from_utf8(&payload[2..])
                    .map_err(|_| "OPEN host is not utf8")?
                    .to_string();
                if host.is_empty() {
                    return Err("OPEN host is empty");
                }
                Ok(Frame::Open { stream, host, port })
            }
            OP_OPEN_OK => Ok(Frame::OpenOk { stream }),
            OP_OPEN_ERR => Ok(Frame::OpenErr {
                stream,
                reason: String::from_utf8_lossy(payload).into_owned(),
            }),
            OP_DATA => Ok(Frame::Data {
                stream,
                bytes: payload.to_vec(),
            }),
            OP_CLOSE => Ok(Frame::Close { stream }),
            _ => Err("unknown frame op"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_every_variant() {
        let frames = [
            Frame::Open {
                stream: 3,
                host: "example.com".into(),
                port: 443,
            },
            Frame::OpenOk { stream: 3 },
            Frame::OpenErr {
                stream: 3,
                reason: "no".into(),
            },
            Frame::Data {
                stream: 4,
                bytes: vec![0, 9, 250, 255],
            },
            Frame::Close { stream: 4 },
        ];
        for frame in frames {
            assert_eq!(frame, Frame::decode(&frame.encode()).unwrap());
        }
    }

    #[test]
    fn rejects_malformed() {
        assert!(Frame::decode(&[]).is_err());
        assert!(Frame::decode(&[OP_OPEN, 0, 0, 0, 1]).is_err());
        assert!(Frame::decode(&[200, 0, 0, 0, 1]).is_err());
    }
}
