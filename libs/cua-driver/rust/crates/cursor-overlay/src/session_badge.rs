// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Renderer-owned public session label shown below an active agent cursor.
//!
//! The label is display metadata only. Cursor ownership continues to use the
//! private runtime key, so rendering a friendly name never weakens session
//! isolation.

use fontdue::layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle};
use std::sync::OnceLock;
use tiny_skia::{
    Color, GradientStop, LinearGradient, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point,
    Rect, SpreadMode, Stroke, Transform,
};

pub const MAX_SESSION_LABEL_CHARS: usize = 28;
pub const BADGE_MAX_WIDTH: f32 = 188.0;
pub const BADGE_HEIGHT: f32 = 28.0;
pub const BADGE_CURSOR_GAP: f32 = 25.0;

const FONT_BYTES: &[u8] = include_bytes!("../assets/Inter.ttf");
const FONT_SIZE: f32 = 11.5;
const HORIZONTAL_PADDING: f32 = 10.0;
const ORB_SIZE: f32 = 10.0;
const ORB_GAP: f32 = 7.0;
const TEXT_OPTICAL_Y_OFFSET: f32 = 1.0;

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

fn mixed_channel(base: u8, accent: u8, accent_weight: f32) -> u8 {
    let weight = accent_weight.clamp(0.0, 1.0);
    ((base as f32 * (1.0 - weight)) + (accent as f32 * weight)).round() as u8
}

fn session_tinted_color(
    base: [u8; 3],
    session_fill: [u8; 4],
    accent_weight: f32,
    alpha: u8,
) -> Color {
    Color::from_rgba8(
        mixed_channel(base[0], session_fill[0], accent_weight),
        mixed_channel(base[1], session_fill[1], accent_weight),
        mixed_channel(base[2], session_fill[2], accent_weight),
        alpha,
    )
}

fn paint_diffuse_glow(
    pixmap: &mut Pixmap,
    rect: Rect,
    radius: f32,
    scale: f32,
    color: [u8; 3],
    alpha: f32,
    y_offset: f32,
) {
    for step in (1..=8).rev() {
        let spread = step as f32 * scale;
        let Some(layer_rect) = Rect::from_xywh(
            rect.x() - spread,
            rect.y() - spread * 0.55 + y_offset * scale,
            rect.width() + spread * 2.0,
            rect.height() + spread * 1.1,
        ) else {
            continue;
        };
        let Some(layer) = rounded_rect(layer_rect, radius + spread) else {
            continue;
        };
        let strength = 0.35 + (8 - step) as f32 * 0.085;
        let paint = Paint {
            shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                color[0],
                color[1],
                color[2],
                (alpha * strength).round().clamp(0.0, 255.0) as u8,
            )),
            anti_alias: true,
            ..Default::default()
        };
        pixmap.fill_path(
            &layer,
            &paint,
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
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
    let max_text_width = (BADGE_MAX_WIDTH - HORIZONTAL_PADDING * 2.0 - ORB_SIZE - ORB_GAP) * scale;

    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings {
        max_width: Some(max_text_width),
        max_height: Some(BADGE_HEIGHT * scale),
        ..Default::default()
    });
    layout.append(&[font], &TextStyle::new(label, font_size, 0));
    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return;
    }
    let glyph_min_x = glyphs
        .iter()
        .map(|glyph| glyph.x)
        .fold(f32::INFINITY, f32::min);
    let glyph_max_x = glyphs
        .iter()
        .map(|glyph| glyph.x + glyph.width as f32)
        .fold(f32::NEG_INFINITY, f32::max);
    let glyph_min_y = glyphs
        .iter()
        .map(|glyph| glyph.y)
        .fold(f32::INFINITY, f32::min);
    let glyph_max_y = glyphs
        .iter()
        .map(|glyph| glyph.y + glyph.height as f32)
        .fold(f32::NEG_INFINITY, f32::max);
    let text_width = (glyph_max_x - glyph_min_x).min(max_text_width);
    let text_height = glyph_max_y - glyph_min_y;
    let badge_width =
        (HORIZONTAL_PADDING * 2.0 * scale + ORB_SIZE * scale + ORB_GAP * scale + text_width)
            .min(BADGE_MAX_WIDTH * scale)
            .max(72.0 * scale);
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
    let corner_radius = badge_height * 0.5;
    let Some(path) = rounded_rect(rect, corner_radius) else {
        return;
    };

    let opacity = alpha_scale.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }

    paint_diffuse_glow(
        pixmap,
        rect,
        corner_radius,
        scale,
        [0, 0, 0],
        8.0 * opacity,
        2.0,
    );
    paint_diffuse_glow(
        pixmap,
        rect,
        corner_radius,
        scale,
        [session_fill[0], session_fill[1], session_fill[2]],
        10.0 * opacity,
        0.0,
    );

    let background_shader = LinearGradient::new(
        Point::from_xy(x, y),
        Point::from_xy(x + badge_width, y + badge_height),
        vec![
            GradientStop::new(
                0.0,
                session_tinted_color([94, 151, 178], session_fill, 0.52, (236.0 * opacity) as u8),
            ),
            GradientStop::new(
                0.46,
                session_tinted_color([43, 92, 119], session_fill, 0.40, (239.0 * opacity) as u8),
            ),
            GradientStop::new(
                1.0,
                session_tinted_color([13, 27, 38], session_fill, 0.18, (245.0 * opacity) as u8),
            ),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    );
    let background = Paint {
        shader: background_shader.unwrap_or_else(|| {
            tiny_skia::Shader::SolidColor(Color::from_rgba8(18, 22, 28, (242.0 * opacity) as u8))
        }),
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
            (160.0 * opacity) as u8,
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

    let orb_center_x = x + HORIZONTAL_PADDING * scale + ORB_SIZE * scale * 0.5;
    let orb_center_y = y + badge_height * 0.5;
    if let Some(orb) = PathBuilder::from_circle(orb_center_x, orb_center_y, ORB_SIZE * scale * 0.5)
    {
        let orb_paint = Paint {
            shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                session_fill[0],
                session_fill[1],
                session_fill[2],
                (255.0 * opacity) as u8,
            )),
            anti_alias: true,
            ..Default::default()
        };
        pixmap.fill_path(
            &orb,
            &orb_paint,
            tiny_skia::FillRule::Winding,
            Transform::identity(),
            None,
        );
        let orb_outline = Paint {
            shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
                255,
                255,
                255,
                (226.0 * opacity) as u8,
            )),
            anti_alias: true,
            ..Default::default()
        };
        pixmap.stroke_path(
            &orb,
            &orb_outline,
            &Stroke {
                width: 1.5 * scale,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }

    let text_origin_x = x + HORIZONTAL_PADDING * scale + (ORB_SIZE + ORB_GAP) * scale - glyph_min_x;
    let text_origin_y =
        y + (badge_height - text_height) * 0.5 - glyph_min_y + TEXT_OPTICAL_Y_OFFSET * scale;
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

    #[test]
    fn badge_gradient_tracks_session_color_and_zero_alpha_paints_nothing() {
        let mut blue = Pixmap::new(240, 160).unwrap();
        let mut purple = Pixmap::new(240, 160).unwrap();
        let mut hidden = Pixmap::new(240, 160).unwrap();
        paint_session_badge(
            &mut blue,
            "Research run",
            120.0,
            60.0,
            1.0,
            1.0,
            [94, 192, 232, 255],
        );
        paint_session_badge(
            &mut purple,
            "Research run",
            120.0,
            60.0,
            1.0,
            1.0,
            [178, 132, 255, 255],
        );
        paint_session_badge(
            &mut hidden,
            "Research run",
            120.0,
            60.0,
            1.0,
            0.0,
            [94, 192, 232, 255],
        );
        assert_ne!(blue.data(), purple.data());
        assert!(hidden.data().chunks_exact(4).all(|pixel| pixel[3] == 0));
    }
}
