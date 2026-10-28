// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app core's parity flows against the web UI.
 *
 * Each flow whose screen exists here runs in the browser, in demo mode with
 * the wasm core the page loads: `window.__cuaParity.replay` sends every core
 * call the bridge makes for a screen through the bridge's own functions, and
 * the transcript must equal the golden. Then every state the replay reached
 * for that screen goes on the real screen through the store, and the screen
 * must show it. Flows for screens not built yet are skipped with a reason,
 * and flows for native-only surfaces are marked `native` (`flows.ts`). The
 * goldens are read, never written.
 */

import { readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

import type { Locator, Page } from "@playwright/test";

import { expect, test } from "./host";

import type { ParityCheckpoint, ParityReplay } from "../src/bridge/parity";
import { panelFacts, railWindows } from "../src/components/space-thread/rail";
import { FLOW_PLAN, type ParityScreen } from "./flows";
import { checkNewSpace } from "./new-space";
import { checkShare, checkTeleport } from "./teleport";
import { checkVolumeFlow } from "./volume";
import { SETTINGS_CHECKS } from "./settings-checks";

const PARITY_DIR = fileURLToPath(new URL("../../../libs/cua/crates/cua-spaces-app-core/parity/", import.meta.url));
const ON_DISK = readdirSync(PARITY_DIR)
  .filter((f) => f.endsWith(".json"))
  .map((f) => f.slice(0, -".json".length))
  .sort();

const golden = (name: string): unknown => JSON.parse(readFileSync(`${PARITY_DIR}golden/${name}.json`, "utf8"));

const PATHS: Record<ParityScreen, string> = {
  spaces: "/spaces",
  keyvault: "/keyvault",
  settings: "/settings",
  "settings/general": "/settings/general",
  "settings/about": "/settings/about",
  "settings/advanced": "/settings/advanced",
  "settings/connected-apps": "/settings/connectors",
  notifications: "/notifications",
  machines: "/machines",
  agents: "/agents/all",
  volume: "/volume",
  onboarding: "/onboarding",
  // A detail opens from Spaces once the registry has loaded.
  "space-detail": "/spaces",
  // New Space, Teleport and Share open over the Spaces page.
  "new-space": "/spaces",
  teleport: "/spaces",
  share: "/spaces",
};

async function open(page: Page, screen: ParityScreen): Promise<void> {
  await page.goto(`${PATHS[screen]}?bridge=demo&parity`);
  await page.waitForFunction(() => window.__cuaParity !== undefined);
  expect(await page.evaluate(() => window.__cuaParity!.coreStatus), "the wasm core loaded").toBe("ready");
  // Let the host's first answer land, so it can't overwrite a checkpoint.
  if (PATHS[screen] === "/spaces") await page.locator("[data-space-id]").first().waitFor();
  if (screen === "onboarding") await page.locator("[data-step]").first().waitFor();
  if (screen === "machines") await page.locator('[data-machine-detail="this-mac"]').waitFor();
  if (screen === "agents") await page.locator("[data-agent-row]").first().waitFor();
}

const navigate = (page: Page, path: string) => page.evaluate((p) => window.__cuaParity!.navigate(p), path);

/* ---- Spaces ---------------------------------------------------------------- */

type SpacesCheckpoint = Extract<ParityCheckpoint, { kind: "spaces" }>;

interface Tile {
  id: string;
  name: string;
  state: string;
  progress: string | null;
}

/** What the Spaces screen must draw for the core's Spaces: the state each
 * status maps to, and the create bar at the core's permille. */
function expectedTiles(cp: SpacesCheckpoint): Tile[] {
  return cp.spaces
    .map((s) => {
      const state =
        s.status === "provisioning"
          ? "creating"
          : s.status === "deleting"
            ? "deleting"
            : s.status === "suspended" && s.progress?.error
              ? "failed" // a failed create: its row keeps why
              : s.status === "suspended" || s.power?.off
                ? "stopped"
                : "running";
      return {
        id: s.id,
        name: s.name,
        state,
        progress: state === "creating" ? `${Math.round((s.progress?.permille ?? 0) / 10)}%` : null,
      };
    })
    .sort((a, b) => a.id.localeCompare(b.id));
}

const readTiles = (page: Page): Promise<Tile[]> =>
  page.locator("[data-space-id]").evaluateAll((els) =>
    els
      .map((el) => ({
        id: el.getAttribute("data-space-id") ?? "",
        name: el.querySelector("[data-space-name]")?.textContent ?? "",
        state: el.getAttribute("data-state") ?? "",
        progress: (el.querySelector("[data-create-progress]") as HTMLElement | null)?.style.width ?? null,
      }))
      .sort((a, b) => a.id.localeCompare(b.id)),
  );

async function checkSpaces(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  const seen = new Set<string>();
  for (const cp of checkpoints) {
    if (cp.kind !== "spaces") continue;
    const want = expectedTiles(cp);
    const key = JSON.stringify(want);
    if (seen.has(key)) continue;
    seen.add(key);
    await page.evaluate(([registry, creates]) => window.__cuaParity!.showSpaces(registry, creates), [cp.registry, cp.creates] as const);
    await expect.poll(() => readTiles(page), { message: `Spaces screen for ${cp.spaces.length} Spaces` }).toEqual(want);
  }
  return seen.size;
}

/* ---- Keyvault -------------------------------------------------------------- */

async function checkKeyvault(page: Page, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  // The page's own first read lands first, so it can't replace a state shown below.
  await expect(page.locator("[data-vault-summary]")).toBeVisible();
  for (const cp of checkpoints) {
    if (cp.kind === "keyvault-page") {
      const unlock = page.getByRole("button", { name: "Unlock", exact: true });
      const setUp = page.getByRole("button", { name: cp.page.labels.setUp, exact: true });
      // A host refetch that lands late (the Electron shell's IPC on a busy
      // machine) can replace the state shown, so it is shown again until the
      // page draws it.
      await expect(async () => {
        await page.evaluate((overview) => window.__cuaParity!.showKeyvault(overview), cp.overview);
        // The page offers Unlock exactly when the core says it can unlock...
        if (cp.page.canUnlock) await expect(unlock).toBeVisible({ timeout: 1_000 });
        else await expect(unlock).toHaveCount(0, { timeout: 1_000 });
        // ...and Set up Keyvault exactly when the core says there is no vault yet.
        if (cp.page.canSetup) await expect(setUp).toBeVisible({ timeout: 1_000 });
        else await expect(setUp).toHaveCount(0, { timeout: 1_000 });
      }).toPass({ timeout: 10_000 });
      checked++;
    } else if (
      cp.kind === "keyvault-vault" &&
      cp.overview.availability !== "no_vault" &&
      cp.state.query === "" &&
      cp.state.selected.length === 0 &&
      !cp.state.app
    ) {
      await page.evaluate((overview) => window.__cuaParity!.showKeyvault(overview), cp.overview);
      // The list holds what the core's vault view holds: its items, under its apps.
      const apps = cp.vault.apps.map((a) => a.name).sort();
      await expect(page.locator("[data-vault-summary]")).toHaveText(`${cp.vault.total} items in ${apps.length} apps`);
      await expect
        .poll(() => page.locator("[data-vault-app]").evaluateAll((els) => els.map((el) => el.getAttribute("data-vault-app")).sort()))
        .toEqual(apps);
      checked++;
    }
  }
  return checked;
}

/* ---- A Space's detail: facts and Stream section ------------------------------ */

type Of<K extends ParityCheckpoint["kind"]> = Extract<ParityCheckpoint, { kind: K }>;

/**
 * Where a Space's full detail is drawn. The classic page lists it under
 * the desktop; the call layout (a daemon with the Space thread) shows the
 * first facts and windows in its right rail and the rest in Details ›
 * More, so the flows open More and read the same lists there.
 */
async function detailRoot(page: Page): Promise<Page | Locator> {
  // The page settles on one layout once the thread answers (or does not).
  await expect(page.locator("[data-call-layout], [data-section], [data-fact]").first()).toBeVisible();
  if ((await page.locator("[data-call-layout]").count()) === 0) return page;
  const sheet = page.locator("[data-call-more-sheet]");
  // Pressed directly: a sheet the flow left open over the page must not hide it.
  if ((await sheet.count()) === 0) await page.locator("[data-call-more]").dispatchEvent("click");
  await expect(sheet).toBeVisible();
  return sheet;
}

/** Closes Details › More when the call layout opened it. */
async function closeDetail(page: Page): Promise<void> {
  if (await page.locator("[data-call-more-sheet]").count()) {
    await page.locator("[data-call-more-close]").dispatchEvent("click");
    await expect(page.locator("[data-call-more-sheet]")).toHaveCount(0);
  }
}

/** The rail's facts in the call layout: label and value, as the page draws them. */
const readRailFacts = (page: Page) =>
  page.locator("[data-rail-fact]").evaluateAll((els) =>
    els.map((el) => ({ label: el.getAttribute("data-rail-fact"), value: el.querySelector("[data-rail-fact-value]")?.textContent ?? null })),
  );

/** Each fact as the page draws it: label, value, tooltip, warning, copy button. */
const readFacts = (root: Page | Locator) =>
  root.locator("[data-fact]").evaluateAll((els) =>
    els.map((el) => {
      const copy = el.querySelector("[data-copy-text]");
      return {
        label: el.getAttribute("data-fact"),
        value: el.querySelector("[data-fact-value]")?.textContent ?? null,
        help: el.querySelector("[data-help]")?.getAttribute("data-help") ?? null,
        warning: el.querySelector("[data-fact-warning]")?.getAttribute("data-fact-warning") ?? null,
        copy: copy ? `${copy.getAttribute("data-copy-text")} | ${copy.getAttribute("aria-label")}` : null,
      };
    }),
  );

async function checkSpaceDetail(page: Page, cp: Of<"space-detail">): Promise<void> {
  await page.evaluate(([s, u, h]) => window.__cuaParity!.showSpaceDetail(s, u, h), [cp.space, cp.usage, cp.hostArch] as const);
  await navigate(page, `/spaces/${encodeURIComponent(cp.space.id)}`);
  const want = cp.detail.facts.map((f) => ({
    label: f.label,
    value: f.value,
    help: f.help && f.help !== f.value ? f.help : null,
    warning: f.warning?.help ?? null,
    copy: f.copy ? `${f.copy.text} | ${f.copy.help}` : null,
  }));
  await expect(page.locator("[data-detail-title]")).toHaveText(cp.detail.title);
  const call = (await page.locator("[data-call-layout]").count()) > 0;
  // The call layout's rail: the facts the shared rule picks, in the core's words.
  if (call) {
    await expect
      .poll(() => readRailFacts(page), { message: `${cp.space.id}: the rail's facts` })
      .toEqual(panelFacts(cp.detail.facts).map((f) => ({ label: f.label, value: f.value })));
  }
  // Every fact, where the person reaches all of them (the page, or Details › More).
  const root = await detailRoot(page);
  await expect.poll(() => readFacts(root), { message: `${cp.space.id}: the core's facts` }).toEqual(want);
  // The Stream section shows exactly when the core lists it.
  await expect(root.locator('[data-section="stream"]')).toHaveCount(cp.detail.sections.includes("Stream") ? 1 : 0);
  await closeDetail(page);
}

const readStream = (root: Page | Locator) =>
  root.locator("[data-stream-row]").evaluateAll((els) =>
    els.map((el) => {
      const label = el.querySelector("[data-row-label]");
      const pip = el.querySelector("[data-pip]");
      return {
        id: el.getAttribute("data-stream-row"),
        kind: el.getAttribute("data-kind"),
        label: label?.textContent ?? null,
        help: label?.getAttribute("title") ?? null,
        resolution: el.querySelector("[data-row-resolution]")?.textContent ?? null,
        pip: pip ? `${pip.getAttribute("aria-label")} ${pip.getAttribute("aria-pressed")}` : null,
      };
    }),
  );

async function checkStream(page: Page, cp: Of<"stream">): Promise<void> {
  const id = await page.evaluate((input) => window.__cuaParity!.showStream(input), cp.input);
  await navigate(page, `/spaces/${encodeURIComponent(id)}`);
  const want = cp.section.rows.map((r) => ({
    id: r.id,
    kind: r.kind,
    label: r.label,
    help: r.help,
    resolution: r.resolution ?? null,
    pip: r.actions[0] ? `${r.actions[0].help} ${r.actions[0].active}` : null,
  }));
  const call = (await page.locator("[data-call-layout]").count()) > 0;
  // The call layout's rail lists the first windows the shared rule picks; a click streams one.
  if (call) {
    await expect
      .poll(() => page.locator("[data-stream-window]").evaluateAll((els) => els.map((el) => el.getAttribute("data-stream-window"))), {
        message: "the rail's windows",
      })
      .toEqual(railWindows(cp.section.rows).map((r) => r.id));
  }
  // Every row, where the person reaches all of them (the page, or Details › More).
  const root = await detailRoot(page);
  await expect.poll(() => readStream(root), { message: "the core's Stream rows" }).toEqual(want);
  const status = root.locator("[data-stream-status]");
  if (cp.section.statusText) await expect(status).toHaveText(cp.section.statusText);
  else await expect(status).toHaveCount(0);
  await expect(root.locator("[data-stream-filter]")).toHaveValue(cp.input.query ?? "");
  await closeDetail(page);
}

/* ---- This machine and its setup form -------------------------------------- */

const texts = (page: Page, selector: string) => page.locator(selector).evaluateAll((els) => els.map((el) => el.textContent ?? ""));
const attrs = (page: Page, selector: string, name: string) =>
  page.locator(selector).evaluateAll((els, n) => els.map((el) => el.getAttribute(n) ?? ""), name);

/** This machine on the Machines page for the core's host state: its two
 * switches (on or off, and whether they change here), the notice over them,
 * the permissions still to grant and Remove from account, as the core's
 * `machines.view` draws This machine (the SwiftUI page draws the same). */
async function checkHostPanel(page: Page, cp: Of<"host-panel">): Promise<void> {
  const p = cp.panel;
  const m = cp.machine!;
  await page.evaluate((state) => window.__cuaParity!.showHost(state), cp.state);
  const detail = page.locator('[data-machine-detail="this-mac"]');
  if (m.notice) await expect(detail.locator("[data-machine-notice]")).toContainText(m.notice);
  else await expect(detail.locator("[data-machine-notice]")).toHaveCount(0);
  // The notice's button (Sign In, Resume Sharing), when it has one.
  await expect.poll(() => texts(page, "[data-machine-notice] button")).toEqual(m.notice && m.noticeAction ? [m.noticeAction.label] : []);
  await expect.poll(() => attrs(page, "[data-machine-toggle]", "data-machine-toggle")).toEqual(m.toggles.map((t) => t.id));
  await expect
    .poll(() => attrs(page, "[data-machine-toggle] [role=switch]", "aria-checked"))
    .toEqual(m.toggles.map((t) => String(t.on)));
  await expect
    .poll(() => attrs(page, "[data-host-permission]", "data-host-permission"))
    .toEqual(p.permissionsTitle ? p.permissions.map((x) => x.id) : []);
  await expect(detail.locator("[data-machine-remove]")).toHaveCount(m.remove ? 1 : 0);
}

const readForm = (page: Page) =>
  page.locator("[data-host-field]").evaluateAll((els) =>
    els.map((el) => {
      const input = el.querySelector("input:not([type=hidden]):not([type=radio])") as HTMLInputElement | null;
      const sw = el.querySelector('[role="switch"]');
      const radios = el.querySelector('[role="radiogroup"]');
      return {
        id: el.getAttribute("data-host-field"),
        // A switch's state, a text field's text, or the checked choice's label.
        value: sw
          ? sw.getAttribute("aria-checked") === "true"
            ? "on"
            : "off"
          : radios
            ? (radios.querySelector("input:checked")?.closest("label")?.textContent ?? "")
            : (input?.value ?? ""),
        invalid: input?.getAttribute("aria-invalid") === "true",
      };
    }),
  );

async function checkHostForm(page: Page, cp: Of<"host-form">): Promise<void> {
  const v = cp.view;
  await page.evaluate(([state, identity]) => window.__cuaParity!.showHostForm(state, identity), [cp.state, cp.identity] as const);
  await expect(page.locator("[data-host-lede]")).toHaveText(v.lede);
  const want = v.fields.map((f) => ({
    id: f.id,
    value: f.toggle ? (f.on ? "on" : "off") : f.choices?.length ? (f.choices.find((c) => c.id === f.value)?.label ?? "") : f.value,
    invalid: f.invalid,
  }));
  await expect.poll(() => readForm(page), { message: "the core's form fields" }).toEqual(want);
  await expect(page.locator("[data-host-advanced]")).toHaveAttribute("data-host-advanced", v.advancedOpen ? "open" : "closed");
  const submit = page.locator("[data-host-submit]");
  await expect(submit).toHaveText(v.submitLabel);
  if (v.canSubmit) await expect(submit).toBeEnabled();
  else await expect(submit).toBeDisabled();
  if (v.error) await expect(page.locator("[data-host-form-error]")).toHaveText(v.error);
  else await expect(page.locator("[data-host-form-error]")).toHaveCount(0);
}

/* ---- The Agents page --------------------------------------------------------- */

async function checkAgents(page: Page, cp: Of<"agents">): Promise<void> {
  await page.evaluate(([agents, nowMs]) => window.__cuaParity!.showAgents(agents, nowMs), [cp.agents, cp.nowMs] as const);
  const read = () =>
    page.locator("[data-agent-row]").evaluateAll((els) =>
      els.map((el) => ({
        name: el.getAttribute("data-agent-row"),
        detail: el.querySelector("[data-row-detail]")?.textContent ?? null,
        state: el.querySelector("[data-row-state]")?.textContent ?? null,
      })),
    );
  await expect.poll(read, { message: "the core's agent rows" }).toEqual(cp.rows.map((r) => ({ name: r.name, detail: r.detail, state: r.state })));
}

/* ---- The Machines page --------------------------------------------------------- */

async function checkMachines(page: Page, cp: Of<"machines">): Promise<void> {
  await page.evaluate(([rows, now]) => window.__cuaParity!.showMachines(rows, now), [cp.rows, cp.now] as const);
  const read = () =>
    page.locator("[data-machine-id]").evaluateAll((els) =>
      els.map((el) => ({
        id: el.getAttribute("data-machine-id"),
        name: el.querySelector(".font-medium")?.textContent ?? null,
        online: el.getAttribute("data-online") === "true",
      })),
    );
  await expect.poll(read, { message: "the core's machine rows" }).toEqual(cp.merged.map((m) => ({ id: m.id, name: m.name, online: m.online })));
}

/* ---- Usage events ------------------------------------------------------------- */

const tracked = (page: Page) => page.evaluate(() => window.__cuaParity!.tracked());
const track = (page: Page, signals: unknown[]) => page.evaluate((s) => window.__cuaParity!.track(s as never), signals);

/** Sends every step's events through the bridge: the host records exactly
 * the core's, in order; with usage data off it records none; and nothing
 * but fixed words leaves the page. */
async function checkTelemetry(page: Page, cps: Of<"telemetry">[]): Promise<number> {
  // The page recorded its own launch (the demo host is a shell that doesn't),
  // saying whether the first run is due (the demo session finished it).
  await expect.poll(() => tracked(page)).toContainEqual({ type: "launched", onboardingEligible: false });
  const before = (await tracked(page)).length;
  for (const cp of cps) await track(page, cp.signals);
  const ours = (await tracked(page)).slice(before);
  expect(ours, "the host recorded the core's events").toEqual(cps.flatMap((c) => c.signals));

  await page.evaluate(() => window.__cuaParity!.setTelemetry(false));
  const off = (await tracked(page)).length;
  for (const cp of cps) await track(page, cp.signals);
  expect((await tracked(page)).length, "nothing is sent with usage data off").toBe(off);
  await page.evaluate(() => window.__cuaParity!.setTelemetry(true));

  const n = (await tracked(page)).length;
  await track(page, [
    { type: "step", step: "signed_in", ok: true, identity: "maya@example.com" },
    { type: "feature", feature: "/Users/maya/secret" },
    { type: "share", action: "share", role: "viewer", outcome: "ok", errorKind: "none", failedStage: "none", email: "grace@example.com" },
  ]);
  expect((await tracked(page)).slice(n), "only schema fields and fixed words are sent").toEqual([
    { type: "step", step: "signed_in", ok: true },
    { type: "share", action: "share", role: "viewer", outcome: "ok", errorKind: "none", failedStage: "none" },
  ]);
  return cps.filter((c) => c.signals.length > 0).length;
}

/** Every state the replay reached that a web screen draws, on that screen. */
async function checkScreens(page: Page, screen: ParityScreen, checkpoints: ParityCheckpoint[]): Promise<number> {
  let checked = 0;
  if (screen === "spaces" || screen === "space-detail") checked += await checkSpaces(page, checkpoints);
  if (screen === "keyvault") checked += await checkKeyvault(page, checkpoints);
  if (screen === "new-space") checked += await checkNewSpace(page, checkpoints);
  const seen = new Set<string>();
  const once = (cp: ParityCheckpoint) => {
    const key = JSON.stringify(cp);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  };
  // This machine first (the page it opens on), then the details it navigates to.
  for (const cp of checkpoints) {
    if (cp.kind === "host-panel" && once(cp)) (await checkHostPanel(page, cp), checked++);
  }
  for (const cp of checkpoints) {
    if (cp.kind === "host-form" && once(cp)) (await checkHostForm(page, cp), checked++);
  }
  for (const cp of checkpoints) {
    if (cp.kind === "agents" && once(cp)) (await checkAgents(page, cp), checked++);
  }
  for (const cp of checkpoints) {
    if (cp.kind === "machines" && once(cp)) (await checkMachines(page, cp), checked++);
  }
  const telemetry = checkpoints.filter((c): c is Of<"telemetry"> => c.kind === "telemetry");
  if (telemetry.length) checked += await checkTelemetry(page, telemetry);
  for (const cp of checkpoints) {
    if (cp.kind === "space-detail" && once(cp)) (await checkSpaceDetail(page, cp), checked++);
    else if (cp.kind === "stream" && once(cp)) (await checkStream(page, cp), checked++);
  }
  return checked;
}

/* ---- The flows ------------------------------------------------------------- */

test.describe("parity flows", () => {
  for (const name of ON_DISK) {
    test(name, async ({ page }) => {
      const plan = FLOW_PLAN[name];
      expect(plan, `classify ${name} in e2e/flows.ts`).toBeDefined();
      if (plan!.status === "native") {
        // Covered by the Swift parity runner; reported apart from skips.
        test.info().annotations.push({ type: "native", description: plan!.reason });
        test.skip(true, plan!.reason);
        return;
      }
      if (plan!.status === "skip") {
        test.skip(true, plan!.reason);
        return;
      }
      const { screen } = plan!;
      await open(page, screen);

      const replay: ParityReplay = await page.evaluate((n) => window.__cuaParity!.replay(n), name);
      expect(replay.golden, "the page's wasm carries the golden on disk (rebuild with pnpm core)").toEqual(golden(name));
      expect(replay.bridged.length, "the flow goes through the bridge").toBeGreaterThan(0);
      expect(replay.transcript, `${name} transcript`).toEqual(replay.golden);

      // Settings flows by name; the Teleport and Share sheets; Volume, onboarding and the
      // Settings driver card through the Volume checker; the rest by checkpoint kind.
      const cps = replay.checkpoints;
      const settingsCheck = SETTINGS_CHECKS[name];
      const states = settingsCheck
        ? await settingsCheck(page, cps)
        : screen === "teleport"
          ? await checkTeleport(page, cps)
          : screen === "share"
            ? await checkShare(page, cps)
            : screen === "volume" || screen === "onboarding" || screen === "settings/connected-apps"
              ? await checkVolumeFlow(page, name, cps)
              : await checkScreens(page, screen, cps);
      expect(states, "the replay reached a state the screen draws").toBeGreaterThan(0);
      test.info().annotations.push({
        type: "parity",
        description: `${screen} screen, ${states} states checked; bridge answered ${replay.bridged.join(", ")}`,
      });
    });
  }
});

test("the page's core and e2e/flows.ts name every flow on disk", async ({ page }) => {
  await open(page, "spaces");
  const inCore = await page.evaluate(() => window.__cuaParity!.flows());
  expect([...inCore].sort()).toEqual(ON_DISK);
  expect(Object.keys(FLOW_PLAN).sort()).toEqual(ON_DISK);
});
