// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The shared openkoalabots scenario (`samples/openkoalabot-example-scenario/scenario.json`),
//! executed through this implementation's own core: the same modules the
//! Tauri shell calls. Headless: nothing here opens a window or draws on the
//! operator's desktop.

use crate::bots::{AgentOptions, BotInfo, Bots};
use crate::files::{send_verified, xorshift_bytes};
use crate::presence::Presence;
use crate::stream::watch_desktop;
use crate::teleport::{Decision, ships_with_cua_spaces, teleport};
use crate::thread::BotThread;
use crate::{Core, CoreConfig, Error, Result};
use cua_spaces::Space;
use cua_spaces::groups::{GroupChat, GroupChatError, GroupChatStore, GroupSpeaker};
use cua_spaces::presence::PresenceEvent;
use cua_spaces::routines::{FileStorage, RoutineFiring, RoutineStore, Schedule};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where the scenario runs.
#[derive(Clone, Debug)]
pub struct Lane {
    /// `fixture`, `docker` or `cloud`.
    pub name: String,
    /// spacesd URL (fixture, docker).
    pub url: Option<String>,
    /// spacesd token (fixture, docker).
    pub token: Option<String>,
    /// `{importRoot}` in the teleport check (`$HOME` in a real guest).
    pub import_root: String,
    /// The image to create in Cua Cloud (cloud).
    pub cloud_image: Option<String>,
}

impl Lane {
    /// From `OPENKOALABOTS_SCENARIO_*` / `OPENKOALABOTS_CLOUD_IMAGE`.
    pub fn from_env(name: &str) -> Self {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            name: name.to_string(),
            url: var("OPENKOALABOTS_SCENARIO_URL"),
            token: var("OPENKOALABOTS_SCENARIO_TOKEN"),
            import_root: var("OPENKOALABOTS_SCENARIO_IMPORT_ROOT")
                .unwrap_or_else(|| "$HOME".into()),
            cloud_image: var("OPENKOALABOTS_CLOUD_IMAGE"),
        }
    }
}

/// One step's outcome.
#[derive(Clone, Debug, serde::Serialize)]
pub struct StepResult {
    pub id: String,
    pub status: &'static str,
    pub ms: u64,
    pub detail: String,
}

/// The run's outcome, in the shape every runner writes.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ScenarioResult {
    #[serde(rename = "impl")]
    pub implementation: &'static str,
    pub lane: String,
    pub ok: bool,
    #[serde(rename = "totalMs")]
    pub total_ms: u64,
    pub steps: Vec<StepResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
}

enum Outcome {
    Pass(String),
    Skip(String),
}

struct Ctx {
    lane: Lane,
    /// The spec's `model` block (the scripted model for agent turns).
    model: Value,
    nonce: String,
    marker: String,
    root: tempfile::TempDir,
    core: Core,
    space: Option<Space>,
    space_id: Option<String>,
}

impl Ctx {
    fn fill(&self, s: &str) -> String {
        s.replace("{nonce}", &self.nonce)
            .replace("{marker}", &self.marker)
            .replace("{importRoot}", &self.lane.import_root)
    }

    fn space(&self) -> Result<&Space> {
        self.space
            .as_ref()
            .ok_or_else(|| Error::Invalid("no Space".into()))
    }
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn u(v: &Value, k: &str, default: u64) -> u64 {
    v.get(k).and_then(Value::as_u64).unwrap_or(default)
}

/// Runs `spec` (the parsed scenario.json from `_spec_dir`) on `lane`. This
/// runner reads none of the spec's fixtures: the agent step's fake CLI is
/// skipped (see `agent`), and the teleport step needs no generated profile
/// (session teleport ships with Cua Spaces; see `teleport_step`).
pub async fn run(spec: &Value, _spec_dir: &Path, lane: Lane) -> Result<ScenarioResult> {
    let started = Instant::now();
    let nonce = crate::nonce();
    let root = tempfile::tempdir()?;
    let marker = format!("openkoalabots-{nonce}");
    let core = Core::new(
        CoreConfig::in_dir(root.path().join("a")).with_cloud_from_env(lane.name == "cloud"),
    )?;
    let mut ctx = Ctx {
        lane,
        model: spec.get("model").cloned().unwrap_or(Value::Null),
        nonce,
        marker,
        root,
        core,
        space: None,
        space_id: None,
    };
    let steps = spec
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Invalid("spec has no steps".into()))?;
    let mut results = Vec::new();
    for step in steps {
        let id = s(step, "id").unwrap_or("?").to_string();
        let op = s(step, "op").unwrap_or("");
        let t0 = Instant::now();
        // Every step after the first needs the Space; `delete` (`always`)
        // still runs after any other step failed.
        let outcome = if op != "space.open" && ctx.space.is_none() {
            Err(Error::Invalid("no Space was opened".into()))
        } else {
            // Steps that poll an agent turn get their polling budget plus
            // room for the harness install on a fresh Space.
            let polled = u(step, "pollMs", 0) * u(step, "maxPolls", 0) + 180_000;
            let budget = Duration::from_millis(u(step, "timeoutMs", polled.max(300_000)));
            match tokio::time::timeout(budget, run_step(&mut ctx, op, step)).await {
                Ok(r) => r,
                Err(_) => Err(Error::Timeout(format!("step exceeded {budget:?}"))),
            }
        };
        let (status, detail) = match outcome {
            Ok(Outcome::Pass(d)) => ("pass", d),
            Ok(Outcome::Skip(d)) => ("skip", d),
            Err(e) => ("fail", e.to_string()),
        };
        eprintln!("[{status}] {id}: {detail}");
        results.push(StepResult {
            id,
            status,
            ms: t0.elapsed().as_millis() as u64,
            detail,
        });
    }
    let ok = results.iter().all(|r| r.status != "fail");
    Ok(ScenarioResult {
        implementation: "tauri",
        lane: ctx.lane.name.clone(),
        ok,
        total_ms: started.elapsed().as_millis() as u64,
        steps: results,
        space: ctx.space_id.clone(),
    })
}

async fn run_step(ctx: &mut Ctx, op: &str, step: &Value) -> Result<Outcome> {
    if let Some(req) = s(step, "requires")
        && let Some(space) = &ctx.space
        && !space.supports(req)
    {
        return Ok(Outcome::Skip(format!("the Space lacks `{req}`")));
    }
    match op {
        "space.open" => open(ctx, step).await,
        "stream.desktop" => stream(ctx, step).await,
        "agent.thread" => agent(ctx, step).await,
        "file.send" => file(ctx, step).await,
        "teleport.app" => teleport_step(ctx, step).await,
        "presence.pair" => presence(ctx, step).await,
        "routine.schedule" => routine(ctx, step).await,
        "group.chat" => group(ctx, step).await,
        "space.delete" => delete(ctx).await,
        other => Err(Error::Invalid(format!("unknown op {other:?}"))),
    }
}

async fn open(ctx: &mut Ctx, step: &Value) -> Result<Outcome> {
    let mode = step
        .get("modes")
        .and_then(|m| s(m, &ctx.lane.name))
        .unwrap_or("add")
        .to_string();
    let name = ctx.fill(s(step, "name").unwrap_or("openkoalabot-example-scenario-{nonce}"));
    let info =
        match mode.as_str() {
            "add" => {
                let url =
                    ctx.lane.url.clone().ok_or_else(|| {
                        Error::Invalid("OPENKOALABOTS_SCENARIO_URL is unset".into())
                    })?;
                ctx.core
                    .add_space(&url, ctx.lane.token.clone(), Some(name))
                    .await?
            }
            "create" => {
                let image =
                    ctx.lane.cloud_image.clone().ok_or_else(|| {
                        Error::Invalid("OPENKOALABOTS_CLOUD_IMAGE is unset".into())
                    })?;
                // Cloud resources this suite creates are named cua-e2e-*.
                let name = format!("cua-e2e-openkoalabots-{}", ctx.nonce);
                ctx.core.create_cloud_space(Some(image), Some(name)).await?
            }
            other => return Err(Error::Invalid(format!("unknown space.open mode {other}"))),
        };
    ctx.space_id = Some(info.id.clone());
    let space = ctx.core.space(&info.id).await?;
    ctx.space = Some(space);
    Ok(Outcome::Pass(format!(
        "{mode}: {} (spacesd {}, {} features)",
        info.id,
        info.spacesd_version,
        info.features.len()
    )))
}

async fn stream(ctx: &mut Ctx, step: &Value) -> Result<Outcome> {
    let space = ctx.space()?;
    let poll = Duration::from_millis(u(step, "pollMs", 100));
    let polls = u(step, "maxPolls", 300) as u32;
    let min = u(step, "minFrames", 1);
    let w = watch_desktop(
        space,
        u(step, "maxFps", 5) as u32,
        u(step, "maxDimension", 800) as u32,
    )
    .await?;
    let result = async {
        w.watch.wait_frames(min, poll, polls).await?;
        let first = w.watch.snapshot();
        if step
            .get("firstFrameKeyframe")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && first.first_frame_keyframe != Some(true)
        {
            return Err(Error::Invalid("the first frame was not a keyframe".into()));
        }
        if step
            .get("keyframeRequest")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            w.request_keyframe()?;
            w.watch.wait_frames(first.frames + 1, poll, polls).await?;
        }
        Ok(first)
    }
    .await;
    let snap = w.watch.snapshot();
    let stats = w.close().await?;
    let first = result?;
    Ok(Outcome::Pass(format!(
        "{} frames ({} keyframes), {}x{} {}, first frame keyframe; {} keyframe requests",
        snap.frames,
        snap.keyframes,
        first.width,
        first.height,
        first.codec,
        stats.keyframe_requests
    )))
}

async fn agent(ctx: &mut Ctx, step: &Value) -> Result<Outcome> {
    let space = ctx.space()?.clone();
    // Agent runs speak the Agent Client Protocol through cua-agents' runner,
    // so a fake `claude` shell script can no longer stand in for a model.
    // Real harness runs against a scripted mock provider:
    // `cargo test -p cua-agents --test e2e_live`
    // (libs/cua/crates/cua-agents/tests/e2e/run-agents-e2e.sh).
    if step.get("fakeCli").is_some() {
        return Ok(Outcome::Skip(
            "the step's fake CLI predates ACP agent runs; real runs are covered by \
             cua-agents' e2e_live"
                .into(),
        ));
    }
    let mut thread = BotThread::new(&space, s(step, "agent").unwrap_or("claude-code")).await?;
    let poll = Duration::from_millis(u(step, "pollMs", 200));
    let polls = u(step, "maxPolls", 300) as u32;
    let turns = step
        .get("turns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let result = async {
        for t in &turns {
            let text = ctx.fill(s(t, "prompt").or(s(t, "message")).unwrap_or(""));
            let expect = s(t, "expectOutput").map(|e| ctx.fill(e));
            thread.send(&text).await?;
            thread.wait_turn(expect.as_deref(), poll, polls).await?;
        }
        Ok::<_, Error>(())
    }
    .await;
    let _ = thread.stop().await;
    result?;
    Ok(Outcome::Pass(format!(
        "{} turns on {} ({} transcript lines)",
        turns.len(),
        thread.run_id().unwrap_or("?"),
        thread.transcript().len()
    )))
}

async fn file(ctx: &mut Ctx, step: &Value) -> Result<Outcome> {
    let space = ctx.space()?.clone();
    let g = step
        .get("generate")
        .ok_or_else(|| Error::Invalid("file step has no generate".into()))?;
    let seed = s(g, "seed").unwrap_or("0x9E3779B97F4A7C15");
    let seed = u64::from_str_radix(seed.trim_start_matches("0x"), 16)
        .map_err(|e| Error::Invalid(format!("seed: {e}")))?;
    let bytes = xorshift_bytes(u(g, "bytes", 1 << 20) as usize, seed);
    let local = ctx
        .root
        .path()
        .join(ctx.fill(s(g, "name").unwrap_or("openkoalabots-{nonce}.bin")));
    std::fs::write(&local, &bytes)?;
    let want = s(step, "sha256").unwrap_or_default().to_string();
    let host = crate::files::sha256_file(&local)?;
    if !want.is_empty() && host != want {
        return Err(Error::Invalid(format!(
            "generated sha256 {host} != spec {want}"
        )));
    }
    let sent = send_verified(&space, &local, s(step, "subdir").unwrap_or("")).await?;
    let cmd = s(step, "guestSha256Command")
        .unwrap_or("sha256sum '{path}' | cut -d' ' -f1")
        .replace("{path}", &sent.guest_path);
    let guest = ctx.core.bash(&space, &cmd, Duration::from_secs(60)).await?;
    if let Some(c) = s(step, "cleanupCommand") {
        let _ = ctx
            .core
            .bash(
                &space,
                &c.replace("{path}", &sent.guest_path),
                Duration::from_secs(30),
            )
            .await;
    }
    if guest.trim() != host {
        return Err(Error::Invalid(format!(
            "guest sha256 {:?} != host {host}",
            guest.trim()
        )));
    }
    Ok(Outcome::Pass(format!(
        "{} bytes to {}, sha256 {}… matched by the guest",
        sent.bytes,
        sent.guest_path,
        &host[..12]
    )))
}

async fn teleport_step(ctx: &mut Ctx, step: &Value) -> Result<Outcome> {
    let space = ctx.space()?.clone();
    let app = s(step, "app").unwrap_or("firefox");
    let mut shown = 0usize;
    let receipt = match teleport(&ctx.core, &space, app, s(step, "scope"), |m| {
        // The explicit consent callback: it sees exactly what would move.
        shown = m.items.len();
        Some(Decision {
            include: None,
            acknowledge_sensitive: true,
        })
    })
    .await
    {
        Ok(r) => r,
        // The in-process MIT runtime has no teleport extension.
        Err(e) if ships_with_cua_spaces(&e) => return Ok(Outcome::Skip(e.to_string())),
        Err(e) => return Err(e),
    };
    if receipt.imported.is_empty() {
        return Err(Error::Invalid(format!("nothing imported: {receipt:?}")));
    }
    let cmd = ctx.fill(s(step, "verifyCommand").unwrap_or("true"));
    let out = space.bash(&cmd, Duration::from_secs(60)).await?;
    let want = s(step, "expectContains").unwrap_or("");
    if !out.stdout.contains(want) {
        return Err(Error::Invalid(format!(
            "marker not found in the guest ({cmd}): {}",
            out.render()
        )));
    }
    Ok(Outcome::Pass(format!(
        "{} of {shown} manifest items approved; imported {:?}; {} bundle bytes; marker at {}",
        receipt.transferred_paths.len(),
        receipt.imported,
        receipt.bundle_bytes,
        out.stdout.trim()
    )))
}

async fn presence(ctx: &mut Ctx, step: &Value) -> Result<Outcome> {
    let space_a = ctx.space()?.clone();
    let id = ctx.space_id.clone().unwrap_or_default();
    let timeout = Duration::from_millis(u(step, "timeoutMs", 20_000));
    let max = u(step, "maxEvents", 50) as usize;
    let clients = step
        .get("clients")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if clients.len() != 2 {
        return Err(Error::Invalid("presence.pair needs two clients".into()));
    }
    // A second, independent runtime with its own registry.
    let core_b = Core::new(
        CoreConfig::in_dir(ctx.root.path().join("b")).with_cloud_from_env(ctx.lane.name == "cloud"),
    )?;
    let (url, token) = match ctx.lane.url.clone() {
        Some(u) => (u, ctx.lane.token.clone()),
        None => (id.clone(), space_a.env_token().map(str::to_string)),
    };
    let info_b = core_b
        .add_space(&url, token, Some("openkoalabots-b".into()))
        .await?;
    let space_b = core_b.space(&info_b.id).await?;
    let who = |i: usize| {
        let c = &clients[i];
        (
            ctx.fill(s(c, "id").unwrap_or("client")),
            s(c, "displayName").unwrap_or("client").to_string(),
            c.get("agent").and_then(Value::as_bool).unwrap_or(false),
        )
    };
    let (ida, namea, agenta) = who(0);
    let (idb, nameb, agentb) = who(1);
    // With takeKoalaColor the operator asks for the second client's stable
    // color first, so the server has to assign it another one.
    let take_color = step
        .get("takeKoalaColor")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stable_b = cua_spaces::presence::color_for(&idb).to_string();
    let mut a = Presence::join_with_color(
        &space_a,
        &ida,
        &namea,
        agenta,
        take_color.then_some(stable_b.as_str()),
        timeout,
    )
    .await?;
    let b = Presence::join(&space_b, &idb, &nameb, agentb, timeout).await?;
    let a_id = a.me().participant_id.clone();
    if !b.avatars().iter().any(|x| x.participant_id == a_id) {
        return Err(Error::Invalid(
            "the second client's roster lacks the first".into(),
        ));
    }
    let b_id = b.me().participant_id.clone();
    let joined = a
        .wait_for(timeout, max, |e| {
            matches!(e, PresenceEvent::Joined { participant } if participant.participant_id == b_id)
        })
        .await?;
    if let PresenceEvent::Joined { participant } = &joined
        && (participant.display_name != nameb || (participant.kind == "agent") != agentb)
    {
        return Err(Error::Invalid(format!("joined as {participant:?}")));
    }
    let (x, y) = step
        .get("cursor")
        .map(|c| {
            (
                c.get("x").and_then(Value::as_f64).unwrap_or(0.25),
                c.get("y").and_then(Value::as_f64).unwrap_or(0.75),
            )
        })
        .unwrap_or((0.25, 0.75));
    b.move_cursor(x, y).await?;
    a.wait_for(timeout, max, |e| {
        matches!(e, PresenceEvent::CursorMoved { participant_id, cursor }
            if *participant_id == b_id && (cursor.x - x).abs() < 1e-6 && (cursor.y - y).abs() < 1e-6)
    })
    .await?;
    let check_roster = step.get("roster").and_then(Value::as_bool).unwrap_or(false);
    if check_roster {
        // The SDK roster, folded from every event the operator read.
        match a.roster().get(&b_id) {
            Some((p, Some(c)))
                if (p.kind == "agent") == agentb
                    && (c.x - x).abs() < 1e-6
                    && (c.y - y).abs() < 1e-6 => {}
            other => {
                return Err(Error::Invalid(format!(
                    "the operator's roster has {nameb} as {other:?}"
                )));
            }
        }
    }
    // The cursor color and the Bot's avatar color are one color: the app's
    // avatar color, fed from the operator's roster, must be exactly the
    // cursor's; and when the operator took the stable color, the server must
    // have assigned another.
    let mut color_note = String::new();
    if check_roster && agentb {
        let cursor = a
            .roster()
            .get(&b_id)
            .map(|(p, _)| p.color.to_lowercase())
            .unwrap_or_default();
        let avatar = crate::bots::avatar_color(Some(a.roster()), &idb);
        if avatar != cursor {
            return Err(Error::Invalid(format!(
                "{nameb}'s cursor is {cursor}, its avatar is {avatar}"
            )));
        }
        if take_color && avatar == stable_b {
            return Err(Error::Invalid(format!(
                "{namea} holds {stable_b}, yet {nameb} kept it"
            )));
        }
        color_note = if avatar == stable_b {
            format!("; cursor color = avatar color {avatar}")
        } else {
            format!("; cursor color = avatar color {avatar} (reassigned from {stable_b})")
        };
    }
    b.leave().await?;
    a.wait_for(
        timeout,
        max,
        |e| matches!(e, PresenceEvent::Left { participant_id, .. } if *participant_id == b_id),
    )
    .await?;
    if check_roster && a.roster().get(&b_id).is_some() {
        return Err(Error::Invalid(format!(
            "{nameb} is still in the operator's roster after leaving"
        )));
    }
    let seen = a.avatars().len();
    a.leave().await?;
    if ctx.lane.url.is_some() {
        let _ = core_b.spaces().remove(&info_b.id).await;
    }
    Ok(Outcome::Pass(format!(
        "{namea} saw {nameb} join, move to ({x}, {y}) and leave ({seen} avatar left){}{}",
        if check_roster {
            "; the SDK roster showed the cursor, then dropped it"
        } else {
            ""
        },
        color_note
    )))
}

/// The live Bots roster for a `needs: "model"` step, or why it skips.
fn model_bots(ctx: &Ctx) -> std::result::Result<Arc<Bots>, String> {
    let url_env = s(&ctx.model, "urlEnv").unwrap_or("OPENKOALABOTS_SCENARIO_MODEL_URL");
    let Some(url) = std::env::var(url_env).ok().filter(|v| !v.is_empty()) else {
        return Err(format!("no model endpoint ({url_env} is unset)"));
    };
    Ok(Arc::new(Bots::new(
        ctx.core.clone(),
        AgentOptions {
            base_url: Some(url),
            model: s(&ctx.model, "name").map(str::to_string),
            env_from_host: s(&ctx.model, "keyVar")
                .map(str::to_string)
                .into_iter()
                .collect(),
        },
    )))
}

fn bot_info(ctx: &Ctx, v: &Value, agent: &str) -> BotInfo {
    BotInfo {
        id: ctx.fill(s(v, "id").unwrap_or("bot-{nonce}")),
        name: s(v, "name").unwrap_or("Bot").to_string(),
        agent: agent.to_string(),
        space: ctx.space_id.clone().unwrap_or_default(),
    }
}

async fn routine(ctx: &mut Ctx, step: &Value) -> Result<Outcome> {
    let bots = match model_bots(ctx) {
        Ok(b) => b,
        Err(why) => return Ok(Outcome::Skip(why)),
    };
    let r = routine_checks(ctx, step, &bots).await;
    bots.stop_all().await;
    r
}

async fn routine_checks(ctx: &mut Ctx, step: &Value, bots: &Arc<Bots>) -> Result<Outcome> {
    let bot = bot_info(
        ctx,
        step.get("bot").unwrap_or(&Value::Null),
        s(step, "agent").unwrap_or("claude-code"),
    );
    bots.upsert(vec![bot.clone()]).await;
    let spec = step.get("routine").cloned().unwrap_or(Value::Null);
    let schedule: Schedule =
        serde_json::from_value(spec.get("schedule").cloned().unwrap_or(Value::Null))?;
    let file = ctx.root.path().join(format!("routines-{}.json", ctx.nonce));
    let mut store = RoutineStore::new(Box::new(FileStorage(file.clone())));
    store.attach(bots.clone());
    let ms = |k: &str, d: u64| chrono::Duration::milliseconds(u(step, k, d) as i64);
    let created = chrono::Utc::now() - ms("createdAgoMs", 61_000);
    let title = ctx.fill(s(&spec, "title").unwrap_or("Routine"));
    let r = store.create(
        &bot.id,
        &title,
        &ctx.fill(s(&spec, "prompt").unwrap_or("")),
        schedule,
        true,
        created,
    );
    let early = store.tick(r.created_at + ms("notDueAtMs", 30_000)).await;
    if !early.is_empty() {
        return Err(Error::Invalid(format!("fired before its slot: {early:?}")));
    }
    // The real scheduler loop.
    let shared = Arc::new(tokio::sync::Mutex::new(store));
    let interval = Duration::from_millis(u(step, "schedulerIntervalMs", 250));
    let handle = cua_spaces::routines::spawn_scheduler(shared.clone(), interval);
    let poll = Duration::from_millis(u(step, "pollMs", 1000));
    let polls = u(step, "maxPolls", 600) as u32;
    let mut fired = false;
    for _ in 0..(polls.min(1200)) {
        if shared.lock().await.log.iter().any(|l| l.routine_id == r.id) {
            fired = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A few more scheduler ticks: the routine must not fire again (no
    // backlog). Checked now, before the first turn's harness install can
    // outlast the next slot.
    tokio::time::sleep(interval * 4).await;
    handle.abort();
    if !fired {
        return Err(Error::Timeout(
            "the scheduler never fired the routine".into(),
        ));
    }
    let mut store = shared.lock().await;
    let records: Vec<_> = store
        .log
        .iter()
        .filter(|l| l.routine_id == r.id)
        .cloned()
        .collect();
    let run_id = match records.as_slice() {
        [one] => match &one.firing {
            RoutineFiring::Started { run_id } => run_id.clone(),
            other => {
                return Err(Error::Invalid(format!(
                    "the firing was {}",
                    other.summary()
                )));
            }
        },
        many => {
            return Err(Error::Invalid(format!(
                "fired {} times, not once",
                many.len()
            )));
        }
    };
    let fired_at = store
        .routine(&r.id)
        .and_then(|x| x.last_fired_at)
        .unwrap_or_else(chrono::Utc::now);
    let again = store.tick(fired_at + chrono::Duration::seconds(1)).await;
    if !again.is_empty() {
        return Err(Error::Invalid(format!(
            "a second tick fired again: {again:?}"
        )));
    }
    let reloaded = RoutineStore::new(Box::new(FileStorage(file)));
    let saved = reloaded
        .routine(&r.id)
        .ok_or_else(|| Error::Invalid("the routine was not saved".into()))?;
    if saved.last_run_id.as_deref() != Some(run_id.as_str()) || saved.last_fired_at.is_none() {
        return Err(Error::Invalid(format!("saved as {saved:?}")));
    }
    if !reloaded
        .due(fired_at + chrono::Duration::seconds(1))
        .is_empty()
    {
        return Err(Error::Invalid("the reloaded routine is still due".into()));
    }
    drop(store);
    let expect = ctx.fill(s(step, "expectOutput").unwrap_or(""));
    let view = bots.wait_turn(&bot.id, Some(&expect), poll, polls).await?;
    let want_prompt = ctx.fill(s(step, "expectPrompt").unwrap_or("[routine]"));
    let prompt = view
        .transcript
        .iter()
        .find(|l| l.speaker == crate::thread::Speaker::User)
        .map(|l| l.text.clone())
        .unwrap_or_default();
    if !prompt.starts_with(&want_prompt) {
        return Err(Error::Invalid(format!("the run's prompt was {prompt:?}")));
    }
    Ok(Outcome::Pass(format!(
        "not due at +{}s; the scheduler fired it once ({run_id}), \"{}\"; no backlog; reloaded with lastRunID",
        u(step, "notDueAtMs", 30_000) / 1000,
        crate::bots::latest_reply(&view.transcript).unwrap_or_default()
    )))
}

async fn group(ctx: &mut Ctx, step: &Value) -> Result<Outcome> {
    let bots = match model_bots(ctx) {
        Ok(b) => b,
        Err(why) => return Ok(Outcome::Skip(why)),
    };
    let r = group_checks(ctx, step, &bots).await;
    bots.stop_all().await;
    r
}

async fn group_checks(ctx: &mut Ctx, step: &Value, bots: &Arc<Bots>) -> Result<Outcome> {
    let agent = s(step, "agent").unwrap_or("claude-code");
    let members: Vec<BotInfo> = step
        .get("bots")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|b| bot_info(ctx, b, agent)).collect())
        .unwrap_or_default();
    let title = ctx.fill(s(step, "title").unwrap_or("Group"));
    for n in step
        .get("rejectSizes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let n = n.as_u64().unwrap_or(0) as usize;
        let ids: Vec<String> = (0..n).map(|i| format!("bot-{i}")).collect();
        match GroupChat::new(&title, &ids) {
            Err(GroupChatError::TooFewBots(_)) if n < cua_spaces::groups::MIN_BOTS => {}
            Err(GroupChatError::TooManyBots(_)) if n > cua_spaces::groups::MAX_BOTS => {}
            other => return Err(Error::Invalid(format!("a group of {n}: {other:?}"))),
        }
    }
    bots.upsert(members.clone()).await;
    let mut store = GroupChatStore::new();
    store.attach(bots.clone());
    let ids: Vec<String> = members.iter().map(|b| b.id.clone()).collect();
    let chat = store
        .create(&title, &ids)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let message = ctx.fill(s(step, "message").unwrap_or(""));
    let ds = store.send(&message, &chat.id).await;
    if ds.len() != members.len() || ds.iter().any(|d| !d.accepted) {
        return Err(Error::Invalid(format!("deliveries: {ds:?}")));
    }
    // Each member was framed with the room: the others' names.
    for b in &members {
        let first = bots
            .view(&b.id)
            .await
            .and_then(|v| {
                v.transcript
                    .into_iter()
                    .find(|l| l.speaker == crate::thread::Speaker::User)
            })
            .map(|l| l.text)
            .unwrap_or_default();
        let others_named = members
            .iter()
            .filter(|o| o.id != b.id)
            .all(|o| first.contains(&o.name));
        if !first.starts_with(&format!("[group:{title}]")) || !others_named {
            return Err(Error::Invalid(format!("{} was sent {first:?}", b.name)));
        }
    }
    let expect = ctx.fill(s(step, "expectReply").unwrap_or(""));
    let poll = Duration::from_millis(u(step, "pollMs", 1000));
    let polls = u(step, "maxPolls", 600);
    let replied = |store: &GroupChatStore| -> Vec<String> {
        let c = store.chat(&chat.id).expect("the chat");
        members
            .iter()
            .filter(|b| {
                c.messages.iter().any(|l| {
                    l.speaker
                        == GroupSpeaker::Bot {
                            bot_id: b.id.clone(),
                        }
                        && !l.undelivered
                        && l.text.contains(&expect)
                })
            })
            .map(|b| b.name.clone())
            .collect()
    };
    for _ in 0..polls {
        store.collect_replies(&chat.id).await;
        if replied(&store).len() == members.len() {
            break;
        }
        tokio::time::sleep(poll).await;
    }
    let got = replied(&store);
    if got.len() != members.len() {
        let lines: Vec<_> = store
            .chat(&chat.id)
            .map(|c| c.messages.iter().map(|l| l.text.clone()).collect())
            .unwrap_or_default();
        return Err(Error::Timeout(format!(
            "replies from {got:?} only; transcript {lines:?}"
        )));
    }
    let again = store.collect_replies(&chat.id).await;
    if !again.is_empty() {
        return Err(Error::Invalid(format!("collecting again added {again:?}")));
    }
    Ok(Outcome::Pass(format!(
        "1 and 7 refused; {} ({}) got one message, framed with the room; replies attributed to {} once each",
        title,
        store
            .chat(&chat.id)
            .map(|c| c.membership_label())
            .unwrap_or_default(),
        got.join(" and ")
    )))
}

async fn delete(ctx: &mut Ctx) -> Result<Outcome> {
    let id = ctx
        .space_id
        .take()
        .ok_or_else(|| Error::Invalid("nothing to delete".into()))?;
    ctx.space = None;
    let what = ctx.core.delete_space(&id).await?;
    ctx.space_id = Some(id);
    Ok(Outcome::Pass(what))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_parses_and_names_every_op_this_runner_knows() {
        let spec: Value = serde_json::from_str(include_str!(
            "../../../../openkoalabot-example-scenario/scenario.json"
        ))
        .unwrap();
        let ops: Vec<&str> = spec["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["op"].as_str().unwrap())
            .collect();
        for op in &ops {
            assert!(
                [
                    "space.open",
                    "stream.desktop",
                    "agent.thread",
                    "file.send",
                    "teleport.app",
                    "presence.pair",
                    "routine.schedule",
                    "group.chat",
                    "space.delete"
                ]
                .contains(op),
                "{op}"
            );
        }
        assert_eq!(ops.first(), Some(&"space.open"));
        assert_eq!(ops.last(), Some(&"space.delete"));
    }
}
