// Cyclops browser/WASM SDK inside mcp-v8 (r33drichards/mcp-js) — run_js payload.
//
// Verified end to end against stock mcp-v8 v0.18.2 (2026-08-18): module import
// over HTTP, WASM compile + uniffi checksum init, builder contract, and an
// authenticated listPools round trip through the policy-gated fetch.
//
// The prelude below fills the gaps between mcp-v8's bare V8 isolate and the
// browser environment web_sys expects. On mcp-v8 releases newer than v0.18.2
// (with the spec-grade fetch classes) most of it becomes unnecessary; the
// Window shim is always required because the runtime has no `window`.

// ── Prelude part 0: timer clamp ─────────────────────────────────────────
// mcp-v8 v0.18.2's clearTimeout only cancels the JS callback; the underlying
// op_timer_sleep keeps the event loop pending for the full delay. @ubjs/core
// arms a ~25-day keep-alive setTimeout around every async Rust call (a Node
// idiom), which would wedge the isolate forever. Never arm timers that
// outlive any plausible execution. (Fixed upstream in mcp-js by unref'ing
// cleared timers; keep the clamp for older servers.)
{
  const realSetTimeout = globalThis.setTimeout
  let fakeId = -2
  globalThis.setTimeout = function setTimeout(handler, timeout) {
    if (Number(timeout) > 60000) return fakeId--
    return realSetTimeout(handler, timeout)
  }
}

// ── Prelude part 1: Window ──────────────────────────────────────────────
// web_sys::window() is an `instanceof Window` check against js_sys::global().
// Satisfy it without touching the global prototype chain (V8 global proxies
// may have an immutable prototype).
globalThis.Window = class Window {
  static [Symbol.hasInstance](value) {
    return value === globalThis
  }
}

// ── Prelude part 2: Headers (v0.18.2 only) ─────────────────────────────
// v0.18.2 defines Headers inside the fetch bootstrap closure, not as a
// global; BrowserHttpClient needs one for Request construction.
if (typeof globalThis.Headers !== "function") {
  globalThis.Headers = class Headers {
    constructor(init) {
      this._map = new Map()
      if (init && typeof init === "object") {
        for (const key of Object.keys(init)) this.set(key, init[key])
      }
    }
    get(name) {
      const v = this._map.get(String(name).toLowerCase())
      return v === undefined ? null : v
    }
    has(name) {
      return this._map.has(String(name).toLowerCase())
    }
    set(name, value) {
      this._map.set(String(name).toLowerCase(), String(value))
    }
    append(name, value) {
      const k = String(name).toLowerCase()
      const prev = this._map.get(k)
      this._map.set(k, prev === undefined ? String(value) : prev + ", " + String(value))
    }
    delete(name) {
      this._map.delete(String(name).toLowerCase())
    }
    entries() {
      return this._map.entries()
    }
    [Symbol.iterator]() {
      return this._map.entries()
    }
  }
}

// ── Prelude part 3: Request + fetch(Request) (v0.18.2 only) ────────────
// v0.18.2's fetch accepts only a URL string; translate a Request-shaped
// object back into (url, init) form.
if (typeof globalThis.Request !== "function") {
  globalThis.Request = class Request {
    constructor(url, init) {
      this.url = String(url)
      this.init = init || {}
      this.headers = new Headers()
    }
  }
  const coreFetch = globalThis.fetch
  globalThis.fetch = function fetch(input, init) {
    if (input instanceof Request) {
      const headers = {}
      for (const [name, value] of input.headers.entries()) headers[name] = value
      return coreFetch(input.url, {
        method: input.init.method || "GET",
        headers,
        body: input.init.body,
      })
    }
    return coreFetch(input, init)
  }
}

// ── Prelude part 4: Response instanceof (v0.18.2 only) ─────────────────
// The glue's dyn_into::<Response>() checks `instanceof Response`; core fetch
// returns plain response-shaped objects, so accept anything response-shaped.
if (typeof globalThis.Response !== "function") {
  globalThis.Response = class Response {
    static [Symbol.hasInstance](value) {
      return (
        !!value &&
        typeof value === "object" &&
        typeof value.status === "number" &&
        typeof value.arrayBuffer === "function"
      )
    }
  }
}

// ── The actual SDK usage ────────────────────────────────────────────────
// SDK_BASE serves dist-mcp-v8/ (fleet-sdk-web.js + index_bg.wasm);
// API_BASE is the Cyclops backend (the mock, or a real deployment).
const SDK_BASE = "http://127.0.0.1:8788"
const API_BASE = "http://127.0.0.1:8788"
const ACCESS_TOKEN = "test-token"

const sdk = await import(`${SDK_BASE}/fleet-sdk-web.js`)
const wasmBytes = await (await fetch(`${SDK_BASE}/index_bg.wasm`)).arrayBuffer()
await sdk.uniffiInitAsync(wasmBytes)
console.log("wasm init ok: " + wasmBytes.byteLength + " bytes")

const configuration = new sdk.CyclopsTokenProviderConfigurationBuilder()
  .baseUrl(API_BASE)
  .poolPollIntervalMs(5000n)
  .poolPollLimit(120)
  .claimPollIntervalMs(5000n)
  .claimPollLimit(120)
  .build()

const client = sdk.CyclopsClient.connectBrowserWithAccessToken(
  configuration,
  ACCESS_TOKEN,
)

const pools = await client.listPools("test-pool")
console.log("listPools ok: " + JSON.stringify(pools))
