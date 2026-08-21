use cursor_overlay::{
    inspect_artifact, motion_spec, render_frame, CompiledTheme, CursorAction, CursorConfig,
    DeliveryModifier, MotionConfig, OverlayCommand, ReducedMotion, RenderStateCore, TargetModifier,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

const SIZE: u32 = 256;
const MOVEMENT_WIDTH: u32 = 1024;
const MOVEMENT_HEIGHT: u32 = 576;
const ACTION_FPS: u32 = 30;
const MOVEMENT_TICK_MS: u32 = 16;
const MOVEMENT_FRAME_COUNT: u32 = DURATION_SECS * 1000 / MOVEMENT_TICK_MS;
const DURATION_SECS: u32 = 4;
const PREVIEW_BACKING_SCALE: f32 = 1.5;
const MOVEMENT_BACKING_SCALE: f32 = 1.0;
const RUNTIME_SESSION_LABEL: &str = "Research";
const MOVEMENT_TARGETS: [(f64, f64); 4] = [
    (400.0, 350.0),
    (650.0, 200.0),
    (550.0, 390.0),
    (280.0, 350.0),
];
const PRODUCTION_END_HEADING: f64 = std::f64::consts::FRAC_PI_4;
const FIRST_ACTION_SEED_OFFSET: f64 = 140.0;

struct Args {
    output: PathBuf,
    theme: Option<PathBuf>,
    dev: bool,
    motion_overrides: Option<PathBuf>,
}

#[derive(Serialize)]
struct GalleryManifest<'a> {
    schema: &'static str,
    theme_id: &'a str,
    theme_name: &'a str,
    theme_version: &'a str,
    content_hash: String,
    fps: u32,
    movement_fps: f64,
    movement_tick_ms: u32,
    duration_secs: u32,
    motion: MotionManifest,
    actions: Vec<ActionManifest>,
}

/// Movement-contract echo in the gallery manifest. The values are what the
/// renderer actually used, so a recording can never be mistaken for the
/// production defaults when overrides were active.
#[derive(Serialize)]
struct MotionManifest {
    /// Repository spec revision the render was based on.
    spec_schema: String,
    /// Platform tick path that produced the movement frames:
    /// `swift_constants` (macOS) or `motion` (Windows/Linux).
    tick_path: &'static str,
    /// Effective `MotionConfig` used by the movement scenes.
    effective: MotionConfigEcho,
    /// Spring-settle constants the tick path used.
    spring_settle: SpringSettleEcho,
    /// Movement fields overridden for this render (empty for defaults).
    overridden_fields: Vec<&'static str>,
}

#[derive(Serialize)]
struct MotionConfigEcho {
    peak_speed: f64,
    min_start_speed: f64,
    min_end_speed: f64,
    turn_radius: f64,
    spring: f64,
    glide_duration_ms: f64,
}

#[derive(Serialize)]
struct SpringSettleEcho {
    mode: &'static str,
    k: f64,
    c: f64,
    overshoot: f64,
}

/// Playground override file (`cua.cursor-gallery-motion-override/1`).
///
/// Only fields that affect the current production movement path are
/// accepted; every value is re-clamped in Rust before use, so the file is
/// advisory input, never authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionOverrideFile {
    schema: String,
    #[serde(default)]
    peak_speed: Option<f64>,
    #[serde(default)]
    min_start_speed: Option<f64>,
    #[serde(default)]
    min_end_speed: Option<f64>,
    #[serde(default)]
    turn_radius: Option<f64>,
    #[serde(default)]
    spring: Option<f64>,
    #[serde(default)]
    glide_duration_ms: Option<f64>,
}

const MOTION_OVERRIDE_SCHEMA: &str = "cua.cursor-gallery-motion-override/1";

impl MotionOverrideFile {
    /// Parse and validate an override file. Unknown fields are rejected by
    /// `deny_unknown_fields`; values are clamped to the same production
    /// bounds the runtime uses.
    fn load(path: &Path) -> Self {
        let raw = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read motion overrides {path:?}: {error}"));
        let file: MotionOverrideFile = serde_json::from_str(&raw)
            .unwrap_or_else(|error| panic!("parse motion overrides {path:?}: {error}"));
        if file.schema != MOTION_OVERRIDE_SCHEMA {
            panic!(
                "motion overrides {path:?}: expected schema {MOTION_OVERRIDE_SCHEMA}, found {}",
                file.schema
            );
        }
        file.clamped()
    }

    fn clamped(&self) -> Self {
        fn finite(v: f64) -> f64 {
            if v.is_finite() {
                v
            } else {
                panic!("motion override values must be finite numbers")
            }
        }
        let peak_speed = self.peak_speed.map(|v| finite(v).clamp(50.0, 5000.0));
        let effective_peak = peak_speed.unwrap_or_else(|| motion_spec().shared.peak_speed);
        // The end floor keeps the arrival impulse positive: a 0 floor would
        // stall the glide exactly at u = 1 (speed → 0 before arrival).
        Self {
            schema: self.schema.clone(),
            peak_speed,
            min_start_speed: self
                .min_start_speed
                .map(|v| finite(v).clamp(1.0, effective_peak)),
            min_end_speed: self
                .min_end_speed
                .map(|v| finite(v).clamp(1.0, effective_peak)),
            turn_radius: self.turn_radius.map(|v| finite(v).clamp(1.0, 1000.0)),
            spring: self.spring.map(|v| finite(v).clamp(0.3, 1.0)),
            glide_duration_ms: self.glide_duration_ms.map(|v| finite(v).clamp(0.0, 5000.0)),
        }
    }

    fn apply(&self, mut config: MotionConfig) -> (MotionConfig, Vec<&'static str>) {
        let mut overridden = Vec::new();
        if let Some(v) = self.peak_speed {
            config.peak_speed = v;
            overridden.push("peak_speed");
        }
        if let Some(v) = self.min_start_speed {
            config.min_start_speed = v;
            overridden.push("min_start_speed");
        }
        if let Some(v) = self.min_end_speed {
            config.min_end_speed = v;
            overridden.push("min_end_speed");
        }
        if let Some(v) = self.turn_radius {
            config.turn_radius = v;
            overridden.push("turn_radius");
        }
        if let Some(v) = self.spring {
            config.spring = v;
            overridden.push("spring");
        }
        if let Some(v) = self.glide_duration_ms {
            config.glide_duration_ms = v;
            overridden.push("glide_duration_ms");
        }
        (config, overridden)
    }
}

/// The movement override applied to this render, if any. Held in an Option so
/// the static (non-dev) export path provably never renders overridden physics.
static MOTION_OVERRIDES: std::sync::OnceLock<Option<MotionOverrideFile>> =
    std::sync::OnceLock::new();

fn movement_motion_config() -> (MotionConfig, Vec<&'static str>) {
    let defaults = MotionConfig::default();
    match MOTION_OVERRIDES.get().and_then(|o| o.as_ref()) {
        Some(file) => file.apply(defaults),
        None => (defaults, Vec::new()),
    }
}

#[derive(Serialize)]
struct ActionManifest {
    id: &'static str,
    authored_frames: usize,
    still_frame: u16,
    playback: &'static str,
}

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

fn parse_args() -> Args {
    let mut values = std::env::args().skip(1);
    let output = values
        .next()
        .map(PathBuf::from)
        .expect("usage: export_gallery_frames <output-directory> [--theme <artifact>] [--dev] [--motion-overrides <file>]");
    let mut theme = None;
    let mut dev = false;
    let mut motion_overrides = None;
    while let Some(value) = values.next() {
        match value.as_str() {
            "--theme" => {
                theme = Some(
                    values
                        .next()
                        .map(PathBuf::from)
                        .expect("--theme requires a compiled .cua-theme path"),
                );
            }
            "--dev" => dev = true,
            "--motion-overrides" => {
                motion_overrides = Some(
                    values
                        .next()
                        .map(PathBuf::from)
                        .expect("--motion-overrides requires a JSON override file path"),
                );
            }
            other => panic!("unknown argument `{other}`"),
        }
    }
    if !dev && motion_overrides.is_some() {
        panic!(
            "--motion-overrides requires --dev: overrides exist only in the repository playground"
        );
    }
    Args {
        output,
        theme,
        dev,
        motion_overrides,
    }
}

fn main() {
    let args = parse_args();
    let output = args.output.as_path();
    MOTION_OVERRIDES
        .set(
            args.motion_overrides
                .as_deref()
                .map(MotionOverrideFile::load),
        )
        .expect("motion overrides are initialised exactly once");
    let theme = args
        .theme
        .as_deref()
        .map(inspect_artifact)
        .transpose()
        .expect("load compiled cursor theme")
        .unwrap_or_else(|| (*cursor_overlay::embedded_default_theme()).clone());
    let theme = Arc::new(theme);

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
    export_states(states, &theme);
    export_reduced_states(output, &theme);
    if args.dev {
        export_states(runtime_preview_states(output), &theme);
        export_movement_states(output, &theme);
    } else {
        export_states(preview_states(output), &theme);
    }
    write_manifest(output, &theme);
}

fn runtime_preview_states(output: &Path) -> Vec<(PathBuf, GalleryState)> {
    CursorAction::ALL
        .into_iter()
        .map(|action| {
            (
                output.join("runtime").join(action.as_str()),
                GalleryState {
                    action,
                    delivery: Some(DeliveryModifier::Background),
                    target: Some(TargetModifier::Browser),
                    session_label: Some(RUNTIME_SESSION_LABEL),
                },
            )
        })
        .collect()
}

fn export_states(states: Vec<(PathBuf, GalleryState)>, theme: &Arc<CompiledTheme>) {
    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .min(12)
        .min(states.len().max(1));
    let chunk_size = states.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        for chunk in states.chunks(chunk_size) {
            let theme = theme.clone();
            scope.spawn(move || {
                for (path, state) in chunk {
                    export_state(path, *state, &theme);
                }
            });
        }
    });
}

fn configured_core(theme: &Arc<CompiledTheme>) -> RenderStateCore {
    let mut config = CursorConfig::default();
    config.cursor_id = "gallery-session".into();
    let mut core = RenderStateCore::new(config);
    core.theme = Some(theme.clone());
    core.motion.idle_hide_ms = 0.0;
    core.pos = (
        f64::from(SIZE) / (2.0 * f64::from(PREVIEW_BACKING_SCALE)),
        f64::from(SIZE) / (2.0 * f64::from(PREVIEW_BACKING_SCALE)),
    );
    core.heading = f64::from(std::f32::consts::FRAC_PI_4);
    core
}

fn apply_production_command(core: &mut RenderStateCore, command: OverlayCommand) {
    #[cfg(target_os = "macos")]
    core.apply_command_base(command, true, true);

    #[cfg(not(target_os = "macos"))]
    core.apply_command_base(command, false, false);
}

fn apply_gallery_state(core: &mut RenderStateCore, state: GalleryState) {
    if let Some(session_label) = state.session_label {
        apply_production_command(core, OverlayCommand::SetSessionLabel(session_label.into()));
    }
    apply_production_command(
        core,
        OverlayCommand::BeginAction {
            action: state.action,
            delivery: state.delivery,
            target: state.target,
        },
    );
}

fn export_state(output: &Path, state: GalleryState, theme: &Arc<CompiledTheme>) {
    fs::create_dir_all(output).expect("create frame output");
    for frame in 0..ACTION_FPS * DURATION_SECS {
        let mut core = configured_core(theme);
        apply_gallery_state(&mut core, state);
        core.visual.elapsed_secs = f64::from(frame) / f64::from(ACTION_FPS);
        save_frame(&core, output.join(format!("{frame:04}.png")), SIZE, SIZE);
    }
}

fn movement_core(theme: &Arc<CompiledTheme>, action: CursorAction) -> RenderStateCore {
    let mut core = configured_core(theme);
    let (motion, _) = movement_motion_config();
    core.motion = motion;
    // Previews never hide the cursor mid-glide (configured_core contract).
    core.motion.idle_hide_ms = 0.0;
    core.pos = (
        MOVEMENT_TARGETS[0].0 - FIRST_ACTION_SEED_OFFSET,
        MOVEMENT_TARGETS[0].1 - FIRST_ACTION_SEED_OFFSET,
    );
    core.heading = PRODUCTION_END_HEADING;
    apply_gallery_state(
        &mut core,
        GalleryState {
            action,
            delivery: None,
            target: None,
            session_label: None,
        },
    );
    core
}

fn tick_production_motion(core: &mut RenderStateCore, dt: f64) -> bool {
    #[cfg(target_os = "macos")]
    return core.tick_swift_constants(dt);

    #[cfg(not(target_os = "macos"))]
    return core.tick_motion(dt);
}

fn movement_target(core: &mut RenderStateCore, position: (f64, f64)) {
    apply_production_command(
        core,
        OverlayCommand::MoveTo {
            x: position.0,
            y: position.1,
            end_heading_radians: PRODUCTION_END_HEADING,
        },
    );
}

fn export_movement_states(output: &Path, theme: &Arc<CompiledTheme>) {
    let worker_count = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .min(CursorAction::ALL.len());
    let chunk_size = CursorAction::ALL.len().div_ceil(worker_count);
    std::thread::scope(|scope| {
        for actions in CursorAction::ALL.chunks(chunk_size) {
            let theme = theme.clone();
            scope.spawn(move || {
                for action in actions {
                    let path = output.join("movement").join(action.as_str());
                    fs::create_dir_all(&path).expect("create movement frame output");
                    let mut core = movement_core(&theme, *action);
                    let mut active_target = 0;
                    movement_target(&mut core, MOVEMENT_TARGETS[active_target]);
                    for frame in 0..MOVEMENT_FRAME_COUNT {
                        let dt = if frame == 0 {
                            0.0
                        } else {
                            f64::from(MOVEMENT_TICK_MS) / 1000.0
                        };
                        let arrived = tick_production_motion(&mut core, dt);
                        save_frame_at_scale(
                            &core,
                            path.join(format!("{frame:04}.png")),
                            MOVEMENT_WIDTH,
                            MOVEMENT_HEIGHT,
                            MOVEMENT_BACKING_SCALE,
                        );
                        if arrived && active_target + 1 < MOVEMENT_TARGETS.len() {
                            active_target += 1;
                            movement_target(&mut core, MOVEMENT_TARGETS[active_target]);
                        }
                    }
                }
            });
        }
    });
}

fn export_reduced_states(output: &Path, theme: &Arc<CompiledTheme>) {
    let reduced = output.join("reduced");
    let actions_output = reduced.join("actions");
    let runtime_output = reduced.join("runtime");
    let movement_output = reduced.join("movement");
    fs::create_dir_all(&actions_output).expect("create reduced-motion action output");
    fs::create_dir_all(&runtime_output).expect("create reduced-motion runtime output");
    fs::create_dir_all(&movement_output).expect("create reduced-motion movement output");
    for action in CursorAction::ALL {
        let mut core = configured_core(theme);
        core.visual.reduced_motion = ReducedMotion::On;
        apply_gallery_state(
            &mut core,
            GalleryState {
                action,
                delivery: None,
                target: None,
                session_label: None,
            },
        );
        save_frame(
            &core,
            actions_output.join(format!("{}.png", action.as_str())),
            SIZE,
            SIZE,
        );

        let mut runtime = configured_core(theme);
        runtime.visual.reduced_motion = ReducedMotion::On;
        apply_gallery_state(
            &mut runtime,
            GalleryState {
                action,
                delivery: Some(DeliveryModifier::Background),
                target: Some(TargetModifier::Browser),
                session_label: Some(RUNTIME_SESSION_LABEL),
            },
        );
        save_frame(
            &runtime,
            runtime_output.join(format!("{}.png", action.as_str())),
            SIZE,
            SIZE,
        );

        let mut movement = movement_core(theme, action);
        movement.visual.reduced_motion = ReducedMotion::On;
        save_frame_at_scale(
            &movement,
            movement_output.join(format!("{}.png", action.as_str())),
            MOVEMENT_WIDTH,
            MOVEMENT_HEIGHT,
            MOVEMENT_BACKING_SCALE,
        );
    }
}

fn save_frame(core: &RenderStateCore, path: PathBuf, width: u32, height: u32) {
    save_frame_at_scale(core, path, width, height, PREVIEW_BACKING_SCALE);
}

fn save_frame_at_scale(
    core: &RenderStateCore,
    path: PathBuf,
    width: u32,
    height: u32,
    backing_scale: f32,
) {
    let pixmap = render_frame(core, width, height, 0.0, 0.0, None, backing_scale);
    let pixels = unpremultiply_rgba(pixmap.data().to_vec());
    image::save_buffer_with_format(
        path,
        &pixels,
        width,
        height,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .expect("write frame");
}

fn write_manifest(output: &Path, theme: &CompiledTheme) {
    let actions = CursorAction::ALL
        .into_iter()
        .map(|action| {
            let animation = theme
                .animation_for_action(action)
                .expect("compiled theme contains every cursor action");
            ActionManifest {
                id: action.as_str(),
                authored_frames: animation.frames.len(),
                still_frame: animation.still_frame,
                playback: match action.playback() {
                    cursor_overlay::PlaybackKind::Resting => "resting",
                    cursor_overlay::PlaybackKind::OneShot => "one_shot",
                    cursor_overlay::PlaybackKind::Held => "held",
                    cursor_overlay::PlaybackKind::Loop => "loop",
                },
            }
        })
        .collect();
    let spec = motion_spec();
    let (effective, overridden_fields) = movement_motion_config();
    let manifest = GalleryManifest {
        schema: "cua.cursor-gallery/2",
        theme_id: &theme.id,
        theme_name: &theme.name,
        theme_version: &theme.version,
        content_hash: theme.content_hash(),
        fps: ACTION_FPS,
        movement_fps: 1000.0 / f64::from(MOVEMENT_TICK_MS),
        movement_tick_ms: MOVEMENT_TICK_MS,
        duration_secs: DURATION_SECS,
        motion: motion_manifest(spec, &effective, overridden_fields),
        actions,
    };
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("serialize gallery manifest"),
    )
    .expect("write gallery manifest");
}

/// Build the movement echo for the manifest. `swift_constants` keeps its
/// fixed spec constants regardless of the runtime `spring` value — the
/// documented macOS limitation — while `motion` derives them.
fn motion_manifest(
    spec: &cursor_overlay::MotionSpec,
    effective: &MotionConfig,
    overridden_fields: Vec<&'static str>,
) -> MotionManifest {
    #[cfg(target_os = "macos")]
    let (tick_path, spring_settle): (&'static str, SpringSettleEcho) = {
        let settle = &spec.spring_settle.macos;
        (
            "swift_constants",
            SpringSettleEcho {
                mode: "fixed",
                k: settle.k,
                c: settle.c,
                overshoot: settle.overshoot,
            },
        )
    };
    #[cfg(not(target_os = "macos"))]
    let (tick_path, spring_settle): (&'static str, SpringSettleEcho) = {
        let settle = &spec.spring_settle.windows_linux;
        (
            "motion",
            SpringSettleEcho {
                mode: "derived",
                k: settle.k_per_spring * effective.spring,
                c: settle.c_per_spring * effective.spring,
                overshoot: settle.overshoot,
            },
        )
    };
    MotionManifest {
        spec_schema: spec.schema.clone(),
        tick_path,
        effective: MotionConfigEcho {
            peak_speed: effective.peak_speed,
            min_start_speed: effective.min_start_speed,
            min_end_speed: effective.min_end_speed,
            turn_radius: effective.turn_radius,
            spring: effective.spring,
            glide_duration_ms: effective.glide_duration_ms,
        },
        spring_settle,
        overridden_fields,
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

    #[test]
    fn development_inventory_keeps_one_exact_runtime_composition_per_action() {
        let root = Path::new("gallery");
        let states = runtime_preview_states(root);
        assert_eq!(states.len(), CursorAction::ALL.len());
        for (path, state) in states {
            assert_eq!(path, root.join("runtime").join(state.action.as_str()));
            assert_eq!(state.delivery, Some(DeliveryModifier::Background));
            assert_eq!(state.target, Some(TargetModifier::Browser));
            assert_eq!(state.session_label, Some(RUNTIME_SESSION_LABEL));
        }
    }

    #[test]
    fn movement_preview_drives_the_production_motion_path() {
        let theme = cursor_overlay::embedded_default_theme();
        let mut core = movement_core(&theme, CursorAction::Navigate);
        let start = core.pos;
        assert_eq!(core.motion.glide_duration_ms, 0.0);

        movement_target(&mut core, MOVEMENT_TARGETS[0]);
        assert!(core.path.is_some());

        tick_production_motion(&mut core, f64::from(MOVEMENT_TICK_MS) / 1000.0);
        assert_ne!(core.pos, start);
        assert_eq!(core.visual.resolved_action, CursorAction::Navigate);
    }

    #[test]
    fn movement_core_uses_spec_defaults_without_overrides() {
        let theme = cursor_overlay::embedded_default_theme();
        let core = movement_core(&theme, CursorAction::Observe);
        let mut expected = MotionConfig::default();
        // The exporter never hides the cursor mid-preview.
        expected.idle_hide_ms = 0.0;
        assert_eq!(core.motion, expected);
    }

    #[test]
    fn motion_overrides_apply_clamp_and_stay_scoped_to_dev() {
        let file = MotionOverrideFile {
            schema: MOTION_OVERRIDE_SCHEMA.into(),
            peak_speed: Some(99_999.0),
            min_start_speed: Some(-5.0),
            min_end_speed: Some(0.0),
            turn_radius: Some(2_000.0),
            spring: Some(0.05),
            glide_duration_ms: Some(9_000.0),
        };
        let clamped = file.clamped();
        assert_eq!(clamped.peak_speed, Some(5000.0));
        assert_eq!(clamped.min_start_speed, Some(1.0));
        assert_eq!(clamped.min_end_speed, Some(1.0));
        assert_eq!(clamped.turn_radius, Some(1000.0));
        assert_eq!(clamped.spring, Some(0.3));
        assert_eq!(clamped.glide_duration_ms, Some(5000.0));

        let (effective, overridden) = clamped.apply(MotionConfig::default());
        assert_eq!(effective.peak_speed, 5000.0);
        assert_eq!(effective.min_end_speed, 1.0);
        assert_eq!(
            overridden,
            vec![
                "peak_speed",
                "min_start_speed",
                "min_end_speed",
                "turn_radius",
                "spring",
                "glide_duration_ms"
            ]
        );
    }

    #[test]
    fn floors_cannot_exceed_the_effective_peak_speed() {
        let file = MotionOverrideFile {
            schema: MOTION_OVERRIDE_SCHEMA.into(),
            peak_speed: Some(120.0),
            min_start_speed: Some(400.0),
            min_end_speed: Some(350.0),
            turn_radius: None,
            spring: None,
            glide_duration_ms: None,
        };
        let clamped = file.clamped();
        assert_eq!(clamped.min_start_speed, Some(120.0));
        assert_eq!(clamped.min_end_speed, Some(120.0));
    }

    #[test]
    fn motion_overrides_reject_unknown_fields() {
        // Unknown fields are denied by the schema, so a stale or hostile
        // override file cannot smuggle non-movement knobs into a render.
        assert!(serde_json::from_str::<MotionOverrideFile>(
            r#"{"schema": "cua.cursor-gallery-motion-override/1", "idle_hide_ms": 5.0}"#
        )
        .is_err());
        // The schema string is a namespaced constant, never free-form.
        assert_ne!(MOTION_OVERRIDE_SCHEMA, "cua.other/9");
    }

    #[test]
    fn empty_overrides_render_exact_production_defaults() {
        let file = MotionOverrideFile {
            schema: MOTION_OVERRIDE_SCHEMA.into(),
            peak_speed: None,
            min_start_speed: None,
            min_end_speed: None,
            turn_radius: None,
            spring: None,
            glide_duration_ms: None,
        };
        let (effective, overridden) = file.apply(MotionConfig::default());
        assert_eq!(effective, MotionConfig::default());
        assert!(overridden.is_empty());
    }

    #[test]
    fn manifest_motion_block_reports_the_spec_contract() {
        let spec = motion_spec();
        let (effective, overridden) = movement_motion_config();
        let block = motion_manifest(spec, &effective, overridden);
        assert_eq!(block.spec_schema, spec.schema);
        assert!(matches!(block.tick_path, "swift_constants" | "motion"));
        assert_eq!(block.effective.peak_speed, spec.shared.peak_speed);
        assert_eq!(block.effective.turn_radius, spec.shared.turn_radius);
        assert_eq!(block.effective.spring, spec.shared.spring);
        assert!(block.overridden_fields.is_empty());
    }

    #[test]
    fn movement_preview_stays_inside_the_canvas() {
        const EDGE_GUARD: u32 = 12;
        let theme = cursor_overlay::embedded_default_theme();
        let mut core = movement_core(&theme, CursorAction::Observe);

        let mut active_target = 0;
        movement_target(&mut core, MOVEMENT_TARGETS[active_target]);
        for frame in 0..MOVEMENT_FRAME_COUNT {
            let dt = if frame == 0 {
                0.0
            } else {
                f64::from(MOVEMENT_TICK_MS) / 1000.0
            };
            let arrived = tick_production_motion(&mut core, dt);

            let pixmap = render_frame(
                &core,
                MOVEMENT_WIDTH,
                MOVEMENT_HEIGHT,
                0.0,
                0.0,
                None,
                MOVEMENT_BACKING_SCALE,
            );
            let mut bounds = None;
            for y in 0..MOVEMENT_HEIGHT {
                for x in 0..MOVEMENT_WIDTH {
                    let alpha = pixmap.data()[((y * MOVEMENT_WIDTH + x) * 4 + 3) as usize];
                    if alpha == 0 {
                        continue;
                    }
                    let (min_x, min_y, max_x, max_y) = bounds.get_or_insert((x, y, x, y));
                    *min_x = (*min_x).min(x);
                    *min_y = (*min_y).min(y);
                    *max_x = (*max_x).max(x);
                    *max_y = (*max_y).max(y);
                }
            }
            if let Some((min_x, min_y, max_x, max_y)) = bounds {
                assert!(
                    min_x >= EDGE_GUARD
                        && min_y >= EDGE_GUARD
                        && max_x < MOVEMENT_WIDTH - EDGE_GUARD
                        && max_y < MOVEMENT_HEIGHT - EDGE_GUARD,
                    "movement frame {frame} at {:?} entered the canvas edge guard: ({min_x}, {min_y})..({max_x}, {max_y})",
                    core.pos
                );
            }
            if arrived && active_target + 1 < MOVEMENT_TARGETS.len() {
                active_target += 1;
                movement_target(&mut core, MOVEMENT_TARGETS[active_target]);
            }
        }
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
