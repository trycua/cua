// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Render the agent cursor along a fixed script of moves, clicks, a drag and
//! a scroll with the real overlay renderer, over a light and a dark app
//! window, as raw RGBA frames on stdout. Pipe them into ffmpeg:
//!
//! ```sh
//! cargo run -p cursor-overlay --example render_motion_clip -- heading \
//!   | ffmpeg -f rawvideo -pix_fmt rgba -s 1600x1000 -r 60 -i - \
//!       -c:v libx264 -pix_fmt yuv420p -crf 24 heading.mp4
//! ```
//!
//! The argument is the `cursor.rotate` mode (`off`, `lean` or `heading`,
//! the default). The float and the halo are on, as by default.

use cursor_overlay::{
    paint_cursor, CursorAction, CursorConfig, CursorOptions, OverlayCommand, RenderStateCore,
    RotateMode,
};
use std::io::Write;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

const SCALE: f32 = 2.0;
const W: f32 = 800.0;
const H: f32 = 500.0;
const FPS: f64 = 60.0;

#[derive(Clone, Copy)]
enum Step {
    Move(f64, f64),
    Click,
    Press,
    Release,
    Scroll(f64),
    Wait(f64),
}

/// A few long moves, a downward move, short hops, clicks, a drag and a
/// scroll, crossing both windows.
const SCRIPT: &[Step] = &[
    Step::Wait(0.4),
    Step::Move(690.0, 410.0),
    Step::Click,
    Step::Move(270.0, 90.0),
    Step::Move(270.0, 420.0),
    Step::Move(298.0, 430.0),
    Step::Move(318.0, 414.0),
    Step::Move(300.0, 440.0),
    Step::Click,
    Step::Move(120.0, 250.0),
    Step::Press,
    Step::Move(560.0, 220.0),
    Step::Release,
    Step::Move(640.0, 150.0),
    Step::Scroll(1.1),
    Step::Wait(0.6),
];

struct Scene {
    clicked: Option<(f64, f64, f64)>,
    card: (f64, f64),
    dragging: bool,
    scroll: f64,
}

fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, rgb: u32) {
    let color = Color::from_rgba8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255);
    let paint = Paint {
        shader: tiny_skia::Shader::SolidColor(color),
        anti_alias: true,
        ..Default::default()
    };
    let (x, y, w, h, r) = (x * SCALE, y * SCALE, w * SCALE, h * SCALE, r * SCALE);
    let mut pb = PathBuilder::new();
    if r <= 0.0 {
        if let Some(rect) = Rect::from_xywh(x, y, w, h) {
            pb.push_rect(rect);
        }
    } else {
        let k = 0.552_284_8 * r;
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
        pb.line_to(x + w, y + h - r);
        pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
        pb.line_to(x + r, y + h);
        pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
        pb.line_to(x, y + r);
        pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
        pb.close();
    }
    if let Some(path) = pb.finish() {
        pm.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn window(pm: &mut Pixmap, x: f32, dark: bool, scene: &Scene, t: f64) {
    let (body, bar, side, line, strong, button) = if dark {
        (0x1E1F22, 0x2B2D31, 0x25272B, 0x3A3D43, 0x50545C, 0x3B82F6)
    } else {
        (0xFFFFFF, 0xECEEF1, 0xF4F5F7, 0xE3E6EA, 0xC9CED6, 0x2563EB)
    };
    let (y, w, h) = (24.0, 370.0, 452.0);
    fill_rect(pm, x, y, w, h, 10.0, body);
    fill_rect(pm, x, y, w, 30.0, 10.0, bar);
    fill_rect(pm, x, y + 20.0, w, 10.0, 0.0, bar);
    for (i, c) in [0xFF5F57u32, 0xFEBC2E, 0x28C840].iter().enumerate() {
        fill_rect(
            pm,
            x + 14.0 + i as f32 * 16.0,
            y + 10.0,
            10.0,
            10.0,
            5.0,
            *c,
        );
    }
    fill_rect(pm, x, y + 30.0, 96.0, h - 30.0, 0.0, side);
    for i in 0..7 {
        fill_rect(
            pm,
            x + 14.0,
            y + 50.0 + i as f32 * 26.0,
            64.0,
            8.0,
            4.0,
            line,
        );
    }
    // Content rows; the dark window's list scrolls.
    let offset = if dark { scene.scroll as f32 } else { 0.0 };
    for i in 0..14 {
        let ry = y + 52.0 + i as f32 * 30.0 - offset;
        if ry < y + 40.0 || ry > y + h - 70.0 {
            continue;
        }
        let len = 120.0 + ((i * 53) % 110) as f32;
        fill_rect(
            pm,
            x + 116.0,
            ry,
            len,
            9.0,
            4.5,
            if i % 4 == 0 { strong } else { line },
        );
    }
    // A button near the bottom right; it flashes when clicked.
    let (bx, by) = (x + w - 120.0, y + h - 52.0);
    let flash = scene.clicked.is_some_and(|(cx, cy, at)| {
        (cx - f64::from(bx + 50.0)).abs() < 60.0
            && (cy - f64::from(by + 15.0)).abs() < 40.0
            && t - at < 0.25
    });
    fill_rect(
        pm,
        bx,
        by,
        100.0,
        30.0,
        8.0,
        if flash { 0x1D4ED8 } else { button },
    );
    fill_rect(pm, bx + 26.0, by + 12.0, 48.0, 6.0, 3.0, 0xFFFFFF);
}

fn background(scene: &Scene, t: f64) -> Pixmap {
    let mut pm = Pixmap::new((W * SCALE) as u32, (H * SCALE) as u32).unwrap();
    fill_rect(&mut pm, 0.0, 0.0, W, H, 0.0, 0x9AA3AF);
    window(&mut pm, 10.0, false, scene, t);
    window(&mut pm, 420.0, true, scene, t);
    // The card the drag carries.
    let (cx, cy) = scene.card;
    fill_rect(
        &mut pm,
        cx as f32 - 36.0,
        cy as f32 - 14.0,
        72.0,
        40.0,
        8.0,
        0xF59E0B,
    );
    if scene.dragging {
        fill_rect(
            &mut pm,
            cx as f32 - 30.0,
            cy as f32 - 4.0,
            40.0,
            6.0,
            3.0,
            0xFFFFFF,
        );
    }
    pm
}

fn main() {
    let rotate = match std::env::args().nth(1).as_deref() {
        Some("lean") => RotateMode::Lean,
        Some("off") => RotateMode::Off,
        _ => RotateMode::Heading,
    };
    let mut core = RenderStateCore::new(CursorConfig::default());
    core.options_override = Some(CursorOptions {
        rotate,
        ..CursorOptions::DEFAULT
    });
    core.motion.idle_hide_ms = 60_000.0;
    core.apply_command_base(
        OverlayCommand::SnapTo {
            x: 0.0,
            y: 0.0,
            heading_radians: None,
        },
        false,
        false,
    );
    let start = cursor_overlay::anchor_for_pointer(120.0, 110.0, core.heading);
    core.pos = start;

    let mut scene = Scene {
        clicked: None,
        card: (120.0, 250.0),
        dragging: false,
        scroll: 0.0,
    };
    let dt = 1.0 / FPS;
    let mut t = 0.0;
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let mut target = (120.0, 110.0);
    let mut frame = |core: &mut RenderStateCore, scene: &mut Scene, t: f64| -> bool {
        let arrived = core.tick_motion(dt);
        let (px, py) = cursor_overlay::pointer_for_anchor(core.pos.0, core.pos.1, core.heading);
        if scene.dragging {
            scene.card = (px, py + 14.0);
        }
        let mut pm = background(scene, t);
        paint_cursor(&mut pm, core, 0.0, 0.0, None, SCALE);
        out.write_all(pm.data()).unwrap();
        arrived
    };
    for step in SCRIPT {
        match *step {
            Step::Move(x, y) => {
                target = (x, y);
                core.apply_command_base(
                    OverlayCommand::MoveTo {
                        x,
                        y,
                        end_heading_radians: cursor_overlay::trajectory::REST_HEADING,
                        target: None,
                    },
                    false,
                    false,
                );
                loop {
                    t += dt;
                    if frame(&mut core, &mut scene, t) || core.trajectory.is_none() {
                        break;
                    }
                }
                // A short beat before the next call, as a real agent has.
                for _ in 0..(0.12 * FPS) as usize {
                    t += dt;
                    frame(&mut core, &mut scene, t);
                }
            }
            Step::Click => {
                core.apply_command_base(
                    OverlayCommand::ClickPulse {
                        x: target.0,
                        y: target.1,
                    },
                    false,
                    false,
                );
                scene.clicked = Some((target.0, target.1, t));
                for _ in 0..(0.35 * FPS) as usize {
                    t += dt;
                    frame(&mut core, &mut scene, t);
                }
            }
            Step::Press | Step::Release => {
                let press = matches!(step, Step::Press);
                core.apply_command_base(OverlayCommand::SetPressed(press), false, false);
                scene.dragging = press;
                for _ in 0..(0.15 * FPS) as usize {
                    t += dt;
                    frame(&mut core, &mut scene, t);
                }
            }
            Step::Scroll(secs) => {
                core.apply_command_base(
                    OverlayCommand::BeginAction {
                        action: CursorAction::Scroll,
                        delivery: None,
                        target: None,
                    },
                    false,
                    false,
                );
                let frames = (secs * FPS) as usize;
                for i in 0..frames {
                    t += dt;
                    let p = i as f64 / frames as f64;
                    scene.scroll = 120.0 * (p * p * (3.0 - 2.0 * p));
                    frame(&mut core, &mut scene, t);
                }
                core.apply_command_base(
                    OverlayCommand::EndAction(CursorAction::Scroll),
                    false,
                    false,
                );
            }
            Step::Wait(secs) => {
                for _ in 0..(secs * FPS) as usize {
                    t += dt;
                    frame(&mut core, &mut scene, t);
                }
            }
        }
    }
    eprintln!("{:.2} s", t);
}
