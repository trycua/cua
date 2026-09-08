// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Renderer-owned public session label shown below an active agent cursor.
//!
//! The label is display metadata only. Cursor ownership continues to use the
//! private runtime key, so rendering a friendly name never weakens session
//! isolation.

use crate::{
    badge_glyphs::{paint_badge_chip, BadgeChip, BadgeGlyph},
    DeliveryModifier, TargetModifier,
};
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
pub const BADGE_CHIP_SIZE: f32 = 18.0;
pub const BADGE_CHIP_GAP: f32 = 4.0;
pub const BADGE_CHIP_GROUP_GAP: f32 = 7.0;

const FONT_BYTES: &[u8] = include_bytes!("../assets/Inter.ttf");
const FONT_SIZE: f32 = 11.5;
const HORIZONTAL_PADDING: f32 = 10.0;
const TEXT_OPTICAL_Y_OFFSET: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BadgeExtents {
    pub horizontal: f32,
    pub above: f32,
    pub below: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BadgeLabelLayout {
    pub text: String,
    pub origin: (f32, f32),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionBadgeLayout {
    pub rect: Rect,
    pub corner_radius: f32,
    pub label: Option<BadgeLabelLayout>,
    pub delivery_chip: Option<BadgeChip>,
    pub target_chip: Option<BadgeChip>,
    pub pill_alpha: f32,
    pub label_alpha: f32,
    pub chip_alpha: f32,
    pub scale: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionBadgeInput<'a> {
    pub label: Option<&'a str>,
    pub delivery: Option<DeliveryModifier>,
    pub target: Option<TargetModifier>,
    pub cursor: (f32, f32),
    pub backing_scale: f32,
    pub label_alpha: f32,
    pub chip_alpha: f32,
    pub clip: Option<(f32, f32)>,
}

pub const fn session_badge_extents() -> BadgeExtents {
    const GLOW_ALLOWANCE: f32 = 8.0;
    BadgeExtents {
        horizontal: BADGE_MAX_WIDTH * 0.5 + GLOW_ALLOWANCE,
        above: GLOW_ALLOWANCE,
        below: BADGE_CURSOR_GAP + BADGE_HEIGHT + GLOW_ALLOWANCE,
    }
}

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

#[derive(Debug, Clone, Copy)]
struct TextBounds {
    min_x: f32,
    min_y: f32,
    width: f32,
    height: f32,
}

fn text_layout(font: &fontdue::Font, text: &str, font_size: f32) -> Layout {
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings::default());
    layout.append(&[font], &TextStyle::new(text, font_size, 0));
    layout
}

fn text_bounds(layout: &Layout) -> Option<TextBounds> {
    let glyphs = layout.glyphs();
    if glyphs.is_empty() {
        return None;
    }
    let min_x = glyphs
        .iter()
        .map(|glyph| glyph.x)
        .fold(f32::INFINITY, f32::min);
    let max_x = glyphs
        .iter()
        .map(|glyph| glyph.x + glyph.width as f32)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = glyphs
        .iter()
        .map(|glyph| glyph.y)
        .fold(f32::INFINITY, f32::min);
    let max_y = glyphs
        .iter()
        .map(|glyph| glyph.y + glyph.height as f32)
        .fold(f32::NEG_INFINITY, f32::max);
    Some(TextBounds {
        min_x,
        min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

fn ellipsize_to_width(
    font: &fontdue::Font,
    label: &str,
    font_size: f32,
    max_width: f32,
) -> Option<(String, TextBounds)> {
    let layout = text_layout(font, label, font_size);
    let bounds = text_bounds(&layout)?;
    if bounds.width <= max_width {
        return Some((label.to_owned(), bounds));
    }

    let mut characters = label.chars().collect::<Vec<_>>();
    if characters.last() == Some(&'…') {
        characters.pop();
    }
    while !characters.is_empty() {
        characters.pop();
        let candidate = format!("{}…", characters.iter().collect::<String>());
        let layout = text_layout(font, &candidate, font_size);
        let bounds = text_bounds(&layout)?;
        if bounds.width <= max_width {
            return Some((candidate, bounds));
        }
    }
    let layout = text_layout(font, "…", font_size);
    text_bounds(&layout).map(|bounds| ("…".to_owned(), bounds))
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

pub fn session_badge_layout(input: SessionBadgeInput<'_>) -> Option<SessionBadgeLayout> {
    let Some(font) = font() else {
        return None;
    };
    let scale = input.backing_scale.max(1.0);
    let label_alpha = input.label_alpha.clamp(0.0, 1.0);
    let chip_alpha = input.chip_alpha.clamp(0.0, 1.0);
    let show_label = input.label.is_some() && label_alpha > 0.001;
    let delivery = (chip_alpha > 0.001).then_some(input.delivery).flatten();
    let target = (chip_alpha > 0.001).then_some(input.target).flatten();
    let chip_count = usize::from(delivery.is_some()) + usize::from(target.is_some());
    if !show_label && chip_count == 0 {
        return None;
    }

    let font_size = FONT_SIZE * scale;
    let chip_width = match chip_count {
        0 => 0.0,
        1 => BADGE_CHIP_SIZE,
        _ => BADGE_CHIP_SIZE * 2.0 + BADGE_CHIP_GAP,
    } * scale;
    let fixed_width = HORIZONTAL_PADDING * 2.0 * scale
        + chip_width
        + if show_label && chip_count > 0 {
            BADGE_CHIP_GROUP_GAP * scale
        } else {
            0.0
        };
    let max_text_width = (BADGE_MAX_WIDTH * scale - fixed_width).max(0.0);
    let label_data = if show_label {
        input
            .label
            .and_then(|label| ellipsize_to_width(font, label, font_size, max_text_width))
    } else {
        None
    };
    let text_width = label_data.as_ref().map_or(0.0, |(_, bounds)| bounds.width);
    let minimum_width = if show_label {
        55.0 * scale
    } else {
        fixed_width
    };
    let badge_width = (fixed_width + text_width)
        .min(BADGE_MAX_WIDTH * scale)
        .max(minimum_width);
    let badge_height = BADGE_HEIGHT * scale;
    let unclamped_x = input.cursor.0 - badge_width * 0.5;
    let unclamped_y = input.cursor.1 + BADGE_CURSOR_GAP * scale;
    let (x, y) = if let Some((clip_width, clip_height)) = input.clip {
        (
            unclamped_x.clamp(
                2.0 * scale,
                (clip_width - badge_width - 2.0 * scale).max(2.0 * scale),
            ),
            unclamped_y.clamp(
                2.0 * scale,
                (clip_height - badge_height - 2.0 * scale).max(2.0 * scale),
            ),
        )
    } else {
        (unclamped_x, unclamped_y)
    };
    let rect = Rect::from_xywh(x, y, badge_width, badge_height)?;
    let corner_radius = badge_height * 0.5;

    let mut cursor_x = x + HORIZONTAL_PADDING * scale;
    let label = label_data.map(|(text, bounds)| {
        let origin = (
            cursor_x - bounds.min_x,
            y + (badge_height - bounds.height) * 0.5 - bounds.min_y + TEXT_OPTICAL_Y_OFFSET * scale,
        );
        cursor_x += bounds.width + BADGE_CHIP_GROUP_GAP * scale;
        BadgeLabelLayout { text, origin }
    });
    let chip_y = y + (badge_height - BADGE_CHIP_SIZE * scale) * 0.5;
    let delivery_chip = delivery.and_then(|delivery| {
        let rect = Rect::from_xywh(
            cursor_x,
            chip_y,
            BADGE_CHIP_SIZE * scale,
            BADGE_CHIP_SIZE * scale,
        )?;
        cursor_x += (BADGE_CHIP_SIZE + BADGE_CHIP_GAP) * scale;
        Some(BadgeChip {
            glyph: BadgeGlyph::from(delivery),
            rect,
            filled: true,
        })
    });
    let target_chip = target.and_then(|target| {
        let rect = Rect::from_xywh(
            cursor_x,
            chip_y,
            BADGE_CHIP_SIZE * scale,
            BADGE_CHIP_SIZE * scale,
        )?;
        Some(BadgeChip {
            glyph: BadgeGlyph::from(target),
            rect,
            filled: false,
        })
    });

    Some(SessionBadgeLayout {
        rect,
        corner_radius,
        label,
        delivery_chip,
        target_chip,
        pill_alpha: label_alpha.max(chip_alpha),
        label_alpha,
        chip_alpha,
        scale,
    })
}

/// Paint the badge below the cursor without rotating it with cursor heading.
pub fn paint_session_badge(
    pixmap: &mut Pixmap,
    layout: &SessionBadgeLayout,
    session_fill: [u8; 4],
    alpha_scale: f32,
) {
    let Some(font) = font() else {
        return;
    };
    let rect = layout.rect;
    let x = rect.x();
    let y = rect.y();
    let badge_width = rect.width();
    let badge_height = rect.height();
    let scale = layout.scale;
    let opacity = (alpha_scale * layout.pill_alpha).clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return;
    }
    let Some(path) = rounded_rect(rect, layout.corner_radius) else {
        return;
    };

    paint_diffuse_glow(
        pixmap,
        rect,
        layout.corner_radius,
        scale,
        [0, 0, 0],
        8.0 * opacity,
        2.0,
    );
    paint_diffuse_glow(
        pixmap,
        rect,
        layout.corner_radius,
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
                session_tinted_color([94, 151, 178], session_fill, 0.66, (236.0 * opacity) as u8),
            ),
            GradientStop::new(
                0.46,
                session_tinted_color([43, 92, 119], session_fill, 0.52, (239.0 * opacity) as u8),
            ),
            GradientStop::new(
                1.0,
                session_tinted_color([13, 27, 38], session_fill, 0.26, (245.0 * opacity) as u8),
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
    // The rim carries session identity now that the orb is gone. Tinting it
    // costs no width and leaves label contrast untouched.
    let outline = Paint {
        shader: tiny_skia::Shader::SolidColor(session_tinted_color(
            [255, 255, 255],
            session_fill,
            0.55,
            (190.0 * opacity) as u8,
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

    if let Some(label) = &layout.label {
        let label_opacity = (alpha_scale * layout.label_alpha).clamp(0.0, 1.0);
        let text = text_layout(font, &label.text, FONT_SIZE * scale);
        for glyph in text.glyphs() {
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
                let alpha = ((coverage as f32) * label_opacity) as u8;
                pixel.copy_from_slice(&[alpha, alpha, alpha, alpha]);
            }
            pixmap.draw_pixmap(
                (label.origin.0 + glyph.x).round() as i32,
                (label.origin.1 + glyph.y).round() as i32,
                glyph_pixmap.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
    }
    let chip_opacity = (alpha_scale * layout.chip_alpha).clamp(0.0, 1.0);
    if let Some(chip) = layout.delivery_chip {
        paint_badge_chip(pixmap, chip, session_fill, chip_opacity);
    }
    if let Some(chip) = layout.target_chip {
        paint_badge_chip(pixmap, chip, session_fill, chip_opacity);
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
            let layout = session_badge_layout(SessionBadgeInput {
                label: Some("Research run"),
                delivery: Some(DeliveryModifier::Foreground),
                target: Some(TargetModifier::Browser),
                cursor: (120.0 * scale, 60.0 * scale),
                backing_scale: scale,
                label_alpha: 1.0,
                chip_alpha: 1.0,
                clip: Some((pixmap.width() as f32, pixmap.height() as f32)),
            })
            .unwrap();
            paint_session_badge(&mut pixmap, &layout, [94, 192, 232, 255], 1.0);
            assert!(pixmap.data().chunks_exact(4).any(|pixel| pixel[3] > 0));
        }
    }

    #[test]
    fn badge_gradient_tracks_session_color_and_zero_alpha_paints_nothing() {
        let mut blue = Pixmap::new(240, 160).unwrap();
        let mut purple = Pixmap::new(240, 160).unwrap();
        let mut hidden = Pixmap::new(240, 160).unwrap();
        let layout = session_badge_layout(SessionBadgeInput {
            label: Some("Research run"),
            delivery: None,
            target: None,
            cursor: (120.0, 60.0),
            backing_scale: 1.0,
            label_alpha: 1.0,
            chip_alpha: 0.0,
            clip: Some((240.0, 160.0)),
        })
        .unwrap();
        paint_session_badge(&mut blue, &layout, [94, 192, 232, 255], 1.0);
        paint_session_badge(&mut purple, &layout, [178, 132, 255, 255], 1.0);
        paint_session_badge(&mut hidden, &layout, [94, 192, 232, 255], 0.0);
        assert_ne!(blue.data(), purple.data());
        assert!(hidden.data().chunks_exact(4).all(|pixel| pixel[3] == 0));
    }

    #[test]
    fn modifier_only_badge_is_compact_and_keeps_delivery_before_target() {
        let layout = session_badge_layout(SessionBadgeInput {
            label: None,
            delivery: Some(DeliveryModifier::Background),
            target: Some(TargetModifier::Pixel),
            cursor: (120.0, 60.0),
            backing_scale: 1.0,
            label_alpha: 0.0,
            chip_alpha: 1.0,
            clip: None,
        })
        .unwrap();
        assert!(layout.label.is_none());
        assert!(layout.rect.width() < 80.0);
        let delivery = layout.delivery_chip.unwrap();
        let target = layout.target_chip.unwrap();
        assert!(delivery.filled);
        assert!(!target.filled);
        assert!(delivery.rect.x() < target.rect.x());
    }

    #[test]
    fn label_starts_at_the_leading_padding_with_no_reserved_marker() {
        let layout = session_badge_layout(SessionBadgeInput {
            label: Some("Research run"),
            delivery: None,
            target: None,
            cursor: (120.0, 60.0),
            backing_scale: 1.0,
            label_alpha: 1.0,
            chip_alpha: 0.0,
            clip: None,
        })
        .unwrap();
        let label = layout.label.unwrap();
        assert!((label.origin.0 - (layout.rect.x() + HORIZONTAL_PADDING)).abs() < 1.5);
    }

    #[test]
    fn long_label_stays_inside_the_bounded_badge() {
        let layout = session_badge_layout(SessionBadgeInput {
            label: Some("A very long public session label"),
            delivery: Some(DeliveryModifier::Foreground),
            target: Some(TargetModifier::Desktop),
            cursor: (120.0, 60.0),
            backing_scale: 1.0,
            label_alpha: 1.0,
            chip_alpha: 1.0,
            clip: None,
        })
        .unwrap();
        assert!(layout.rect.width() <= BADGE_MAX_WIDTH);
        assert!(layout.label.unwrap().text.ends_with('…'));
    }
}
