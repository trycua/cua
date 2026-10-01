// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Host-owned delivery and target glyphs painted inside the session badge.
//!
//! These glyphs deliberately sit outside cursor theme artifacts. Themes own
//! action artwork; the host owns the compact, upright execution-context marks
//! that must remain consistent across platforms and themes.

use crate::{DeliveryModifier, TargetModifier};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeGlyph {
    Background,
    Foreground,
    Ax,
    Pixel,
    Browser,
    Desktop,
}

impl From<DeliveryModifier> for BadgeGlyph {
    fn from(value: DeliveryModifier) -> Self {
        match value {
            DeliveryModifier::Background => Self::Background,
            DeliveryModifier::Foreground => Self::Foreground,
        }
    }
}

impl From<TargetModifier> for BadgeGlyph {
    fn from(value: TargetModifier) -> Self {
        match value {
            TargetModifier::Ax => Self::Ax,
            TargetModifier::Pixel => Self::Pixel,
            TargetModifier::Browser => Self::Browser,
            TargetModifier::Desktop => Self::Desktop,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BadgeChip {
    pub glyph: BadgeGlyph,
    pub rect: Rect,
    pub filled: bool,
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

fn paint_color(color: [u8; 4], alpha_scale: f32) -> Paint<'static> {
    Paint {
        shader: tiny_skia::Shader::SolidColor(Color::from_rgba8(
            color[0],
            color[1],
            color[2],
            (f32::from(color[3]) * alpha_scale.clamp(0.0, 1.0)).round() as u8,
        )),
        anti_alias: true,
        ..Default::default()
    }
}

fn stroke_path(pixmap: &mut Pixmap, path: &Path, width: f32, alpha: f32) {
    pixmap.stroke_path(
        path,
        &paint_color([255, 255, 255, 238], alpha),
        &Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        },
        Transform::identity(),
        None,
    );
}

fn line_path(points: &[(f32, f32)]) -> Option<Path> {
    let (&(x, y), rest) = points.split_first()?;
    let mut builder = PathBuilder::new();
    builder.move_to(x, y);
    for &(x, y) in rest {
        builder.line_to(x, y);
    }
    builder.finish()
}

fn circle_path(x: f32, y: f32, radius: f32) -> Option<Path> {
    PathBuilder::from_circle(x, y, radius)
}

pub(crate) fn paint_badge_chip(
    pixmap: &mut Pixmap,
    chip: BadgeChip,
    session_fill: [u8; 4],
    alpha: f32,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return;
    }
    let rect = chip.rect;
    let scale = (rect.width() / 18.0).max(0.5);
    let Some(well) = rounded_rect(rect, 5.0 * scale) else {
        return;
    };
    if chip.filled {
        pixmap.fill_path(
            &well,
            &paint_color(
                [session_fill[0], session_fill[1], session_fill[2], 220],
                alpha,
            ),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        stroke_path(pixmap, &well, 1.0 * scale, alpha * 0.72);
    } else {
        pixmap.fill_path(
            &well,
            &paint_color([255, 255, 255, 22], alpha),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        stroke_path(pixmap, &well, 1.0 * scale, alpha * 0.42);
    }

    let x = rect.x();
    let y = rect.y();
    let left = x + 4.0 * scale;
    let right = x + 14.0 * scale;
    let top = y + 4.0 * scale;
    let bottom = y + 14.0 * scale;
    let cx = x + 9.0 * scale;
    let cy = y + 9.0 * scale;
    let width = 1.35 * scale;

    match chip.glyph {
        BadgeGlyph::Background => {
            for (offset, opacity) in [(0.0, 0.58), (2.4 * scale, 1.0)] {
                let Some(layer_rect) =
                    Rect::from_xywh(left + offset, top + offset, 7.0 * scale, 7.0 * scale)
                else {
                    continue;
                };
                if let Some(layer) = rounded_rect(layer_rect, 1.8 * scale) {
                    stroke_path(pixmap, &layer, width, alpha * opacity);
                }
            }
        }
        BadgeGlyph::Foreground => {
            let Some(window_rect) = Rect::from_xywh(left, top + scale, 10.0 * scale, 9.0 * scale)
            else {
                return;
            };
            if let Some(window) = rounded_rect(window_rect, 1.8 * scale) {
                stroke_path(pixmap, &window, width, alpha);
            }
            if let Some(title) = line_path(&[
                (left + scale, top + 3.6 * scale),
                (right - scale, top + 3.6 * scale),
            ]) {
                stroke_path(pixmap, &title, width, alpha);
            }
        }
        BadgeGlyph::Ax => {
            if let Some(connectors) =
                line_path(&[(cx, top + scale), (cx, cy), (left + scale, bottom - scale)])
            {
                stroke_path(pixmap, &connectors, width, alpha);
            }
            if let Some(right_connector) = line_path(&[(cx, cy), (right - scale, bottom - scale)]) {
                stroke_path(pixmap, &right_connector, width, alpha);
            }
            for (x, y) in [
                (cx, top + scale),
                (left + scale, bottom - scale),
                (right - scale, bottom - scale),
            ] {
                if let Some(node) = circle_path(x, y, 1.35 * scale) {
                    pixmap.fill_path(
                        &node,
                        &paint_color([255, 255, 255, 245], alpha),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
        BadgeGlyph::Pixel => {
            for points in [
                [
                    (left, top + 3.0 * scale),
                    (left, top),
                    (left + 3.0 * scale, top),
                ],
                [
                    (right - 3.0 * scale, top),
                    (right, top),
                    (right, top + 3.0 * scale),
                ],
                [
                    (right, bottom - 3.0 * scale),
                    (right, bottom),
                    (right - 3.0 * scale, bottom),
                ],
                [
                    (left + 3.0 * scale, bottom),
                    (left, bottom),
                    (left, bottom - 3.0 * scale),
                ],
            ] {
                if let Some(path) = line_path(&points) {
                    stroke_path(pixmap, &path, width, alpha);
                }
            }
        }
        BadgeGlyph::Browser => {
            if let Some(globe) = circle_path(cx, cy, 5.0 * scale) {
                stroke_path(pixmap, &globe, width, alpha);
            }
            if let Some(equator) = line_path(&[(left, cy), (right, cy)]) {
                stroke_path(pixmap, &equator, width * 0.9, alpha);
            }
            if let Some(meridian) = line_path(&[
                (cx, top),
                (cx - 2.0 * scale, cy),
                (cx, bottom),
                (cx + 2.0 * scale, cy),
                (cx, top),
            ]) {
                stroke_path(pixmap, &meridian, width * 0.85, alpha);
            }
        }
        BadgeGlyph::Desktop => {
            let Some(screen_rect) = Rect::from_xywh(left, top, 10.0 * scale, 7.6 * scale) else {
                return;
            };
            if let Some(screen) = rounded_rect(screen_rect, 1.3 * scale) {
                stroke_path(pixmap, &screen, width, alpha);
            }
            if let Some(stand) = line_path(&[
                (cx, top + 7.6 * scale),
                (cx, bottom),
                (cx - 3.0 * scale, bottom),
                (cx + 3.0 * scale, bottom),
            ]) {
                stroke_path(pixmap, &stand, width, alpha);
            }
        }
    }
}
