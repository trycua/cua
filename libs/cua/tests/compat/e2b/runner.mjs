// Shared-case runner for the TypeScript `e2b` SDK.
//
// Runs the same cases/*.json as runner.py against whatever the environment
// points at (real E2B, the Cua gateway) and prints one JSON object of
// observations in the same shape, so compare.py can diff it against a
// recording. See cases.md for the case language.
//
//   node runner.mjs --sdk <dir of the e2b package> [--only a,b] [--skip a,b] [--tag t] > run.json
//
// Case arguments use the Python SDK's names and units (seconds, snake_case).
// This runner converts them to the TypeScript SDK's (milliseconds, camelCase
// options objects).
import { createHash, randomBytes } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const opt = (name, dflt = "") => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : dflt;
};
const SDK_DIR = path.resolve(opt("--sdk", path.join(HERE, "ts", "node_modules", "e2b")));
const ONLY = new Set(opt("--only").split(",").filter(Boolean));
const SKIP = new Set(opt("--skip").split(",").filter(Boolean));
const TAG = opt("--tag");
const CASES = opt("--cases", path.join(HERE, "cases"));
const TEMPLATE = process.env.CONFORMANCE_TEMPLATE || undefined;

const pkg = JSON.parse(readFileSync(path.join(SDK_DIR, "package.json"), "utf8"));
const sdk = await import(pathToFileURL(path.join(SDK_DIR, pkg.module || pkg.main || "dist/index.mjs")).href);
const { Sandbox } = sdk;
const LANG = "ts";
const RUN_ID = "r" + createHash("sha256").update(randomBytes(16)).digest("hex").slice(0, 12);
const COMMON = JSON.parse(readFileSync(path.join(HERE, "common.json"), "utf8"));
const MASK_KEYS = new Set(COMMON.mask_keys);

// ── naming ───────────────────────────────────────────────────────────────
const snake = (s) => s.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
const camel = (s) => s.replace(/_([a-z0-9])/g, (_, c) => c.toUpperCase());
const USERDATA = new Set(["metadata", "envs", "headers"]);
const RENAME = { timeout: "timeoutMs", request_timeout: "requestTimeoutMs", stream_idle_timeout: "streamIdleTimeoutMs" };
const SECONDS = new Set(["timeout", "request_timeout", "stream_idle_timeout"]);

function camelize(v, key) {
  if (Array.isArray(v)) return v.map((x) => camelize(x));
  if (v && typeof v === "object" && !(v instanceof Uint8Array) && Object.getPrototypeOf(v) === Object.prototype) {
    if (key && USERDATA.has(key)) return v;
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [camel(k), camelize(x, k)]));
  }
  return v;
}

function toOpts(kw) {
  const out = {};
  for (const [k, v] of Object.entries(kw || {})) {
    if (k === "no_template") continue;
    const name = RENAME[k] || camel(k);
    out[name] = SECONDS.has(k) ? (v === null ? v : v * 1000) : camelize(v, k);
  }
  return out;
}

// ── values ───────────────────────────────────────────────────────────────
function genBytes(spec) {
  const n = Number(spec.n);
  const seed = Number(spec.seed ?? 1);
  const pattern = spec.pattern ?? "mix";
  const b = new Uint8Array(n);
  if (pattern === "zero") return b;
  if (pattern === "text") {
    const unit = new TextEncoder().encode(spec.unit ?? "line of text\n");
    for (let i = 0; i < n; i++) b[i] = unit[i % unit.length];
    return b;
  }
  for (let i = 0; i < n; i++) b[i] = (i * 167 + seed + (i >> 8) * 13) & 255;
  return b;
}

class Ctx {
  constructor() {
    this.vars = { run_id: RUN_ID };
    this.collected = {};
    this.cbSpecs = {};
  }
  lookup(p) {
    const [head, ...rest] = p.split(".");
    let v = this.vars[head];
    for (const part of rest) {
      if (v instanceof Error || v === undefined || v === null) throw new Error(`cannot resolve ${p}`);
      if (Array.isArray(v)) v = v[Number(part)];
      else v = part in v ? v[part] : v[camel(part)];
    }
    return v;
  }
  resolve(v) {
    if (typeof v === "string") {
      const m = v.match(/^\$([A-Za-z_][\w.]*)$/);
      if (m) return this.lookup(m[1]);
      if (v.includes("${")) return v.replace(/\$\{([\w.]+)\}/g, (_, p) => String(this.lookup(p)));
      return v;
    }
    if (Array.isArray(v)) return v.map((x) => this.resolve(x));
    if (v && typeof v === "object") {
      if ("$bytes" in v) return genBytes(v.$bytes);
      if ("$utf8" in v) return new TextEncoder().encode(v.$utf8);
      if ("$stream" in v) {
        const data = genBytes(v.$stream);
        const chunk = 1 << 16;
        let off = 0;
        return new ReadableStream({
          pull(c) {
            if (off >= data.length) return c.close();
            c.enqueue(data.slice(off, off + chunk));
            off += chunk;
          },
        });
      }
      if ("$text" in v) return v.$text.unit.repeat(Number(v.$text.repeat ?? 1));
      if ("$repeat" in v) {
        const r = v.$repeat;
        return Array.from({ length: r.n }, (_, i) => this.resolve(JSON.parse(JSON.stringify(r.item).replaceAll("{i}", String(i)))));
      }
      if ("$query" in v) return this.resolve(v.$query);
      if ("$template" in v) return buildTemplate(v.$template);
      if ("$size" in v) return v.$size;
      if ("$cb" in v) return this.callback(v);
      return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, this.resolve(x)]));
    }
    return v;
  }
  callback(spec) {
    const name = spec.$cb;
    this.cbSpecs[name] = spec;
    const bucket = (this.collected[name] ??= []);
    const prefix = spec.prefix ?? "";
    return (x) => {
      if (spec.as === "event") x = `${x.name}:${x.type}`;
      else if (spec.as === "message") x = x?.message ?? String(x);
      else if (spec.as === "text" && x instanceof Uint8Array) x = new TextDecoder("utf-8").decode(x);
      x = norm(x);
      bucket.push(prefix && typeof x === "string" ? prefix + x : x);
    };
  }
}

// ── observations ─────────────────────────────────────────────────────────
const canonName = (n) => (n === "Error" || n === "TypeError" ? n : n.replace(/Error$/, "Exception"));

function redact(s) {
  s = s.replaceAll(RUN_ID, "<run>");
  s = s.replace(/\bpid \d+/g, "pid <pid>");
  s = s.replace(/\b[a-z0-9]{20}\b/g, "<id>");
  s = s.replace(/[0-9a-z]{18,}/g, "<id>");
  s = s.replace(/\b[a-z0-9]+(?:-[a-z0-9]+)*-\d+\//g, "<ns>/");
  s = s.replace(/127\.0\.0\.1:\d+/g, "<host>");
  return s;
}

function excInfo(e) {
  const mro = [];
  for (let p = Object.getPrototypeOf(e); p && p !== Object.prototype; p = Object.getPrototypeOf(p)) {
    const n = p.constructor?.name;
    if (n && n !== "Error") mro.push(canonName(n));
  }
  const d = { type: canonName(e?.constructor?.name ?? "Error"), mro };
  for (const k of ["exitCode", "stdout", "stderr", "error"]) if (k in (e ?? {})) d[snake(k)] = norm(e[k]);
  d.message = redact(String(e?.message ?? e));
  return d;
}

function norm(v, key) {
  if (key !== undefined && MASK_KEYS.has(snake(key))) return v === null || v === undefined ? null : "<masked>";
  if (v === null || v === undefined) return null;
  if (typeof v === "string") {
    if (v.length > 4096) return { $str: v.length, sha256: createHash("sha256").update(v, "utf8").digest("hex").slice(0, 16) };
    return v.includes(RUN_ID) ? redact(v) : v;
  }
  if (typeof v === "boolean") return v;
  if (typeof v === "number") return Number.isInteger(v) ? v : Math.round(v * 1000) / 1000;
  if (typeof v === "bigint") return Number(v);
  if (v instanceof Uint8Array || v instanceof ArrayBuffer) {
    const b = Buffer.from(v instanceof ArrayBuffer ? new Uint8Array(v) : v);
    return { $bytes: b.length, sha256: createHash("sha256").update(b).digest("hex").slice(0, 16) };
  }
  if (v instanceof Error) return { raised: excInfo(v) };
  if (v instanceof Date) {
    if (key !== undefined && snake(key) === "end_at") return `end_in:${Math.round((v.getTime() - Date.now()) / 1000 / 30) * 30}`;
    return "<datetime>";
  }
  if (Array.isArray(v)) return v.map((x) => norm(x));
  if (v instanceof Sandbox) return { $sandbox: true };
  if (typeof v.wait === "function" && "pid" in v) return { $handle: true, pid: "<pid>" };
  if (typeof v.then === "function") return "<awaitable>";
  const out = {};
  const userData = key !== undefined && ["envs", "metadata", "headers"].includes(snake(key));
  for (const k of Object.keys(v)) {
    if (k.startsWith("_") || typeof v[k] === "function") continue;
    out[userData ? k : snake(k)] = userData ? norm(v[k]) : norm(v[k], k);
  }
  if (Object.keys(out).length === 0 && Object.getPrototypeOf(v) !== Object.prototype) return { $object: v.constructor?.name };
  return out;
}

function pick(v, fields) {
  if (!fields || fields.length === 0) return v;
  if (Array.isArray(v)) return v.map((x) => pick(x, fields));
  if (v && typeof v === "object") {
    const out = {};
    for (const f of fields) {
      let cur = v;
      for (const p of f.split(".")) cur = cur && typeof cur === "object" ? cur[p] : undefined;
      out[f] = cur === undefined ? null : cur;
    }
    return out;
  }
  return v;
}

// ── ops ──────────────────────────────────────────────────────────────────
const STATICS = { Sandbox, Volume: sdk.Volume, Secret: sdk.Secret, Template: sdk.Template };

async function httpGet(url, headers = {}, method = "GET", data, mp) {
  headers = { ...headers };
  let body = data;
  if (mp) {
    const boundary = "----cuaconformance0123456789";
    const parts = mp.files.map(
      (f) =>
        `--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="${f.filename}"\r\nContent-Type: application/octet-stream\r\n\r\n${f.content}\r\n`,
    );
    body = Buffer.from(parts.join("") + `--${boundary}--\r\n`);
    headers["Content-Type"] = `multipart/form-data; boundary=${boundary}`;
  }
  try {
    const r = await fetch(url, { method, headers, body, signal: AbortSignal.timeout(20000) });
    const text = await r.text();
    let parsed = redact(text.slice(0, 2000));
    try {
      const v = JSON.parse(parsed);
      if (v && typeof v === "object") parsed = v;
    } catch {}
    return { status: r.status, body: parsed };
  } catch (e) {
    return { error: e?.cause?.code || e?.name || "Error" };
  }
}

function buildTemplate(spec) {
  const ctxdir = mkdtempSync(path.join(tmpdir(), "cua-cases-ctx-"));
  for (const [rel, text] of Object.entries(spec.context ?? {})) {
    const p = path.join(ctxdir, rel);
    mkdirSync(path.dirname(p), { recursive: true });
    writeFileSync(p, text);
  }
  let t = sdk.Template({ fileContextPath: ctxdir });
  for (const [method, args = [], kw = {}] of spec.steps) {
    const name = camel(method);
    const opts = toOpts(kw);
    // The TypeScript builder takes a trailing options object where Python takes keywords.
    const call = { copy: () => t.copy(args[0], args[1], opts), runCmd: () => t.runCmd(args[0], kw.user), setStartCmd: () => t.setStartCmd(args[0], args[1]) }[name];
    t = call ? call() : t[name](...args);
  }
  return t;
}

async function apiCall(method, p, body) {
  const base = process.env.E2B_API_URL || "https://api.e2b.app";
  const r = await fetch(base + p, { method, headers: { "X-API-KEY": process.env.E2B_API_KEY, "Content-Type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text();
  let b = redact(text.slice(0, 2000));
  try { const v = JSON.parse(b); if (v && typeof v === "object") b = v; } catch {}
  return { status: r.status, body: b };
}

const sleep = (s) => new Promise((r) => setTimeout(r, s * 1000));

// Per-op argument translation: [path, positional args, keyword options] ->
// the TypeScript call. Anything not listed is "path(...positional, opts)".
const POS_SECONDS = new Set(["set_timeout"]);

async function doOp(ctx, step, created) {
  const op = step.op;
  let a = ctx.resolve(step.args ?? []);
  if (step.args_ts) a = ctx.resolve(step.args_ts);
  const kw = ctx.resolve({ ...(step.kw ?? {}), ...(step.kw_ts ?? {}) });
  if (op === "sleep") return void (await sleep(a[0]));
  if (op === "http") {
    if (kw.tamper) a = [a[0].replace(/(signature=[^&]*)(.)(&|$)/, (_, g, c, e) => g + (c !== "A" ? "A" : "B") + e)];
    return httpGet(a[0], kw.headers, kw.method ?? "GET", kw.data, kw.multipart);
  }
  if (op === "env") return process.env[a[0]];
  if (op === "api") return apiCall(a[0], a[1], a[2]);
  if (op === "attr") return callPath(ctx.lookup(step.on), a[0]);
  if (op === "parallel") {
    const n = Number(step.n ?? 1);
    const rs = await Promise.allSettled(Array.from({ length: n }, () => doOp(ctx, step.step, created)));
    return rs.map((r) => (r.status === "fulfilled" ? r.value : r.reason));
  }
  const [cls, ...restParts] = op.split(".");
  const method = restParts.join(".");
  if (STATICS[cls] !== undefined && method) {
    const S = STATICS[cls];
    if (S === undefined) throw new Error(`${cls} is not in this SDK version`);
    if (cls === "Sandbox") return staticSandbox(method, a, kw, created);
    const fn = S[camel(method)];
    return fn.call(S, ...a, toOpts(kw));
  }
  const on = ctx.lookup(step.on ?? "sb");
  if (op.startsWith("@")) {
    const m = op.slice(1);
    const name = m in on ? m : camel(m);
    const attr = on[name];
    return typeof attr === "function" ? attr.call(on, ...a, ...(Object.keys(kw).length ? [toOpts(kw)] : [])) : attr;
  }
  return instanceCall(on, op, a, kw);
}

function callPath(obj, p) {
  for (const part of p.split(".")) obj = obj[camel(part)];
  return obj;
}

async function staticSandbox(method, a, kw, created) {
  const opts = toOpts(kw);
  switch (method) {
    case "create": {
      const tpl = kw.template ?? (kw.no_template ? undefined : TEMPLATE);
      delete opts.template;
      const sb = tpl ? await Sandbox.create(tpl, opts) : await Sandbox.create(opts);
      created.push(sb);
      return sb;
    }
    case "connect": {
      const sb = await Sandbox.connect(a[0], opts);
      created.push(sb);
      return sb;
    }
    case "list": {
      // The Python call passes the query object and the paging options flat.
      return Sandbox.list(opts);
    }
    case "fork": {
      const r = await Sandbox.fork(a[0], opts);
      created.push(...r.filter((x) => !(x instanceof Error)));
      return r;
    }
    case "set_timeout":
      return Sandbox.setTimeout(a[0], a[1] * 1000, {});
    default:
      return Sandbox[camel(method)](...a, opts);
  }
}

async function instanceCall(on, op, a, kw) {
  const opts = toOpts(kw);
  switch (op) {
    case "set_timeout":
      return on.setTimeout(a[0] * 1000, opts);
    case "connect":
      return on.connect(opts);
    case "get_info":
    case "kill":
    case "is_running":
    case "get_metrics":
    case "pause":
    case "update_network":
    case "get_host":
      return on[camel(op)](...a, ...(op === "get_host" ? [] : [opts]));
    case "download_url":
    case "upload_url":
      return on[camel(op)](a[0], opts);
    case "fork":
      return on.fork(opts);
    case "create_snapshot":
      return on.createSnapshot(opts);
    case "list_snapshots":
      return on.listSnapshots(opts);
    case "pty.create": {
      const size = a[0];
      return on.pty.create({ cols: size.cols, rows: size.rows, ...opts });
    }
    case "pty.resize":
      return on.pty.resize(a[0], { cols: a[1].cols, rows: a[1].rows });
    case "pty.send_stdin":
      return on.pty.sendInput(a[0], a[1]);
    case "pty.connect":
      return on.pty.connect(a[0], opts);
    case "pty.kill":
      return on.pty.kill(a[0]);
    case "files.watch_dir":
      return on.files.watchDir(a[0], a[1], opts);
    case "files.read": {
      return on.files.read(a[0], opts);
    }
    case "files.list": {
      return on.files.list(a[0], opts);
    }
    case "commands.list":
      return on.commands.list(opts);
    case "commands.kill":
    case "commands.close_stdin":
      return on.commands[camel(op.split(".")[1])](a[0], opts);
    default: {
      const parts = op.split(".");
      const method = camel(parts.pop());
      let target = on;
      for (const p of parts) target = target[camel(p)];
      return target[method](...a, ...(Object.keys(opts).length || method === "run" ? [opts] : []));
    }
  }
}

function untilOk(spec, r) {
  let v = norm(r);
  if (spec.path === "#") v = Array.isArray(v) ? v.length : null;
  else if (spec.path !== "") for (const part of spec.path.split(".")) v = v && typeof v === "object" ? v[part] : undefined;
  return v === spec.equals;
}

async function pollUntil(ctx, step, created, r) {
  const spec = step.until;
  const deadline = Date.now() + (spec.timeout ?? 60) * 1000;
  if (step.paginate) r = await paginate(r);
  while (!untilOk(spec, r) && Date.now() < deadline) {
    await sleep(spec.interval ?? 2);
    r = await doOp(ctx, step, created);
    if (step.paginate) r = await paginate(r);
  }
  return r;
}

async function paginate(p, max = 50) {
  const items = [];
  for (let i = 0; i < max; i++) {
    items.push(...(await p.nextItems()));
    if (!p.hasNext) break;
  }
  return items;
}

async function consume(r) {
  const chunks = [];
  const reader = r.getReader ? r.getReader() : null;
  if (reader) {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(Buffer.from(value));
    }
  } else {
    for await (const c of r) chunks.push(Buffer.from(c));
  }
  return new Uint8Array(Buffer.concat(chunks));
}

async function runCase(c, shared, created) {
  const ctx = new Ctx();
  const own = [];
  const out = [];
  try {
    if ((c.sandbox ?? "shared") === "shared") {
      // A slow run (a Space is slower than E2B) must not outlive the shared
      // sandbox's timeout: every shared case starts with a full one.
      try {
        await shared.setTimeout((SHARED_TIMEOUT ?? 900) * 1000);
      } catch {}
      ctx.vars.sb = shared;
    }
    else if (c.sandbox === "own") {
      try {
        const sb = await Sandbox.create(...(TEMPLATE ? [TEMPLATE] : []), toOpts(ctx.resolve(c.create ?? { timeout: 300 })));
        created.push(sb);
        own.push(sb);
        ctx.vars.sb = sb;
      } catch (e) {
        return { steps: { create: { op: "create", raised: excInfo(e) } } };
      }
    }
    for (let i = 0; i < c.steps.length; i++) {
      let step = c.steps[i];
      if (Array.isArray(step)) {
        step = { op: step[0], ...(step.length > 1 ? { args: step[1] } : {}), ...(step.length > 2 ? { kw: step[2] } : {}), ...(step[3] ?? {}) };
      }
      if ((step.skip ?? []).includes("ts")) continue;
      const key = step.as || `s${i}`;
      const rec = { op: step.op };
      try {
        let r = await doOp(ctx, step, created);
        if (step.until) r = await pollUntil(ctx, step, created, r);
        if (step.consume) r = await consume(r);
        if (step.as) ctx.vars[step.as] = r;
        if (step.paginate && !step.until) r = await paginate(r);
        if (!step.quiet) {
          let full = norm(r);
          if (step.grep && Array.isArray(full)) full = full.filter((x) => JSON.stringify(x).includes(step.grep));
          rec.ok = pick(full, step.pick);
        }
        if (step.first && Array.isArray(rec.ok)) rec.ok = rec.ok.length ? rec.ok[0] : null;
        if (step.sort && Array.isArray(rec.ok)) {
          rec.ok = rec.ok.sort((x, y) => (JSON.stringify(x) < JSON.stringify(y) ? -1 : 1));
        }
      } catch (e) {
        if (step.as) ctx.vars[step.as] = e;
        rec.raised = excInfo(e);
      }
      if (step.quiet && !rec.raised) continue;
      out.push([out.some(([k]) => k === key) ? `${key}_${i}` : key, rec]);
    }
  } finally {
    for (const sb of own) await sb.kill().catch(() => {});
  }
  const res = { steps: Object.fromEntries(out) };
  for (const [name, spec] of Object.entries(ctx.cbSpecs)) {
    const got = ctx.collected[name];
    if (!got) continue;
    const joined = got.filter((x) => typeof x === "string").join("");
    if (spec.join) ctx.collected[name] = joined;
    if (spec.probe) ctx.collected[name] = Object.fromEntries(spec.probe.map((p) => [p, joined.includes(p)]));
  }
  for (const k of Object.keys(ctx.collected)) if (k.startsWith("_")) delete ctx.collected[k];
  if (Object.keys(ctx.collected).length) {
    res.collected = Object.fromEntries(
      Object.entries(ctx.collected).map(([k, v]) => [
        k,
        k.endsWith("_set") ? [...new Set(v)].sort() : k.endsWith("_count") ? v.length : v,
      ]),
    );
  }
  return res;
}

function loadCases(p) {
  const files = statSync(p).isDirectory()
    ? readdirSync(p).filter((f) => f.endsWith(".json")).sort().map((f) => path.join(p, f))
    : [p];
  const merged = { cases: [] };
  for (const f of files) {
    const d = JSON.parse(readFileSync(f, "utf8"));
    merged.cases.push(...d.cases);
    for (const [k, v] of Object.entries(d)) if (k !== "cases") merged[k] = v;
  }
  const names = merged.cases.map((c) => c.name);
  if (new Set(names).size !== names.length) throw new Error("duplicate case names");
  return merged;
}

const cases = loadCases(CASES);
const SHARED_TIMEOUT = cases.shared_create?.timeout ?? 900;
const result = { sdk: { lang: LANG, version: pkg.version }, cases: {} };
const created = [];
let shared;
try {
  shared = await Sandbox.create(...(TEMPLATE ? [TEMPLATE] : []), toOpts(cases.shared_create ?? { timeout: 900, metadata: { suite: "cases" } }));
  created.push(shared);
  // Every case works under /tmp/c, writable by any user, whichever case runs first.
  await shared.commands.run("mkdir -p /tmp/c && chmod 777 /tmp/c", { user: "root" });
  for (const c of cases.cases) {
    if ((ONLY.size && !ONLY.has(c.name)) || SKIP.has(c.name)) continue;
    if (TAG && !(c.tags ?? []).includes(TAG)) continue;
    if ((c.skip ?? []).includes(LANG) || c.sync) continue;
    if (c.sdk_min) {
      const have = pkg.version.split(".").map(Number), want = c.sdk_min.split(".").map(Number);
      if (have[0] < want[0] || (have[0] === want[0] && (have[1] < want[1] || (have[1] === want[1] && have[2] < want[2])))) continue;
    }
    const t0 = Date.now();
    result.cases[c.name] = await runCase(c, shared, created);
    process.stderr.write(`${c.name} ${((Date.now() - t0) / 1000).toFixed(1)}s\n`);
  }
} finally {
  for (const sb of created) await sb.kill().catch(() => {});
}
let text = JSON.stringify(result, (k, v) => v, 1);
for (const secret of [process.env.E2B_API_KEY, process.env.E2B_ACCESS_TOKEN]) {
  if (secret && text.includes(secret)) {
    console.error("refusing to print: the output contains the API key");
    process.exit(2);
  }
}
text = text.replace(/e2b_[0-9a-f]{20,}/g, "e2b_<key>");
// Stable key order, like the Python runner's sort_keys.
const sortKeys = (v) =>
  Array.isArray(v) ? v.map(sortKeys) : v && typeof v === "object" ? Object.fromEntries(Object.keys(v).sort().map((k) => [k, sortKeys(v[k])])) : v;
console.log(JSON.stringify(sortKeys(JSON.parse(text)), null, 1));
process.exit(0);
