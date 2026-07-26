//! Bounded compiled cursor-theme artifacts and the local installed-theme store.
//!
//! `.cua-theme` files contain zstd-compressed postcard data behind a fixed
//! header. The privileged overlay never parses Lottie, ZIP, JSON, fonts,
//! expressions, URLs, or arbitrary source paths.

use crate::{CursorAction, CursorVisualState, DeliveryModifier, TargetModifier};
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
use tiny_skia::{PixmapPaint, PixmapRef, Transform};

const MAGIC: &[u8; 8] = b"CUATHEM1";
const HEADER_LEN: usize = 8 + 2 + 4 + 32;
const ARTIFACT_VERSION: u16 = 1;
const MAX_COMPRESSED_BYTES: usize = 24 * 1024 * 1024;
const MAX_DECOMPRESSED_BYTES: usize = 96 * 1024 * 1024;
const MAX_TOTAL_FRAMES: usize = 1_000;
const MAX_FRAMES_PER_ANIMATION: usize = 120;
const MAX_TEXT_BYTES: usize = 200;
const CANVAS: u32 = 128;
const FPS: u16 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompiledFrame {
    /// Premultiplied RGBA8 pixels, exactly 128×128.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompiledAnimation {
    pub still_frame: u16,
    pub frames: Vec<CompiledFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    pub modifiers: BTreeMap<String, CompiledAnimation>,
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

    fn modifier(&self, name: &str) -> Option<&CompiledAnimation> {
        self.modifiers.get(name)
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

fn validate_animation(name: &str, animation: &CompiledAnimation) -> Result<usize> {
    if animation.frames.is_empty() || animation.frames.len() > MAX_FRAMES_PER_ANIMATION {
        bail!("animation `{name}` must contain 1..={MAX_FRAMES_PER_ANIMATION} frames");
    }
    if usize::from(animation.still_frame) >= animation.frames.len() {
        bail!("animation `{name}` has an out-of-range still_frame");
    }
    let expected = (CANVAS * CANVAS * 4) as usize;
    for (index, frame) in animation.frames.iter().enumerate() {
        if frame.pixels.len() != expected {
            bail!(
                "animation `{name}` frame {index} is {} bytes; expected {expected}",
                frame.pixels.len()
            );
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
        for modifier in [
            "background",
            "foreground",
            "ax",
            "pixel",
            "browser",
            "desktop",
        ] {
            if !theme.modifiers.contains_key(modifier) {
                bail!("full profile is missing modifier `{modifier}`");
            }
        }
    } else if !theme.actions.contains_key("idle") || !theme.actions.contains_key("click") {
        bail!("development profile requires at least `idle` and `click`");
    }

    let mut total_frames = 0usize;
    for (name, animation) in theme.actions.iter().chain(theme.modifiers.iter()) {
        total_frames += validate_animation(name, animation)?;
        if total_frames > MAX_TOTAL_FRAMES {
            bail!("theme exceeds the {MAX_TOTAL_FRAMES}-frame limit");
        }
    }
    Ok(())
}

#[cfg(feature = "theme-authoring")]
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
        return Ok(home
            .join("Library")
            .join("Application Support")
            .join("Cua Driver")
            .join("cursor-themes"));
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
        return Ok(None);
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

fn animation_frame<'a>(
    animation: &'a CompiledAnimation,
    elapsed_secs: f64,
    reduced_motion: bool,
) -> Option<&'a CompiledFrame> {
    let index = if reduced_motion {
        usize::from(animation.still_frame)
    } else {
        ((elapsed_secs.max(0.0) * f64::from(FPS)).floor() as usize) % animation.frames.len()
    };
    animation.frames.get(index)
}

fn draw_layer(
    target: &mut tiny_skia::Pixmap,
    animation: &CompiledAnimation,
    elapsed_secs: f64,
    reduced_motion: bool,
    transform: Transform,
    alpha: f32,
) {
    let Some(frame) = animation_frame(animation, elapsed_secs, reduced_motion) else {
        return;
    };
    let Some(source) = PixmapRef::from_bytes(&frame.pixels, CANVAS, CANVAS) else {
        return;
    };
    target.draw_pixmap(
        0,
        0,
        source,
        &PixmapPaint {
            opacity: alpha.clamp(0.0, 1.0),
            ..Default::default()
        },
        transform,
        None,
    );
}

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
    let scale = crate::theme::DISPLAY_SIZE * backing_scale / crate::theme::CANVAS_SIZE;
    let transform = Transform::from_translate(-64.0, -64.0)
        .post_scale(scale, scale)
        .post_rotate((heading - std::f32::consts::FRAC_PI_4).to_degrees())
        .post_translate(anchor_x, anchor_y);
    let reduced = visual.reduced_motion == crate::ReducedMotion::On;
    if let Some(animation) = theme.animation_for_action(visual.resolved_action) {
        draw_layer(
            target,
            animation,
            visual.elapsed_secs,
            reduced,
            transform,
            alpha,
        );
    }
    if let Some(delivery) = visual.delivery {
        let name = match delivery {
            DeliveryModifier::Background => "background",
            DeliveryModifier::Foreground => "foreground",
        };
        if let Some(animation) = theme.modifier(name) {
            draw_layer(
                target,
                animation,
                visual.elapsed_secs,
                reduced,
                transform,
                alpha,
            );
        }
    }
    if let Some(target_modifier) = visual.target {
        let name = match target_modifier {
            TargetModifier::Ax => "ax",
            TargetModifier::Pixel => "pixel",
            TargetModifier::Browser => "browser",
            TargetModifier::Desktop => "desktop",
        };
        if let Some(animation) = theme.modifier(name) {
            draw_layer(
                target,
                animation,
                visual.elapsed_secs,
                reduced,
                transform,
                alpha,
            );
        }
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
            pixels: vec![0; (CANVAS * CANVAS * 4) as usize],
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
            profile: "cua-driver-development-v1".into(),
            source_hash: [7; 32],
            hotspot: [55, 30],
            actions: BTreeMap::from([
                ("idle".into(), animation.clone()),
                ("click".into(), animation),
            ]),
            modifiers: BTreeMap::new(),
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
    fn full_profile_requires_every_action_and_modifier() {
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
}
