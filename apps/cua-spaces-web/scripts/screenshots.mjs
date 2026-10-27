// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Captures every route in light and dark at 1440x900 into docs/screenshots.
// Usage: pnpm build && pnpm screenshots   (BASE_URL=... to use a running server,
// ONLY=new-space for just the New Space wizard and Connect a cloud,
// ONLY=teleport for the Teleport and Share sheets, ONLY=thread for the Space
// thread's call layout at the storyboards' 1500x940, ONLY=permissions for
// agents' rights, ONLY=identity for agents' marks, owners and cards, ONLY=add-agent for
// Add an agent, ONLY=agents-chat for the Agents tab; OUT_DIR=... to save elsewhere)
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

const root = fileURLToPath(new URL("..", import.meta.url));
const outDir = process.env.OUT_DIR ?? `${root}docs/screenshots`;
const ROUTES = ["spaces", "machines", "agents", "volume", "keyvault", "settings", "settings/devices", "settings/experiments", "settings/about", "notifications", "onboarding"];
// Pages under a parameter: [path, file name].
const DETAIL_ROUTES = [
  [`spaces/${encodeURIComponent("relay:mac-mini/qa-windows")}`, "space-detail"],
  [`spaces/${encodeURIComponent("local:agent-sandbox")}`, "space-detail-stopped"],
];
const THEMES = ["light", "dark"];

/** The first run, page by page, from a fresh demo (signed out, not onboarded). */
async function onboardingSteps(theme) {
  // Reduced motion: the miniatures hold the core's still.
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: theme, reducedMotion: "reduce" });
  await context.addInitScript((t) => localStorage.setItem("cua-spaces:theme", t), theme);
  const page = await context.newPage();
  const shot = async (name) => {
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${outDir}/onboarding-${name}-${theme}.png` });
  };
  await page.goto(`${base}/onboarding?demo=fresh`, { waitUntil: "networkidle" });
  await shot("welcome");
  await page.getByRole("button", { name: "Get started" }).click();
  await shot("signin");
  await page.getByRole("button", { name: "Skip", exact: true }).click();
  await page.locator("[data-agents-list]").waitFor();
  await shot("agents");
  await page.getByRole("button", { name: "Set up", exact: true }).click();
  await page.locator("[data-driver-summaries]").waitFor();
  await shot("agents-done");
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await page.locator("[data-drive-card]").waitFor();
  await shot("volume");
  await page.getByRole("button", { name: "Your S3 bucket" }).click();
  await shot("volume-bucket");
  await page.getByRole("button", { name: "This Mac" }).click();
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await shot("this-machine");
  await page.getByTestId("onboarding-client").click();
  await page.getByRole("button", { name: "Continue", exact: true }).click();
  await shot("done");
  await context.close();
}

/** "Teleport an app" and "Share", driven through the real sheets. */
async function teleportSheets(theme) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: theme });
  await context.addInitScript((t) => localStorage.setItem("cua-spaces:theme", t), theme);
  const page = await context.newPage();
  const shot = async (name) => {
    await page.waitForTimeout(350);
    await page.screenshot({ path: `${outDir}/${name}-${theme}.png` });
  };
  await page.goto(`${base}/spaces/${encodeURIComponent("local:design-review")}`, { waitUntil: "networkidle" });
  await page.getByRole("button", { name: "Teleport an app" }).click();
  await page.locator("[data-teleport][data-step=pick]").waitFor();
  await shot("teleport-picker");
  await page.locator("[data-tile='com.google.Chrome']").dblclick();
  await page.getByText("The app with its signed-in state").click();
  await page.getByText("Keep me signed in").click();
  await shot("teleport-sign-ins");
  await page.locator("[data-teleport-plan]").click();
  await page.locator("[data-review-site]").first().waitFor();
  await shot("teleport-review");
  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Share", exact: true }).first().click();
  await page.locator("[data-share-row]").first().waitFor();
  await shot("share-sheet");
  await context.close();
}

/** New Space step by step, the Run on menu, and Connect a cloud. */
async function newSpaceSteps(theme) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: theme });
  await context.addInitScript((t) => localStorage.setItem("cua-spaces:theme", t), theme);
  const page = await context.newPage();
  const shot = async (name) => {
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${outDir}/new-space-${name}-${theme}.png` });
  };
  const next = () => page.locator("[data-wizard-primary]").click();
  await page.goto(`${base}/spaces`, { waitUntil: "networkidle" });
  await page.getByRole("button", { name: "New Space" }).first().click();
  await shot("system");
  await page.locator("[data-image-input]").click();
  await shot("images");
  await page.keyboard.press("Escape");
  await page.locator("[data-run-on]").click();
  await shot("run-on");
  await page.keyboard.press("Escape");
  await page.locator("[data-tile='os:macos']").click();
  await next();
  await page.locator("[data-gpu]").click();
  await shot("resources");
  await next();
  await page.locator("[data-name-input]").fill("release-mac");
  await next();
  await shot("summary");
  await page.locator("[data-wizard-back]").click();
  await page.locator("[data-wizard-back]").click();
  await page.locator("[data-wizard-back]").click();
  await page.locator("[data-field='connect-cloud']").click();
  await page.locator("#cc-region").fill("us-west-2");
  await page.locator("[data-cloud-test]").click();
  await page.locator("[data-cloud-result]").waitFor();
  await page.screenshot({ path: `${outDir}/connect-cloud-${theme}.png` });
  await page.locator("[data-cloud-connect]").click();
  await page.locator("[data-connect-cloud]").waitFor({ state: "detached" });
  await page.locator("[data-run-on]").click();
  await shot("run-on-cloud");
  await page.keyboard.press("Escape");
  await page.locator("[data-wizard-cancel]").click();
  // Create Space: the tile shows its progress.
  await page.getByRole("button", { name: "New Space" }).first().click();
  await next();
  await next();
  await next();
  await next();
  await page.waitForTimeout(1200);
  await page.screenshot({ path: `${outDir}/new-space-creating-${theme}.png` });
  await context.close();
}

/** Devices' approval sheet, then General with Storage (the Cua Volume experiment on). */
async function settingsStates(page, theme) {
  await page.goto(`${base}/settings/devices`, { waitUntil: "networkidle" });
  await page.locator('[data-device-row="dev_work"] [data-device-approve]').click();
  await page.waitForTimeout(300);
  await page.screenshot({ path: `${outDir}/settings-devices-approve-${theme}.png` });
  await page.keyboard.press("Escape");
  await page.goto(`${base}/settings/experiments`, { waitUntil: "networkidle" });
  // The demo starts with Cua Volume on (the Volume page's demo); turn it on only if it's off.
  const volumeSwitch = page.locator('[data-setting-row="experiment:cua_volume"] [data-slot="switch"]');
  if ((await volumeSwitch.getAttribute("aria-checked")) !== "true") await volumeSwitch.click();
  await page.locator('nav[aria-label="Settings"]').getByRole("link", { name: "General", exact: true }).click();
  await page.locator('[data-settings-section="storage"]').waitFor();
  await page.waitForTimeout(250);
  await page.locator('[data-settings-section="storage"]').scrollIntoViewIfNeeded();
  await page.screenshot({ path: `${outDir}/settings-storage-${theme}.png` });
}

/** This machine's buttons, then the page before setup (after Remove host
 * setup) and the setup form. */
async function thisMachineSetup(theme) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: theme });
  await context.addInitScript((t) => localStorage.setItem("cua-spaces:theme", t), theme);
  const page = await context.newPage();
  await page.goto(`${base}/machines`, { waitUntil: "networkidle" });
  await page.locator("[data-host-action]").first().scrollIntoViewIfNeeded();
  await page.waitForTimeout(250);
  await page.screenshot({ path: `${outDir}/this-machine-${theme}.png` });
  await page.locator('[data-host-action="remove"]').click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Remove" }).click();
  await page.locator("[data-host-choice]").first().waitFor();
  await page.waitForTimeout(250);
  await page.screenshot({ path: `${outDir}/this-machine-not-set-up-${theme}.png` });
  await page.locator("[data-host-choice] button").first().click();
  await page.locator("[data-host-form]").scrollIntoViewIfNeeded();
  await page.waitForTimeout(250);
  await page.screenshot({ path: `${outDir}/this-machine-setup-${theme}.png` });
  await context.close();
}

/** The Space thread (CUA-2155) on the demo's ubuntu-build: the call layout,
 * the @ picker, the thread given more room, the members folded, Details'
 * More, and a daemon without the thread (the classic page). */
async function threadShots(theme) {
  const context = await browser.newContext({ viewport: { width: 1500, height: 940 }, deviceScaleFactor: 2, colorScheme: theme, reducedMotion: "reduce" });
  await context.addInitScript((t) => {
    localStorage.setItem("cua-spaces:theme", t);
    localStorage.removeItem("cua-spaces:call-split");
  }, theme);
  const page = await context.newPage();
  const shot = async (name) => {
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${outDir}/thread-${name}-${theme}.png` });
  };
  const space = `${base}/spaces/${encodeURIComponent("relay:linux-box/ubuntu-build")}`;
  await page.goto(space, { waitUntil: "networkidle" });
  await page.locator("[data-task]").first().waitFor();
  await shot("call");
  // More room for the thread: the divider dragged up.
  const divider = page.locator("[data-call-divider]");
  const box = await divider.boundingBox();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, 300, { steps: 8 });
  await page.mouse.up();
  await shot("expanded");
  await page.locator("[data-composer]").click();
  await page.keyboard.type("@");
  await page.locator("[data-mention-picker]").waitFor();
  await shot("mention");
  await page.keyboard.press("Enter");
  await page.keyboard.type("run the e2e suite and post the failures here");
  await shot("composing");
  await page.locator("[data-composer]").fill("");
  await page.locator("[data-members-fold]").first().click();
  await shot("folded");
  await page.locator("[data-members-fold]").first().click();
  await page.locator("[data-call-more]").click();
  await page.locator("[data-call-more-sheet]").waitFor();
  await shot("more");
  await page.keyboard.press("Escape");
  await page.goto(`${space}?demo=nothread`, { waitUntil: "networkidle" });
  await page.locator("[data-detail-title]").waitFor();
  await shot("old-daemon");
  await context.close();
}

/** Who can task, stop and approve each agent (CUA-2156) on the demo's
 * ubuntu-build: the owner editing Hermes's rights, a member (Grace owns the
 * Space: "Runs on Grace's keys", approvals they can't answer, rights
 * read-only), and a guest's view-only composer. */
async function permissionShots(theme) {
  const context = await browser.newContext({ viewport: { width: 1500, height: 940 }, deviceScaleFactor: 2, colorScheme: theme, reducedMotion: "reduce" });
  await context.addInitScript((t) => {
    localStorage.setItem("cua-spaces:theme", t);
    localStorage.removeItem("cua-spaces:call-split");
  }, theme);
  const page = await context.newPage();
  const shot = async (name) => {
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${outDir}/permissions-${name}-${theme}.png` });
  };
  const space = `${base}/spaces/${encodeURIComponent("relay:linux-box/ubuntu-build")}`;
  // More room for the thread, as someone reading it would give it.
  const roomy = async () => {
    const divider = page.locator("[data-call-divider]");
    const box = await divider.boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2, 330, { steps: 8 });
    await page.mouse.up();
  };
  for (const [who, query] of [
    ["owner", ""],
    ["member", "?demo=thread-member"],
    ["guest", "?demo=thread-guest"],
  ]) {
    await page.goto(`${space}${query}`, { waitUntil: "networkidle" });
    await page.locator("[data-task]").first().waitFor();
    await page.locator("[data-member='agent:hermes'] [data-member-toggle]").waitFor();
    await roomy();
    await page.locator("[data-member='agent:hermes'] [data-member-toggle]").click();
    await page.locator("[data-agent-rights='hermes']").waitFor();
    await shot(who);
    if (who === "owner") {
      await page.locator("[data-agent-rights='hermes'] [data-agent-right='task'] button").click();
      await page.getByRole("option", { name: "Members" }).waitFor();
      await shot("owner-menu");
      await page.keyboard.press("Escape");
    }
  }
  await context.close();
}

/** Agent identity (CUA-2262) on the demo's ubuntu-build: the marks, AI
 * tags and owner badges in the thread and In this Space, the card of your
 * own Hermes and of Grace's Claude Code, a look-alike with its owner after
 * the tag and its card, and the thread after removing it. */
async function identityShots(theme) {
  const context = await browser.newContext({ viewport: { width: 1500, height: 940 }, deviceScaleFactor: 2, colorScheme: theme, reducedMotion: "reduce" });
  await context.addInitScript((t) => {
    localStorage.setItem("cua-spaces:theme", t);
    localStorage.removeItem("cua-spaces:call-split");
  }, theme);
  const page = await context.newPage();
  const shot = async (name) => {
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${outDir}/identity-${name}-${theme}.png` });
  };
  const space = `${base}/spaces/${encodeURIComponent("relay:linux-box/ubuntu-build")}`;
  const open = async (query) => {
    await page.goto(`${space}${query}`, { waitUntil: "networkidle" });
    await page.locator("[data-members] [data-owner-badge]").first().waitFor();
    // More room for the thread, as the storyboard shows it.
    const divider = page.locator("[data-call-divider]");
    const box = await divider.boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2, 330, { steps: 8 });
    await page.mouse.up();
  };
  const card = async (handle) => {
    await page.locator(`[data-author-name='${handle}']`).last().click();
    await page.locator(`[data-provenance='${handle}']`).waitFor();
  };
  await open("");
  await shot("thread");
  await card("hermes");
  await shot("card-hermes");
  await page.keyboard.press("Escape");
  await card("claude");
  await shot("card-claude");
  await page.keyboard.press("Escape");
  await open("?demo=thread-lookalike");
  await shot("lookalike");
  await card("claude-2");
  await shot("card-lookalike");
  await page.locator("[data-remove-agent='claude-2']").click();
  await page.locator("[data-member-note='removed']").waitFor();
  await page.mouse.move(10, 10);
  await page.locator("[data-thread]").evaluate((el) => (el.scrollTop = el.scrollHeight));
  await shot("removed");
  await context.close();
}

/** Add an agent (CUA-2264) on the demo's ubuntu-build: the sheet, Start an
 * agent here, Connect your own agent, copied and waiting, an expired code,
 * the agent joined (note, post, card), and someone else's agent asking to
 * join. */
async function addAgentShots(theme) {
  const context = await browser.newContext({ viewport: { width: 1500, height: 940 }, deviceScaleFactor: 2, colorScheme: theme, reducedMotion: "reduce" });
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await context.addInitScript((t) => {
    localStorage.setItem("cua-spaces:theme", t);
    localStorage.removeItem("cua-spaces:call-split");
  }, theme);
  const page = await context.newPage();
  const shot = async (name) => {
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${outDir}/add-agent-${name}-${theme}.png` });
  };
  const space = `${base}/spaces/${encodeURIComponent("relay:linux-box/ubuntu-build")}`;
  const open = async (query) => {
    await page.goto(`${space}${query}`, { waitUntil: "networkidle" });
    await page.locator("[data-members] [data-owner-badge]").first().waitFor();
  };
  const sheet = async (row) => {
    await page.locator("[data-add-agent-open]").click();
    await page.locator("[data-add-agent='pick']").waitFor();
    if (row) await page.locator(`[data-add-row='${row}']`).click();
  };
  const toEnd = () => page.locator("[data-thread]").evaluate((el) => (el.scrollTop = el.scrollHeight));
  // Codes never redeem here, so each state holds still.
  await open("?demo=thread-join-slow");
  await sheet();
  await shot("sheet");
  await page.locator("[data-add-row='start']").click();
  await shot("start");
  await page.locator("[data-add-agent-back]").click();
  await page.locator("[data-add-row='connect']").click();
  await page.locator("[data-prompt-copy='']").waitFor();
  await shot("connect");
  await page.locator("[data-prompt-copy]").click();
  await page.locator("[data-join-waiting]").waitFor();
  await shot("waiting");
  await open("?demo=thread-join-expire");
  await sheet("connect");
  await page.locator("[data-prompt-copy='']").waitFor();
  await page.locator("[data-prompt-copy]").click();
  await page.locator("[data-join-expired]").waitFor({ timeout: 15000 });
  await shot("expired");
  // The demo's Codex runs the code a few seconds after it is made.
  await open("");
  await sheet("connect");
  await page.locator("[data-prompt-copy='']").waitFor();
  await page.locator("[data-prompt-copy]").click();
  await page.locator("[data-add-agent]").waitFor({ state: "detached", timeout: 20000 });
  await page.locator("[data-member-note='joined']").waitFor();
  await page.waitForTimeout(600);
  await toEnd();
  await shot("joined");
  await page.locator("[data-author-name='codex']").last().click();
  await page.locator("[data-provenance='codex']").waitFor();
  await shot("joined-card");
  await page.keyboard.press("Escape");
  await open("?demo=thread-request");
  await page.locator("[data-join-request-state='open']").waitFor();
  await toEnd();
  await shot("request");
  await context.close();
}

/** The Agents tab as a chat (CUA-2266): the list with a DM (Codex's
 * question), Hermes in two Spaces with its approval (All and one Space),
 * Grace's Claude Code at work, a teammate's agent you may not task, the
 * agent page, New agent, and no agents yet. */
async function agentsChatShots(theme) {
  const context = await browser.newContext({ viewport: { width: 1500, height: 940 }, deviceScaleFactor: 2, colorScheme: theme, reducedMotion: "reduce" });
  await context.addInitScript((t) => localStorage.setItem("cua-spaces:theme", t), theme);
  const page = await context.newPage();
  const shot = async (name) => {
    await page.waitForTimeout(600);
    await page.screenshot({ path: `${outDir}/agents-${name}-${theme}.png` });
  };
  const open = async (search) => {
    await page.goto(`${base}/agents${search}`, { waitUntil: "networkidle" });
    await page.locator("[data-dm], [data-agents-empty]").first().waitFor();
    await page.waitForTimeout(1200);
  };
  const agent = (k) => `?agent=${encodeURIComponent(k)}`;
  await open(agent("acct_ada/codex/codex"));
  await shot("codex");
  await open(agent("acct_ada/hermes/hermes"));
  await shot("hermes-all");
  await page.locator("[data-dm] button", { hasText: "ubuntu-build" }).click();
  await page.waitForTimeout(800);
  await shot("hermes-one-space");
  await open(agent("acct_grace/claude-code/claude code"));
  await shot("teammate");
  await page.locator("[data-dm-info]").click();
  await page.locator("[data-agent-page]").waitFor();
  await shot("agent-page");
  await page.keyboard.press("Escape");
  await page.goto(`${base}/agents?demo=thread-member&agent=${encodeURIComponent("acct_grace/hermes/hermes")}`, { waitUntil: "networkidle" });
  await page.locator("[data-dm-locked]").waitFor();
  await shot("locked");
  await open(agent("acct_ada/codex/codex"));
  await page.locator("[data-new-agent]").click();
  await page.locator("[data-add-agent='pick']").waitFor();
  await shot("new-agent");
  await page.keyboard.press("Escape");
  await page.goto(`${base}/agents?demo=nothread`, { waitUntil: "networkidle" });
  await page.locator("[data-agents-empty]").waitFor();
  await shot("empty");
  await context.close();
}

let server;
let base = process.env.BASE_URL;
if (!base) {
  const port = 5175;
  base = `http://localhost:${port}`;
  server = spawn("npx", ["vite", "preview", "--port", String(port), "--strictPort"], { cwd: root, stdio: "ignore" });
  for (let i = 0; i < 50; i++) {
    try {
      if ((await fetch(base)).ok) break;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
}

await mkdir(outDir, { recursive: true });
const browser = await chromium.launch();
try {
  for (const theme of THEMES) {
    if (process.env.ONLY === "thread") {
      await threadShots(theme);
      continue;
    }
    if (process.env.ONLY === "agents-chat") {
      await agentsChatShots(theme);
      continue;
    }
    if (process.env.ONLY === "add-agent") {
      await addAgentShots(theme);
      continue;
    }
    if (process.env.ONLY === "identity") {
      await identityShots(theme);
      continue;
    }
    if (process.env.ONLY === "permissions") {
      await permissionShots(theme);
      continue;
    }
    if (process.env.ONLY === "new-space") {
      await newSpaceSteps(theme);
      continue;
    }
    await teleportSheets(theme);
    if (process.env.ONLY === "teleport") continue;
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, colorScheme: theme });
    await context.addInitScript((t) => localStorage.setItem("cua-spaces:theme", t), theme);
    const page = await context.newPage();
    for (const route of ROUTES) {
      await page.goto(`${base}/${route}`, { waitUntil: "networkidle" });
      await page.waitForTimeout(250);
      await page.screenshot({ path: `${outDir}/${route.replaceAll("/", "-")}-${theme}.png` });
    }
    for (const [route, name] of DETAIL_ROUTES) {
      await page.goto(`${base}/${route}`, { waitUntil: "networkidle" });
      await page.waitForTimeout(250);
      await page.screenshot({ path: `${outDir}/${name}-${theme}.png` });
    }
    await page.goto(`${base}/spaces/${encodeURIComponent("relay:mac-mini/qa-windows")}`, { waitUntil: "networkidle" });
    await page.getByRole("button", { name: /^Delete/ }).click();
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${outDir}/space-detail-delete-${theme}.png` });
    await page.goto(`${base}/spaces`, { waitUntil: "networkidle" });
    await page.keyboard.press(process.platform === "darwin" ? "Meta+k" : "Control+k");
    await page.waitForTimeout(300);
    await page.screenshot({ path: `${outDir}/command-palette-${theme}.png` });
    await page.keyboard.press("Escape");
    // The Agents page mid-stream (the demo's live run writes its reply about
    // 12 s in), then the Connect an agent explainer.
    await page.goto(`${base}/agents`, { waitUntil: "networkidle" });
    await page.waitForTimeout(15_000);
    await page.screenshot({ path: `${outDir}/agents-streaming-${theme}.png` });
    await page.getByRole("button", { name: "Connect an agent" }).first().click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${outDir}/agents-connect-${theme}.png` });
    await settingsStates(page, theme);
    await context.close();
    await onboardingSteps(theme);
    await newSpaceSteps(theme);
    await thisMachineSetup(theme);
  }
} finally {
  await browser.close();
  server?.kill();
}
console.log(`Saved screenshots to ${outDir}`);
