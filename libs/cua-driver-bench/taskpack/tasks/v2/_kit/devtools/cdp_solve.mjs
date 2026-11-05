// Development-only reference solver for the v2 web tasks: drives a headless Chrome over the
// DevTools protocol with real input events (mouse and keyboard), following a JSON step list.
// Not used in trials. Usage: node cdp_solve.mjs <ws-url-of-page> <steps.json>
import { readFileSync } from "node:fs";

const [wsUrl, stepsPath] = process.argv.slice(2);
const steps = JSON.parse(readFileSync(stepsPath, "utf8"));
const ws = new WebSocket(wsUrl);
let nextId = 1;
const pending = new Map();
let dialogPolicy = "dismiss";
const dialogs = [];
ws.addEventListener("message", (msg) => {
  const data = JSON.parse(msg.data);
  if (data.id && pending.has(data.id)) {
    const { resolve, reject } = pending.get(data.id);
    pending.delete(data.id);
    data.error ? reject(new Error(JSON.stringify(data.error))) : resolve(data.result);
  } else if (data.method === "Page.javascriptDialogOpening") {
    dialogs.push(data.params);
    send("Page.handleJavaScriptDialog", { accept: dialogPolicy === "accept", promptText: "" });
  }
});
function send(method, params = {}) {
  const id = nextId++;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function evaluate(expr) {
  const res = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
  if (res.exceptionDetails) throw new Error("eval failed: " + JSON.stringify(res.exceptionDetails).slice(0, 400));
  return res.result.value;
}
async function center(target) {
  if (typeof target === "object" && target !== null && "x" in target) return target;
  for (let i = 0; i < 40; i++) {
    const box = await evaluate(`(() => { const el = document.querySelector(${JSON.stringify(target)});
      if (!el) return null; el.scrollIntoView({block: "center", inline: "center"});
      const r = el.getBoundingClientRect(); if (!r.width || !r.height) return null;
      return {x: r.left + r.width / 2, y: r.top + r.height / 2}; })()`);
    if (box) return box;
    await sleep(100);
  }
  throw new Error("no element " + target);
}
async function mouse(type, p, extra = {}) {
  await send("Input.dispatchMouseEvent", { type, x: p.x, y: p.y, button: "left", clickCount: 1, ...extra });
}
const KEYS = {
  Enter: { code: "Enter", key: "Enter", windowsVirtualKeyCode: 13, text: "\r" },
  Tab: { code: "Tab", key: "Tab", windowsVirtualKeyCode: 9 },
  Backspace: { code: "Backspace", key: "Backspace", windowsVirtualKeyCode: 8 },
  Escape: { code: "Escape", key: "Escape", windowsVirtualKeyCode: 27 },
  ArrowDown: { code: "ArrowDown", key: "ArrowDown", windowsVirtualKeyCode: 40 },
  ArrowUp: { code: "ArrowUp", key: "ArrowUp", windowsVirtualKeyCode: 38 },
  ArrowLeft: { code: "ArrowLeft", key: "ArrowLeft", windowsVirtualKeyCode: 37 },
  ArrowRight: { code: "ArrowRight", key: "ArrowRight", windowsVirtualKeyCode: 39 },
  Space: { code: "Space", key: " ", windowsVirtualKeyCode: 32, text: " " },
  Home: { code: "Home", key: "Home", windowsVirtualKeyCode: 36 },
  End: { code: "End", key: "End", windowsVirtualKeyCode: 35 },
};
async function key(name) {
  const k = KEYS[name];
  if (!k) throw new Error("unknown key " + name);
  await send("Input.dispatchKeyEvent", { type: k.text ? "keyDown" : "rawKeyDown", ...k });
  await send("Input.dispatchKeyEvent", { type: "keyUp", ...k, text: undefined });
}
async function click(target, count = 1) {
  const p = await center(target);
  await mouse("mouseMoved", p, { button: "none" });
  for (let i = 1; i <= count; i++) {
    await mouse("mousePressed", p, { clickCount: i });
    await mouse("mouseReleased", p, { clickCount: i });
  }
}
async function run() {
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  await send("Page.enable");
  await send("Runtime.enable");
  for (const step of steps) {
    const [op, arg] = Object.entries(step)[0];
    if (op === "click") await click(arg);
    else if (op === "dblclick") await click(arg, 2);
    else if (op === "hover") { const p = await center(arg); await mouse("mouseMoved", p, { button: "none" }); }
    else if (op === "type") await send("Input.insertText", { text: arg });
    else if (op === "key") await key(arg);
    else if (op === "fill") {
      await click(arg[0]);
      await evaluate(`(() => { const el = document.querySelector(${JSON.stringify(arg[0])}); if (el.select) el.select(); else document.execCommand("selectAll"); })()`);
      await send("Input.insertText", { text: arg[1] });
    }
    else if (op === "select") await evaluate(`(() => { const el = document.querySelector(${JSON.stringify(arg[0])}); el.value = ${JSON.stringify(arg[1])}; el.dispatchEvent(new Event("input", {bubbles: true})); el.dispatchEvent(new Event("change", {bubbles: true})); })()`);
    else if (op === "drag") {
      const a = await center(arg[0]);
      const b = await center(arg[1]);
      await mouse("mouseMoved", a, { button: "none" });
      await mouse("mousePressed", a);
      const n = step.steps || 12;
      for (let i = 1; i <= n; i++) {
        await mouse("mouseMoved", { x: a.x + ((b.x - a.x) * i) / n, y: a.y + ((b.y - a.y) * i) / n }, { buttons: 1 });
        await sleep(15);
      }
      await mouse("mouseReleased", b);
    }
    else if (op === "wait") await sleep(arg);
    else if (op === "waitFor") await center(arg);
    else if (op === "eval") await evaluate(arg);
    else if (op === "dialogs") dialogPolicy = arg;
    else if (op === "navigate") { await send("Page.navigate", { url: arg }); await sleep(800); }
    else if (op === "assert") {
      const ok = await evaluate(arg);
      if (!ok) throw new Error("assert failed: " + arg);
    }
    else throw new Error("unknown step " + op);
    await sleep(step.after || 60);
  }
  await sleep(400);
  console.log(JSON.stringify({ ok: true, dialogs }));
  ws.close();
}
run().catch((e) => { console.error(String(e)); process.exit(1); });
