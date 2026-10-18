// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Client-side latency telemetry primitives for the on-screen stats overlay.
//!
//! These used to live in `rcdp-dogfood`, which also persisted every bucket to
//! a local SQLite store. That persistence was dropped when rcdp moved into
//! cua-spacesd; only the in-memory aggregation the stats title needs
//! remains here.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, saturating instead of panicking.
pub fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// One aggregation window of client latency and throughput metrics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetricBucket {
    pub timestamp_ms: u64,
    pub duration_ms: u64,
    pub fps: f64,
    pub bitrate_mbps: f64,
    pub control_rtt_ms: Option<f64>,
    pub encode_p50_ms: Option<f64>,
    pub encode_p95_ms: Option<f64>,
    pub encode_p99_ms: Option<f64>,
    pub decode_p50_ms: Option<f64>,
    pub decode_p95_ms: Option<f64>,
    pub decode_p99_ms: Option<f64>,
    pub present_p50_ms: Option<f64>,
    pub present_p95_ms: Option<f64>,
    pub present_p99_ms: Option<f64>,
    pub input_ack_p50_ms: Option<f64>,
    pub input_ack_p95_ms: Option<f64>,
    pub input_ack_p99_ms: Option<f64>,
    pub host_dispatch_p50_ms: Option<f64>,
    pub host_dispatch_p95_ms: Option<f64>,
    pub host_dispatch_p99_ms: Option<f64>,
    pub input_to_present_p50_ms: Option<f64>,
    pub input_to_present_p95_ms: Option<f64>,
    pub input_to_present_p99_ms: Option<f64>,
    pub scroll_gap_p95_ms: Option<f64>,
    pub frames_received: u64,
    pub frames_presented: u64,
    pub client_frames_replaced: u64,
    pub server_frames_replaced: u64,
    pub network_incomplete_frames: u64,
    pub input_events_captured: u64,
    pub input_events_sent: u64,
    pub input_batches_sent: u64,
    pub max_input_in_flight: u32,
    pub max_input_queue_depth: u32,
    pub scroll_events_captured: u64,
    pub scroll_events_sent: u64,
}

/// Nearest-rank percentile over the finite samples, or `None` when empty.
pub fn percentile_f64(values: &[f64], percentile: f64) -> Option<f64> {
    let mut values = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let percentile = percentile.clamp(0.0, 1.0);
    let index = ((values.len() - 1) as f64 * percentile).ceil() as usize;
    values.get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::percentile_f64;

    #[test]
    fn percentile_ignores_non_finite_and_handles_empty() {
        assert_eq!(percentile_f64(&[], 0.5), None);
        assert_eq!(percentile_f64(&[f64::NAN], 0.5), None);
        let samples = [4.0, 1.0, f64::INFINITY, 3.0, 2.0];
        assert_eq!(percentile_f64(&samples, 0.0), Some(1.0));
        assert_eq!(percentile_f64(&samples, 0.5), Some(3.0));
        assert_eq!(percentile_f64(&samples, 1.0), Some(4.0));
    }
}

/// One viewer session's totals for the anonymous `cua_stream_stats` event
/// (sent once, bucketed, when the window closes; every telemetry opt-out
/// applies).
#[derive(Debug, Clone)]
pub struct StreamTotals {
    pub started: Instant,
    /// `quic` or `websocket`.
    pub transport: &'static str,
    pub codec: String,
    pub frames: u64,
    pub max_height: u32,
    /// Worst per-window p95 input-to-present, ms.
    pub p95_latency_ms: Option<f64>,
}

impl StreamTotals {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            transport: "other",
            codec: String::new(),
            frames: 0,
            max_height: 0,
            p95_latency_ms: None,
        }
    }

    /// The transport word of a connection URL.
    pub fn transport_of(url: &str) -> &'static str {
        match url.split(':').next().unwrap_or_default() {
            "quic" => "quic",
            "ws" | "wss" | "http" | "https" => "websocket",
            _ => "other",
        }
    }

    pub fn observe_p95(&mut self, p95_ms: Option<f64>) {
        if let Some(v) = p95_ms.filter(|v| v.is_finite()) {
            self.p95_latency_ms = Some(self.p95_latency_ms.map_or(v, |cur| cur.max(v)));
        }
    }

    /// The event for this session, or None without video.
    pub fn event(&self, now: Instant) -> Option<cua_telemetry::events::Event> {
        cua_telemetry::events::stream_summary(
            self.transport,
            &self.codec,
            None,
            self.frames,
            self.max_height,
            self.p95_latency_ms,
            now.saturating_duration_since(self.started),
        )
    }

    /// Sends the session's stats on the process-wide client (product `cli`:
    /// the viewer is started by `cua`), waiting at most `budget`.
    pub fn send(&self, now: Instant, budget: Duration) {
        if let Some(e) = self.event(now) {
            let t = cua_telemetry::init("cli", env!("CARGO_PKG_VERSION"));
            t.capture(e);
            t.shutdown(budget);
        }
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn totals_become_one_bucketed_event() {
        let t0 = Instant::now();
        let mut s = StreamTotals::new(t0);
        assert!(s.event(t0 + Duration::from_secs(10)).is_none(), "no video");
        s.transport = StreamTotals::transport_of("quic://127.0.0.1:3212");
        s.codec = "h264".into();
        s.frames = 550;
        s.max_height = 1080;
        s.observe_p95(Some(12.0));
        s.observe_p95(Some(45.0));
        s.observe_p95(None);
        let e = s.event(t0 + Duration::from_secs(10)).unwrap();
        assert_eq!(e.props["transport"], "quic");
        assert_eq!(e.props["codec"], "h264");
        assert_eq!(e.props["fps_bucket"], "50_59");
        assert!(e.props.values().all(|v| v.is_string()));
        assert!(!serde_json::to_string(&e.props).unwrap().contains("550"));
        assert_eq!(StreamTotals::transport_of("https://h:3211/"), "websocket");
    }
}
