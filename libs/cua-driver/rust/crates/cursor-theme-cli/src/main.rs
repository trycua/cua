//! Short-lived, unprivileged cursor-theme authoring tool.

use anyhow::{anyhow, bail, Context, Result};
use cursor_overlay::{
    encode_theme, inspect_artifact, install_artifact, list_installed_themes, uninstall_theme,
    validate_compiled_theme, CompiledAnimation, CompiledFrame, CompiledTheme, CursorAction,
    THEME_PROFILE,
};
use rasterlottie::{analyze_animation, Animation, RenderConfig, Renderer};
use serde::Deserialize;
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

#[derive(Debug, Deserialize)]
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
    modifiers: BTreeMap<String, AnimationRef>,
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
                "valid: {} {} ({} actions, {} modifiers, profile {})",
                theme.id,
                theme.version,
                theme.actions.len(),
                theme.modifiers.len(),
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
                println!("modifiers: {}", theme.modifiers.len());
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

fn read_source_archive(path: &Path) -> Result<(Vec<u8>, BTreeMap<String, Vec<u8>>)> {
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
    let manifest: ThemeManifest = parse_json(manifest_bytes, "cua/theme.json")?;
    if manifest.schema != "cua.cursor-theme/1" {
        bail!("unsupported source schema `{}`", manifest.schema);
    }
    if manifest.compatibility.semantics != 1 {
        bail!(
            "unsupported cursor semantic version {}",
            manifest.compatibility.semantics
        );
    }
    if manifest.canvas.width != CANVAS
        || manifest.canvas.height != CANVAS
        || (manifest.canvas.fps - FPS).abs() > f32::EPSILON
    {
        bail!("Cua cursor profile v1 requires a 128×128 canvas at 30 fps");
    }
    if full && manifest.compatibility.profile != THEME_PROFILE {
        bail!("full validation requires profile `{THEME_PROFILE}`");
    }
    if !manifest.variants.is_empty() {
        bail!(
            "cursor-theme variants are not supported by profile v1; publish each visual variant as a separate theme id"
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
        .chain(manifest.modifiers.values())
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
    let allowed_modifiers = [
        "background",
        "foreground",
        "ax",
        "pixel",
        "browser",
        "desktop",
    ];
    let mut modifiers = BTreeMap::new();
    for (name, reference) in manifest.modifiers {
        if !allowed_modifiers.contains(&name.as_str()) {
            bail!("unknown modifier `{name}`");
        }
        modifiers.insert(
            name,
            with_still_frame(
                compiled
                    .get(&reference.animation)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing compiled modifier"))?,
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
        modifiers,
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

fn compile_animation(bytes: &[u8], id: &str) -> Result<CompiledAnimation> {
    let text =
        std::str::from_utf8(bytes).with_context(|| format!("animation `{id}` is not UTF-8"))?;
    let animation =
        Animation::from_json_str(text).with_context(|| format!("parse animation `{id}`"))?;
    if animation.width != CANVAS
        || animation.height != CANVAS
        || (animation.frame_rate - FPS).abs() > f32::EPSILON
    {
        bail!("animation `{id}` must be 128×128 at 30 fps");
    }
    let report = analyze_animation(&animation);
    if !report.is_supported() {
        bail!("animation `{id}` uses unsupported Lottie features: {report}");
    }
    let count = animation.duration_frames().ceil() as usize;
    if count == 0 || count > MAX_FRAMES {
        bail!("animation `{id}` must contain 1..={MAX_FRAMES} frames");
    }
    let renderer = Renderer::default();
    let mut frames = Vec::with_capacity(count);
    for index in 0..count {
        let frame = renderer
            .render_frame(
                &animation,
                animation.in_point + index as f32,
                RenderConfig::default(),
            )
            .with_context(|| format!("render animation `{id}` frame {index}"))?;
        if frame.width != CANVAS || frame.height != CANVAS {
            bail!("renderer returned an unexpected canvas for `{id}`");
        }
        frames.push(CompiledFrame {
            pixels: premultiply_rgba(frame.pixels),
        });
    }
    Ok(CompiledAnimation {
        still_frame: 0,
        frames,
    })
}

fn premultiply_rgba(mut pixels: Vec<u8>) -> Vec<u8> {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        pixel[0] = ((u16::from(pixel[0]) * alpha + 127) / 255) as u8;
        pixel[1] = ((u16::from(pixel[1]) * alpha + 127) / 255) as u8;
        pixel[2] = ((u16::from(pixel[2]) * alpha + 127) / 255) as u8;
    }
    pixels
}

fn preview(theme: &CompiledTheme, output: &Path) -> Result<()> {
    fs::create_dir_all(output).with_context(|| format!("create {}", output.display()))?;
    for (name, animation) in theme.actions.iter().chain(theme.modifiers.iter()) {
        let frame = animation
            .frames
            .get(usize::from(animation.still_frame))
            .ok_or_else(|| anyhow!("missing still frame for `{name}`"))?;
        // The compiled pixels are premultiplied; PNG expects straight RGBA.
        let pixels = unpremultiply_rgba(frame.pixels.clone());
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
    fn premultiplication_round_trip_is_bounded() {
        let original = vec![200, 100, 50, 128, 0, 0, 0, 0];
        let round_trip = unpremultiply_rgba(premultiply_rgba(original.clone()));
        assert!(round_trip[0].abs_diff(original[0]) <= 1);
        assert!(round_trip[1].abs_diff(original[1]) <= 1);
        assert!(round_trip[2].abs_diff(original[2]) <= 1);
        assert_eq!(&round_trip[4..], &[0, 0, 0, 0]);
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
        let file = tempfile::Builder::new()
            .suffix(".lottie")
            .tempfile()
            .unwrap();
        let mut archive = ZipWriter::new(file.reopen().unwrap());
        let options = SimpleFileOptions::default();
        let standard = format!(r#"{{"version":"2","animations":[{{"id":"{standard_id}"}}]}}"#);
        let semantic = format!(
            r#"{{
                "schema":"cua.cursor-theme/1",
                "id":"com.example.test",
                "name":"Test",
                "version":"1.0.0",
                "author":"Example Author",
                "license":"MIT",
                "compatibility":{{"profile":"cua-driver-development-v1","semantics":1}},
                "canvas":{{"width":128,"height":128,"fps":30}},
                "hotspot":{{"x":55,"y":30}},
                "actions":{{
                    "idle":{{"animation":"base","still_frame":0}},
                    "click":{{"animation":"base","still_frame":0}}
                }},
                "modifiers":{{}},
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
    fn rejects_semantic_references_missing_from_standard_manifest() {
        let source = source_archive("different", "{}");
        assert!(compile_source(source.path(), false)
            .unwrap_err()
            .to_string()
            .contains("absent from manifest.json"));
    }

    #[test]
    fn rejects_noop_variants_in_profile_v1() {
        let source = source_archive("base", r#"{"dark":"dark"}"#);
        assert!(compile_source(source.path(), false)
            .unwrap_err()
            .to_string()
            .contains("variants are not supported"));
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
