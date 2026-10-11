// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Desktop groups: `fixtures`, `screenshot`, `windows`, `input`, `a11y`.
//!
//! Effects only ever touch fixture windows this run launched through
//! `ProcessService` (a per-run title, tracked by tag and pid, killed at the
//! end), inside the sandbox's virtual display. Input goes through
//! `ComputerService`, so through cua-driver; oracles are the fixtures' own
//! JSONL logs (what the window actually received) and screenshot pixels.
//!
//! The grid fixture paints cell (col, row) as
//! `(col*255/7, row*255/5, 128)` in 80 px cells, so the doctor finds the grid
//! on screen by colour (no window-manager geometry assumptions) and checks
//! the pixel under a click.

use std::time::{Duration, Instant};

use cua_spacesd_client::diagnose::{Artifact, Check, Status};
use cua_spacesd_client::{pb, Command, ScreenshotOptions};
use serde_json::Value;

use crate::{Ctx, Recorder};

/// Grid geometry (the fixture's defaults).
const CELL: u32 = 80;
const COLS: u32 = 8;
const ROWS: u32 = 6;
const BLUE: u8 = 128;

/// Colour of grid cell (col, row).
pub fn cell_color(col: u32, row: u32) -> [u8; 3] {
    [
        (col * 255 / (COLS - 1)) as u8,
        (row * 255 / (ROWS - 1)) as u8,
        BLUE,
    ]
}

/// A fixture this run started.
#[derive(Clone, Debug)]
pub struct Fixture {
    /// Fixture name ("grid", "form", ...).
    pub name: String,
    /// ProcessService tag.
    pub tag: String,
    /// Window title (unique per run).
    pub title: String,
    /// JSONL log path in the guest.
    pub log: String,
    /// Process id.
    pub pid: u32,
}

/// Fixtures this run started.
#[derive(Clone, Debug, Default)]
pub struct FixtureSet {
    /// Started fixtures.
    pub started: Vec<Fixture>,
    /// Their log directory.
    pub log_dir: String,
}

impl FixtureSet {
    fn get(&self, name: &str) -> Option<Fixture> {
        self.started.iter().find(|f| f.name == name).cloned()
    }
}

fn script(name: &str) -> &'static str {
    match name {
        "grid" => "grid.py",
        "form" => "form.py",
        "tone" => "tone.py",
        "avsync" => "avsync.py",
        _ => "http_server.py",
    }
}

/// The command starting fixture `name` from `root`: the Python/GTK fixtures
/// on Linux and macOS, their WinForms ports (`<name>.ps1`, the same log
/// records) on Windows (libs/images/windows-2022/fixtures).
fn fixture_command(root: &str, name: &str) -> Command {
    if cfg!(windows) {
        let stem = script(name).trim_end_matches(".py");
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(format!("{root}\\{stem}.ps1"))
    } else {
        Command::new("python3").arg(format!("{root}/{}", script(name)))
    }
}

fn log_dir(ctx: &Ctx) -> String {
    std::env::temp_dir()
        .join(format!("cua-doctor-{}-fixtures", ctx.nonce))
        .to_string_lossy()
        .into_owned()
}

/// Starts fixture `name` through ProcessService and waits for its `ready`
/// record. Idempotent per run.
pub async fn start_fixture(ctx: &Ctx, name: &str) -> Result<Fixture, String> {
    if let Some(existing) = ctx.fixtures.lock().await.get(name) {
        return Ok(existing);
    }
    let root = ctx.manifest.manifest.fixtures.root.clone();
    if !ctx.manifest.manifest.fixtures.has(name) {
        return Err(format!("the image ships no {name} fixture"));
    }
    let dir = log_dir(ctx);
    let title = format!("CUA Doctor {name} {}", ctx.nonce);
    let tag = format!("cua-doctor-{}-{name}", ctx.nonce);
    let fixture_name = format!("doctor-{name}");
    let cmd = fixture_command(&root, name)
        .env("CUA_FIXTURE_NAME", &fixture_name)
        .env("CUA_FIXTURE_LOG_DIR", &dir)
        .env("CUA_GRID_TITLE", &title)
        .env("CUA_FORM_TITLE", &title)
        .env("CUA_TONE_CYCLES", "0")
        .tag(&tag);
    let handle = ctx
        .client
        .spawn(cmd)
        .await
        .map_err(|e| format!("start {name}: {e}"))?;
    let pid = handle.pid();
    handle.detach();
    let fixture = Fixture {
        name: name.to_owned(),
        tag,
        title,
        log: format!("{dir}/{fixture_name}.jsonl"),
        pid,
    };
    {
        let mut set = ctx.fixtures.lock().await;
        set.log_dir = dir;
        set.started.push(fixture.clone());
    }
    if wait_event(
        ctx,
        &fixture,
        |e| e["type"] == "ready",
        Duration::from_secs(20),
    )
    .await
    .is_none()
    {
        return Err(format!(
            "{name} fixture never reported ready ({})",
            fixture.log
        ));
    }
    Ok(fixture)
}

/// Every record of a fixture's log (bounded).
pub async fn events(ctx: &Ctx, fixture: &Fixture) -> Vec<Value> {
    match ctx.client.download(&fixture.log).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes)
            .lines()
            .take(20_000)
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Polls the log until `pred` matches a record logged after `since` records.
pub async fn wait_event_after(
    ctx: &Ctx,
    fixture: &Fixture,
    since: usize,
    pred: impl Fn(&Value) -> bool,
    timeout: Duration,
) -> Option<Value> {
    let deadline = Instant::now() + crate::scaled(timeout);
    // Bounded: 200 polls per unscaled budget.
    for _ in 0..(200.0 * crate::timeout_scale()) as usize {
        let all = events(ctx, fixture).await;
        if let Some(found) = all.iter().skip(since).find(|e| pred(e)) {
            return Some(found.clone());
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    None
}

async fn wait_event(
    ctx: &Ctx,
    fixture: &Fixture,
    pred: impl Fn(&Value) -> bool,
    timeout: Duration,
) -> Option<Value> {
    wait_event_after(ctx, fixture, 0, pred, timeout).await
}

/// Kills every fixture this run started and removes their logs.
pub async fn teardown(ctx: &Ctx) {
    let set = std::mem::take(&mut *ctx.fixtures.lock().await);
    for fixture in &set.started {
        let _ = ctx
            .client
            .process()
            .signal_process(pb::SignalProcessRequest {
                process: Some(pb::ProcessSelector {
                    selector: Some(pb::process_selector::Selector::Tag(fixture.tag.clone())),
                }),
                signal: pb::Signal::Kill as i32,
                process_group: true,
            })
            .await;
    }
    if !set.log_dir.is_empty() {
        let _ = ctx.client.remove(&set.log_dir, true).await;
    }
}

/// Finds the window with `title` (polls up to `timeout`).
pub async fn find_window(ctx: &Ctx, title: &str, timeout: Duration) -> Option<pb::WindowInfo> {
    let deadline = Instant::now() + crate::scaled(timeout);
    // Bounded: 200 polls per unscaled budget.
    for _ in 0..(200.0 * crate::timeout_scale()) as usize {
        if let Ok(listed) = ctx
            .client
            .windows()
            .list_windows(pb::ListWindowsRequest {
                filter: Some(pb::WindowFilter {
                    title_contains: title.into(),
                    ..Default::default()
                }),
            })
            .await
        {
            if let Some(w) = listed.into_inner().windows.into_iter().next() {
                return Some(w);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    None
}

/// A decoded screenshot (RGB) and its scale.
pub struct Shot {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGB rows.
    pub rgb: Vec<u8>,
    /// Pixels per logical point.
    pub scale: f64,
    /// The encoded PNG.
    pub png: Vec<u8>,
}

impl Shot {
    /// Pixel at (x, y).
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y * self.width + x) * 3) as usize;
        Some([self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]])
    }

    /// Distinct colours in a coarse sample (a blank frame has 1).
    pub fn distinct_colors(&self) -> usize {
        let mut seen = std::collections::BTreeSet::new();
        for y in (0..self.height).step_by(7) {
            for x in (0..self.width).step_by(7) {
                if let Some(p) = self.pixel(x, y) {
                    seen.insert(p);
                    if seen.len() > 4096 {
                        return seen.len();
                    }
                }
            }
        }
        seen.len()
    }

    /// Top-left pixel of the grid fixture: the first pixel of cell (0,0)'s
    /// colour whose whole 80x80 block (scaled) is that colour.
    pub fn find_grid(&self) -> Option<(u32, u32)> {
        let want = cell_color(0, 0);
        let cell = (CELL as f64 * self.scale).round() as u32;
        let near = |p: [u8; 3]| p.iter().zip(want.iter()).all(|(a, b)| a.abs_diff(*b) <= 4);
        for y in 0..self.height.saturating_sub(cell) {
            for x in 0..self.width.saturating_sub(cell) {
                if !self.pixel(x, y).is_some_and(near) {
                    continue;
                }
                let solid = [
                    (cell - 2, 0),
                    (0, cell - 2),
                    (cell - 2, cell - 2),
                    (cell / 2, cell / 2),
                ]
                .iter()
                .all(|(dx, dy)| self.pixel(x + dx, y + dy).is_some_and(near));
                let left_edge = x == 0 || !self.pixel(x - 1, y).is_some_and(near);
                let top_edge = y == 0 || !self.pixel(x, y - 1).is_some_and(near);
                if solid && left_edge && top_edge {
                    return Some((x, y));
                }
            }
        }
        None
    }
}

/// Captures and decodes the primary display.
pub async fn screenshot(ctx: &Ctx) -> Result<Shot, String> {
    let shot = ctx
        .client
        .screenshot(ScreenshotOptions {
            format: pb::ImageFormat::Png,
            ..Default::default()
        })
        .await
        .map_err(|e| e.to_string())?;
    let png = shot.image.to_vec();
    let decoded = tokio::task::spawn_blocking({
        let png = png.clone();
        move || {
            image::load_from_memory_with_format(&png, image::ImageFormat::Png).map(|i| i.to_rgb8())
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("screenshot is not a PNG: {e}"))?;
    Ok(Shot {
        width: decoded.width(),
        height: decoded.height(),
        rgb: decoded.into_raw(),
        scale: if shot.scale > 0.0 { shot.scale } else { 1.0 },
        png,
    })
}

/// An inline artifact (dropped when larger than the inline cap), also
/// written to the artifacts directory when one is set.
pub fn artifact(ctx: &Ctx, name: &str, media_type: &str, data: &[u8]) -> Artifact {
    let mut path = String::new();
    if let Some(dir) = &ctx.options.artifacts_dir {
        let file = dir.join(name);
        if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&file, data).is_ok() {
            path = file.to_string_lossy().into_owned();
        }
    }
    Artifact {
        name: name.into(),
        media_type: media_type.into(),
        data: if data.len() <= cua_spacesd_client::diagnose::MAX_INLINE_ARTIFACT_BYTES {
            data.to_vec()
        } else {
            Vec::new()
        },
        path,
    }
}

fn display_claims(ctx: &Ctx) -> Vec<&'static str> {
    let _ = ctx;
    vec!["manifest:display", "feature:desktop_stream"]
}

async fn click(ctx: &Ctx, x: f64, y: f64) -> Result<(), String> {
    ctx.client
        .click(x, y)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub async fn run(ctx: &Ctx, rec: &mut Recorder<'_>) {
    let display = matches!(
        pb::DisplayServer::try_from(ctx.caps.display_server).unwrap_or_default(),
        pb::DisplayServer::X11
            | pb::DisplayServer::Wayland
            | pb::DisplayServer::Quartz
            | pb::DisplayServer::Win32
    );
    let claims = display_claims(ctx);
    if !display {
        for id in [
            "screenshot.display",
            "input.click",
            "windows.list",
            "a11y.tree",
        ] {
            rec.skip(
                id,
                &claims,
                "no_display_headless_image",
                "the guest has no display (DISPLAY_SERVER_NONE)".into(),
            )
            .await;
        }
        return;
    }

    // Read-only: the display as it is.
    rec.run("screenshot.display", &claims, Duration::from_secs(20), async {
        let displays = ctx.client.displays().await.unwrap_or_default();
        let primary = displays.iter().find(|d| d.primary).or(displays.first()).cloned();
        match screenshot(ctx).await {
            Ok(shot) => {
                let native = primary.as_ref().and_then(|d| d.native_size);
                let dims_ok = native.as_ref().is_none_or(|n| n.width == shot.width && n.height == shot.height);
                let claimed = ctx.manifest.manifest.display.size();
                let claim_ok = claimed.is_none_or(|(w, h)| {
                    (shot.width as f64 / shot.scale).round() as u32 == w
                        && (shot.height as f64 / shot.scale).round() as u32 == h
                });
                let colors = shot.distinct_colors();
                if let Some(d) = &primary {
                    let b = d.bounds.unwrap_or_default();
                    ctx.fidelity.lock().await.display = format!(
                        "{}x{}@{}",
                        b.width,
                        b.height,
                        if d.scale_factor > 0.0 { d.scale_factor } else { 1.0 }
                    );
                }
                let mut check = Check::new(
                    "screenshot.display",
                    super::verdict(dims_ok && claim_ok && colors > 16),
                    format!(
                        "{}x{} PNG at scale {}, {colors} distinct colours{}{}",
                        shot.width,
                        shot.height,
                        shot.scale,
                        if dims_ok { "" } else { ", size differs from ListDisplays" },
                        if claim_ok { "" } else { ", size differs from the image's claimed resolution" }
                    ),
                )
                .fact("width", shot.width)
                .fact("height", shot.height)
                .fix("a blank or wrongly sized capture means the display or capture backend is broken");
                check.artifacts.push(artifact(ctx, "screenshot.display.png", "image/png", &shot.png));
                check
            }
            Err(error) => Check::new("screenshot.display", Status::Fail, format!("Screenshot: {error}")),
        }
    })
    .await;

    if !rec.wants_group("fixtures")
        && !rec.wants_group("input")
        && !rec.wants_group("windows")
        && !rec.wants_group("a11y")
        && !rec.wants_group("screenshot")
    {
        return;
    }
    if !ctx.manifest.manifest.fixtures.has("grid") || !ctx.manifest.manifest.fixtures.has("form") {
        rec.skip(
            "fixtures.grid",
            &["manifest:fixtures"],
            "not_applicable",
            "the image ships no grid/form fixtures; desktop effect checks need them".into(),
        )
        .await;
        return;
    }
    if let Some((reason, message)) = ctx.effects_refusal() {
        for id in [
            "fixtures.grid",
            "input.click",
            "input.type",
            "windows.list",
            "a11y.tree",
            "a11y.act",
            "screenshot.fixture_pixel",
        ] {
            rec.skip(id, &claims, reason, message.clone()).await;
        }
        return;
    }

    // Fixtures.
    let mut grid = None;
    let mut form = None;
    for name in ["grid", "form"] {
        let id = format!("fixtures.{name}");
        let result = start_fixture(ctx, name).await;
        let check = match &result {
            Ok(f) => Check::new(
                &id,
                Status::Pass,
                format!("{name} fixture ready (pid {}, title {:?})", f.pid, f.title),
            ),
            Err(error) => Check::new(&id, Status::Fail, error.clone())
                .fix("the fixtures (python3 + GTK; PowerShell + WinForms on Windows) must start on the desktop session"),
        };
        rec.push(check, &["manifest:fixtures"]).await;
        match name {
            "grid" => grid = result.ok(),
            _ => form = result.ok(),
        }
    }
    let (Some(grid), Some(form)) = (grid, form) else {
        return;
    };

    // windows.*
    let mut grid_window = None;
    let mut form_window = None;
    rec.run(
        "windows.list",
        &["feature:windows"],
        Duration::from_secs(20),
        async {
            grid_window = find_window(ctx, &grid.title, Duration::from_secs(10)).await;
            form_window = find_window(ctx, &form.title, Duration::from_secs(10)).await;
            let pids: Vec<u32> = [&grid_window, &form_window]
                .iter()
                .filter_map(|w| w.as_ref().and_then(|w| w.app.as_ref()).map(|a| a.pid))
                .collect();
            Check::new(
                "windows.list",
                super::verdict(grid_window.is_some() && form_window.is_some()),
                format!(
                    "fixture windows listed: grid {}, form {} (app pids {pids:?})",
                    grid_window.is_some(),
                    form_window.is_some()
                ),
            )
        },
    )
    .await;

    if let Some(window) = grid_window.clone() {
        rec.run(
            "windows.bounds",
            &["feature:windows"],
            Duration::from_secs(20),
            async {
                let wref = window.r#ref.clone();
                let before = window.bounds.unwrap_or_default();
                let target = pb::Point { x: 40.0, y: 60.0 };
                if let Err(status) = ctx
                    .client
                    .windows()
                    .set_window_bounds(pb::SetWindowBoundsRequest {
                        window: wref.clone(),
                        position: Some(target),
                        width: None,
                        height: None,
                    })
                    .await
                {
                    return Check::new(
                        "windows.bounds",
                        Status::Fail,
                        format!("SetWindowBounds: {}", status.message()),
                    );
                }
                let _ = ctx
                    .client
                    .windows()
                    .activate_window(pb::ActivateWindowRequest {
                        window: wref.clone(),
                    })
                    .await;
                let mut after = before;
                let mut focused = false;
                for _ in 0..30 {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if let Some(w) = find_window(ctx, &grid.title, Duration::from_secs(1)).await {
                        after = w.bounds.unwrap_or_default();
                        focused = w.focused;
                        if (after.x - target.x).abs() <= 48.0
                            && (after.y - target.y).abs() <= 48.0
                            && focused
                        {
                            break;
                        }
                    }
                }
                let moved =
                    (after.x - target.x).abs() <= 48.0 && (after.y - target.y).abs() <= 48.0;
                Check::new(
                    "windows.bounds",
                    super::verdict(moved && focused),
                    format!(
                        "moved grid from ({}, {}) to ({}, {}) (asked ({}, {})), focused={focused}",
                        before.x, before.y, after.x, after.y, target.x, target.y
                    ),
                )
            },
        )
        .await;
    }

    // Find the grid on screen by colour, then check the pixel and click.
    let mut origin = None;
    let mut scale = 1.0;
    rec.run("screenshot.fixture_pixel", &claims, Duration::from_secs(20), async {
        if let Some(w) = &grid_window {
            let _ = ctx.client.windows().activate_window(pb::ActivateWindowRequest { window: w.r#ref.clone() }).await;
        }
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_millis(300)).await;
            match screenshot(ctx).await {
                Ok(shot) => {
                    if let Some(found) = shot.find_grid() {
                        scale = shot.scale;
                        origin = Some(found);
                        let cell = (CELL as f64 * shot.scale) as u32;
                        let (x, y) = (found.0 + 2 * cell + cell / 4, found.1 + 3 * cell + cell / 4);
                        let got = shot.pixel(x, y).unwrap_or_default();
                        let want = cell_color(2, 3);
                        let ok = got.iter().zip(want.iter()).all(|(a, b)| a.abs_diff(*b) <= 6);
                        let mut check = Check::new(
                            "screenshot.fixture_pixel",
                            super::verdict(ok),
                            format!("grid found at {found:?}; cell (2,3) pixel {got:?}, want {want:?}"),
                        );
                        check.artifacts.push(artifact(ctx, "screenshot.grid.png", "image/png", &shot.png));
                        return check;
                    }
                }
                Err(error) => return Check::new("screenshot.fixture_pixel", Status::Fail, error),
            }
        }
        Check::new("screenshot.fixture_pixel", Status::Fail, "the grid fixture never appeared in a screenshot")
    })
    .await;

    let cell_center = |col: u32, row: u32| -> Option<(f64, f64)> {
        let (x, y) = origin?;
        let cell = CELL as f64;
        Some((
            x as f64 / scale + col as f64 * cell + cell / 2.0,
            y as f64 / scale + row as f64 * cell + cell / 2.0,
        ))
    };

    let input_claims: &[&str] = &["feature:driver", "manifest:display"];
    rec.run(
        "input.click",
        input_claims,
        Duration::from_secs(20),
        async {
            let Some((x, y)) = cell_center(2, 3) else {
                return Check::new(
                    "input.click",
                    Status::Fail,
                    "grid not located; cannot aim the click",
                );
            };
            let since = events(ctx, &grid).await.len();
            if let Err(error) = click(ctx, x, y).await {
                return Check::new(
                    "input.click",
                    Status::Fail,
                    format!("Pointer click: {error}"),
                );
            }
            let got = wait_event_after(
                ctx,
                &grid,
                since,
                |e| e["type"] == "button_press",
                Duration::from_secs(5),
            )
            .await;
            match got {
                Some(e) if e["cell"] == serde_json::json!([2, 3]) => Check::new(
                    "input.click",
                    Status::Pass,
                    format!("click at ({x:.0}, {y:.0}) landed in grid cell [2,3]"),
                ),
                Some(e) => Check::new(
                    "input.click",
                    Status::Fail,
                    format!("click landed in cell {}", e["cell"]),
                ),
                None => Check::new(
                    "input.click",
                    Status::Fail,
                    "the grid logged no button_press within 5 s",
                ),
            }
        },
    )
    .await;

    rec.run(
        "input.scroll",
        input_claims,
        Duration::from_secs(20),
        async {
            let Some((x, y)) = cell_center(4, 2) else {
                return Check::new("input.scroll", Status::Fail, "grid not located");
            };
            let since = events(ctx, &grid).await.len();
            let _ = ctx.client.move_to(x, y).await;
            if let Err(error) = ctx
                .client
                .pointer(
                    None,
                    pb::pointer_request::Action::Scroll(pb::PointerScroll {
                        position: Some(pb::Point { x, y }),
                        delta_x: 0.0,
                        delta_y: 3.0,
                        unit: pb::ScrollUnit::Line as i32,
                    }),
                )
                .await
            {
                return Check::new(
                    "input.scroll",
                    Status::Fail,
                    format!("Pointer scroll: {error}"),
                );
            }
            let got = wait_event_after(
                ctx,
                &grid,
                since,
                |e| e["type"] == "scroll",
                Duration::from_secs(5),
            )
            .await;
            Check::new(
                "input.scroll",
                super::verdict(got.is_some()),
                match got {
                    Some(e) => format!(
                        "grid logged scroll {} in cell {}",
                        e["direction"], e["cell"]
                    ),
                    None => "no scroll event within 5 s".into(),
                },
            )
        },
    )
    .await;

    rec.run("input.drag", input_claims, Duration::from_secs(20), async {
        let (Some(from), Some(to)) = (cell_center(1, 1), cell_center(4, 2)) else {
            return Check::new("input.drag", Status::Fail, "grid not located");
        };
        let since = events(ctx, &grid).await.len();
        if let Err(error) = ctx.client.drag(from, to).await {
            return Check::new("input.drag", Status::Fail, format!("Pointer drag: {error}"));
        }
        let release = wait_event_after(
            ctx,
            &grid,
            since,
            |e| e["type"] == "button_release",
            Duration::from_secs(5),
        )
        .await;
        let press = events(ctx, &grid)
            .await
            .into_iter()
            .skip(since)
            .find(|e| e["type"] == "button_press");
        let ok = press
            .as_ref()
            .is_some_and(|p| p["cell"] == serde_json::json!([1, 1]))
            && release.as_ref().is_some_and(|r| {
                r["cell"] == serde_json::json!([4, 2])
                    || r["x_root"].as_f64().is_some_and(|x| (x - to.0).abs() < 8.0)
            });
        Check::new(
            "input.drag",
            super::verdict(ok),
            format!(
                "press in {}, release in {}",
                press.map(|p| p["cell"].to_string()).unwrap_or("-".into()),
                release.map(|r| r["cell"].to_string()).unwrap_or("-".into())
            ),
        )
    })
    .await;

    // Typing into the form's Name entry: focus it (through a11y when
    // available, else by activating the window), then type and edit.
    let form_ref = form_window.as_ref().and_then(|w| w.r#ref.clone());
    rec.run("input.type", input_claims, Duration::from_secs(30), async {
        if form_ref.is_none() {
            return Check::new("input.type", Status::Fail, "form window not found");
        }
        // Look the form up again rather than reusing the ref from
        // windows.list: the earlier input checks can outlive a window
        // catalogue generation, and a stale ref makes the activation fail,
        // which would send the keys to whichever window still has focus.
        let Some(wref) = find_window(ctx, &form.title, Duration::from_secs(10))
            .await
            .and_then(|w| w.r#ref)
        else {
            return Check::new("input.type", Status::Fail, "form window not found");
        };
        if let Err(error) = ctx
            .client
            .windows()
            .activate_window(pb::ActivateWindowRequest {
                window: Some(wref.clone()),
            })
            .await
        {
            return Check::new(
                "input.type",
                Status::Fail,
                format!("ActivateWindow on the form: {error}"),
            );
        }
        if !form_focused(ctx, &form.title, Duration::from_secs(5)).await {
            return Check::new(
                "input.type",
                Status::Fail,
                "ActivateWindow succeeded but the form never became the focused window",
            );
        }
        if ctx.supports("a11y") {
            if let Ok(found) = ctx
                .client
                .accessibility()
                .find(pb::FindRequest {
                    window: Some(wref.clone()),
                    query: Some(pb::AccessibilityQuery {
                        name: "Name".into(),
                        ..Default::default()
                    }),
                    max_results: 5,
                })
                .await
            {
                let entry = found.into_inner().nodes.into_iter().find(|n| {
                    n.role.contains("text")
                        || n.role.contains("entry")
                        || n.native_role.contains("text")
                });
                if let Some(b) = entry.and_then(|n| n.bounds) {
                    let (x, y) = (b.x + b.width / 2.0, b.y + b.height / 2.0);
                    // The click is a screen point: whatever is on top there
                    // receives it. windows.bounds parks the grid over the
                    // form's corner, and on a tiling compositor (Hyprland) a
                    // floating grid stays above the activated, tiled form, so
                    // the click would focus the grid and the keys would go
                    // there. Move the grid off the point first.
                    if let Err(error) = uncover_point(ctx, &grid.title, x, y).await {
                        return Check::new("input.type", Status::Fail, error);
                    }
                    let _ = click(ctx, x, y).await;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let since = events(ctx, &form).await.len();
        let report = match ctx.client.type_text("doctor").await {
            Ok(response) => response.report,
            Err(error) => {
                return Check::new(
                    "input.type",
                    Status::Fail,
                    format!("Keyboard type: {error}"),
                );
            }
        };
        let typed = wait_event_after(
            ctx,
            &form,
            since,
            |e| e["type"] == "entry_changed" && e["text"] == "doctor",
            Duration::from_secs(5),
        )
        .await;
        if typed.is_none() {
            let last = events(ctx, &form)
                .await
                .into_iter()
                .rev()
                .find(|e| e["type"] == "entry_changed");
            let after: Vec<Value> = events(ctx, &form).await.into_iter().skip(since).collect();
            return Check::new(
                "input.type",
                Status::Fail,
                format!(
                    "the Name entry never read \"doctor\" (last: {}); {}",
                    last.map(|e| e["text"].to_string()).unwrap_or("none".into()),
                    type_diagnosis(report.as_ref(), &after)
                ),
            );
        }
        let since = events(ctx, &form).await.len();
        if let Err(error) = ctx.client.press("backspace").await {
            return Check::new(
                "input.type",
                Status::Fail,
                format!("Keyboard press: {error}"),
            );
        }
        let edited = wait_event_after(
            ctx,
            &form,
            since,
            |e| e["type"] == "entry_changed" && e["text"] == "docto",
            Duration::from_secs(5),
        )
        .await;
        Check::new(
            "input.type",
            super::verdict(edited.is_some()),
            if edited.is_some() {
                "typed \"doctor\", Backspace left \"docto\"".to_owned()
            } else {
                "Backspace did not edit the entry".to_owned()
            },
        )
    })
    .await;

    // Accessibility on the form.
    let a11y_claims: &[&str] = &["feature:a11y"];
    let mut submit: Option<(String, String)> = None;
    rec.run("a11y.tree", a11y_claims, Duration::from_secs(20), async {
        let Some(wref) = form_ref.clone() else {
            return Check::new("a11y.tree", Status::Fail, "form window not found");
        };
        match ctx
            .client
            .accessibility()
            .get_tree(pb::GetTreeRequest {
                window: Some(wref),
                max_depth: 16,
                include_hidden: false,
                max_nodes: 2000,
            })
            .await
        {
            Ok(tree) => {
                let tree = tree.into_inner();
                let button = tree.nodes.iter().find(|n| {
                    n.name == "Submit"
                        && (n.role.contains("button") || n.native_role.contains("button"))
                });
                if let Some(b) = button {
                    submit = Some((tree.snapshot_id.clone(), b.element_id.clone()));
                }
                let names: Vec<&str> = tree
                    .nodes
                    .iter()
                    .map(|n| n.name.as_str())
                    .filter(|n| !n.is_empty())
                    .take(12)
                    .collect();
                Check::new(
                    "a11y.tree",
                    super::verdict(button.is_some()),
                    format!(
                        "{} nodes; Submit button {} (names: {names:?})",
                        tree.nodes.len(),
                        if button.is_some() { "found" } else { "MISSING" }
                    ),
                )
                .fact("nodes", tree.nodes.len())
            }
            Err(status) => Check::new(
                "a11y.tree",
                Status::Fail,
                format!("GetTree: {}", status.message()),
            ),
        }
    })
    .await;
    rec.run("a11y.act", a11y_claims, Duration::from_secs(20), async {
        let Some((snapshot, element)) = submit.clone() else {
            return Check::new("a11y.act", Status::Fail, "no Submit element to press");
        };
        let since = events(ctx, &form).await.len();
        if let Err(status) = ctx
            .client
            .accessibility()
            .act(pb::ActRequest {
                element: Some(pb::ElementRef {
                    snapshot_id: snapshot,
                    element_id: element,
                }),
                action: pb::AccessibilityAction::Press as i32,
                ..Default::default()
            })
            .await
        {
            return Check::new(
                "a11y.act",
                Status::Fail,
                format!("Act(press): {}", status.message()),
            );
        }
        let got = wait_event_after(
            ctx,
            &form,
            since,
            |e| e["type"] == "submit",
            Duration::from_secs(5),
        )
        .await;
        Check::new(
            "a11y.act",
            super::verdict(got.is_some()),
            if got.is_some() {
                "Act(press) on Submit logged a submit".to_owned()
            } else {
                "no submit logged within 5 s".to_owned()
            },
        )
    })
    .await;

    // LaunchApp of the app the image claims. Where the image declares the
    // app cannot run (a runtime limitation), launch its stand-in and say so.
    if let Some(claimed) = ctx.manifest.manifest.launch_app.clone() {
        let arch = crate::arch_name(&ctx.caps);
        let (app, limited) = match claimed.limit_for(&ctx.runtime, &arch) {
            Some(limit) => {
                let note = format!(
                    "{} does not run under {}/{}: {}",
                    claimed.app, ctx.runtime, arch, limit.reason
                );
                (limit.instead.as_deref().cloned(), Some(note))
            }
            None => (Some(claimed.clone()), None),
        };
        let Some(app) = app else {
            rec.run(
                "windows.launch_app",
                &["feature:launch_app"],
                Duration::from_secs(1),
                async move {
                    Check::new(
                        "windows.launch_app",
                        Status::Skip,
                        limited.unwrap_or_default(),
                    )
                },
            )
            .await;
            return;
        };
        let limited = limited.map(|n| format!("{n}; launched {} instead: ", app.app));
        rec.run(
            "windows.launch_app",
            &["feature:launch_app"],
            Duration::from_secs(app.timeout_s.max(10) as u64 + 20),
            async {
                let response = ctx
                    .client
                    .windows()
                    .launch_app(pb::LaunchAppRequest {
                        app: Some(pb::AppSpec {
                            app: Some(pb::app_spec::App::Executable(app.app.clone())),
                        }),
                        args: app.args.clone(),
                        wait_for_window: Some(pbjson_types::Duration {
                            seconds: app.timeout_s.max(10) as i64,
                            nanos: 0,
                        }),
                        ..Default::default()
                    })
                    .await;
                let response = match response {
                    Ok(r) => r.into_inner(),
                    Err(status) => {
                        return Check::new(
                            "windows.launch_app",
                            Status::Fail,
                            format!("LaunchApp {}: {}", app.app, status.message()),
                        )
                    }
                };
                let mut windows = response.windows.clone();
                if windows.is_empty() {
                    if let Some(w) = find_window(
                        ctx,
                        &app.window_match,
                        Duration::from_secs(app.timeout_s.max(10) as u64),
                    )
                    .await
                    {
                        windows.push(w);
                    }
                }
                // A crash dialog carries the app's name ("Firefox Crash
                // Reporter"): the app did not come up.
                let crashed: Vec<String> = windows
                    .iter()
                    .filter(|w| is_crash_window(&w.title))
                    .map(|w| w.title.clone())
                    .collect();
                let matched = windows.iter().any(|w| {
                    !is_crash_window(&w.title)
                        && (w.title.contains(&app.window_match)
                            || w.app
                                .as_ref()
                                .is_some_and(|a| a.name.contains(&app.window_match)))
                });
                // Close what we opened.
                for w in &windows {
                    let _ = ctx
                        .client
                        .windows()
                        .close_window(pb::CloseWindowRequest {
                            window: w.r#ref.clone(),
                            force: true,
                        })
                        .await;
                }
                let check = Check::new(
                    "windows.launch_app",
                    super::verdict(matched && crashed.is_empty()),
                    format!(
                        "{}{} (pid {}) mapped {} window(s): {:?}",
                        limited.as_deref().unwrap_or(""),
                        app.app,
                        response.pid,
                        windows.len(),
                        windows.iter().map(|w| w.title.clone()).collect::<Vec<_>>()
                    ),
                );
                if crashed.is_empty() {
                    check
                } else {
                    check.fix(format!("{} crashed on start: {crashed:?}", app.app))
                }
            },
        )
        .await;
    }
}

/// Whether a window title is a crash dialog rather than the app itself.
pub(crate) fn is_crash_window(title: &str) -> bool {
    let t = title.to_ascii_lowercase();
    t.contains("crash reporter") || t.contains("crash report") || t.ends_with("has crashed")
}

/// Where a `w`x`h` window can go so it does not contain (`x`, `y`): beside the
/// point on whichever side of the display has room, else below or above it.
fn position_clear_of(
    (x, y): (f64, f64),
    (w, h): (f64, f64),
    (display_w, display_h): (f64, f64),
) -> Option<(f64, f64)> {
    const GAP: f64 = 24.0;
    let clamp_y = (y - h / 2.0).clamp(0.0, (display_h - h).max(0.0));
    let clamp_x = (x - w / 2.0).clamp(0.0, (display_w - w).max(0.0));
    [
        (x + GAP <= display_w - w).then_some((x + GAP, clamp_y)),
        (x - GAP - w >= 0.0).then_some((x - GAP - w, clamp_y)),
        (y + GAP <= display_h - h).then_some((clamp_x, y + GAP)),
        (y - GAP - h >= 0.0).then_some((clamp_x, y - GAP - h)),
    ]
    .into_iter()
    .flatten()
    .next()
}

/// Moves the fixture window titled `title` so it no longer covers (`x`, `y`).
async fn uncover_point(ctx: &Ctx, title: &str, x: f64, y: f64) -> Result<(), String> {
    let Some(window) = find_window(ctx, title, Duration::from_secs(5)).await else {
        return Ok(());
    };
    let Some(b) = window.bounds else {
        return Ok(());
    };
    if x < b.x || y < b.y || x >= b.x + b.width || y >= b.y + b.height {
        return Ok(());
    }
    let display = ctx
        .caps
        .displays
        .first()
        .and_then(|d| d.bounds)
        .map_or((1280.0, 800.0), |d| (d.width, d.height));
    let Some((nx, ny)) = position_clear_of((x, y), (b.width, b.height), display) else {
        return Err(format!(
            "the grid fixture covers the form's Name entry at ({x}, {y}) and cannot be moved clear"
        ));
    };
    ctx.client
        .windows()
        .set_window_bounds(pb::SetWindowBoundsRequest {
            window: window.r#ref,
            position: Some(pb::Point { x: nx, y: ny }),
            width: None,
            height: None,
        })
        .await
        .map(|_| ())
        .map_err(|status| format!("moving the grid off the Name entry: {}", status.message()))
}

/// Polls until the window titled `title` is the focused one.
async fn form_focused(ctx: &Ctx, title: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + crate::scaled(timeout);
    for _ in 0..(100.0 * crate::timeout_scale()) as usize {
        if find_window(ctx, title, Duration::ZERO)
            .await
            .is_some_and(|w| w.focused)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

/// What the form saw after a failed type, and what the server reported, so a
/// CI failure says whether keys reached the window at all and where focus was.
fn type_diagnosis(report: Option<&pb::DeliveryReport>, after: &[Value]) -> String {
    let presses: Vec<&Value> = after.iter().filter(|e| e["type"] == "key_press").collect();
    let focus = presses
        .last()
        .map(|e| e["focus"].to_string())
        .unwrap_or_else(|| "-".into());
    let focus_events: Vec<&str> = after
        .iter()
        .filter_map(|e| e["type"].as_str())
        .filter(|t| t.starts_with("focus_"))
        .collect();
    let delivery = report.map_or_else(
        || "no delivery report".to_string(),
        |r| format!("delivery {r:?}"),
    );
    format!(
        "form key presses {} (focused widget {focus}), focus events {focus_events:?}, {delivery}",
        presses.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_app_limits_pick_the_stand_in() {
        let app: cua_spacesd_client::manifest::LaunchApp =
            serde_json::from_value(serde_json::json!({
                "app": "firefox", "window_match": "Firefox", "timeout_s": 60,
                "limits": [{"runtime": "gvisor", "arch": "x86_64", "reason": "no ARCH_SET_GS",
                            "instead": {"app": "chromium", "window_match": "Chromium"}}]
            }))
            .unwrap();
        let limit = app.limit_for("gvisor", "x86_64").expect("limited");
        assert_eq!(limit.instead.as_ref().unwrap().app, "chromium");
        assert!(app.limit_for("gvisor", "arm64").is_none());
        assert!(app.limit_for("container", "x86_64").is_none());
        let plain: cua_spacesd_client::manifest::LaunchApp =
            serde_json::from_value(serde_json::json!({"app": "firefox"})).unwrap();
        assert!(plain.limits.is_empty() && plain.limit_for("gvisor", "x86_64").is_none());
    }

    #[test]
    fn crash_dialogs_are_not_the_app() {
        assert!(is_crash_window("Firefox Crash Reporter"));
        assert!(is_crash_window("Mozilla Crash Reporter"));
        assert!(is_crash_window("Thunar has crashed"));
        assert!(!is_crash_window("Mozilla Firefox"));
        assert!(!is_crash_window("New Tab — Mozilla Firefox"));
    }

    fn grid_shot(ox: u32, oy: u32, width: u32, height: u32) -> Shot {
        let mut rgb = vec![30u8; (width * height * 3) as usize];
        for row in 0..ROWS {
            for col in 0..COLS {
                let c = cell_color(col, row);
                for y in 0..CELL {
                    for x in 0..CELL {
                        let (px, py) = (ox + col * CELL + x, oy + row * CELL + y);
                        if px < width && py < height {
                            let i = ((py * width + px) * 3) as usize;
                            rgb[i..i + 3].copy_from_slice(&c);
                        }
                    }
                }
            }
        }
        Shot {
            width,
            height,
            rgb,
            scale: 1.0,
            png: Vec::new(),
        }
    }

    #[test]
    fn grid_colours_match_the_fixture() {
        assert_eq!(cell_color(2, 3), [72, 153, 128]);
        assert_eq!(cell_color(7, 5), [255, 255, 128]);
    }

    #[test]
    fn finds_the_grid_by_colour() {
        let shot = grid_shot(137, 61, 1280, 800);
        assert_eq!(shot.find_grid(), Some((137, 61)));
        assert_eq!(
            shot.pixel(137 + 2 * 80 + 20, 61 + 3 * 80 + 20),
            Some([72, 153, 128])
        );
        assert!(shot.distinct_colors() > 16);
        let blank = Shot {
            width: 100,
            height: 100,
            rgb: vec![0; 30_000],
            scale: 1.0,
            png: Vec::new(),
        };
        assert_eq!(blank.find_grid(), None);
        assert_eq!(blank.distinct_colors(), 1);
    }
}

#[cfg(test)]
mod type_diagnosis_tests {
    use super::{position_clear_of, type_diagnosis};

    #[test]
    fn grid_moves_clear_of_the_entry_point() {
        // The CI layout: 640x480 grid at (40, 60), Name entry centre (150, 62).
        let (x, y) = position_clear_of((150.0, 62.0), (640.0, 480.0), (1280.0, 800.0)).unwrap();
        assert!(!(150.0 >= x && 150.0 < x + 640.0 && 62.0 >= y && 62.0 < y + 480.0));
        assert!(x >= 0.0 && y >= 0.0 && x + 640.0 <= 1280.0 && y + 480.0 <= 800.0);
        // A window as large as the display cannot be moved clear.
        assert_eq!(
            position_clear_of((10.0, 10.0), (1280.0, 800.0), (1280.0, 800.0)),
            None
        );
    }

    #[test]
    fn reports_key_presses_focus_and_delivery() {
        let after = vec![
            serde_json::json!({"type": "focus_out"}),
            serde_json::json!({"type": "key_press", "focus": "Notes"}),
            serde_json::json!({"type": "key_release", "focus": "Notes"}),
        ];
        let text = type_diagnosis(None, &after);
        assert!(text.contains("form key presses 1"), "{text}");
        assert!(text.contains("\"Notes\""), "{text}");
        assert!(text.contains("focus_out"), "{text}");
        assert!(text.contains("no delivery report"), "{text}");
        assert!(type_diagnosis(None, &[]).contains("form key presses 0"));
    }
}
