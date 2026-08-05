use cursor_overlay::{
    render_frame, CursorAction, CursorConfig, DeliveryModifier, OverlayCommand, RenderStateCore,
    TargetModifier,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

const SIZE: u32 = 256;
const FPS: u32 = 30;
const DURATION_SECS: u32 = 4;
const PREVIEW_BACKING_SCALE: f32 = 1.5;
const RUNTIME_SESSION_LABEL: &str = "Research";

#[derive(Clone, Copy)]
struct GalleryState {
    action: CursorAction,
    delivery: Option<DeliveryModifier>,
    target: Option<TargetModifier>,
    session_label: Option<&'static str>,
}

#[cfg(test)]
fn runtime_state() -> GalleryState {
    GalleryState {
        action: CursorAction::Observe,
        delivery: Some(DeliveryModifier::Background),
        target: Some(TargetModifier::Browser),
        session_label: Some(RUNTIME_SESSION_LABEL),
    }
}

const DELIVERIES: [Option<DeliveryModifier>; 3] = [
    None,
    Some(DeliveryModifier::Background),
    Some(DeliveryModifier::Foreground),
];

const TARGETS: [Option<TargetModifier>; 5] = [
    None,
    Some(TargetModifier::Ax),
    Some(TargetModifier::Pixel),
    Some(TargetModifier::Browser),
    Some(TargetModifier::Desktop),
];

fn delivery_slug(delivery: Option<DeliveryModifier>) -> &'static str {
    match delivery {
        None => "none",
        Some(DeliveryModifier::Background) => "background",
        Some(DeliveryModifier::Foreground) => "foreground",
    }
}

fn target_slug(target: Option<TargetModifier>) -> &'static str {
    match target {
        None => "none",
        Some(TargetModifier::Ax) => "ax",
        Some(TargetModifier::Pixel) => "pixel",
        Some(TargetModifier::Browser) => "browser",
        Some(TargetModifier::Desktop) => "desktop",
    }
}

fn preview_slug(state: GalleryState) -> String {
    format!(
        "{}--{}--{}",
        state.action.as_str(),
        delivery_slug(state.delivery),
        target_slug(state.target),
    )
}

fn preview_states(output: &Path) -> Vec<(PathBuf, GalleryState)> {
    let mut states = Vec::with_capacity(CursorAction::ALL.len() * DELIVERIES.len() * TARGETS.len());
    for action in CursorAction::ALL {
        for delivery in DELIVERIES {
            for target in TARGETS {
                let state = GalleryState {
                    action,
                    delivery,
                    target,
                    session_label: Some(RUNTIME_SESSION_LABEL),
                };
                states.push((output.join("previews").join(preview_slug(state)), state));
            }
        }
    }
    states
}

fn export_states(states: Vec<(PathBuf, GalleryState)>) {
    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .min(12)
        .min(states.len().max(1));
    let chunk_size = states.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        for chunk in states.chunks(chunk_size) {
            scope.spawn(move || {
                for (path, state) in chunk {
                    export_state(path, *state);
                }
            });
        }
    });
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .expect("usage: export_gallery_frames <output-directory>");
    let output = Path::new(&output);

    let mut states = Vec::new();
    for action in CursorAction::ALL {
        states.push((
            output.join("actions").join(action.as_str()),
            GalleryState {
                action,
                delivery: None,
                target: None,
                session_label: None,
            },
        ));
    }
    export_states(states);
    export_states(preview_states(output));
}

fn export_state(output: &Path, state: GalleryState) {
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
        if let Some(session_label) = state.session_label {
            core.apply_command_base(
                OverlayCommand::SetSessionLabel(session_label.into()),
                false,
                false,
            );
        }
        core.apply_command_base(
            OverlayCommand::BeginAction {
                action: state.action,
                delivery: state.delivery,
                target: state.target,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_preview_uses_the_complete_production_composition() {
        let state = runtime_state();
        assert_eq!(state.action, CursorAction::Observe);
        assert_eq!(state.delivery, Some(DeliveryModifier::Background));
        assert_eq!(state.target, Some(TargetModifier::Browser));
        assert_eq!(state.session_label, Some(RUNTIME_SESSION_LABEL));
    }

    #[test]
    fn preview_inventory_covers_every_runtime_combination_once() {
        let root = Path::new("gallery");
        let states = preview_states(root);
        assert_eq!(states.len(), 12 * 3 * 5);

        let slugs = states
            .iter()
            .map(|(_, state)| preview_slug(*state))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(slugs.len(), states.len());
        assert!(slugs.contains("observe--background--browser"));
        assert!(slugs.contains("idle--none--none"));
        assert!(slugs.contains("system--foreground--desktop"));
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
