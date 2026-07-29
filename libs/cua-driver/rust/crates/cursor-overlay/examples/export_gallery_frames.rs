use cursor_overlay::{
    render_frame, CursorAction, CursorConfig, DeliveryModifier, OverlayCommand, RenderStateCore,
    TargetModifier,
};
use std::{fs, path::Path};

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
    export_session_badge(&output.join("session").join("badge"));
}

fn export_session_badge(output: &Path) {
    fs::create_dir_all(output).expect("create session badge frame output");
    for frame in 0..FPS * DURATION_SECS {
        let mut config = CursorConfig::default();
        config.cursor_id = "gallery-session".into();
        let mut core = RenderStateCore::new(config);
        core.motion.idle_hide_ms = 0.0;
        core.pos = (
            f64::from(SIZE) / (2.0 * f64::from(PREVIEW_BACKING_SCALE)),
            64.0,
        );
        core.apply_command_base(
            OverlayCommand::SetSessionLabel("Research".into()),
            false,
            false,
        );
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: CursorAction::Observe,
                delivery: Some(DeliveryModifier::Background),
                target: Some(TargetModifier::Browser),
            },
            false,
            false,
        );
        core.tick_motion(f64::from(frame) / f64::from(FPS));
        let pixmap = render_frame(&core, SIZE, SIZE, 0.0, 0.0, None, PREVIEW_BACKING_SCALE);
        let pixels = unpremultiply_rgba(pixmap.data().to_vec());
        image::save_buffer_with_format(
            output.join(format!("{frame:04}.png")),
            &pixels,
            SIZE,
            SIZE,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .expect("write session badge frame");
    }
}

fn export_state(
    output: &Path,
    action: CursorAction,
    delivery: Option<DeliveryModifier>,
    target: Option<TargetModifier>,
) {
    fs::create_dir_all(output).expect("create frame output");
    for frame in 0..FPS * DURATION_SECS {
        let mut config = CursorConfig::default();
        config.cursor_id = "gallery-session".into();
        let mut core = RenderStateCore::new(config);
        core.motion.idle_hide_ms = 0.0;
        core.pos = (
            f64::from(SIZE) / (2.0 * f64::from(PREVIEW_BACKING_SCALE)),
            f64::from(SIZE) / (2.0 * f64::from(PREVIEW_BACKING_SCALE)),
        );
        core.heading = f64::from(std::f32::consts::FRAC_PI_4);
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action,
                delivery,
                target,
            },
            false,
            false,
        );
        core.visual.elapsed_secs = f64::from(frame) / f64::from(FPS);
        let pixmap = render_frame(&core, SIZE, SIZE, 0.0, 0.0, None, PREVIEW_BACKING_SCALE);
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
