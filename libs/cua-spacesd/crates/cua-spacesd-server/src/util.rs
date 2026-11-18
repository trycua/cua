// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Small conversions shared by the services.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cua_proto::wkt;

/// `SystemTime` → protobuf `Timestamp`.
pub fn timestamp(time: SystemTime) -> wkt::Timestamp {
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => wkt::Timestamp {
            seconds: since.as_secs() as i64,
            nanos: since.subsec_nanos() as i32,
        },
        Err(before) => {
            let before = before.duration();
            wkt::Timestamp {
                seconds: -(before.as_secs() as i64),
                nanos: 0,
            }
        }
    }
}

/// Current time as a protobuf `Timestamp`.
pub fn now_ts() -> wkt::Timestamp {
    timestamp(SystemTime::now())
}

/// Protobuf `Timestamp` → `SystemTime` (clamped at the epoch).
pub fn system_time(ts: &wkt::Timestamp) -> SystemTime {
    if ts.seconds < 0 {
        return UNIX_EPOCH;
    }
    UNIX_EPOCH + Duration::new(ts.seconds as u64, ts.nanos.max(0) as u32)
}

/// Optional protobuf `Duration` → `Duration`; negative or absent → `None`.
pub fn duration(value: Option<&wkt::Duration>) -> Option<Duration> {
    let value = value?;
    if value.seconds < 0 || value.nanos < 0 {
        return None;
    }
    Some(Duration::new(value.seconds as u64, value.nanos as u32))
}

/// `Duration` → protobuf `Duration`.
pub fn proto_duration(value: Duration) -> wkt::Duration {
    wkt::Duration {
        seconds: value.as_secs() as i64,
        nanos: value.subsec_nanos() as i32,
    }
}

/// Seconds since the Unix epoch.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Lowercase hex of a SHA-256 digest.
pub fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    hex::encode(digest.as_ref())
}

/// Random URL-safe identifier with `bytes` bytes of entropy.
pub fn random_id(bytes: usize) -> String {
    use base64::Engine as _;
    use rand::RngCore as _;
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Constant-time byte comparison (length leaks, content does not).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq as _;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}
