// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Captures every route in light and dark at 1440x900 into docs/screenshots.
// Usage: pnpm build && pnpm screenshots   (BASE_URL=... to use a running server,
// ONLY=new-space for just the New Space wizard and Connect a cloud,
// ONLY=teleport for the Teleport and Share sheets)
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

const root = fileURLToPath(new URL("..", import.meta.url));
const outDir = `${root}docs/screenshots`;
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
