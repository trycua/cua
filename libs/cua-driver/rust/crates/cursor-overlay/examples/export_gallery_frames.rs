use cursor_overlay::{
    theme::paint_default_theme, CursorAction, CursorVisualState, DeliveryModifier, ReducedMotion,
    TargetModifier,
};
use std::{fs, path::Path};
use tiny_skia::Pixmap;

const SIZE: u32 = 256;
const FPS: u32 = 30;
const DURATION_SECS: u32 = 4;
const PREVIEW_BACKING_SCALE: f32 = 1.5;

fn main() {
    let output = std::env::args()
        .nth(1)
        .expect("usage: export_gallery_frames <output-directory>");
    let output = Path::new(&output);

    let mut states = Vec::new();
    for action in CursorAction::ALL {
        states.push((
            output.join("actions").join(action.as_str()),
            action,
            None,
            None,
        ));
    }

    for (name, delivery, target) in [
        ("background", Some(DeliveryModifier::Background), None),
        ("foreground", Some(DeliveryModifier::Foreground), None),
        ("ax", None, Some(TargetModifier::Ax)),
        ("pixel", None, Some(TargetModifier::Pixel)),
        ("browser", None, Some(TargetModifier::Browser)),
        ("desktop", None, Some(TargetModifier::Desktop)),
    ] {
        states.push((
            output.join("modifiers").join(name),
            CursorAction::Idle,
            delivery,
            target,
        ));
    }

    states.push((
        output.join("combined").join("foreground-pixel-click"),
        CursorAction::Click,
        Some(DeliveryModifier::Foreground),
        Some(TargetModifier::Pixel),
    ));

    std::thread::scope(|scope| {
        for (path, action, delivery, target) in states {
            scope.spawn(move || export_state(&path, action, delivery, target));
        }
    });
}

fn export_state(
    output: &Path,
    action: CursorAction,
    delivery: Option<DeliveryModifier>,
    target: Option<TargetModifier>,
) {
    fs::create_dir_all(output).expect("create frame output");
    for frame in 0..FPS * DURATION_SECS {
        let mut pixmap = Pixmap::new(SIZE, SIZE).expect("create frame");
        let visual = CursorVisualState {
            requested_action: action,
            resolved_action: action,
            delivery,
            target,
            elapsed_secs: f64::from(frame) / f64::from(FPS),
            ending_secs: None,
            reduced_motion: ReducedMotion::Off,
            preempted_count: 0,
        };
        paint_default_theme(
            &mut pixmap,
            &visual,
            SIZE as f32 / 2.0,
            SIZE as f32 / 2.0,
            std::f32::consts::FRAC_PI_4,
            PREVIEW_BACKING_SCALE,
            1.0,
        );
        let pixels = unpremultiply_rgba(pixmap.data().to_vec());
        image::save_buffer_with_format(
            output.join(format!("{frame:04}.png")),
            &pixels,
            SIZE,
            SIZE,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .expect("write frame");
    }
}

fn unpremultiply_rgba(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        if alpha == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }
        pixel[0] = ((u16::from(pixel[0]) * 255 + alpha / 2) / alpha).min(255) as u8;
        pixel[1] = ((u16::from(pixel[1]) * 255 + alpha / 2) / alpha).min(255) as u8;
        pixel[2] = ((u16::from(pixel[2]) * 255 + alpha / 2) / alpha).min(255) as u8;
    }
    pixels
}
