//! Short-lived, unprivileged cursor-theme authoring tool.

use anyhow::{anyhow, bail, Context, Result};
use cursor_overlay::{
    encode_theme, inspect_artifact, install_artifact, list_installed_themes, uninstall_theme,
    validate_compiled_theme, CompiledAnimation, CompiledDrawCommand, CompiledFrame,
    CompiledGeometry, CompiledStroke, CompiledTheme, CompiledTransform, CursorAction,
    CursorVisualState, ReducedMotion, THEME_PROFILE,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
};
use zip::ZipArchive;

const MAX_ARCHIVE_BYTES: usize = 24 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 80;
const CANVAS: u32 = 128;
const FPS: f32 = 30.0;
const MAX_FRAMES: usize = 120;

type SourceArchiveEntries = BTreeMap<String, Vec<u8>>;
type SourceArchive = (Vec<u8>, SourceArchiveEntries);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeManifest {
    schema: String,
    id: String,
    name: String,
    version: String,
    author: String,
    license: String,
    compatibility: Compatibility,
    canvas: Canvas,
    hotspot: Hotspot,
    actions: BTreeMap<String, AnimationRef>,
    #[serde(default)]
    variants: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct Compatibility {
    profile: String,
    semantics: u32,
}

#[derive(Debug, Deserialize)]
struct Canvas {
    width: u32,
    height: u32,
    fps: f32,
}

#[derive(Debug, Deserialize)]
struct Hotspot {
    x: u16,
    y: u16,
}

#[derive(Debug, Deserialize)]
struct AnimationRef {
    animation: String,
    #[serde(default)]
    still_frame: u16,
}

#[derive(Debug, Deserialize)]
struct DotLottieManifest {
    animations: Vec<DotLottieAnimation>,
}

#[derive(Debug, Deserialize)]
struct DotLottieAnimation {
    id: String,
}

fn usage() -> &'static str {
    "Usage:
  cua-driver cursor-theme validate <source.lottie> [--development]
  cua-driver cursor-theme build <source.lottie> --output <theme.cua-theme> [--development]
  cua-driver cursor-theme inspect <theme.cua-theme> [--json]
  cua-driver cursor-theme preview <theme.cua-theme> --output <directory>
  cua-driver cursor-theme install <theme.cua-theme>
  cua-driver cursor-theme list [--json]
  cua-driver cursor-theme uninstall <theme-id>"
}

fn main() {
    if let Err(error) = run() {
        eprintln!("cua-driver cursor-theme: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(command) = args.first().map(String::as_str) else {
        bail!(usage());
    };
    match command {
        "validate" => {
            let source = required_path(&args, 1, "source .lottie")?;
            let full = !args.iter().any(|value| value == "--development");
            let theme = compile_source(&source, full)?;
            println!(
                "valid: {} {} ({} actions, profile {})",
                theme.id,
                theme.version,
                theme.actions.len(),
                theme.profile
            );
        }
        "build" => {
            let source = required_path(&args, 1, "source .lottie")?;
            let output = flag_path(&args, "--output")?;
            let full = !args.iter().any(|value| value == "--development");
            let theme = compile_source(&source, full)?;
            let bytes = encode_theme(&theme)?;
            fs::write(&output, bytes).with_context(|| format!("write {}", output.display()))?;
            println!(
                "built {} {} → {}",
                theme.id,
                theme.version,
                output.display()
            );
        }
        "inspect" => {
            let artifact = required_path(&args, 1, "compiled theme")?;
            let theme = inspect_artifact(&artifact)?;
            if args.iter().any(|value| value == "--json") {
                println!("{}", serde_json::to_string_pretty(&theme)?);
            } else {
                println!("id: {}", theme.id);
                println!("name: {}", theme.name);
                println!("version: {}", theme.version);
                println!("author: {}", theme.author);
                println!("license: {}", theme.license);
                println!("profile: {}", theme.profile);
                println!("content hash: {}", theme.content_hash());
                println!("actions: {}", theme.actions.len());
            }
        }
        "preview" => {
            let artifact = required_path(&args, 1, "compiled theme")?;
            let output = flag_path(&args, "--output")?;
            preview(&inspect_artifact(&artifact)?, &output)?;
            println!("preview written to {}", output.display());
        }
        "install" => {
            let artifact = required_path(&args, 1, "compiled theme")?;
            let bytes =
                fs::read(&artifact).with_context(|| format!("read {}", artifact.display()))?;
            let target = install_artifact(&bytes)?;
            println!("installed {}", target.display());
        }
        "list" => {
            let themes = list_installed_themes()?;
            if args.iter().any(|value| value == "--json") {
                println!("{}", serde_json::to_string_pretty(&themes)?);
            } else {
                for theme in themes {
                    println!("{theme}");
                }
            }
        }
        "uninstall" => {
            let id = args.get(1).ok_or_else(|| anyhow!("missing theme id"))?;
            if uninstall_theme(id)? {
                println!("uninstalled {id}");
            } else {
                println!("theme {id} was not installed");
            }
        }
        "--help" | "-h" | "help" => println!("{}", usage()),
        other => bail!("unknown cursor-theme command `{other}`\n\n{}", usage()),
    }
    Ok(())
}

fn required_path(args: &[String], index: usize, label: &str) -> Result<PathBuf> {
    args.get(index)
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("missing {label}"))
}

fn flag_path(args: &[String], flag: &str) -> Result<PathBuf> {
    let index = args
        .iter()
        .position(|value| value == flag)
        .ok_or_else(|| anyhow!("missing required {flag}"))?;
    args.get(index + 1)
        .filter(|value| !value.starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("missing value for {flag}"))
}

fn read_source_archive(path: &Path) -> Result<SourceArchive> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() > MAX_ARCHIVE_BYTES {
        bail!("source archive exceeds the {MAX_ARCHIVE_BYTES}-byte limit");
    }
    let mut archive = ZipArchive::new(Cursor::new(&bytes)).context("open dotLottie archive")?;
    if archive.len() > MAX_ENTRIES {
        bail!("source archive exceeds the {MAX_ENTRIES}-entry limit");
    }
    let mut entries = BTreeMap::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("read archive entry")?;
        if entry.is_dir() {
            continue;
        }
        let Some(path) = entry.enclosed_name() else {
            bail!("archive entry contains an absolute or parent path");
        };
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("archive entry contains a non-normal path segment");
        }
        if let Some(mode) = entry.unix_mode() {
            let file_type = mode & 0o170000;
            if file_type != 0 && file_type != 0o100000 {
                bail!("archive contains a symlink or non-regular entry");
            }
        }
        let name = path.to_string_lossy().replace('\\', "/");
        let allowed = name == "manifest.json"
            || name == "cua/theme.json"
            || (name.starts_with("a/") && name.ends_with(".json"));
        if !allowed {
            bail!("unsupported archive entry `{name}`");
        }
        if entries.contains_key(&name) {
            bail!("duplicate archive entry `{name}`");
        }
        let declared = usize::try_from(entry.size()).unwrap_or(usize::MAX);
        if declared > MAX_ENTRY_BYTES {
            bail!("archive entry `{name}` exceeds the per-entry limit");
        }
        let mut data = Vec::with_capacity(declared.min(64 * 1024));
        entry
            .by_ref()
            .take((MAX_ENTRY_BYTES + 1) as u64)
            .read_to_end(&mut data)
            .with_context(|| format!("read archive entry `{name}`"))?;
        if data.len() > MAX_ENTRY_BYTES {
            bail!("archive entry `{name}` exceeds the per-entry limit");
        }
        total = total.saturating_add(data.len());
        if total > MAX_TOTAL_BYTES {
            bail!("archive exceeds the decompressed-size limit");
        }
        entries.insert(name, data);
    }
    Ok((bytes, entries))
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8], name: &str) -> Result<T> {
    let text = std::str::from_utf8(bytes).with_context(|| format!("{name} is not UTF-8"))?;
    serde_json::from_str(text).with_context(|| format!("parse {name}"))
}

fn compile_source(path: &Path, full: bool) -> Result<CompiledTheme> {
    let (source, entries) = read_source_archive(path)?;
    let manifest_bytes = entries
        .get("cua/theme.json")
        .ok_or_else(|| anyhow!("archive is missing cua/theme.json"))?;
    let manifest_value: Value = parse_json(manifest_bytes, "cua/theme.json")?;
    let source_schema = manifest_value
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if source_schema == "cua.cursor-theme/1" {
        bail!(
            "cursor-theme source targets the retired v1 contract; remove `modifiers` and rebuild as cua.cursor-theme/2"
        );
    }
    let manifest: ThemeManifest =
        serde_json::from_value(manifest_value).context("parse cua/theme.json")?;
    if manifest.schema != "cua.cursor-theme/2" {
        bail!("unsupported source schema `{}`", manifest.schema);
    }
    if manifest.compatibility.semantics != 2 {
        bail!(
            "unsupported cursor semantic version {}",
            manifest.compatibility.semantics
        );
    }
    if manifest.canvas.width != CANVAS
        || manifest.canvas.height != CANVAS
        || (manifest.canvas.fps - FPS).abs() > f32::EPSILON
    {
        bail!("Cua cursor profile v2 requires a 128×128 canvas at 30 fps");
    }
    if full && manifest.compatibility.profile != THEME_PROFILE {
        bail!("full validation requires profile `{THEME_PROFILE}`");
    }
    if !manifest.variants.is_empty() {
        bail!(
            "cursor-theme variants are not supported by profile v2; publish each visual variant as a separate theme id"
        );
    }

    // Parse the standard manifest too: it must exist and be valid JSON. The
    // Cua manifest remains authoritative for semantic mapping.
    let standard = entries
        .get("manifest.json")
        .ok_or_else(|| anyhow!("archive is missing manifest.json"))?;
    let standard: DotLottieManifest = parse_json(standard, "manifest.json")?;

    let referenced: BTreeSet<&str> = manifest
        .actions
        .values()
        .map(|item| item.animation.as_str())
        .collect();
    let standard_ids: BTreeSet<&str> = standard
        .animations
        .iter()
        .map(|animation| animation.id.as_str())
        .collect();
    if standard_ids.len() != standard.animations.len() {
        bail!("manifest.json contains duplicate animation ids");
    }
    for id in &referenced {
        if !standard_ids.contains(id) {
            bail!("Cua semantic manifest references `{id}`, which is absent from manifest.json");
        }
    }
    let mut compiled = BTreeMap::new();
    for id in referenced {
        let name = format!("a/{id}.json");
        let source = entries
            .get(&name)
            .ok_or_else(|| anyhow!("semantic manifest references missing animation `{id}`"))?;
        compiled.insert(id.to_owned(), compile_animation(source, id)?);
    }

    let mut actions = BTreeMap::new();
    for (name, reference) in manifest.actions {
        if !CursorAction::ALL
            .iter()
            .any(|action| action.as_str() == name)
        {
            bail!("unknown action `{name}`");
        }
        actions.insert(
            name,
            with_still_frame(
                compiled
                    .get(&reference.animation)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing compiled action"))?,
                reference.still_frame,
            )?,
        );
    }
    let mut hasher = Sha256::new();
    hasher.update(&source);
    let source_hash: [u8; 32] = hasher.finalize().into();
    let theme = CompiledTheme {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        author: manifest.author,
        license: manifest.license,
        profile: manifest.compatibility.profile,
        source_hash,
        hotspot: [manifest.hotspot.x, manifest.hotspot.y],
        actions,
    };
    validate_compiled_theme(&theme, full)?;
    Ok(theme)
}

fn with_still_frame(
    mut animation: CompiledAnimation,
    still_frame: u16,
) -> Result<CompiledAnimation> {
    if usize::from(still_frame) >= animation.frames.len() {
        bail!("still_frame {still_frame} is outside the animation");
    }
    animation.still_frame = still_frame;
    Ok(animation)
}

fn number(value: &Value, label: &str) -> Result<f32> {
    value
        .as_f64()
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
        .ok_or_else(|| anyhow!("{label} must be a finite number"))
}

fn numeric_array(value: &Value, label: &str) -> Result<Vec<f32>> {
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .map(|value| number(value, label))
            .collect::<Result<Vec<_>>>();
    }
    Ok(vec![number(value, label)?])
}

fn property_key<'a>(property: &'a Value, label: &str) -> Result<&'a Value> {
    property
        .get("k")
        .ok_or_else(|| anyhow!("{label} is missing its value"))
}

fn handle_component(keyframe: &Value, side: &str, axis: &str, fallback: f32) -> f32 {
    keyframe
        .get(side)
        .and_then(|value| value.get(axis))
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_array()?.first()?.as_f64())
        })
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

fn cubic_coordinate(t: f32, first: f32, second: f32) -> f32 {
    let one_minus = 1.0 - t;
    3.0 * one_minus * one_minus * t * first + 3.0 * one_minus * t * t * second + t * t * t
}

fn eased_progress(progress: f32, current: &Value, next: &Value) -> f32 {
    if current.get("h").and_then(Value::as_i64) == Some(1) {
        return 0.0;
    }
    let x1 = handle_component(current, "o", "x", 0.0).clamp(0.0, 1.0);
    let y1 = handle_component(current, "o", "y", 0.0);
    let x2 = handle_component(next, "i", "x", 1.0).clamp(0.0, 1.0);
    let y2 = handle_component(next, "i", "y", 1.0);
    let target = progress.clamp(0.0, 1.0);
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..14 {
        let middle = (low + high) * 0.5;
        if cubic_coordinate(middle, x1, x2) < target {
            low = middle;
        } else {
            high = middle;
        }
    }
    cubic_coordinate((low + high) * 0.5, y1, y2).clamp(0.0, 1.0)
}

fn property_value(property: &Value, frame: f32, label: &str) -> Result<Vec<f32>> {
    let animated = property.get("a").and_then(Value::as_i64).unwrap_or(0) == 1;
    let key = property_key(property, label)?;
    if !animated {
        return numeric_array(key, label);
    }
    let keyframes = key
        .as_array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| anyhow!("{label} has no keyframes"))?;
    let first = &keyframes[0];
    let first_time = first
        .get("t")
        .map(|value| number(value, label))
        .transpose()?
        .unwrap_or(0.0);
    if frame <= first_time {
        return numeric_array(
            first
                .get("s")
                .ok_or_else(|| anyhow!("{label} keyframe has no start value"))?,
            label,
        );
    }
    for pair in keyframes.windows(2) {
        let current = &pair[0];
        let next = &pair[1];
        let start_time = current
            .get("t")
            .map(|value| number(value, label))
            .transpose()?
            .unwrap_or(0.0);
        let end_time = next
            .get("t")
            .map(|value| number(value, label))
            .transpose()?
            .unwrap_or(start_time);
        if frame < end_time {
            let start = numeric_array(
                current
                    .get("s")
                    .ok_or_else(|| anyhow!("{label} keyframe has no start value"))?,
                label,
            )?;
            let end = if let Some(value) = current.get("e") {
                numeric_array(value, label)?
            } else {
                numeric_array(
                    next.get("s")
                        .ok_or_else(|| anyhow!("{label} keyframe has no end value"))?,
                    label,
                )?
            };
            if start.len() != end.len() {
                bail!("{label} changes dimensionality between keyframes");
            }
            let linear = if end_time > start_time {
                (frame - start_time) / (end_time - start_time)
            } else {
                0.0
            };
            let progress = eased_progress(linear, current, next);
            return Ok(start
                .iter()
                .zip(end.iter())
                .map(|(start, end)| start + (end - start) * progress)
                .collect());
        }
    }
    numeric_array(
        keyframes
            .last()
            .and_then(|value| value.get("s"))
            .ok_or_else(|| anyhow!("{label} final keyframe has no value"))?,
        label,
    )
}

fn property_scalar(property: &Value, frame: f32, label: &str) -> Result<f32> {
    property_value(property, frame, label)?
        .first()
        .copied()
        .ok_or_else(|| anyhow!("{label} is empty"))
}

fn property_pair(property: &Value, frame: f32, label: &str) -> Result<[f32; 2]> {
    let values = property_value(property, frame, label)?;
    if values.len() < 2 {
        bail!("{label} must contain two numbers");
    }
    Ok([values[0], values[1]])
}

fn transform_property(
    transform: &Value,
    key: &str,
    frame: f32,
    default: [f32; 2],
    label: &str,
) -> Result<[f32; 2]> {
    transform
        .get(key)
        .map(|property| property_pair(property, frame, label))
        .unwrap_or(Ok(default))
}

fn compile_transform(
    transform: &Value,
    frame: f32,
    label: &str,
) -> Result<(CompiledTransform, f32)> {
    let anchor = transform_property(transform, "a", frame, [0.0, 0.0], label)?;
    let position = transform_property(transform, "p", frame, [0.0, 0.0], label)?;
    let scale = transform_property(transform, "s", frame, [100.0, 100.0], label)?;
    let rotation = transform
        .get("r")
        .map(|property| property_scalar(property, frame, label))
        .transpose()?
        .unwrap_or(0.0);
    let opacity = transform
        .get("o")
        .map(|property| property_scalar(property, frame, label))
        .transpose()?
        .unwrap_or(100.0);
    Ok((
        CompiledTransform {
            anchor,
            position,
            scale: [scale[0] / 100.0, scale[1] / 100.0],
            rotation_degrees: rotation,
        },
        (opacity / 100.0).clamp(0.0, 1.0),
    ))
}

fn color(property: &Value, frame: f32, label: &str) -> Result<[u8; 4]> {
    let values = property_value(property, frame, label)?;
    if values.len() < 3 {
        bail!("{label} must contain at least three channels");
    }
    let channel = |index: usize, fallback: f32| {
        (values
            .get(index)
            .copied()
            .unwrap_or(fallback)
            .clamp(0.0, 1.0)
            * 255.0)
            .round() as u8
    };
    Ok([
        channel(0, 0.0),
        channel(1, 0.0),
        channel(2, 0.0),
        channel(3, 1.0),
    ])
}

fn static_pair(property: &Value, label: &str) -> Result<[f32; 2]> {
    if property.get("a").and_then(Value::as_i64).unwrap_or(0) != 0 {
        bail!("{label} geometry animation is not supported by the bounded vector profile");
    }
    property_pair(property, 0.0, label)
}

fn compile_geometry(shape: &Value, label: &str) -> Result<CompiledGeometry> {
    match shape.get("ty").and_then(Value::as_str) {
        Some("sh") => {
            let property = shape
                .get("ks")
                .ok_or_else(|| anyhow!("{label} path is missing geometry"))?;
            if property.get("a").and_then(Value::as_i64).unwrap_or(0) != 0 {
                bail!("{label} path animation is not supported by the bounded vector profile");
            }
            let path = property_key(property, label)?;
            let points = |key: &str| -> Result<Vec<[f32; 2]>> {
                path.get(key)
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("{label} path is missing `{key}`"))?
                    .iter()
                    .map(|value| {
                        let values = numeric_array(value, label)?;
                        if values.len() < 2 {
                            bail!("{label} path coordinate must contain two numbers");
                        }
                        Ok([values[0], values[1]])
                    })
                    .collect()
            };
            Ok(CompiledGeometry::Path {
                vertices: points("v")?,
                in_tangents: points("i")?,
                out_tangents: points("o")?,
                closed: path.get("c").and_then(Value::as_bool).unwrap_or(false),
            })
        }
        Some("el") => Ok(CompiledGeometry::Ellipse {
            center: static_pair(
                shape
                    .get("p")
                    .ok_or_else(|| anyhow!("{label} ellipse has no position"))?,
                label,
            )?,
            size: static_pair(
                shape
                    .get("s")
                    .ok_or_else(|| anyhow!("{label} ellipse has no size"))?,
                label,
            )?,
        }),
        Some("rc") => Ok(CompiledGeometry::Rectangle {
            center: static_pair(
                shape
                    .get("p")
                    .ok_or_else(|| anyhow!("{label} rectangle has no position"))?,
                label,
            )?,
            size: static_pair(
                shape
                    .get("s")
                    .ok_or_else(|| anyhow!("{label} rectangle has no size"))?,
                label,
            )?,
            roundness: shape
                .get("r")
                .map(|property| property_scalar(property, 0.0, label))
                .transpose()?
                .unwrap_or(0.0),
        }),
        Some(other) => bail!("{label} uses unsupported Lottie shape `{other}`"),
        None => bail!("{label} shape has no type"),
    }
}

enum CompiledStyle {
    Fill {
        color: [u8; 4],
        opacity: f32,
    },
    Stroke {
        stroke: CompiledStroke,
        opacity: f32,
    },
}

fn compile_layer(layer: &Value, frame: f32, id: &str) -> Result<Vec<CompiledDrawCommand>> {
    let label = format!("animation `{id}`");
    if layer.get("ty").and_then(Value::as_i64) != Some(4)
        || layer.get("ddd").and_then(Value::as_i64).unwrap_or(0) != 0
        || layer.get("parent").is_some()
        || layer.get("tt").is_some()
        || layer.get("masksProperties").is_some()
        || layer.get("ef").is_some()
    {
        bail!("{label} uses a layer outside the bounded vector profile");
    }
    let in_point = layer
        .get("ip")
        .map(|value| number(value, &label))
        .transpose()?
        .unwrap_or(0.0);
    let out_point = layer
        .get("op")
        .map(|value| number(value, &label))
        .transpose()?
        .unwrap_or(0.0);
    if frame < in_point || frame >= out_point {
        return Ok(Vec::new());
    }
    let (layer_transform, layer_opacity) = compile_transform(
        layer
            .get("ks")
            .ok_or_else(|| anyhow!("{label} layer has no transform"))?,
        frame,
        &label,
    )?;
    let shapes = layer
        .get("shapes")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("{label} shape layer has no shapes"))?;
    let mut geometries = Vec::new();
    let mut styles = Vec::new();
    for shape in shapes {
        match shape.get("ty").and_then(Value::as_str) {
            Some("sh" | "el" | "rc") => geometries.push(compile_geometry(shape, &label)?),
            Some("fl") => {
                let fill_color = color(
                    shape
                        .get("c")
                        .ok_or_else(|| anyhow!("{label} fill has no color"))?,
                    frame,
                    &label,
                )?;
                let opacity = shape
                    .get("o")
                    .map(|property| property_scalar(property, frame, &label))
                    .transpose()?
                    .unwrap_or(100.0)
                    / 100.0;
                styles.push(CompiledStyle::Fill {
                    color: fill_color,
                    opacity,
                });
            }
            Some("st") => {
                let stroke_color = color(
                    shape
                        .get("c")
                        .ok_or_else(|| anyhow!("{label} stroke has no color"))?,
                    frame,
                    &label,
                )?;
                let opacity = shape
                    .get("o")
                    .map(|property| property_scalar(property, frame, &label))
                    .transpose()?
                    .unwrap_or(100.0)
                    / 100.0;
                let width = property_scalar(
                    shape
                        .get("w")
                        .ok_or_else(|| anyhow!("{label} stroke has no width"))?,
                    frame,
                    &label,
                )?;
                styles.push(CompiledStyle::Stroke {
                    stroke: CompiledStroke {
                        color: stroke_color,
                        width,
                        line_cap: shape.get("lc").and_then(Value::as_u64).unwrap_or(2) as u8,
                        line_join: shape.get("lj").and_then(Value::as_u64).unwrap_or(2) as u8,
                    },
                    opacity,
                });
            }
            Some("tr") => {
                let (shape_transform, shape_opacity) = compile_transform(shape, frame, &label)?;
                if shape_transform != CompiledTransform::default()
                    || (shape_opacity - 1.0).abs() > f32::EPSILON
                {
                    bail!("{label} uses a non-identity shape transform; move it to the layer transform");
                }
            }
            Some(other) => bail!("{label} uses unsupported Lottie shape `{other}`"),
            None => bail!("{label} shape has no type"),
        }
    }
    if geometries.is_empty() || styles.is_empty() {
        bail!("{label} shape layer must contain geometry and paint");
    }
    Ok(styles
        .into_iter()
        .map(|style| match style {
            CompiledStyle::Fill { color, opacity } => CompiledDrawCommand {
                geometries: geometries.clone(),
                transform: layer_transform,
                opacity: (layer_opacity * opacity).clamp(0.0, 1.0),
                fill: Some(color),
                stroke: None,
            },
            CompiledStyle::Stroke { stroke, opacity } => CompiledDrawCommand {
                geometries: geometries.clone(),
                transform: layer_transform,
                opacity: (layer_opacity * opacity).clamp(0.0, 1.0),
                fill: None,
                stroke: Some(stroke),
            },
        })
        .collect())
}

fn compile_animation(bytes: &[u8], id: &str) -> Result<CompiledAnimation> {
    let text =
        std::str::from_utf8(bytes).with_context(|| format!("animation `{id}` is not UTF-8"))?;
    let animation: Value =
        serde_json::from_str(text).with_context(|| format!("parse animation `{id}`"))?;
    let width = animation.get("w").and_then(Value::as_u64).unwrap_or(0) as u32;
    let height = animation.get("h").and_then(Value::as_u64).unwrap_or(0) as u32;
    let frame_rate = animation
        .get("fr")
        .map(|value| number(value, id))
        .transpose()?
        .unwrap_or(0.0);
    if width != CANVAS || height != CANVAS || (frame_rate - FPS).abs() > f32::EPSILON {
        bail!("animation `{id}` must be 128×128 at 30 fps");
    }
    if animation
        .get("assets")
        .and_then(Value::as_array)
        .is_some_and(|assets| !assets.is_empty())
    {
        bail!("animation `{id}` cannot contain external or nested assets");
    }
    let in_point = animation
        .get("ip")
        .map(|value| number(value, id))
        .transpose()?
        .unwrap_or(0.0);
    let out_point = animation
        .get("op")
        .map(|value| number(value, id))
        .transpose()?
        .unwrap_or(0.0);
    let count = (out_point - in_point).ceil() as usize;
    if count == 0 || count > MAX_FRAMES {
        bail!("animation `{id}` must contain 1..={MAX_FRAMES} frames");
    }
    let layers = animation
        .get("layers")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("animation `{id}` has no layers"))?;
    let mut frames = Vec::with_capacity(count);
    for index in 0..count {
        let frame_number = in_point + index as f32;
        let mut commands = Vec::new();
        for layer in layers.iter().rev() {
            commands.extend(compile_layer(layer, frame_number, id)?);
        }
        frames.push(CompiledFrame { commands });
    }
    Ok(CompiledAnimation {
        still_frame: 0,
        frames,
    })
}

fn preview(theme: &CompiledTheme, output: &Path) -> Result<()> {
    fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))?;
    for name in theme.actions.keys() {
        let mut visual = CursorVisualState {
            reduced_motion: ReducedMotion::On,
            ..CursorVisualState::default()
        };
        if let Some(action) = CursorAction::ALL
            .into_iter()
            .find(|action| action.as_str() == name)
        {
            visual.requested_action = action;
            visual.resolved_action = action;
        } else {
            bail!("unknown compiled action `{name}`");
        }
        let mut pixmap = tiny_skia::Pixmap::new(CANVAS, CANVAS)
            .ok_or_else(|| anyhow!("create preview pixmap"))?;
        cursor_overlay::paint_compiled_theme(
            &mut pixmap,
            theme,
            &visual,
            CANVAS as f32 * 0.5,
            CANVAS as f32 * 0.5,
            std::f32::consts::FRAC_PI_4,
            CANVAS as f32 / 48.0,
            1.0,
        );
        // tiny-skia stores premultiplied pixels; PNG expects straight RGBA.
        let pixels = unpremultiply_rgba(pixmap.data().to_vec());
        let path = output.join(format!("{name}.png"));
        image::save_buffer_with_format(
            &path,
            &pixels,
            CANVAS,
            CANVAS,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .with_context(|| format!("write {}", path.display()))?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn preview_unpremultiplication_is_bounded() {
        let straight = unpremultiply_rgba(vec![100, 50, 25, 128, 0, 0, 0, 0]);
        assert!(straight[0].abs_diff(199) <= 1);
        assert!(straight[1].abs_diff(100) <= 1);
        assert!(straight[2].abs_diff(50) <= 1);
        assert_eq!(&straight[4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn usage_lists_every_management_command() {
        for command in [
            "validate",
            "build",
            "inspect",
            "preview",
            "install",
            "list",
            "uninstall",
        ] {
            assert!(usage().contains(command));
        }
    }

    fn source_archive(standard_id: &str, variants: &str) -> tempfile::NamedTempFile {
        source_archive_with_schema(standard_id, variants, "cua.cursor-theme/2")
    }

    fn source_archive_with_schema(
        standard_id: &str,
        variants: &str,
        schema: &str,
    ) -> tempfile::NamedTempFile {
        let file = tempfile::Builder::new()
            .suffix(".lottie")
            .tempfile()
            .unwrap();
        let mut archive = ZipWriter::new(file.reopen().unwrap());
        let options = SimpleFileOptions::default();
        let standard = format!(r#"{{"version":"2","animations":[{{"id":"{standard_id}"}}]}}"#);
        let legacy = schema == "cua.cursor-theme/1";
        let profile = if legacy {
            "cua-driver-full-v1"
        } else {
            "cua-driver-development-v2"
        };
        let semantics = if legacy { 1 } else { 2 };
        let modifiers = if legacy { r#","modifiers":{}"# } else { "" };
        let semantic = format!(
            r#"{{
                "schema":"{schema}",
                "id":"com.example.test",
                "name":"Test",
                "version":"1.0.0",
                "author":"Example Author",
                "license":"MIT",
                "compatibility":{{"profile":"{profile}","semantics":{semantics}}},
                "canvas":{{"width":128,"height":128,"fps":30}},
                "hotspot":{{"x":55,"y":30}},
                "actions":{{
                    "idle":{{"animation":"base","still_frame":0}},
                    "click":{{"animation":"base","still_frame":0}}
                }}{modifiers},
                "variants":{variants}
            }}"#
        );
        let animation = r#"{"v":"5.12.2","fr":30,"ip":0,"op":1,"w":128,"h":128,"nm":"base","ddd":0,"assets":[],"layers":[]}"#;
        for (name, contents) in [
            ("manifest.json", standard.as_str()),
            ("cua/theme.json", semantic.as_str()),
            ("a/base.json", animation),
        ] {
            archive.start_file(name, options).unwrap();
            archive.write_all(contents.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
        file
    }

    fn archive_with_entries(entries: &[(&str, &[u8])]) -> tempfile::NamedTempFile {
        let file = tempfile::Builder::new()
            .suffix(".lottie")
            .tempfile()
            .unwrap();
        let mut archive = ZipWriter::new(file.reopen().unwrap());
        let options = SimpleFileOptions::default();
        for (name, contents) in entries {
            archive.start_file(*name, options).unwrap();
            archive.write_all(contents).unwrap();
        }
        archive.finish().unwrap();
        file
    }

    fn vector_animation(extra_layer_fields: &str, ellipse_animated: u8) -> String {
        format!(
            r#"{{
                "v":"5.12.2",
                "fr":30,
                "ip":0,
                "op":1,
                "w":128,
                "h":128,
                "nm":"base",
                "ddd":0,
                "assets":[],
                "layers":[{{
                    "ty":4,
                    "ddd":0,
                    "ip":0,
                    "op":1,
                    "ks":{{
                        "a":{{"a":0,"k":[0,0]}},
                        "p":{{"a":0,"k":[0,0]}},
                        "s":{{"a":0,"k":[100,100]}},
                        "r":{{"a":0,"k":0}},
                        "o":{{"a":0,"k":100}}
                    }},
                    "shapes":[
                        {{
                            "ty":"el",
                            "p":{{"a":0,"k":[64,64]}},
                            "s":{{"a":{ellipse_animated},"k":[20,20]}}
                        }},
                        {{
                            "ty":"fl",
                            "c":{{"a":0,"k":[0.3686,0.7529,0.9098,1]}},
                            "o":{{"a":0,"k":100}}
                        }}
                    ]
                    {extra_layer_fields}
                }}]
            }}"#
        )
    }

    #[test]
    fn compiles_a_bounded_development_theme() {
        let source = source_archive("base", "{}");
        let theme = compile_source(source.path(), false).unwrap();
        assert_eq!(theme.id, "com.example.test");
        assert_eq!(theme.author, "Example Author");
        assert_eq!(theme.license, "MIT");
        assert_eq!(theme.actions.len(), 2);
        assert!(encode_theme(&theme).is_ok());
    }

    #[test]
    fn compiles_supported_geometry_into_vector_commands() {
        let animation = vector_animation("", 0);
        let compiled = compile_animation(animation.as_bytes(), "base").unwrap();
        assert_eq!(compiled.frames.len(), 1);
        assert_eq!(compiled.frames[0].commands.len(), 1);
        assert!(matches!(
            compiled.frames[0].commands[0].geometries[0],
            CompiledGeometry::Ellipse { .. }
        ));
    }

    #[test]
    fn rejects_animated_geometry_and_masked_layers() {
        let animated = vector_animation("", 1);
        assert!(compile_animation(animated.as_bytes(), "base")
            .unwrap_err()
            .to_string()
            .contains("geometry animation is not supported"));

        let masked = vector_animation(r#","masksProperties":[] "#, 0);
        assert!(compile_animation(masked.as_bytes(), "base")
            .unwrap_err()
            .to_string()
            .contains("outside the bounded vector profile"));
    }

    #[test]
    fn rejects_semantic_references_missing_from_standard_manifest() {
        let source = source_archive("different", "{}");
        assert!(compile_source(source.path(), false)
            .unwrap_err()
            .to_string()
            .contains("absent from manifest.json"));
    }

    #[test]
    fn rejects_noop_variants_in_profile_v2() {
        let source = source_archive("base", r#"{"dark":"dark"}"#);
        assert!(compile_source(source.path(), false)
            .unwrap_err()
            .to_string()
            .contains("variants are not supported"));
    }

    #[test]
    fn rejects_v1_sources_with_rebuild_guidance() {
        let source = source_archive_with_schema("base", "{}", "cua.cursor-theme/1");
        let message = compile_source(source.path(), false)
            .unwrap_err()
            .to_string();
        assert!(message.contains("retired v1 contract"));
        assert!(message.contains("remove `modifiers`"));
        assert!(message.contains("cua.cursor-theme/2"));
    }

    #[test]
    fn rejects_parent_paths() {
        let traversal = archive_with_entries(&[("../theme.json", b"{}")]);
        assert!(read_source_archive(traversal.path())
            .unwrap_err()
            .to_string()
            .contains("absolute or parent path"));
    }

    #[test]
    fn rejects_unsupported_and_oversized_entries() {
        let unsupported = archive_with_entries(&[("images/pointer.png", b"not-an-image")]);
        assert!(read_source_archive(unsupported.path())
            .unwrap_err()
            .to_string()
            .contains("unsupported archive entry"));

        let oversized = vec![0_u8; MAX_ENTRY_BYTES + 1];
        let oversized = archive_with_entries(&[("a/huge.json", oversized.as_slice())]);
        assert!(read_source_archive(oversized.path())
            .unwrap_err()
            .to_string()
            .contains("per-entry limit"));
    }
}
