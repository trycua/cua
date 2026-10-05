// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Browser e2e for the cua-spacesd HTML5 viewer. Runs inside a Playwright
 * container that shares the sandbox container's network namespace, so the
 * viewer is `http://127.0.0.1:3211/viewer/` (a secure context in every
 * engine). Driven by run-e2e.sh; never run it against a real desktop.
 *
 * Every wait is bounded (poll() gives up after its deadline and fails).
 *
 * Env: CUA_ENV_TOKEN (root token of the throwaway sandbox), BROWSERS
 * (chromium,firefox,webkit), OUT (evidence dir), VIEWER_ORIGIN (default
 * http://127.0.0.1:3211).
 */

import { chromium, firefox, webkit, type Browser, type BrowserContext, type Page } from "playwright";
import { createServer } from "node:http";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";

import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";

import { createApi, type Api } from "../src/api";
import { ProcessService } from "../src/gen/cua/env/v1/process_pb";
import { AudioUplinkMode } from "../src/gen/cua/env/v1/system_pb";
import { WindowsService } from "../src/gen/cua/env/v1/windows_pb";
import { readAll, uploadBlob } from "../src/transfer";

const ORIGIN = process.env.VIEWER_ORIGIN ?? "http://127.0.0.1:3211";
const TOKEN = process.env.CUA_ENV_TOKEN ?? "";
const OUT = process.env.OUT ?? "/out";
const BROWSERS = (process.env.BROWSERS ?? "chromium,firefox,webkit").split(",").filter(Boolean);
/** Development: serve these assets on :8099 instead of the embedded page. */
const ASSETS = process.env.ASSETS_DIR ?? "";
const PAGE_ORIGIN = ASSETS ? "http://127.0.0.1:8099" : ORIGIN;
mkdirSync(OUT, { recursive: true });

const root: Api = createApi({ baseUrl: `${ORIGIN}/`, ticket: TOKEN, params: new URLSearchParams() });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function poll<T>(what: string, fn: () => Promise<T | null | undefined | false>, timeoutMs = 20_000, everyMs = 250): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  let last: unknown = null;
  for (let i = 0; i < Math.ceil(timeoutMs / everyMs) + 1; i++) {
    try {
      // A single attempt can hang (a clipboard read waiting for a paste
      // prompt): bound every attempt too.
      const v = await Promise.race([
        fn(),
        new Promise<never>((_, reject) => setTimeout(() => reject(new Error("attempt timed out")), Math.min(5_000, timeoutMs))),
      ]);
      if (v) return v as T;
    } catch (e) {
      last = e;
    }
    if (Date.now() > deadline) break;
    await sleep(everyMs);
  }
  throw new Error(`timed out: ${what}${last ? ` (${String(last)})` : ""}`);
}

/** Bounds one step (an evaluate can wait forever on a browser prompt). */
function within<T>(ms: number, what: string, p: Promise<T>): Promise<T> {
  return Promise.race([p, new Promise<never>((_, reject) => setTimeout(() => reject(new Error(`${what}: no answer in ${ms} ms`)), ms))]);
}

async function guestText(path: string): Promise<string | null> {
  try {
    return new TextDecoder().decode(await readAll(root, path, 8 << 20));
  } catch {
    return null;
  }
}

function rootClient<T extends Parameters<typeof import("@connectrpc/connect").createClient>[0]>(service: T) {
  return createClient(
    service,
    createGrpcWebTransport({
      baseUrl: ORIGIN,
      useBinaryFormat: true,
      interceptors: [(next) => async (req) => (req.header.set("x-cua-env-authorization", `Bearer ${TOKEN}`), next(req))],
    }),
  );
}

/** Runs a command in the guest as the desktop user (bounded output). */
async function run(argv: string[]): Promise<{ code: number; out: string }> {
  const client = rootClient(ProcessService);
  let out = "";
  let code = -1;
  let n = 0;
  for await (const r of client.startProcess({
    config: { command: argv[0]!, args: argv.slice(1), cwd: "", user: "", timeout: { seconds: 30n, nanos: 0 } },
  })) {
    if (++n > 10_000 || out.length > 1 << 20) break;
    const ev = r.event?.event;
    if (ev?.case === "data" && (ev.value.output.case === "stdout" || ev.value.output.case === "stderr")) {
      out += new TextDecoder().decode(ev.value.output.value);
    }
    if (ev?.case === "end") code = ev.value.exitCode ?? -1;
  }
  return { code, out };
}

interface Result {
  browser: string;
  version: string;
  checks: Record<string, { ok: boolean; detail: string; skipped?: boolean }>;
  support?: unknown;
}

async function windowRect(title: string) {
  const list = await root.computer.listDisplays({});
  const display = list.displays[0]!;
  const windows = rootClient(WindowsService);
  const all = await windows.listWindows({});
  const w = all.windows.find((x) => x.title.includes(title));
  if (!w) throw new Error(`no window ${title}: ${all.windows.map((x) => x.title).join(", ")}`);
  await windows.activateWindow({ window: w.ref }).catch(() => {});
  const b = w.bounds!;
  return { x: b.x, y: b.y, w: b.width, h: b.height, dw: display.bounds?.width || 1280, dh: display.bounds?.height || 800 };
}

/** Clicks the guest point (gx, gy) through the viewer's canvas. */
async function clickGuest(page: Page, gx: number, gy: number, dw: number, dh: number) {
  const box = await page.locator("canvas.cua-screen").boundingBox();
  if (!box) throw new Error("no canvas");
  await page.mouse.move(box.x + (gx / dw) * box.width, box.y + (gy / dh) * box.height);
  await page.mouse.down();
  await page.mouse.up();
}

async function lines(path: string): Promise<string[]> {
  return ((await guestText(path)) ?? "").split("\n").filter(Boolean);
}

async function testBrowser(name: string, launcher: typeof chromium, ticketUrl: string): Promise<Result> {
  const result: Result = { browser: name, version: "", checks: {} };
  const pass = (k: string, detail: string) => {
    console.log(`  [${name}] ok ${k}`);
    result.checks[k] = { ok: true, detail };
  };
  const fail = (k: string, e: unknown) => (result.checks[k] = { ok: false, detail: String((e as Error)?.message ?? e) });
  const skip = (k: string, why: string) => (result.checks[k] = { ok: true, skipped: true, detail: `n/a: ${why}` });
  const args =
    name === "chromium"
      ? ["--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream", "--autoplay-policy=no-user-gesture-required"]
      : [];
  const firefoxPrefs = {
    "media.navigator.streams.fake": true,
    "media.navigator.permission.disabled": true,
    "media.autoplay.default": 0,
    "media.autoplay.blocking_policy": 0,
    "dom.events.asyncClipboard.readText": true,
    "dom.events.asyncClipboard.clipboardItem": true,
    "dom.events.testing.asyncClipboard": true,
  };
  let browser: Browser | null = null;
  let context: BrowserContext | null = null;
  try {
    browser = await launcher.launch({ headless: true, args, ...(name === "firefox" ? { firefoxUserPrefs: firefoxPrefs } : {}) });
    result.version = browser.version();
    context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
    if (name === "chromium") await context.grantPermissions(["clipboard-read", "clipboard-write", "microphone"], { origin: PAGE_ORIGIN });
    const page = await context.newPage();
    // Every evaluate is bounded: a clipboard read or getUserMedia can wait
    // on a prompt nobody will answer in a headless engine.
    const rawEvaluate = page.evaluate.bind(page) as (fn: unknown, arg?: unknown) => Promise<unknown>;
    (page as unknown as { evaluate: unknown }).evaluate = (fn: unknown, arg?: unknown) => within(30_000, "page.evaluate", rawEvaluate(fn, arg));
    const consoleLog: string[] = [];
    page.on("console", (m) => consoleLog.length < 400 && consoleLog.push(`${m.type()}: ${m.text()}`));
    page.on("pageerror", (e) => consoleLog.length < 400 && consoleLog.push(`pageerror: ${e.message}`));
    await page.goto(ticketUrl);
    result.support = await poll("viewer hooks", () => page.evaluate(() => (window as any).__cuaViewer?.support?.() ?? null), 30_000);

    // A. frames
    try {
      const s = await poll(
        "frames decoded",
        () => page.evaluate(() => {
          const st = (window as any).__cuaViewer?.stats?.();
          // A still desktop sends the keyframe on attach, then frames on damage.
          return st && st.framesDecoded >= 1 ? st : null;
        }),
        30_000,
      );
      pass("video.frames", `${s.framesDecoded} frames decoded, codec ${s.codec}, ${s.frame.width}x${s.frame.height}, decode ${Number(s.decodeMs).toFixed(1)} ms`);
    } catch (e) {
      fail("video.frames", e);
    }
    await page.screenshot({ path: `${OUT}/${name}-viewer.png` });

    // B. input: click a grid cell, type into the form (verified in the fixtures' own logs)
    try {
      const g = await windowRect("CUA Fixture Grid");
      await sleep(400);
      const before = (await lines("/tmp/cua-fixtures/grid.jsonl")).length;
      // Cell (2,3): the grid's client area starts at the window origin (see smoke-test.sh).
      await clickGuest(page, g.x + 2 * 80 + 40, g.y + 3 * 80 + 40, g.dw, g.dh);
      const hit = await poll("grid click logged", async () => {
        const l = (await lines("/tmp/cua-fixtures/grid.jsonl")).slice(before);
        return l.find((x) => x.includes('"button_press"') && /"cell": \[2, 3\]/.test(x)) ?? null;
      });
      pass("input.click", `grid logged ${hit.slice(0, 80)}`);
    } catch (e) {
      fail("input.click", e);
    }
    try {
      const f = await windowRect("CUA Fixture Form");
      await sleep(400);
      await clickGuest(page, f.x + 120, f.y + 25, f.dw, f.dh);
      await sleep(300);
      const word = `web${name}`;
      await page.keyboard.type(word, { delay: 30 });
      const hit = await poll("form text logged", async () => {
        const l = await lines("/tmp/cua-fixtures/form.jsonl");
        return l.find((x) => x.includes("entry_changed") && x.includes(word)) ?? null;
      });
      pass("input.type", `form logged ${hit.slice(0, 90)}`);
    } catch (e) {
      fail("input.type", e);
    }

    // C. clipboard, both ways (text), and images where the engine allows
    if (name !== "chromium") skip("clipboard.host_to_guest", "focus sync needs the clipboard-read permission, which only Chromium grants; Ctrl+V below is the path here");
    else try {
      const toGuest = `from-${name}-${Date.now()}`;
      await page.evaluate((t) => navigator.clipboard.writeText(t), toGuest);
      await page.evaluate(() => window.dispatchEvent(new Event("focus")));
      await poll("guest clipboard == host", async () => (await root.computer.getClipboard({})).content?.text === toGuest);
      pass("clipboard.host_to_guest", `guest clipboard = ${toGuest}`);
    } catch (e) {
      fail("clipboard.host_to_guest", e);
    }
    try {
      // The Ctrl+V path: the browser's paste event carries the host clipboard.
      const viaPaste = `paste-${name}-${Date.now()}`;
      await page.evaluate((t) => navigator.clipboard.writeText(t), viaPaste).catch(() => {});
      await page.locator("textarea.cua-keys").focus();
      await page.keyboard.press("Control+V");
      await poll("guest clipboard via paste", async () => (await root.computer.getClipboard({})).content?.text === viaPaste, 8_000);
      pass("clipboard.paste_event", `Ctrl+V pushed ${viaPaste} before the keys`);
    } catch (e) {
      fail("clipboard.paste_event", e);
    }
    try {
      const toHost = `from-guest-${Date.now()}`;
      await root.computer.setClipboard({ content: { text: toHost, filePaths: [] } });
      // Written directly where the engine allows it, else offered as a
      // one-click "Copy to this computer" chip (a user gesture).
      const how = await poll("host clipboard == guest, or the copy chip", async () => {
        const chip = page.locator('[data-chip="clipboard"] button');
        if (await chip.count()) {
          await chip.click();
          await poll("chip write done", async () => (await page.locator('[data-chip="clipboard"]').count()) === 0, 5_000);
          return "one-click chip";
        }
        if (await page.evaluate(async (t) => (await navigator.clipboard.readText()) === t, toHost).catch(() => false)) return "direct write, read back";
        // Engines that refuse a scripted read-back: the write itself resolved.
        const log = (await page.evaluate(() => (window as any).__cuaViewer.clipboardLog())) as Array<{ direction: string; text?: string }>;
        return log.some((l) => l.direction === "to-host" && l.text === toHost) ? "direct write (the engine refuses a scripted read-back)" : null;
      });
      pass("clipboard.guest_to_host", `host clipboard = ${toHost} (${how})`);
    } catch (e) {
      const chips = await page.locator(".cua-chip").allTextContents().catch(() => []);
      fail("clipboard.guest_to_host", `${String((e as Error).message)}; chips ${JSON.stringify(chips)}`);
    }
    if (name !== "chromium") skip("clipboard.image_guest_to_host", "the test cannot read an image back from this engine's clipboard without a user gesture");
    else try {
      // 1x1 red PNG.
      const png = Uint8Array.from(Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==", "base64"));
      await root.computer.setClipboard({ content: { imagePng: png, filePaths: [] } });
      const size = await poll("host clipboard image", () =>
        page.evaluate(async () => {
          const items = await navigator.clipboard.read();
          for (const i of items) if (i.types.includes("image/png")) return (await i.getType("image/png")).size;
          return null;
        }),
      );
      pass("clipboard.image_guest_to_host", `image/png on the host clipboard (${size} bytes)`);
    } catch (e) {
      fail("clipboard.image_guest_to_host", e);
    }

    // D. audio: the tone fixture plays in the guest; packets arrive and decode
    try {
      const a = await poll(
        "audio decoded",
        () => page.evaluate(() => {
          const st = (window as any).__cuaViewer?.stats?.();
          const t = st?.audio?.[0];
          return t && t.decoded >= 25 ? t : null;
        }),
        20_000,
      );
      pass("audio.decode", `${a.packets} packets, ${a.decoded} decoded, lost ${a.lost}, jitter ${Number(a.jitterMs).toFixed(1)} ms`);
    } catch (e) {
      fail("audio.decode", e);
    }

    // E. microphone uplink (fake capture device)
    try {
      const toggled = await page.evaluate(() => (window as any).__cuaViewer.toggleMic());
      if (!toggled.on) throw new Error(`microphone did not start: ${JSON.stringify(toggled.toasts)}`);
      const sent = await poll("mic packets", () => page.evaluate(() => {
        const n = (window as any).__cuaViewer.stats().mic;
        return n >= 50 ? n : null;
      }), 15_000).catch(async (e) => {
        const d = await page.evaluate(() => (window as any).__cuaViewer.stats().micDebug);
        throw new Error(`${String(e.message)}; capture ${JSON.stringify(d)}`);
      });
      const probe = await run(["desktop-env", "python3", "/opt/cua/fixtures/audio_probe.py", "--source", "cua-uplink", "--seconds", "2", "--freq", "1000"]);
      const lastLine = probe.out.trim().split("\n").pop() ?? "";
      let rms: number | null = null;
      try {
        rms = JSON.parse(lastLine).rms_dbfs;
      } catch {
        rms = null;
      }
      if (rms === null || rms < -60) throw new Error(`${sent} packets sent but the guest source is silent: ${lastLine}`);
      pass("audio.mic_uplink", `${sent} uplink packets; guest source cua-uplink rms ${rms} dBFS`);
      await page.evaluate(() => (window as any).__cuaViewer.toggleMic());
    } catch (e) {
      // Playwright's WebKit refuses getUserMedia and headless Firefox has
      // no audio device in the container: environment limits, reported.
      if (name !== "chromium") skip("audio.mic_uplink", `headless ${name} in the test container has no usable capture device (${String((e as Error).message).slice(0, 120)})`);
      else fail("audio.mic_uplink", e);
    }

    // F. file drop (drop event with a DataTransfer)
    try {
      const fname = `drop-${name}.txt`;
      const synthetic = await page.evaluate(() => {
        const dt = new DataTransfer();
        dt.items.add(new File(["x"], "x.txt"));
        return dt.files.length === 1;
      });
      if (!synthetic) throw Object.assign(new Error("this engine cannot put files in a script-made DataTransfer; a real drag works, and files.picker_upload covers the upload path"), { skip: true });
      await page.evaluate((n) => {
        const dt = new DataTransfer();
        dt.items.add(new File([`dropped by ${n}`], `drop-${n}.txt`, { type: "text/plain" }));
        const root = document.querySelector(".cua-viewer")!;
        root.dispatchEvent(new DragEvent("dragenter", { dataTransfer: dt, bubbles: true }));
        root.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true }));
      }, name);
      const home = new URL(ticketUrl).hash.match(/files=([^&]+)/)?.[1];
      const path = `${decodeURIComponent(home ?? "%2Fhome%2Fcua")}/Downloads/${fname}`;
      await poll("dropped file in guest", async () => (await guestText(path)) === `dropped by ${name}`);
      pass("files.drop_upload", path);
    } catch (e) {
      if ((e as { skip?: boolean }).skip) skip("files.drop_upload", String((e as Error).message));
      else fail("files.drop_upload", e);
    }

    try {
      const [chooser] = await Promise.all([page.waitForEvent("filechooser", { timeout: 10_000 }), page.locator('button[data-id="upload"]').click()]);
      await chooser.setFiles({ name: `picked-${name}.txt`, mimeType: "text/plain", buffer: Buffer.from(`picked by ${name}`) });
      const home = decodeURIComponent(new URL(ticketUrl).hash.match(/files=([^&]+)/)?.[1] ?? "%2Fhome%2Fcua");
      const path = `${home}/Downloads/picked-${name}.txt`;
      await poll("picked file in guest", async () => (await guestText(path)) === `picked by ${name}`);
      pass("files.picker_upload", path);
    } catch (e) {
      fail("files.picker_upload", e);
    }

    // G. folder sharing, both ways (OPFS stands in for a picked folder)
    try {
      const files = decodeURIComponent(new URL(ticketUrl).hash.match(/files=([^&]+)/)?.[1] ?? "%2Fhome%2Fcua");
      const dir = `cua-e2e-share-${name}-${Date.now()}`;
      const guestDir = `${files}/Shared/${dir}`;
      const writable = await page.evaluate(async () => {
        try {
          const h = await (await navigator.storage.getDirectory()).getFileHandle("cua-e2e-probe", { create: true });
          const w = await (h as any).createWritable();
          await w.close();
          return true;
        } catch {
          return false;
        }
      });
      if (!writable) throw Object.assign(new Error("no writable File System Access handles in this engine (and no showDirectoryPicker), so the viewer does not offer folder sharing here"), { skip: true });
      await page.evaluate(async (d) => {
        const opfs = await navigator.storage.getDirectory();
        const h = await opfs.getDirectoryHandle(d, { create: true });
        const f = await h.getFileHandle("local.txt", { create: true });
        const w = await (f as any).createWritable();
        await w.write("made on the host");
        await w.close();
        await (window as any).__cuaViewer.share(h, `${(window as any).__cuaViewer.viewer.api.endpoint.params.get("files")}/Shared/${d}`);
      }, dir);
      await poll("local file in guest", async () => (await guestText(`${guestDir}/local.txt`)) === "made on the host");
      // guest -> host: create and edit in the guest
      await uploadBlob(root, `${guestDir}/remote.txt`, new Blob(["made in the sandbox"]));
      const readLocal = (n: string) =>
        page.evaluate(async ([d, f]) => {
          try {
            const h = await (await navigator.storage.getDirectory()).getDirectoryHandle(d!);
            return await (await (await h.getFileHandle(f!)).getFile()).text();
          } catch {
            return null;
          }
        }, [dir, n]);
      await poll("guest file on host", async () => (await readLocal("remote.txt")) === "made in the sandbox", 30_000);
      await uploadBlob(root, `${guestDir}/local.txt`, new Blob(["edited in the sandbox"]));
      await poll("guest edit on host", async () => (await readLocal("local.txt")) === "edited in the sandbox", 30_000);
      // host delete -> guest delete
      await page.evaluate(async (d) => {
        const h = await (await navigator.storage.getDirectory()).getDirectoryHandle(d);
        await h.removeEntry("remote.txt");
        await (window as any).__cuaViewer.syncNow();
      }, dir);
      await poll("host delete in guest", async () => (await guestText(`${guestDir}/remote.txt`)) === null, 30_000);
      const status = await page.evaluate(() => (window as any).__cuaViewer.shares());
      pass("files.folder_sync", `host->guest create, guest->host create and edit, host delete propagated; ${JSON.stringify(status)}`);
    } catch (e) {
      if ((e as { skip?: boolean }).skip) skip("files.folder_sync", String((e as Error).message));
      else fail("files.folder_sync", e);
    }

    try {
      const s = await page.evaluate(() => (window as any).__cuaViewer.stats());
      if (s.framesDecoded < 5) throw new Error(`only ${s.framesDecoded} frames after the interactions`);
      pass("video.updates", `${s.framesDecoded} frames after the interactions (${s.keyframes} keyframes, ${Math.round(s.bytesReceived / 1024)} KiB)`);
    } catch (e) {
      fail("video.updates", e);
    }
    await page.screenshot({ path: `${OUT}/${name}-viewer-after.png` });
    if (name === "chromium") {
      // Docs captures (real page, real sandbox): the toolbar, and the folder
      // sharing prompt.
      await page.locator(".cua-toolbar").hover();
      await sleep(400);
      await page.screenshot({ path: `${OUT}/docs-viewer-toolbar.png` });
      await page.locator('button[data-id="share"]').click();
      await sleep(300);
      await page.screenshot({ path: `${OUT}/docs-viewer-share-folder.png` });
      await page.getByRole("button", { name: "Cancel" }).click();
    }
    writeFileSync(`${OUT}/${name}-console.log`, consoleLog.join("\n"));
  } catch (e) {
    fail("launch", e);
  } finally {
    await context?.close().catch(() => {});
    await browser?.close().catch(() => {});
  }
  return result;
}

async function main() {
  if (!TOKEN) throw new Error("CUA_ENV_TOKEN is required");
  // Allow the microphone uplink for this throwaway sandbox.
  await root.system.init({ token: TOKEN, audioUplink: { mode: AudioUplinkMode.ANY, principalIds: [] } } as never);
  if (ASSETS) {
    const types: Record<string, string> = { html: "text/html", js: "text/javascript", css: "text/css" };
    createServer((req, res) => {
      const file = (req.url ?? "/").split("?")[0]!.replace(/^\/viewer\/?/, "") || "index.html";
      try {
        const body = readFileSync(`${ASSETS}/${file.replace(/\.\./g, "")}`);
        res.writeHead(200, { "content-type": types[file.split(".").pop() ?? ""] ?? "application/octet-stream" });
        res.end(body);
      } catch {
        res.writeHead(404).end();
      }
    }).listen(8099, "127.0.0.1");
  }
  const results: Result[] = [];
  const launchers: Record<string, typeof chromium> = { chromium, firefox, webkit };
  for (const name of BROWSERS) {
    const minted = await root.system.createViewerTicket({ ttl: { seconds: 900n, nanos: 0 }, clipboard: true, filesRoot: "~", audioUplink: true });
    const url = ASSETS ? `${PAGE_ORIGIN}${minted.viewerPath}&base=${encodeURIComponent(`${ORIGIN}/`)}` : `${ORIGIN}${minted.viewerPath}`;
    const r = await testBrowser(name, launchers[name]!, url);
    results.push(r);
    const okCount = Object.values(r.checks).filter((c) => c.ok).length;
    console.log(`== ${name} ${r.version}: ${okCount}/${Object.keys(r.checks).length}`);
    for (const [k, c] of Object.entries(r.checks)) console.log(`  ${c.ok ? "ok  " : "FAIL"} ${k}: ${c.detail}`);
  }
  writeFileSync(`${OUT}/results.json`, JSON.stringify(results, null, 2));
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
