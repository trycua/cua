//! Bounded compiled cursor-theme artifacts and the local installed-theme store.
//!
//! `.cua-theme` files contain zstd-compressed postcard data behind a fixed
//! header. The privileged overlay never parses Lottie, ZIP, JSON, fonts,
//! expressions, URLs, or arbitrary source paths.

use crate::{CursorAction, CursorVisualState};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Stroke, Transform};

const MAGIC: &[u8; 8] = b"CUATHEM3";
const LEGACY_MAGIC: &[u8; 8] = b"CUATHEM2";
const HEADER_LEN: usize = 8 + 2 + 4 + 32;
const ARTIFACT_VERSION: u16 = 3;
const MAX_COMPRESSED_BYTES: usize = 24 * 1024 * 1024;
const MAX_DECOMPRESSED_BYTES: usize = 24 * 1024 * 1024;
const MAX_TOTAL_FRAMES: usize = 1_000;
const MAX_FRAMES_PER_ANIMATION: usize = 120;
const MAX_COMMANDS_PER_FRAME: usize = 128;
const MAX_GEOMETRIES_PER_COMMAND: usize = 64;
const MAX_PATH_POINTS: usize = 512;
const MAX_TEXT_BYTES: usize = 200;
const CANVAS: u32 = 128;
const FPS: u16 = 30;
const DEFAULT_THEME_BYTES: &[u8] = include_bytes!("../assets/cua.default.cua-theme");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CompiledGeometry {
    Path {
        vertices: Vec<[f32; 2]>,
        in_tangents: Vec<[f32; 2]>,
        out_tangents: Vec<[f32; 2]>,
        closed: bool,
    },
    Ellipse {
        center: [f32; 2],
        size: [f32; 2],
    },
    Rectangle {
        center: [f32; 2],
        size: [f32; 2],
        roundness: f32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CompiledTransform {
    pub anchor: [f32; 2],
    pub position: [f32; 2],
    /// Multipliers, where `[1.0, 1.0]` is the Lottie 100% scale.
    pub scale: [f32; 2],
    pub rotation_degrees: f32,
}

impl Default for CompiledTransform {
    fn default() -> Self {
        Self {
            anchor: [0.0, 0.0],
            position: [0.0, 0.0],
            scale: [1.0, 1.0],
            rotation_degrees: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CompiledStroke {
    pub color: [u8; 4],
    pub width: f32,
    pub line_cap: u8,
    pub line_join: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledDrawCommand {
    pub geometries: Vec<CompiledGeometry>,
    pub transform: CompiledTransform,
    pub opacity: f32,
    pub fill: Option<[u8; 4]>,
    pub stroke: Option<CompiledStroke>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledFrame {
    /// Bounded vector commands in the theme's 128×128 coordinate space.
    pub commands: Vec<CompiledDrawCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledAnimation {
    pub still_frame: u16,
    pub frames: Vec<CompiledFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompiledTheme {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub license: String,
    pub profile: String,
    pub source_hash: [u8; 32],
    pub hotspot: [u16; 2],
    pub actions: BTreeMap<String, CompiledAnimation>,
}

impl CompiledTheme {
    pub fn content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        if let Ok(bytes) = postcard::to_allocvec(self) {
            hasher.update(bytes);
        }
        hex_digest(hasher.finalize().as_slice())
    }

    pub fn animation_for_action(&self, action: CursorAction) -> Option<&CompiledAnimation> {
        self.actions.get(action.as_str())
    }
}

fn valid_theme_id(value: &str) -> bool {
    value.len() <= MAX_TEXT_BYTES
        && value.contains('.')
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

fn bounded_number(value: f32) -> bool {
    value.is_finite() && value.abs() <= 4096.0
}

fn validate_geometry(name: &str, frame: usize, geometry: &CompiledGeometry) -> Result<()> {
    let validate_pair = |label: &str, pair: [f32; 2]| -> Result<()> {
        if !pair.into_iter().all(bounded_number) {
            bail!("animation `{name}` frame {frame} has an invalid {label}");
        }
        Ok(())
    };
    match geometry {
        CompiledGeometry::Path {
            vertices,
            in_tangents,
            out_tangents,
            ..
        } => {
            if vertices.is_empty()
                || vertices.len() > MAX_PATH_POINTS
                || in_tangents.len() != vertices.len()
                || out_tangents.len() != vertices.len()
            {
                bail!("animation `{name}` frame {frame} has an invalid vector path");
            }
            for point in vertices
                .iter()
                .chain(in_tangents.iter())
                .chain(out_tangents.iter())
            {
                validate_pair("path coordinate", *point)?;
            }
        }
        CompiledGeometry::Ellipse { center, size } => {
            validate_pair("ellipse center", *center)?;
            validate_pair("ellipse size", *size)?;
            if size[0] < 0.0 || size[1] < 0.0 {
                bail!("animation `{name}` frame {frame} has a negative ellipse size");
            }
        }
        CompiledGeometry::Rectangle {
            center,
            size,
            roundness,
        } => {
            validate_pair("rectangle center", *center)?;
            validate_pair("rectangle size", *size)?;
            if size[0] < 0.0
                || size[1] < 0.0
                || !roundness.is_finite()
                || *roundness < 0.0
                || *roundness > 2048.0
            {
                bail!("animation `{name}` frame {frame} has invalid rectangle geometry");
            }
        }
    }
    Ok(())
}

fn validate_animation(name: &str, animation: &CompiledAnimation) -> Result<usize> {
    if animation.frames.is_empty() || animation.frames.len() > MAX_FRAMES_PER_ANIMATION {
        bail!("animation `{name}` must contain 1..={MAX_FRAMES_PER_ANIMATION} frames");
    }
    if usize::from(animation.still_frame) >= animation.frames.len() {
        bail!("animation `{name}` has an out-of-range still_frame");
    }
    for (index, frame) in animation.frames.iter().enumerate() {
        if frame.commands.len() > MAX_COMMANDS_PER_FRAME {
            bail!("animation `{name}` frame {index} has too many vector commands");
        }
        for command in &frame.commands {
            if command.geometries.is_empty()
                || command.geometries.len() > MAX_GEOMETRIES_PER_COMMAND
                || !command.opacity.is_finite()
                || !(0.0..=1.0).contains(&command.opacity)
            {
                bail!("animation `{name}` frame {index} has an invalid vector command");
            }
            for pair in [
                command.transform.anchor,
                command.transform.position,
                command.transform.scale,
            ] {
                if !pair.into_iter().all(bounded_number) {
                    bail!("animation `{name}` frame {index} has an invalid transform");
                }
            }
            if !bounded_number(command.transform.rotation_degrees) {
                bail!("animation `{name}` frame {index} has an invalid rotation");
            }
            if command.fill.is_none() && command.stroke.is_none() {
                bail!("animation `{name}` frame {index} has a command without paint");
            }
            if let Some(stroke) = command.stroke {
                if !stroke.width.is_finite()
                    || stroke.width <= 0.0
                    || stroke.width > 512.0
                    || !(1..=3).contains(&stroke.line_cap)
                    || !(1..=3).contains(&stroke.line_join)
                {
                    bail!("animation `{name}` frame {index} has an invalid stroke");
                }
            }
            for geometry in &command.geometries {
                validate_geometry(name, index, geometry)?;
            }
        }
    }
    Ok(animation.frames.len())
}

pub fn validate_compiled_theme(theme: &CompiledTheme, full: bool) -> Result<()> {
    if !valid_theme_id(&theme.id) {
        bail!("theme id must be a bounded reverse-DNS identifier");
    }
    for (label, value) in [
        ("name", theme.name.as_str()),
        ("version", theme.version.as_str()),
        ("author", theme.author.as_str()),
        ("license", theme.license.as_str()),
        ("profile", theme.profile.as_str()),
    ] {
        if value.is_empty() || value.len() > MAX_TEXT_BYTES {
            bail!("{label} must contain 1..={MAX_TEXT_BYTES} bytes");
        }
    }
    if theme.hotspot[0] >= CANVAS as u16 || theme.hotspot[1] >= CANVAS as u16 {
        bail!("hotspot must be within the {CANVAS}×{CANVAS} canvas");
    }

    if full {
        for action in CursorAction::ALL {
            if !theme.actions.contains_key(action.as_str()) {
                bail!("full profile is missing action `{}`", action.as_str());
            }
        }
    } else if !theme.actions.contains_key("idle") || !theme.actions.contains_key("click") {
        bail!("development profile requires at least `idle` and `click`");
    }

    let mut total_frames = 0usize;
    for (name, animation) in &theme.actions {
        total_frames += validate_animation(name, animation)?;
        if total_frames > MAX_TOTAL_FRAMES {
            bail!("theme exceeds the {MAX_TOTAL_FRAMES}-frame limit");
        }
    }
    Ok(())
}

#[cfg(any(test, feature = "theme-authoring"))]
pub fn encode_theme(theme: &CompiledTheme) -> Result<Vec<u8>> {
    validate_compiled_theme(theme, theme.profile == crate::THEME_PROFILE)?;
    let payload = postcard::to_allocvec(theme).context("serialize compiled theme")?;
    if payload.len() > MAX_DECOMPRESSED_BYTES {
        bail!("compiled theme exceeds the decoded size limit");
    }
    let compressed =
        zstd::stream::encode_all(Cursor::new(&payload), 12).context("compress compiled theme")?;
    if compressed.len() > MAX_COMPRESSED_BYTES {
        bail!("compiled theme exceeds the compressed size limit");
    }
    let mut output = Vec::with_capacity(HEADER_LEN + compressed.len());
    output.extend_from_slice(MAGIC);
    output.extend_from_slice(&ARTIFACT_VERSION.to_le_bytes());
    output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    output.extend_from_slice(&theme.source_hash);
    output.extend_from_slice(&compressed);
    Ok(output)
}

pub fn decode_theme(bytes: &[u8]) -> Result<CompiledTheme> {
    if bytes.len() >= LEGACY_MAGIC.len() && &bytes[..LEGACY_MAGIC.len()] == LEGACY_MAGIC {
        bail!(
            "cursor-theme artifact targets the retired v1 contract; rebuild the source as cua.cursor-theme/2"
        );
    }
    if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
        bail!("not a Cua cursor-theme artifact");
    }
    if bytes.len() - HEADER_LEN > MAX_COMPRESSED_BYTES {
        bail!("cursor-theme artifact exceeds the compressed size limit");
    }
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != ARTIFACT_VERSION {
        bail!("unsupported cursor-theme artifact version {version}");
    }
    let decoded_len = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
    if decoded_len > MAX_DECOMPRESSED_BYTES {
        bail!("cursor-theme artifact exceeds the decoded size limit");
    }
    let decoder = zstd::stream::read::Decoder::new(Cursor::new(&bytes[HEADER_LEN..]))
        .context("open cursor-theme payload")?;
    let mut decoded = Vec::with_capacity(decoded_len.min(1024 * 1024));
    decoder
        .take((MAX_DECOMPRESSED_BYTES + 1) as u64)
        .read_to_end(&mut decoded)
        .context("decode cursor-theme payload")?;
    if decoded.len() != decoded_len {
        bail!("cursor-theme decoded length does not match its header");
    }
    let theme: CompiledTheme =
        postcard::from_bytes(&decoded).context("parse cursor-theme payload")?;
    if theme.source_hash != bytes[14..46] {
        bail!("cursor-theme source hash does not match its header");
    }
    validate_compiled_theme(&theme, theme.profile == crate::THEME_PROFILE)?;
    Ok(theme)
}

pub fn theme_store_root() -> Result<PathBuf> {
    if let Some(override_path) = std::env::var_os("CUA_DRIVER_CURSOR_THEME_DIR") {
        let path = PathBuf::from(override_path);
        if !path.is_absolute() {
            bail!("CUA_DRIVER_CURSOR_THEME_DIR must be absolute");
        }
        return Ok(path);
    }
    #[cfg(target_os = "windows")]
    {
        let root = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("LOCALAPPDATA is unavailable"))?;
        return Ok(root.join("Cua Driver").join("cursor-themes"));
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("HOME is unavailable"))?;
        Ok(home
            .join("Library")
            .join("Application Support")
            .join("Cua Driver")
            .join("cursor-themes"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(root) = std::env::var_os("XDG_DATA_HOME") {
            return Ok(PathBuf::from(root).join("cua-driver").join("cursor-themes"));
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("HOME is unavailable"))?;
        Ok(home
            .join(".local")
            .join("share")
            .join("cua-driver")
            .join("cursor-themes"))
    }
}

fn installed_path(id: &str) -> Result<PathBuf> {
    if !valid_theme_id(id) {
        bail!("invalid cursor theme id");
    }
    Ok(theme_store_root()?.join(format!("{id}.cua-theme")))
}

#[cfg(feature = "theme-authoring")]
pub fn install_artifact(bytes: &[u8]) -> Result<PathBuf> {
    let theme = decode_theme(bytes)?;
    let root = theme_store_root()?;
    fs::create_dir_all(&root).context("create cursor-theme store")?;
    let target = installed_path(&theme.id)?;
    let temporary = root.join(format!(".{}.{}.tmp", theme.id, std::process::id()));
    fs::write(&temporary, bytes).context("write staged cursor theme")?;
    fs::rename(&temporary, &target).context("atomically install cursor theme")?;
    Ok(target)
}

#[cfg(feature = "theme-authoring")]
pub fn uninstall_theme(id: &str) -> Result<bool> {
    if id == crate::DEFAULT_THEME_ID {
        bail!("the embedded default theme cannot be uninstalled");
    }
    let path = installed_path(id)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("remove installed cursor theme"),
    }
}

pub fn list_installed_themes() -> Result<Vec<String>> {
    let mut ids = vec![crate::DEFAULT_THEME_ID.to_owned()];
    let root = theme_store_root()?;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
        Err(error) => return Err(error).context("read cursor-theme store"),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(id) = name.strip_suffix(".cua-theme") {
            if valid_theme_id(id) {
                ids.push(id.to_owned());
            }
        }
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn theme_cache() -> &'static Mutex<BTreeMap<String, Arc<CompiledTheme>>> {
    static CACHE: OnceLock<Mutex<BTreeMap<String, Arc<CompiledTheme>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub fn load_installed_theme(id: &str) -> Result<Option<Arc<CompiledTheme>>> {
    if id == crate::DEFAULT_THEME_ID {
        return Ok(Some(embedded_default_theme()));
    }
    if let Some(theme) = theme_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(id)
        .cloned()
    {
        return Ok(Some(theme));
    }
    let path = installed_path(id)?;
    let metadata = fs::symlink_metadata(&path).context("locate installed cursor theme")?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("installed cursor theme is not a regular file");
    }
    if metadata.len() as usize > HEADER_LEN + MAX_COMPRESSED_BYTES {
        bail!("installed cursor theme exceeds the size limit");
    }
    let bytes = fs::read(&path).context("read installed cursor theme")?;
    let theme = decode_theme(&bytes)?;
    if theme.id != id {
        bail!("installed cursor theme id does not match its filename");
    }
    let theme = Arc::new(theme);
    theme_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id.to_owned(), Arc::clone(&theme));
    Ok(Some(theme))
}

pub fn resolve_theme_selection(id: &str) -> Result<Option<Arc<CompiledTheme>>> {
    load_installed_theme(id)
}

/// Decode the checked-in default artifact once per process.
///
/// The bytes are produced by `assets/build_default_theme.py`, validated by the
/// same authoring compiler as custom themes, and embedded in the binary. A
/// corrupt checked-in artifact is a build/test defect rather than a recoverable
/// user configuration error.
pub fn embedded_default_theme() -> Arc<CompiledTheme> {
    static DEFAULT: OnceLock<Arc<CompiledTheme>> = OnceLock::new();
    Arc::clone(DEFAULT.get_or_init(|| {
        let theme =
            decode_theme(DEFAULT_THEME_BYTES).expect("embedded cua.default theme must be valid");
        assert_eq!(
            theme.id,
            crate::DEFAULT_THEME_ID,
            "embedded default theme id must remain stable"
        );
        Arc::new(theme)
    }))
}

fn animation_frame(
    animation: &CompiledAnimation,
    elapsed_secs: f64,
    reduced_motion: bool,
) -> Option<&CompiledFrame> {
    let index = if reduced_motion {
        usize::from(animation.still_frame)
    } else {
        ((elapsed_secs.max(0.0) * f64::from(FPS)).floor() as usize) % animation.frames.len()
    };
    animation.frames.get(index)
}

fn compiled_transform(transform: CompiledTransform) -> Transform {
    Transform::from_translate(-transform.anchor[0], -transform.anchor[1])
        .post_scale(transform.scale[0], transform.scale[1])
        .post_rotate(transform.rotation_degrees)
        .post_translate(transform.position[0], transform.position[1])
}

fn append_ellipse(builder: &mut PathBuilder, center: [f32; 2], size: [f32; 2]) {
    const K: f32 = 0.552_284_8;
    let rx = size[0] * 0.5;
    let ry = size[1] * 0.5;
    let [cx, cy] = center;
    builder.move_to(cx + rx, cy);
    builder.cubic_to(cx + rx, cy + ry * K, cx + rx * K, cy + ry, cx, cy + ry);
    builder.cubic_to(cx - rx * K, cy + ry, cx - rx, cy + ry * K, cx - rx, cy);
    builder.cubic_to(cx - rx, cy - ry * K, cx - rx * K, cy - ry, cx, cy - ry);
    builder.cubic_to(cx + rx * K, cy - ry, cx + rx, cy - ry * K, cx + rx, cy);
    builder.close();
}

fn append_rectangle(builder: &mut PathBuilder, center: [f32; 2], size: [f32; 2], roundness: f32) {
    const K: f32 = 0.552_284_8;
    let width = size[0];
    let height = size[1];
    let x = center[0] - width * 0.5;
    let y = center[1] - height * 0.5;
    let radius = roundness.min(width * 0.5).min(height * 0.5).max(0.0);
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
}

fn append_geometry(builder: &mut PathBuilder, geometry: &CompiledGeometry) {
    match geometry {
        CompiledGeometry::Path {
            vertices,
            in_tangents,
            out_tangents,
            closed,
        } => {
            let Some(first) = vertices.first() else {
                return;
            };
            builder.move_to(first[0], first[1]);
            let segment_count = if *closed {
                vertices.len()
            } else {
                vertices.len().saturating_sub(1)
            };
            for index in 0..segment_count {
                let next = (index + 1) % vertices.len();
                let from = vertices[index];
                let to = vertices[next];
                let out = out_tangents[index];
                let incoming = in_tangents[next];
                if out != [0.0, 0.0] || incoming != [0.0, 0.0] {
                    builder.cubic_to(
                        from[0] + out[0],
                        from[1] + out[1],
                        to[0] + incoming[0],
                        to[1] + incoming[1],
                        to[0],
                        to[1],
                    );
                } else {
                    builder.line_to(to[0], to[1]);
                }
            }
            if *closed {
                builder.close();
            }
        }
        CompiledGeometry::Ellipse { center, size } => append_ellipse(builder, *center, *size),
        CompiledGeometry::Rectangle {
            center,
            size,
            roundness,
        } => append_rectangle(builder, *center, *size, *roundness),
    }
}

fn resolved_color(mut color: [u8; 4], tint: Option<[u8; 4]>) -> [u8; 4] {
    if color[0..3] == crate::theme::DEFAULT_CURSOR_FILL[0..3] {
        if let Some(tint) = tint {
            color[0..3].copy_from_slice(&tint[0..3]);
        }
    }
    color
}

fn vector_paint(color: [u8; 4], opacity: f32, tint: Option<[u8; 4]>) -> Paint<'static> {
    let color = resolved_color(color, tint);
    let alpha = (f32::from(color[3]) / 255.0 * opacity.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    Paint {
        shader: tiny_skia::Shader::SolidColor(
            Color::from_rgba(
                f32::from(color[0]) / 255.0,
                f32::from(color[1]) / 255.0,
                f32::from(color[2]) / 255.0,
                alpha,
            )
            .unwrap_or(Color::TRANSPARENT),
        ),
        anti_alias: true,
        ..Default::default()
    }
}

fn draw_frame(
    target: &mut tiny_skia::Pixmap,
    frame: &CompiledFrame,
    outer_transform: Transform,
    alpha: f32,
    tint: Option<[u8; 4]>,
) {
    for command in &frame.commands {
        let mut builder = PathBuilder::new();
        for geometry in &command.geometries {
            append_geometry(&mut builder, geometry);
        }
        let Some(path) = builder.finish() else {
            continue;
        };
        let transform = compiled_transform(command.transform).post_concat(outer_transform);
        let opacity = command.opacity * alpha;
        if let Some(fill) = command.fill {
            target.fill_path(
                &path,
                &vector_paint(fill, opacity, tint),
                FillRule::Winding,
                transform,
                None,
            );
        }
        if let Some(stroke) = command.stroke {
            let stroke_style = Stroke {
                width: stroke.width,
                line_cap: match stroke.line_cap {
                    1 => LineCap::Butt,
                    3 => LineCap::Square,
                    _ => LineCap::Round,
                },
                line_join: match stroke.line_join {
                    1 => LineJoin::Miter,
                    3 => LineJoin::Bevel,
                    _ => LineJoin::Round,
                },
                ..Default::default()
            };
            target.stroke_path(
                &path,
                &vector_paint(stroke.color, opacity, tint),
                &stroke_style,
                transform,
                None,
            );
        }
    }
}

fn draw_layer(
    target: &mut tiny_skia::Pixmap,
    animation: &CompiledAnimation,
    elapsed_secs: f64,
    reduced_motion: bool,
    transform: Transform,
    alpha: f32,
    tint: Option<[u8; 4]>,
) {
    let Some(frame) = animation_frame(animation, elapsed_secs, reduced_motion) else {
        return;
    };
    draw_frame(target, frame, transform, alpha, tint);
}

#[allow(clippy::too_many_arguments)]
pub fn paint_compiled_theme(
    target: &mut tiny_skia::Pixmap,
    theme: &CompiledTheme,
    visual: &CursorVisualState,
    anchor_x: f32,
    anchor_y: f32,
    heading: f32,
    backing_scale: f32,
    alpha: f32,
) {
    paint_compiled_theme_with_tint(
        target,
        theme,
        visual,
        anchor_x,
        anchor_y,
        heading,
        backing_scale,
        alpha,
        None,
    );
}

/// Paint a compiled theme, optionally replacing the embedded default's blue
/// palette key with a stable per-session fill.
///
/// Custom themes pass `None` and retain their authored colors. Only the
/// checked-in `cua.default` path passes a tint.
#[allow(clippy::too_many_arguments)]
pub fn paint_compiled_theme_with_tint(
    target: &mut tiny_skia::Pixmap,
    theme: &CompiledTheme,
    visual: &CursorVisualState,
    anchor_x: f32,
    anchor_y: f32,
    heading: f32,
    backing_scale: f32,
    alpha: f32,
    tint: Option<[u8; 4]>,
) {
    let scale = crate::theme::DISPLAY_SIZE * backing_scale / crate::theme::CANVAS_SIZE;
    let (float_dx, float_dy, float_rotation) = if theme.id == crate::DEFAULT_THEME_ID {
        crate::theme::shared_float_motion(visual)
    } else {
        (0.0, 0.0, 0.0)
    };
    let transform = Transform::from_translate(-64.0, -64.0)
        .post_scale(scale, scale)
        .post_rotate((heading - std::f32::consts::FRAC_PI_4 + float_rotation).to_degrees())
        .post_translate(anchor_x + float_dx * scale, anchor_y + float_dy * scale);
    let reduced = visual.reduced_motion == crate::ReducedMotion::On;
    if let Some(animation) = theme.animation_for_action(visual.resolved_action) {
        draw_layer(
            target,
            animation,
            visual.elapsed_secs,
            reduced,
            transform,
            alpha,
            tint,
        );
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn inspect_artifact(path: &Path) -> Result<CompiledTheme> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    decode_theme(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> CompiledFrame {
        CompiledFrame {
            commands: vec![CompiledDrawCommand {
                geometries: vec![CompiledGeometry::Ellipse {
                    center: [64.0, 64.0],
                    size: [10.0, 10.0],
                }],
                transform: CompiledTransform::default(),
                opacity: 1.0,
                fill: Some([94, 192, 232, 255]),
                stroke: None,
            }],
        }
    }

    fn minimal_theme() -> CompiledTheme {
        let animation = CompiledAnimation {
            still_frame: 0,
            frames: vec![frame()],
        };
        CompiledTheme {
            id: "com.example.test".into(),
            name: "Test".into(),
            version: "1.0.0".into(),
            author: "Example Author".into(),
            license: "MIT".into(),
            profile: "cua-driver-development-v2".into(),
            source_hash: [7; 32],
            hotspot: [55, 30],
            actions: BTreeMap::from([
                ("idle".into(), animation.clone()),
                ("click".into(), animation),
            ]),
        }
    }

    #[test]
    fn artifact_round_trip_and_bounds() {
        let theme = minimal_theme();
        let bytes = encode_theme(&theme).unwrap();
        assert_eq!(decode_theme(&bytes).unwrap(), theme);
        assert!(decode_theme(&bytes[..20]).is_err());
    }

    #[test]
    fn rejects_v1_artifacts_with_rebuild_guidance() {
        let mut bytes = encode_theme(&minimal_theme()).unwrap();
        bytes[..LEGACY_MAGIC.len()].copy_from_slice(LEGACY_MAGIC);
        let message = decode_theme(&bytes).unwrap_err().to_string();
        assert!(message.contains("retired v1 contract"));
        assert!(message.contains("cua.cursor-theme/2"));
    }

    #[test]
    fn full_profile_requires_every_action() {
        let mut theme = minimal_theme();
        theme.profile = crate::THEME_PROFILE.into();
        assert!(validate_compiled_theme(&theme, true).is_err());
    }

    #[test]
    fn rejects_paths_disguised_as_ids() {
        let mut theme = minimal_theme();
        theme.id = "../bad".into();
        assert!(validate_compiled_theme(&theme, false).is_err());
    }

    #[test]
    fn embedded_default_is_a_full_compiled_theme() {
        let theme = embedded_default_theme();
        assert_eq!(theme.id, crate::DEFAULT_THEME_ID);
        assert_eq!(theme.actions.len(), CursorAction::ALL.len());
        assert!(theme
            .actions
            .values()
            .flat_map(|animation| &animation.frames)
            .any(|frame| !frame.commands.is_empty()));
        validate_compiled_theme(&theme, true).unwrap();
        assert!(load_installed_theme(crate::DEFAULT_THEME_ID)
            .unwrap()
            .is_some());
    }

    #[test]
    fn embedded_default_is_a_compact_vector_artifact() {
        assert!(
            DEFAULT_THEME_BYTES.len() < 100 * 1024,
            "the default vector artifact should remain compact, got {} bytes",
            DEFAULT_THEME_BYTES.len()
        );
        assert_eq!(
            &DEFAULT_THEME_BYTES[..MAGIC.len()],
            MAGIC,
            "the embedded theme must use the vector artifact version"
        );
    }

    #[test]
    fn embedded_default_was_compiled_from_the_checked_in_lottie() {
        let source = include_bytes!("../assets/cua.default.lottie");
        let expected: [u8; 32] = Sha256::digest(source).into();
        assert_eq!(embedded_default_theme().source_hash, expected);
    }

    #[test]
    fn default_tint_preserves_white_and_replaces_blue() {
        assert_eq!(
            resolved_color([94, 192, 232, 255], Some([12, 34, 56, 255])),
            [12, 34, 56, 255]
        );
        assert_eq!(
            resolved_color([255, 255, 255, 255], Some([12, 34, 56, 255])),
            [255, 255, 255, 255]
        );
    }
}
