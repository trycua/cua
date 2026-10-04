// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

#![allow(dead_code)]
//! Synthetic test content (bounded sizes: tests stay well under 100 MiB).

/// A BGRA colour grid (8x8 cells of saturated colours).
pub fn color_grid(w: u32, h: u32) -> Vec<u8> {
    let palette: [[u8; 3]; 8] = [
        [255, 255, 255],
        [0, 0, 0],
        [0, 0, 255],
        [0, 255, 0],
        [255, 0, 0],
        [0, 255, 255],
        [255, 0, 255],
        [255, 255, 0],
    ];
    let mut out = vec![255u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let cell = ((x * 8 / w) + (y * 8 / h)) as usize % 8;
            let o = ((y * w + x) * 4) as usize;
            out[o..o + 3].copy_from_slice(&palette[cell]);
        }
    }
    out
}

/// Text-like content: dark "glyph" strokes on a light background, in lines.
pub fn text_like(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let mut out = vec![240u8; (w * h * 4) as usize];
    let mut state = seed.wrapping_mul(2_654_435_761).max(1);
    for line in 0..(h / 20) {
        let top = line * 20 + 4;
        let mut x = 8;
        while x + 10 < w {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let glyph_w = 4 + state % 6;
            if !state.is_multiple_of(7) {
                for gy in 0..12 {
                    for gx in 0..glyph_w {
                        // Vertical and horizontal strokes, 1-2 px wide.
                        let stroke = gx == 0
                            || gx == glyph_w - 1
                            || gy == 0
                            || gy == 6
                            || (state >> gy) & 1 == 1 && gx == glyph_w / 2;
                        if stroke {
                            let o = (((top + gy) * w + x + gx) * 4) as usize;
                            out[o] = 20;
                            out[o + 1] = 20;
                            out[o + 2] = 30;
                        }
                    }
                }
            }
            x += glyph_w + 3;
        }
    }
    out
}

/// A gradient background with a moving square (motion content).
pub fn motion(w: u32, h: u32, frame: u32) -> Vec<u8> {
    let mut out = vec![255u8; (w * h * 4) as usize];
    let size = h / 4;
    let bx = (frame * 7) % (w - size);
    let by = (frame * 3) % (h - size);
    for y in 0..h {
        for x in 0..w {
            let o = ((y * w + x) * 4) as usize;
            let inside = x >= bx && x < bx + size && y >= by && y < by + size;
            if inside {
                out[o..o + 3].copy_from_slice(&[40, 200, 240]);
            } else {
                out[o] = (x * 255 / w) as u8;
                out[o + 1] = (y * 255 / h) as u8;
                out[o + 2] = ((x + y + frame * 4) % 256) as u8;
            }
        }
    }
    out
}

/// Luma-weighted PSNR between two BGRA images (alpha ignored).
pub fn psnr_bgra(a: &[u8], b: &[u8]) -> f64 {
    let strip = |v: &[u8]| -> Vec<u8> {
        v.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect()
    };
    cua_media_codec::convert::psnr(&strip(a), &strip(b))
}
