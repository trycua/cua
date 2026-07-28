// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Renderer-owned public session label shown below an active agent cursor.
//!
//! The label is display metadata only. Cursor ownership continues to use the
//! private runtime key, so rendering a friendly name never weakens session
//! isolation.

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use std::sync::OnceLock;
use tiny_skia::{Color, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform};

pub const MAX_SESSION_LABEL_CHARS: usize = 28;
pub const BADGE_MAX_WIDTH: f32 = 176.0;
pub const BADGE_HEIGHT: f32 = 24.0;
pub const BADGE_CURSOR_GAP: f32 = 30.0;

const FONT_BYTES: &[u8] = include_bytes!("../assets/Inter.ttf");
const FONT_SIZE: f32 = 11.0;
const HORIZONTAL_PADDING: f32 = 10.0;
const DOT_SIZE: f32 = 5.0;
const DOT_GAP: f32 = 6.0;

fn font() -> Option<&'static fontdue::Font> {
    static FONT: OnceLock<Option<fontdue::Font>> = OnceLock::new();
    FONT.get_or_init(|| {
        fontdue::Font::from_bytes(
            FONT_BYTES,
            fontdue::FontSettings {
                scale: 40.0,
                ..Default::default()
            },
        )
        .ok()
    })
    .as_ref()
}

/// Convert an untrusted public session label into compact display text.
///
/// Controls are removed, all whitespace runs collapse to one ASCII space,
/// and the visible value is bounded by Unicode scalar count. Runtime keys and
/// transport identifiers must never be passed to this function.
pub fn sanitize_session_label(input: &str) -> Option<String> {
    let mut normalized = String::new();
    let mut pending_space = false;
    for character in input.chars() {
        if character.is_control() {
            continue;
        }
        if character.is_whitespace() {
            pending_space = !normalized.is_empty();
            continue;
        }
        if pending_space {
            normalized.push(' ');
            pending_space = false;
        }
        normalized.push(character);
    }

    let normalized = normalized.trim();
    if normalized.is_empty() {
        return None;
    }
    let mut chars = normalized.chars();
    let mut result: String = chars.by_ref().take(MAX_SESSION_LABEL_CHARS).collect();
    if chars.next().is_some() {
        result.pop();
        result.push('…');
    }
    Some(result)
}

fn rounded_rect(rect: Rect, radius: f32) -> Option<Path> {
    const K: f32 = 0.552_284_8;
    let x = rect.x();
    let y = rect.y();
    let width = rect.width();
    let height = rect.height();
    let radius = radius.min(width * 0.5).min(height * 0.5).max(0.0);
    let mut builder = PathBuilder::new();
    builder.move_to(x + radius, y);
    builder.line_to(x + width - radius, y);
    builder.cubic_to(
        x + width - radius + radius * K,
        y,
        x + width,
        y + radius - radius * K,
        x + width,
        y + radius,
    );
    builder.line_to(x + width, y + height - radius);
    builder.cubic_to(
        x + width,
        y + height - radius + radius * K,
        x + width - radius + radius * K,
        y + height,
        x + width - radius,
        y + height,
    );
    builder.line_to(x + radius, y + height);
    builder.cubic_to(
        x + radius - radius * K,
        y + height,
        x,
        y + height - radius + radius * K,
        x,
        y + height - radius,
    );
    builder.line_to(x, y + radius);
    builder.cubic_to(
        x,
        y + radius - radius * K,
        x + radius - radius * K,
        y,
        x + radius,
        y,
    );
    builder.close();
    builder.finish()
}

/// Paint the label below the cursor without rotating it with cursor heading.
pub fn paint_session_badge(
    pixmap: &mut Pixmap,
    label: &str,
    cursor_x: f32,
    cursor_y: f32,
    backing_scale: f32,
    alpha_scale: f32,
    session_fill: [u8; 4],
) {
    let Some(font) = font() else {
        return;
    };
    let scale = backing_scale.max(1.0);
    let font_size = FONT_SIZE * scale;
    let max_text_width = (BADGE_MAX_WIDTH - HORIZONTAL_PADDING * 2.0 - DOT_SIZE - DOT_GAP) * scale;

    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings {
        max_width: Some(max_text_width),
        max_height: Some(BADGE_HEIGHT * scale),
        ..Default::default()
    });
    layout.append(&[font], &TextStyle::new(label, font_size, 0));
    let glyphs = layout.glyphs();
    let text_width = glyphs
        .iter()
        .map(|glyph| glyph.x + glyph.width as f32)
        .fold(0.0_f32, f32::max)
        .min(max_text_width);
    let badge_width =
        (HORIZONTAL_PADDING * 2.0 * scale + DOT_SIZE * scale + DOT_GAP * scale + text_width)
            .min(BADGE_MAX_WIDTH * scale)
            .max(52.0 * scale);
    let badge_height = BADGE_HEIGHT * scale;
    let x = (cursor_x - badge_width * 0.5).clamp(
        2.0 * scale,
        pixmap.width() as f32 - badge_width - 2.0 * scale,
    );
    let y = (cursor_y + BADGE_CURSOR_GAP * scale).clamp(
        2.0 * scale,
        pixmap.height() as f32 - badge_height - 2.0 * scale,
    );
    let Some(rect) = Rect::from_xywh(x, y, badge_width, badge_height) else {
        return;
    };
    let Some(path) = rounded_rect(rect, badge_height * 0.5) else {
        return;
    };

    let opacity = alpha_scale.clamp(0.0, 1.0);
    let shadow = Paint {
        shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(0, 0, 0, (76.0 * opacity) as u8)),
        anti_alias: true,
        ..Default::default()
    };
    pixmap.fill_path(
        &path,
        &shadow,
        tiny_skia::FillRule::Winding,
        Transform::from_translate(0.0, 2.0 * scale),
        None,
    );

    let background = Paint {
        shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
            18,
            22,
            28,
            (224.0 * opacity) as u8,
        )),
        anti_alias: true,
        ..Default::default()
    };
    pixmap.fill_path(
        &path,
        &background,
        tiny_skia::FillRule::Winding,
        Transform::identity(),
        None,
    );
    let outline = Paint {
        shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
            255,
            255,
            255,
            (92.0 * opacity) as u8,
        )),
        anti_alias: true,
        ..Default::default()
    };
    pixmap.stroke_path(
        &path,
        &outline,
        &Stroke {
            width: scale,
            ..Default::default()
        },
        Transform::identity(),
        None,
    );

    let dot_x = x + HORIZONTAL_PADDING * scale;
    let dot_y = y + (badge_height - DOT_SIZE * scale) * 0.5;
    if let Some(dot) = Rect::from_xywh(dot_x, dot_y, DOT_SIZE * scale, DOT_SIZE * scale) {
        let dot_paint = Paint {
            shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                session_fill[0],
                session_fill[1],
                session_fill[2],
                (255.0 * opacity) as u8,
            )),
            anti_alias: true,
            ..Default::default()
        };
        pixmap.fill_rect(dot, &dot_paint, Transform::identity(), None);
    }

    let text_origin_x = dot_x + (DOT_SIZE + DOT_GAP) * scale;
    let text_origin_y = y + (badge_height - FONT_SIZE * scale) * 0.5 - 1.0 * scale;
    for glyph in glyphs {
        let (metrics, bitmap) = font.rasterize_config(glyph.key);
        if metrics.width == 0 || metrics.height == 0 {
            continue;
        }
        let Some(mut glyph_pixmap) = Pixmap::new(metrics.width as u32, metrics.height as u32)
        else {
            continue;
        };
        for (pixel, coverage) in glyph_pixmap
            .data_mut()
            .chunks_exact_mut(4)
            .zip(bitmap.iter().copied())
        {
            let alpha = ((coverage as f32) * opacity) as u8;
            pixel.copy_from_slice(&[alpha, alpha, alpha, alpha]);
        }
        pixmap.draw_pixmap(
            (text_origin_x + glyph.x).round() as i32,
            (text_origin_y + glyph.y).round() as i32,
            glyph_pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_public_labels() {
        assert_eq!(
            sanitize_session_label("  Research\n\t run \u{0007} "),
            Some("Research run".to_owned())
        );
        assert_eq!(sanitize_session_label("\n\t"), None);
        let long = sanitize_session_label("abcdefghijklmnopqrstuvwxyz0123456789").unwrap();
        assert_eq!(long.chars().count(), MAX_SESSION_LABEL_CHARS);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn badge_paints_at_one_and_two_x_backing_scales() {
        for scale in [1.0, 2.0] {
            let mut pixmap = Pixmap::new((240.0 * scale) as u32, (160.0 * scale) as u32).unwrap();
            paint_session_badge(
                &mut pixmap,
                "Research run",
                120.0 * scale,
                60.0 * scale,
                scale,
                1.0,
                [94, 192, 232, 255],
            );
            assert!(pixmap.data().chunks_exact(4).any(|pixel| pixel[3] > 0));
        }
    }
}
