// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! H.264 decode on a chosen backend, plus the fixture's self-describing
//! pixels: the timecode strip and the photon square (fixtures/benchfix.py).

use cua_media_codec::types::{Backend, DecodedFrame, VideoCodec};

pub const CELL: u32 = 16;
pub const CELLS: u32 = 48;
pub const PHOTON_X: u32 = 16;
pub const PHOTON_Y: u32 = 32;
pub const PHOTON: u32 = 96;

pub fn backend_named(name: &str) -> Result<Backend, String> {
    match name {
        "videotoolbox" => Ok(Backend::VideoToolbox),
        "openh264" => Ok(Backend::OpenH264),
        other => Err(format!("unknown decoder backend {other}")),
    }
}

pub enum Outcome {
    Frame(DecodedFrame),
    Pending,
    NeedKeyframe,
}

/// Keeps the same invariants as `cua_media_client::decode::VideoDecoder`
/// (epoch reset, gap waits for a keyframe) but on an explicit backend.
pub struct Decoder {
    backend: Backend,
    inner: Option<Box<dyn cua_media_codec::video::VideoDecoder>>,
    codec_epoch: Option<u64>,
    last_sequence: Option<u64>,
    waiting: bool,
    pub decode_us: Vec<u32>,
    pub errors: u64,
}

impl Decoder {
    pub fn new(backend: Backend) -> Result<Self, String> {
        // Fail fast when the backend is missing on this host.
        let probe = cua_media_codec::backends::open_decoder(backend, VideoCodec::H264)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            backend,
            inner: Some(probe),
            codec_epoch: None,
            last_sequence: None,
            waiting: true,
            decode_us: Vec::new(),
            errors: 0,
        })
    }

    pub fn decode(
        &mut self,
        sequence: u64,
        codec_epoch: u64,
        keyframe: bool,
        data: &[u8],
        pts: u64,
    ) -> Outcome {
        let gap = self
            .last_sequence
            .is_some_and(|last| sequence != last.wrapping_add(1));
        self.last_sequence = Some(sequence);
        if self.codec_epoch != Some(codec_epoch) {
            self.codec_epoch = Some(codec_epoch);
            if let Some(decoder) = self.inner.as_mut() {
                let _ = decoder.reset();
            }
            self.waiting = true;
        }
        if gap && !keyframe {
            let first = !self.waiting;
            self.waiting = true;
            return if first {
                Outcome::NeedKeyframe
            } else {
                Outcome::Pending
            };
        }
        if self.waiting {
            if !keyframe {
                return Outcome::Pending;
            }
            self.waiting = false;
        }
        if self.inner.is_none() {
            match cua_media_codec::backends::open_decoder(self.backend, VideoCodec::H264) {
                Ok(decoder) => self.inner = Some(decoder),
                Err(_) => {
                    self.errors += 1;
                    return Outcome::NeedKeyframe;
                }
            }
        }
        let started = std::time::Instant::now();
        let decoder = self.inner.as_mut().expect("opened above");
        match decoder.decode(data, pts) {
            Ok(Some(frame)) => {
                self.decode_us.push(started.elapsed().as_micros() as u32);
                Outcome::Frame(frame)
            }
            Ok(None) => Outcome::Pending,
            Err(_) => {
                self.errors += 1;
                let _ = decoder.reset();
                self.waiting = true;
                Outcome::NeedKeyframe
            }
        }
    }

    /// True while dependent frames are dropped until a keyframe arrives.
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    /// Mark references lost (e.g. transport reported an incomplete frame).
    pub fn lost(&mut self) {
        self.waiting = true;
    }
}

fn luma(frame: &DecodedFrame, x: u32, y: u32, size: u32) -> Option<u32> {
    if x + size > frame.width || y + size > frame.height {
        return None;
    }
    let mut sum = 0u32;
    for dy in 0..size {
        let row = ((y + dy) * frame.width) as usize;
        for dx in 0..size {
            sum += u32::from(frame.y[row + (x + dx) as usize]);
        }
    }
    Some(sum / (size * size))
}

fn bit(frame: &DecodedFrame, ox: u32, oy: u32, cell: u32, index: u32) -> Option<u32> {
    let x = ox + index * cell + cell / 2 - 2;
    let y = oy + cell / 2 - 2;
    luma(frame, x, y, 4).map(|l| u32::from(l > 128))
}

/// Timecode reader. Locks onto the strip's origin and re-scans when the
/// strip is lost (window drag moves it).
#[derive(Default)]
pub struct Timecode {
    origin: Option<(u32, u32)>,
    pub cell: u32,
    pub reads: u64,
    pub misses: u64,
    pub scans: u64,
}

impl Timecode {
    pub fn new(scale: f64) -> Self {
        Self {
            cell: ((f64::from(CELL) * scale).round() as u32).max(4),
            ..Self::default()
        }
    }

    pub fn origin(&self) -> Option<(u32, u32)> {
        self.origin
    }

    fn read_at(&self, frame: &DecodedFrame, ox: u32, oy: u32) -> Option<u32> {
        let c = self.cell;
        let sync_a = [1, 0, 1, 0];
        for (i, want) in sync_a.iter().enumerate() {
            if bit(frame, ox, oy, c, i as u32)? != *want {
                return None;
            }
        }
        let sync_b = [0, 1, 0, 1];
        for (i, want) in sync_b.iter().enumerate() {
            if bit(frame, ox, oy, c, 44 + i as u32)? != *want {
                return None;
            }
        }
        let mut value = 0u32;
        for i in 0..32 {
            value = (value << 1) | bit(frame, ox, oy, c, 4 + i)?;
        }
        let mut check = 0u32;
        for i in 0..8 {
            check = (check << 1) | bit(frame, ox, oy, c, 36 + i)?;
        }
        let b = value.to_be_bytes();
        (u32::from(b[0] ^ b[1] ^ b[2] ^ b[3]) == check).then_some(value)
    }

    fn scan(&mut self, frame: &DecodedFrame) -> Option<(u32, u32, u32)> {
        self.scans += 1;
        let width = self.cell * CELLS;
        if frame.width < width || frame.height < self.cell {
            return None;
        }
        // Cheap prefilter on cell 0 (white) and 1 (black), then full read.
        for oy in (0..=frame.height - self.cell).step_by(2) {
            for ox in (0..=frame.width - width).step_by(2) {
                let (Some(a), Some(b)) = (
                    bit(frame, ox, oy, self.cell, 0),
                    bit(frame, ox, oy, self.cell, 1),
                ) else {
                    continue;
                };
                if a != 1 || b != 0 {
                    continue;
                }
                if let Some(value) = self.read_at(frame, ox, oy) {
                    // Centre the sampling points: walk to the far edge of
                    // the range of origins that still read, take the middle.
                    let mut x1 = ox;
                    while x1 + 1 < ox + self.cell && self.read_at(frame, x1 + 1, oy).is_some() {
                        x1 += 1;
                    }
                    let cx = (ox + x1) / 2;
                    let mut y1 = oy;
                    while y1 + 1 < oy + self.cell && self.read_at(frame, cx, y1 + 1).is_some() {
                        y1 += 1;
                    }
                    return Some((cx, (oy + y1) / 2, value));
                }
            }
        }
        None
    }

    /// The strip's value (unix ms mod 2^32), when readable.
    pub fn read(&mut self, frame: &DecodedFrame, allow_scan: bool) -> Option<u32> {
        if let Some((ox, oy)) = self.origin {
            if let Some(value) = self.read_at(frame, ox, oy) {
                self.reads += 1;
                return Some(value);
            }
        }
        if !allow_scan {
            self.misses += 1;
            return None;
        }
        match self.scan(frame) {
            Some((ox, oy, value)) => {
                self.origin = Some((ox, oy));
                self.reads += 1;
                Some(value)
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// Photon square state relative to the strip origin: white = true.
    pub fn photon(&self, frame: &DecodedFrame) -> Option<bool> {
        let (ox, oy) = self.origin?;
        let scale = f64::from(self.cell) / f64::from(CELL);
        let x = ox + (f64::from(PHOTON_X + PHOTON / 2) * scale) as u32 - 4;
        let y = oy + (f64::from(PHOTON_Y + PHOTON / 2) * scale) as u32 - 4;
        luma(frame, x, y, 8).map(|l| l > 128)
    }

    /// Frame pixel at the photon square's centre (click target).
    pub fn photon_center(&self) -> Option<(f64, f64)> {
        let (ox, oy) = self.origin?;
        let scale = f64::from(self.cell) / f64::from(CELL);
        Some((
            f64::from(ox) + f64::from(PHOTON_X + PHOTON / 2) * scale,
            f64::from(oy) + f64::from(PHOTON_Y + PHOTON / 2) * scale,
        ))
    }
}

/// Rebuild a full unix-ms value from its low 32 bits, nearest to `now_ms`.
pub fn unwrap_ms(low: u32, now_ms: i64) -> i64 {
    let base = now_ms & !0xFFFF_FFFF;
    let mut best = base | i64::from(low);
    for candidate in [best - (1 << 32), best + (1 << 32)] {
        if (candidate - now_ms).abs() < (best - now_ms).abs() {
            best = candidate;
        }
    }
    best
}

/// Mean luma of the whole frame (flash detection for the A/V fixture).
pub fn mean_luma(frame: &DecodedFrame) -> f64 {
    let step = 7usize;
    let mut sum = 0u64;
    let mut n = 0u64;
    for v in frame.y.iter().step_by(step) {
        sum += u64::from(*v);
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum as f64 / n as f64
    }
}

/// FNV-1a 64 of the Y plane (cheap frame identity).
pub fn hash(frame: &DecodedFrame) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &frame.y {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(value: u32, ox: u32, oy: u32, w: u32, h: u32) -> DecodedFrame {
        let mut y = vec![128u8; (w * h) as usize];
        let b = value.to_be_bytes();
        let check = b[0] ^ b[1] ^ b[2] ^ b[3];
        let mut bits = vec![1, 0, 1, 0];
        bits.extend((0..32).map(|i| (value >> (31 - i)) & 1));
        bits.extend((0..8).map(|i| u32::from(check >> (7 - i)) & 1));
        bits.extend([0, 1, 0, 1]);
        for (i, bit) in bits.iter().enumerate() {
            for dy in 0..CELL {
                for dx in 0..CELL {
                    let px = ox + i as u32 * CELL + dx;
                    y[((oy + dy) * w + px) as usize] = if *bit == 1 { 235 } else { 16 };
                }
            }
        }
        let chroma = (w.div_ceil(2) * h.div_ceil(2)) as usize;
        DecodedFrame {
            width: w,
            height: h,
            y,
            u: vec![128; chroma],
            v: vec![128; chroma],
            pts_us: 0,
        }
    }

    #[test]
    fn reads_and_locates_the_strip() {
        let frame = strip(0xDEAD_BEEF, 10, 20, 1024, 64);
        let mut tc = Timecode::new(1.0);
        assert_eq!(tc.read(&frame, true), Some(0xDEAD_BEEF));
        // The scan centres the sampling points in the cells.
        let (x, y) = tc.origin().unwrap();
        assert!(x.abs_diff(10) <= 2 && y.abs_diff(20) <= 2, "origin {x},{y}");
        // Locked: no rescan needed.
        let scans = tc.scans;
        assert_eq!(tc.read(&frame, false), Some(0xDEAD_BEEF));
        assert_eq!(tc.scans, scans);
    }

    #[test]
    fn rejects_bad_checksum() {
        let mut frame = strip(12345, 0, 0, 800, 16);
        // Flip one value bit (cell 10).
        for dy in 0..CELL {
            for dx in 0..CELL {
                let i = (dy * 800 + 10 * CELL + dx) as usize;
                frame.y[i] = 255 - frame.y[i];
            }
        }
        let mut tc = Timecode::new(1.0);
        assert_eq!(tc.read(&frame, true), None);
    }

    #[test]
    fn unwraps_near_now() {
        let now: i64 = 1_790_000_000_123;
        let low = (now as u64 & 0xFFFF_FFFF) as u32;
        assert_eq!(unwrap_ms(low, now + 50), now);
        assert_eq!(unwrap_ms(low.wrapping_add(3), now), now + 3);
    }
}
