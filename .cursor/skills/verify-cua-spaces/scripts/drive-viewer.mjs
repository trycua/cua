// Drives the cua-spacesd HTML5 viewer like a user: open the link `cua sb
// view` printed, wait for the stream, click the bottom-right (white) cell of
// the x11-pad grid, type "ok", then capture. Writes <out>.json, <out>.png and
// <out>.aria.txt, and checks the pad's own event log for the click and keys.
//
//   PW_DIR=<dir with playwright-core> CHROMIUM=<binary> node drive-viewer.mjs LINK OUT PAD_LOG

import { createRequire } from "node:module";
import { readFileSync, writeFileSync } from "node:fs";

const { chromium } = createRequire(`${process.env.PW_DIR}/`)("playwright-core");
const [link, out, padLog] = process.argv.slice(2);
const padEvents = () =>
  readFileSync(padLog, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => JSON.parse(l));
const result = { link: link.replace(/ticket=[^&]*/, "ticket=<redacted>"), steps: [] };
const step = (name, data) => {
  result.steps.push({ name, ...data });
  console.log(name, JSON.stringify(data));
};

const browser = await chromium.launch({ executablePath: process.env.CHROMIUM, args: ["--no-sandbox"] });
let failed = null;
try {
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  const consoleLines = [];
  page.on("console", (m) => consoleLines.push(`${m.type()}: ${m.text()}`));
  await page.goto(link);
  // Ready: the root's data-status leaves "connecting" (the viewer sets it).
  const status = await page
    .waitForFunction(
      () => {
        const s = document.querySelector("#app")?.dataset.status;
        return s === "streaming" || s === "failed" ? s : null;
      },
      null,
      { timeout: 30_000 },
    )
    .then((h) => h.jsonValue())
    .catch(() => "timeout");
  const overlay = (await page.locator(".cua-overlay").innerText().catch(() => "")).trim();
  step("open", { status, overlay, title: await page.title() });
  if (status !== "streaming") throw new Error(`viewer did not stream: ${status} ${overlay}`);
  await page.waitForFunction(() => (window.__cuaViewer?.stats().framesDecoded ?? 0) > 0, null, { timeout: 15_000 });
  step("frames", { stats: await page.evaluate(() => window.__cuaViewer.stats()) });

  // A viewer that just closed still holds the input lease for up to 5 s
  // (the server answers RateLimited "leased to Web viewer"): wait it out.
  await page.waitForTimeout(6_000);
  const before = padEvents().length;
  const canvas = page.locator("canvas.cua-screen");
  const box = await canvas.boundingBox();
  await page.mouse.click(box.x + box.width * 0.75, box.y + box.height * 0.75);
  await page.keyboard.type("ok");
  // The pad journal is the side effect: wait for a click and both keys.
  let fresh = [];
  for (let i = 0; i < 40; i++) {
    fresh = padEvents().slice(before);
    const keys = fresh.filter((e) => e.event === "key").map((e) => e.char).join("");
    if (fresh.some((e) => e.event === "button") && keys.includes("ok")) break;
    await page.waitForTimeout(250);
  }
  const click = fresh.find((e) => e.event === "button");
  const keys = fresh.filter((e) => e.event === "key").map((e) => e.char).join("");
  step("input", { click, keys });
  if (!click || click.x < 640 || click.y < 400) throw new Error(`no click in the white cell: ${JSON.stringify(click)}`);
  if (!keys.includes("ok")) throw new Error(`keys not delivered: ${JSON.stringify(keys)}`);

  writeFileSync(`${out}.aria.txt`, await page.locator("body").ariaSnapshot());
  await page.screenshot({ path: `${out}.png` });
  result.console = consoleLines.slice(-20);
} catch (error) {
  failed = String(error?.message ?? error);
  result.error = failed;
} finally {
  await browser.close();
  writeFileSync(`${out}.json`, JSON.stringify(result, null, 2));
}
console.log(failed ? `FAIL ${failed}` : `PASS evidence ${out}.{json,png,aria.txt}`);
process.exit(failed ? 1 : 0);
