// The `cua` API a run_script script sees. Evaluated once before the script,
// as prelude.js, so stack frames from script.js are always the caller's.
//
// The only bridge to the driver is `__host(op, argsJson, line)`, which this
// prelude captures and then deletes from the global object.
(() => {
  "use strict";
  const host = globalThis.__host;
  const finish = globalThis.__done;
  const emit = globalThis.__log;
  delete globalThis.__host;
  delete globalThis.__done;
  delete globalThis.__log;

  // Line of the innermost script.js frame that led to this call.
  const scriptLine = (stack) => {
    const match = /script\.js:(\d+)/.exec(String(stack || ""));
    return match ? Number(match[1]) : 0;
  };

  const call = (op, args) => {
    const line = scriptLine(new Error().stack);
    const raw = host(op, JSON.stringify(args === undefined ? {} : args), line);
    const reply = JSON.parse(raw);
    if (!reply.ok) {
      const error = new Error(`${op} failed: ${reply.error}`);
      error.name = "CuaError";
      error.op = op;
      error.code = reply.code || null;
      error.call = reply.call || null;
      error.line = line || null;
      throw error;
    }
    return reply.result;
  };

  const show = (value) => {
    if (typeof value === "string") return value;
    if (value instanceof Error) return `${value.name}: ${value.message}`;
    try {
      const text = JSON.stringify(value);
      return text === undefined ? String(value) : text;
    } catch (_) {
      return String(value);
    }
  };
  const console = {};
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    console[level] = (...values) => emit(level, values.map(show).join(" "));
  }
  globalThis.console = console;

  // An element target: a token string, an element row number from this
  // app's last getState(), [x, y] window pixels, or {role, name, nth}.
  const target = (app, value) => {
    if (value === undefined || value === null) return {};
    if (typeof value === "string") return { element_token: value };
    if (typeof value === "number") {
      if (!app.snapshotId) {
        throw new Error(
          `element ${value} needs a getState() first, or pass a token string or {role, name}`,
        );
      }
      return { element_token: `${app.snapshotId}:${value}` };
    }
    if (Array.isArray(value)) return { x: value[0], y: value[1] };
    if (typeof value === "object") {
      const out = {};
      for (const key of ["role", "name", "label", "nth", "element_token", "x", "y", "timeout_ms"]) {
        if (value[key] !== undefined) out[key] = value[key];
      }
      return out;
    }
    throw new Error(`cannot target ${show(value)}`);
  };

  const keysOf = (keys) =>
    Array.isArray(keys) ? keys : String(keys).split("+").map((key) => key.trim()).filter(Boolean);

  class App {
    constructor(info) {
      this.pid = info.pid;
      this.windowId = info.window_id;
      this.name = info.app || null;
      this.title = info.title || null;
      this.snapshotId = null;
      // Pinned to one window only when the caller asked for one; otherwise
      // every call follows the app's frontmost window (dialogs included).
      this.pinned = Boolean(info.pinned);
    }
    where() {
      return this.pinned ? { pid: this.pid, window_id: this.windowId } : { pid: this.pid };
    }
    async window() {
      const info = call("get_app", this.where());
      this.windowId = info.window_id;
      this.title = info.title || this.title;
      return info;
    }
    async getState(options = {}) {
      const info = await this.window();
      const state = call("get_window_state", { pid: this.pid, window_id: info.window_id, ...options });
      if (state && state.snapshot_id) this.snapshotId = state.snapshot_id;
      return state;
    }
    async query(selector = {}) {
      return call("find", { ...this.where(), ...selector });
    }
    async waitFor(selector, options = {}) {
      return call("wait_for", { ...this.where(), ...selector, ...options });
    }
    async verify(expect, options = {}) {
      const info = await this.window();
      return call("verify_state", {
        pid: this.pid,
        window_id: info.window_id,
        expect: Array.isArray(expect) ? expect : [expect],
        ...options,
      });
    }
    async click(on, options = {}) {
      return call("click", { ...this.where(), ...target(this, on), ...options });
    }
    async doubleClick(on, options = {}) {
      return call("double_click", { ...this.where(), ...target(this, on), ...options });
    }
    async rightClick(on, options = {}) {
      return call("right_click", { ...this.where(), ...target(this, on), ...options });
    }
    async typeText(text, on, options = {}) {
      return call("type_text", { ...this.where(), ...target(this, on), text: String(text), ...options });
    }
    async pressKey(key, on, options = {}) {
      const parts = keysOf(key);
      if (parts.length > 1) return this.hotkey(parts, on, options);
      return call("press_key", { ...this.where(), ...target(this, on), key: parts[0], ...options });
    }
    async hotkey(keys, on, options = {}) {
      return call("hotkey", { ...this.where(), ...target(this, on), keys: keysOf(keys), ...options });
    }
    async scroll(on, direction = "down", amount, options = {}) {
      const args = { ...this.where(), ...target(this, on), direction, ...options };
      if (amount !== undefined) args.amount = amount;
      return call("scroll", args);
    }
    async setValue(on, value, options = {}) {
      return call("set_value", { ...this.where(), ...target(this, on), value: String(value), ...options });
    }
    async drag(from, to, options = {}) {
      const info = await this.window();
      return call("drag", {
        pid: this.pid,
        window_id: info.window_id,
        from_x: from[0],
        from_y: from[1],
        to_x: to[0],
        to_y: to[1],
        ...options,
      });
    }
    async zoom(rect, options = {}) {
      const info = await this.window();
      const [x1, y1, x2, y2] = Array.isArray(rect) ? rect : [rect.x1, rect.y1, rect.x2, rect.y2];
      return call("zoom", { pid: this.pid, window_id: info.window_id, x1, y1, x2, y2, ...options });
    }
  }

  const appFrom = (info, pinned) => new App({ ...info, pinned });

  const cua = {
    computer: { target: JSON.parse(host("platform", "{}", 0)).result },
    async getApp(which) {
      if (which === undefined || which === null) throw new Error("getApp needs an app name, bundle id, or {pid, windowId}");
      if (typeof which === "string") return appFrom(call("get_app", { app: which }), false);
      const args = {};
      if (which.pid !== undefined) args.pid = which.pid;
      if (which.windowId !== undefined) args.window_id = which.windowId;
      if (which.window_id !== undefined) args.window_id = which.window_id;
      if (which.app !== undefined) args.app = which.app;
      if (which.title !== undefined) args.window = which.title;
      return appFrom(call("get_app", args), args.window_id !== undefined || args.window !== undefined);
    },
    async launch(which, options = {}) {
      const args = typeof which === "string"
        ? (/^[\w-]+(\.[\w-]+)+$/.test(which) ? { bundle_id: which } : { name: which })
        : { ...which };
      const launched = call("launch_app", { ...args, ...options });
      const pid = launched && launched.pid;
      if (!pid) return launched;
      return appFrom(call("get_app", { pid, timeout_ms: 5000 }), false);
    },
    async listApps(options = {}) {
      const listed = call("list_apps", options);
      return (listed && listed.apps) || listed;
    },
    async listWindows(options = {}) {
      const listed = call("list_windows", options);
      return (listed && listed.windows) || listed;
    },
    async sleep(ms) {
      return call("sleep", { ms });
    },
    /** Any allowed driver tool by name, with its own arguments. */
    async call(tool, args = {}) {
      return call(String(tool), args);
    },
  };
  Object.freeze(cua.computer);
  Object.freeze(cua);
  Object.defineProperty(globalThis, "cua", { value: cua, writable: false, configurable: false });

  globalThis.__settle = (promise) =>
    promise.then(
      (value) => {
        let encoded;
        try {
          encoded = JSON.stringify({ ok: true, value: value === undefined ? null : value });
        } catch (error) {
          encoded = JSON.stringify({ ok: true, value: show(value), note: `not JSON: ${error}` });
        }
        finish(encoded);
      },
      (error) => {
        const isError = error instanceof Error;
        finish(
          JSON.stringify({
            ok: false,
            name: isError ? error.name : null,
            message: isError ? error.message : show(error),
            line: (isError && (scriptLine(error.stack) || error.line)) || null,
            op: (isError && error.op) || null,
            code: (isError && error.code) || null,
            call: (isError && error.call) || null,
          }),
        );
      },
    );
})();
