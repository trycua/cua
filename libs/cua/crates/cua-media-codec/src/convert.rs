// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Colour conversion between BGRA, NV12 and I420 (BT.709, limited range),
//! plus a PSNR helper for tests and benchmarks.
//!
//! Fixed-point coefficients are derived from the BT.709 luma weights
//! (Kr = 0.2126, Kb = 0.0722) scaled to the 219/224-step video range, in
//! 16.16 fixed point.

const SHIFT: u32 = 16;
const HALF: i32 = 1 << (SHIFT - 1);

// Forward (RGB -> limited-range YCbCr).
const YR: i32 = 11_966;
const YG: i32 = 40_254;
const YB: i32 = 4_064;
const UR: i32 = -6_597;
const UG: i32 = -22_187;
const UB: i32 = 28_784;
const VR: i32 = 28_784;
const VG: i32 = -26_148;
const VB: i32 = -2_636;

// Inverse (limited-range YCbCr -> RGB).
const CY: i32 = 76_309;
const CRV: i32 = 117_489;
const CGU: i32 = 13_975;
const CGV: i32 = 34_925;
const CBU: i32 = 138_438;

#[inline]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[inline]
fn luma(r: i32, g: i32, b: i32) -> u8 {
    clamp_u8(((YR * r + YG * g + YB * b + HALF) >> SHIFT) + 16)
}

#[inline]
fn chroma(r: i32, g: i32, b: i32) -> (u8, u8) {
    (
        clamp_u8(((UR * r + UG * g + UB * b + HALF) >> SHIFT) + 128),
        clamp_u8(((VR * r + VG * g + VB * b + HALF) >> SHIFT) + 128),
    )
}

/// Planar I420 buffers with tight strides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I420Buffer {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Y plane (`width * height`).
    pub y: Vec<u8>,
    /// U plane (`chroma_width * chroma_height`).
    pub u: Vec<u8>,
    /// V plane (`chroma_width * chroma_height`).
    pub v: Vec<u8>,
}

impl I420Buffer {
    /// Allocates a black frame.
    pub fn new(width: u32, height: u32) -> Self {
        let (cw, ch) = chroma_dims(width, height);
        Self {
            width,
            height,
            y: vec![16; width as usize * height as usize],
            u: vec![128; cw * ch],
            v: vec![128; cw * ch],
        }
    }

    /// Chroma plane width.
    pub fn chroma_width(&self) -> usize {
        chroma_dims(self.width, self.height).0
    }
}

/// Chroma plane dimensions for 4:2:0.
pub fn chroma_dims(width: u32, height: u32) -> (usize, usize) {
    (width.div_ceil(2) as usize, height.div_ceil(2) as usize)
}

/// Converts BGRA (any stride) into `out`, resizing it if needed.
pub fn bgra_to_i420_into(
    width: u32,
    height: u32,
    bgra: &[u8],
    stride: usize,
    out: &mut I420Buffer,
) {
    let (w, h) = (width as usize, height as usize);
    assert!(stride >= w * 4, "stride smaller than a row");
    assert!(
        bgra.len() >= stride * (h.saturating_sub(1)) + w * 4,
        "BGRA buffer too small"
    );
    if out.width != width || out.height != height {
        *out = I420Buffer::new(width, height);
    }
    let (cw, _) = chroma_dims(width, height);
    for row in 0..h {
        let src = &bgra[row * stride..row * stride + w * 4];
        let dst = &mut out.y[row * w..row * w + w];
        for (d, px) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
            *d = luma(i32::from(px[2]), i32::from(px[1]), i32::from(px[0]));
        }
    }
    for crow in 0..h.div_ceil(2) {
        let r0 = crow * 2;
        let r1 = (r0 + 1).min(h - 1);
        for ccol in 0..cw {
            let c0 = ccol * 2;
            let c1 = (c0 + 1).min(w - 1);
            let mut r = 0i32;
            let mut g = 0i32;
            let mut b = 0i32;
            for (rr, cc) in [(r0, c0), (r0, c1), (r1, c0), (r1, c1)] {
                let o = rr * stride + cc * 4;
                b += i32::from(bgra[o]);
                g += i32::from(bgra[o + 1]);
                r += i32::from(bgra[o + 2]);
            }
            let (u, v) = chroma((r + 2) >> 2, (g + 2) >> 2, (b + 2) >> 2);
            out.u[crow * cw + ccol] = u;
            out.v[crow * cw + ccol] = v;
        }
    }
}

/// Converts BGRA (any stride) to a new I420 buffer.
pub fn bgra_to_i420(width: u32, height: u32, bgra: &[u8], stride: usize) -> I420Buffer {
    let mut out = I420Buffer::new(width, height);
    bgra_to_i420_into(width, height, bgra, stride, &mut out);
    out
}

/// Converts NV12 into I420.
pub fn nv12_to_i420(
    width: u32,
    height: u32,
    y: &[u8],
    y_stride: usize,
    uv: &[u8],
    uv_stride: usize,
) -> I420Buffer {
    let mut out = I420Buffer::new(width, height);
    let (w, h) = (width as usize, height as usize);
    for row in 0..h {
        out.y[row * w..row * w + w].copy_from_slice(&y[row * y_stride..row * y_stride + w]);
    }
    let (cw, ch) = chroma_dims(width, height);
    for row in 0..ch {
        let src = &uv[row * uv_stride..row * uv_stride + cw * 2];
        for col in 0..cw {
            out.u[row * cw + col] = src[col * 2];
            out.v[row * cw + col] = src[col * 2 + 1];
        }
    }
    out
}

/// Copies strided I420 planes into a tight buffer.
pub fn i420_compact(
    width: u32,
    height: u32,
    y: &[u8],
    u: &[u8],
    v: &[u8],
    strides: [usize; 3],
) -> I420Buffer {
    let mut out = I420Buffer::new(width, height);
    let (w, h) = (width as usize, height as usize);
    for row in 0..h {
        out.y[row * w..row * w + w].copy_from_slice(&y[row * strides[0]..row * strides[0] + w]);
    }
    let (cw, ch) = chroma_dims(width, height);
    for row in 0..ch {
        out.u[row * cw..row * cw + cw].copy_from_slice(&u[row * strides[1]..row * strides[1] + cw]);
        out.v[row * cw..row * cw + cw].copy_from_slice(&v[row * strides[2]..row * strides[2] + cw]);
    }
    out
}

/// Converts I420 to NV12 (tight strides): returns `(y, uv)`.
pub fn i420_to_nv12(buf: &I420Buffer) -> (Vec<u8>, Vec<u8>) {
    let mut uv = Vec::with_capacity(buf.u.len() * 2);
    for (u, v) in buf.u.iter().zip(&buf.v) {
        uv.push(*u);
        uv.push(*v);
    }
    (buf.y.clone(), uv)
}

/// Converts tight I420 planes to packed BGRA (alpha 255).
pub fn i420_to_bgra(width: u32, height: u32, y: &[u8], u: &[u8], v: &[u8]) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let (cw, _) = chroma_dims(width, height);
    let mut out = vec![255u8; w * h * 4];
    for row in 0..h {
        for col in 0..w {
            let yy = (i32::from(y[row * w + col]) - 16) * CY;
            let ci = (row / 2) * cw + col / 2;
            let uu = i32::from(u[ci]) - 128;
            let vv = i32::from(v[ci]) - 128;
            let o = (row * w + col) * 4;
            out[o] = clamp_u8((yy + CBU * uu + HALF) >> SHIFT);
            out[o + 1] = clamp_u8((yy - CGU * uu - CGV * vv + HALF) >> SHIFT);
            out[o + 2] = clamp_u8((yy + CRV * vv + HALF) >> SHIFT);
        }
    }
    out
}

/// Peak signal-to-noise ratio in dB between two equally sized 8-bit
/// buffers. Identical inputs return `f64::INFINITY`.
pub fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "psnr inputs differ in length");
    if a.is_empty() {
        return f64::INFINITY;
    }
    let sse: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let d = i64::from(*x) - i64::from(*y);
            (d * d) as u64
        })
        .sum();
    if sse == 0 {
        return f64::INFINITY;
    }
    let mse = sse as f64 / a.len() as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primaries_map_to_bt709_limited_values() {
        // White, black, and the BT.709 limited-range red point.
        let px = [
            255u8, 255, 255, 255, 0, 0, 0, 255, 0, 0, 255, 255, 0, 0, 255, 255,
        ];
        let buf = bgra_to_i420(2, 2, &px, 8);
        assert_eq!(buf.y[0], 235);
        assert_eq!(buf.y[1], 16);
        assert_eq!(buf.y[2], 63); // 16 + 219 * 0.2126
    }

    #[test]
    fn round_trip_is_near_lossless_on_flat_colour_blocks() {
        let (w, h) = (16u32, 16u32);
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for (i, px) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let block = ((i % 16) / 2 + (i / 16 / 2) * 8) as u8;
            px.copy_from_slice(&[
                block.wrapping_mul(37),
                block.wrapping_mul(91),
                block.wrapping_mul(13),
                255,
            ]);
        }
        let yuv = bgra_to_i420(w, h, &bgra, (w * 4) as usize);
        let back = i420_to_bgra(w, h, &yuv.y, &yuv.u, &yuv.v);
        assert!(psnr(&bgra, &back) > 38.0, "psnr {}", psnr(&bgra, &back));
    }

    #[test]
    fn nv12_and_i420_are_interchangeable() {
        let buf = bgra_to_i420(6, 4, &[90u8; 6 * 4 * 4], 24);
        let (y, uv) = i420_to_nv12(&buf);
        let again = nv12_to_i420(6, 4, &y, 6, &uv, 6);
        assert_eq!(buf, again);
    }

    #[test]
    fn odd_sizes_are_handled() {
        let buf = bgra_to_i420(3, 3, &[200u8; 3 * 3 * 4], 12);
        assert_eq!(buf.u.len(), 4);
        let bgra = i420_to_bgra(3, 3, &buf.y, &buf.u, &buf.v);
        assert_eq!(bgra.len(), 36);
    }
}
