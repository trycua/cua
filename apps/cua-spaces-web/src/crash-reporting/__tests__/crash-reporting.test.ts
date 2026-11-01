// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it, vi } from "vitest";
import type { BridgeMode, DataAdapter } from "../../bridge/adapter";
import type { CrashReportingConfig } from "../../bridge/contracts/host";
import type { HostEvent } from "../../bridge/protocol";
import { crashConfigFromHost } from "../../bridge/ops/telemetry";
import { sentryOptions, startCrashReporting, wantsUiTestCrash, type SentryLike } from "..";
import { scrubEvent, scrubString, stripUrl } from "../scrub";

const ON: CrashReportingConfig = { allowed: true, dsn: "https://k@o1.ingest.sentry.io/2", release: "cua-spaces-desktop@1.0.0", environment: "stable", shell: "electron" };

function fakeSentry() {
  const calls: { init: Record<string, unknown>[]; close: number } = { init: [], close: 0 };
  const sentry: SentryLike = {
    init: (o) => void calls.init.push(o),
    close: () => {
      calls.close++;
      return Promise.resolve(true);
    },
    globalHandlersIntegration: () => "global",
    dedupeIntegration: () => "dedupe",
    linkedErrorsIntegration: () => "linked",
    functionToStringIntegration: () => "fn",
    eventFiltersIntegration: () => "filters",
  };
  return { sentry, calls };
}

function fakeAdapter(mode: BridgeMode, answer: () => Promise<CrashReportingConfig>) {
  const listeners = new Set<(e: HostEvent) => void>();
  const call = vi.fn(async (op: string) => {
    if (op !== "crashReporting.config") throw new Error(`unexpected ${op}`);
    return answer();
  });
  const adapter = { mode, call, subscribe: (l: (e: HostEvent) => void) => (listeners.add(l), () => listeners.delete(l)), dispose: vi.fn() } as unknown as DataAdapter;
  return { adapter, call, emit: (e: HostEvent) => listeners.forEach((l) => l(e)), listeners };
}

describe("the host's answer", () => {
  it("reads as off unless it is complete and allowed", () => {
    expect(crashConfigFromHost(ON)).toEqual(ON);
    expect(crashConfigFromHost({ ...ON, allowed: false }).allowed).toBe(false);
    expect(crashConfigFromHost({ ...ON, dsn: "" }).allowed).toBe(false);
    expect(crashConfigFromHost({ ...ON, release: undefined }).allowed).toBe(false);
    expect(crashConfigFromHost(null).allowed).toBe(false);
    expect(crashConfigFromHost("yes").allowed).toBe(false);
  });

  it("offers the forced crash on a local build only", () => {
    expect(crashConfigFromHost({ ...ON, environment: "local", crashTest: "ui" }).crashTest).toBe("ui");
    expect(crashConfigFromHost({ ...ON, environment: "stable", crashTest: "ui" }).crashTest).toBeUndefined();
    expect(wantsUiTestCrash({ ...ON, environment: "beta", crashTest: "ui" })).toBe(false);
    expect(wantsUiTestCrash({ ...ON, environment: "local", crashTest: "ui" })).toBe(true);
  });
});

describe("the SDK's options", () => {
  it("records errors only, with no breadcrumbs, tracing or user", () => {
    const { sentry } = fakeSentry();
    const o = sentryOptions(sentry, ON, () => true);
    expect(o).toMatchObject({ dsn: ON.dsn, release: ON.release, environment: "stable", sendDefaultPii: false, defaultIntegrations: false, maxBreadcrumbs: 0 });
    expect(o.integrations).toEqual(["global", "dedupe", "linked", "fn", "filters"]);
    expect(o.initialScope).toEqual({ tags: { shell: "electron", component: "ui" } });
    expect(o.tracesSampleRate).toBeUndefined();
    expect((o.beforeBreadcrumb as () => unknown)()).toBeNull();
    expect((o.beforeSendTransaction as () => unknown)()).toBeNull();
  });

  it("scrubs each event, and drops it once the switch is off", () => {
    const { sentry } = fakeSentry();
    let allowed = true;
    const send = sentryOptions(sentry, ON, () => allowed).beforeSend as (e: unknown) => unknown;
    const event = { message: "Space 'Jane'", exception: { values: [{ type: "Error", value: "jane@x.co" }] } };
    expect(JSON.stringify(send(event))).not.toMatch(/Jane|jane/);
    allowed = false;
    expect(send(event)).toBeNull();
  });
});

describe("starting", () => {
  it("does nothing outside the native shells, and never creates an adapter there", async () => {
    for (const mode of ["demo", "tauri"] as const) {
      const load = vi.fn();
      expect(await startCrashReporting({ mode, load })).toBeNull();
      expect(load).not.toHaveBeenCalled();
    }
  });

  it.each(["electron", "webkit"] as const)("%s: stays off, without loading the SDK, when the host says no", async (mode) => {
    const load = vi.fn();
    const { adapter } = fakeAdapter(mode, async () => ({ ...ON, allowed: false }));
    expect(await startCrashReporting({ adapter, load })).toBeNull();
    expect(load).not.toHaveBeenCalled();
  });

  it("stays off when the host fails or has no DSN", async () => {
    const load = vi.fn();
    expect(await startCrashReporting({ adapter: fakeAdapter("webkit", () => Promise.reject(new Error("unimplemented"))).adapter, load })).toBeNull();
    expect(await startCrashReporting({ adapter: fakeAdapter("electron", async () => ({ ...ON, dsn: "" })).adapter, load })).toBeNull();
    expect(load).not.toHaveBeenCalled();
  });

  it("stays off when the SDK fails to load", async () => {
    const { adapter } = fakeAdapter("electron", async () => ON);
    expect(await startCrashReporting({ adapter, load: () => Promise.reject(new Error("offline")) })).toBeNull();
  });

  it("starts the SDK when allowed, and closes it when the switch goes off", async () => {
    const { sentry, calls } = fakeSentry();
    let answer = ON;
    const { adapter, emit, listeners } = fakeAdapter("electron", async () => answer);
    const handle = await startCrashReporting({ adapter, load: async () => sentry });
    expect(handle).not.toBeNull();
    expect(calls.init).toHaveLength(1);
    // Another event changes nothing; the switch off closes it.
    emit({ type: "session.signedOut" } as HostEvent);
    await Promise.resolve();
    expect(calls.close).toBe(0);
    answer = { ...ON, allowed: false };
    emit({ type: "settings.changed" });
    await vi.waitFor(() => expect(calls.close).toBe(1));
    expect(listeners.size).toBe(0);
    const send = calls.init[0]!.beforeSend as (e: unknown) => unknown;
    expect(send({})).toBeNull();
  });

  it("stop is safe to call twice", async () => {
    const { sentry, calls } = fakeSentry();
    const { adapter } = fakeAdapter("webkit", async () => ON);
    const handle = await startCrashReporting({ adapter, load: async () => sentry });
    handle!.stop();
    handle!.stop();
    expect(calls.close).toBe(1);
  });

  it("runs the forced crash on a local build that asks for it", async () => {
    vi.useFakeTimers();
    try {
      const crash = vi.fn();
      const { sentry } = fakeSentry();
      await startCrashReporting({ adapter: fakeAdapter("electron", async () => ({ ...ON, environment: "local", crashTest: "ui" })).adapter, load: async () => sentry, crash });
      expect(crash).not.toHaveBeenCalled();
      vi.advanceTimersByTime(3500);
      expect(crash).toHaveBeenCalledTimes(1);
      const other = vi.fn();
      await startCrashReporting({ adapter: fakeAdapter("electron", async () => ({ ...ON, crashTest: "ui" })).adapter, load: async () => sentry, crash: other });
      vi.advanceTimersByTime(3500);
      expect(other).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("scrubbing", () => {
  it("shortens home folders, hides emails and cuts query strings", () => {
    expect(scrubString("/Users/jane/a.js")).toBe("~/a.js");
    expect(scrubString("C:\\Users\\Jane Doe\\a.js")).toBe("~\\a.js");
    expect(scrubString("mail a.b+c@example.com")).toBe("mail [email]");
    expect(stripUrl("app://x/spaces/1?token=a#h")).toBe("app://x/spaces/1");
  });

  const event = () => ({
    message: "Open 'Jane's Space'",
    logentry: { message: "x" },
    user: { id: "1", ip_address: "{{auto}}" },
    server_name: "janes-mbp",
    breadcrumbs: [{ message: "ui.click" }],
    exception: {
      values: [
        {
          type: "TypeError",
          value: "bad file /Users/jane/x.txt",
          module: "ui",
          stacktrace: { frames: [{ filename: "app://cua/assets/index.js?v=1", abs_path: "app://cua/assets/index.js#x", function: "go", lineno: 4, vars: { a: 1 }, context_line: "secret" }] },
        },
      ],
    },
    request: { url: "app://cua/spaces/s1?name=Jane#frag", headers: { "User-Agent": "x" }, data: "b" },
    tags: { shell: "swift", path: "/home/bob/x" },
    contexts: { device: { name: "janes-mbp", family: "Mac" }, browser: { name: "Safari" } },
  });

  it("keeps the type, module and frames, and clears everything else that names someone", () => {
    const out = scrubEvent(event());
    expect(out.exception!.values![0]).toEqual({
      type: "TypeError",
      value: "",
      module: "ui",
      stacktrace: { frames: [{ filename: "app://cua/assets/index.js", abs_path: "app://cua/assets/index.js", function: "go", lineno: 4 }] },
    });
    expect(out.request).toEqual({ url: "app://cua/spaces/s1" });
    expect(out.tags).toEqual({ shell: "swift", path: "~/x" });
    expect(out.contexts).toEqual({ device: { family: "Mac" }, browser: { name: "Safari" } });
    expect(out).toMatchObject({ server_name: "" });
    for (const k of ["message", "logentry", "user", "breadcrumbs"]) expect(out).not.toHaveProperty(k);
    expect(JSON.stringify(out)).not.toMatch(/jane|Jane|bob|frag|token/);
  });

  it("does not change the event it was given", () => {
    const e = event();
    scrubEvent(e);
    expect(e).toEqual(event());
  });
});
