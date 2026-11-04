// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as path from "node:path";
import { describe, expect, it, vi } from "vitest";
import {
  configTelemetryEnabled,
  consentMayHaveChanged,
  crashConsent,
  crashEnv,
  crashReportingAnswer,
  crashTestKind,
  environmentFor,
  onConsentMayHaveChanged,
  parseCrashConfig,
  releaseFor,
  scrubEvent,
  scrubString,
  setCrashSettings,
  type ConsentSource,
} from "../src/crash-reporting";

const HOME = "/Users/jane";

/** A machine with the given files and environment. */
function machine(files: Record<string, string>, env: Record<string, string> = {}): ConsentSource {
  const at = (f: string) => path.normalize(f);
  const fs = new Map(Object.entries(files).map(([k, v]) => [at(k), v]));
  return { env: { HOME, ...env }, read: (f) => fs.get(at(f)) ?? null, exists: (f) => fs.has(at(f)), home: HOME };
}
const NOTICE = { "/Users/jane/.cua/telemetry/notice_shown": "" };

describe("config and names", () => {
  it("reads the DSN, and an empty or broken file is off", () => {
    expect(parseCrashConfig('{"org":"trycua","dsn":" https://k@o1.ingest.sentry.io/2 "}').dsn).toBe("https://k@o1.ingest.sentry.io/2");
    expect(parseCrashConfig('{"dsn":""}').dsn).toBe("");
    expect(parseCrashConfig("not json").dsn).toBe("");
    expect(parseCrashConfig(null).dsn).toBe("");
    expect(parseCrashConfig('{"dsn":5}').dsn).toBe("");
  });

  it("names the release and the channel", () => {
    expect(releaseFor("0.7.2")).toBe("cua-spaces-desktop@0.7.2");
    expect(environmentFor("0.7.2", true)).toBe("stable");
    expect(environmentFor("0.7.2-beta.1", true)).toBe("beta");
    expect(environmentFor("0.7.2", false)).toBe("local");
    expect(environmentFor("0.7.2-beta.1", false)).toBe("local");
  });

  it("gives the Rust side its four variables", () => {
    expect(crashEnv({ dsn: "d", release: "r", environment: "beta" })).toEqual({
      CUA_CRASH_DSN: "d",
      CUA_CRASH_RELEASE: "r",
      CUA_CRASH_ENVIRONMENT: "beta",
      CUA_CRASH_SHELL: "electron",
    });
  });
});

describe("config.toml", () => {
  it("reads [telemetry] enabled as the SDK does", () => {
    expect(configTelemetryEnabled('[telemetry]\nenabled = "off"\n')).toBe(false);
    expect(configTelemetryEnabled('[telemetry]\nenabled = "on" # comment\n')).toBe(true);
    expect(configTelemetryEnabled("[telemetry]\nenabled = false")).toBe(false);
    expect(configTelemetryEnabled("[telemetry]\nenabled = 1")).toBe(true);
    expect(configTelemetryEnabled("telemetry.enabled = 'no'")).toBe(false);
    expect(configTelemetryEnabled('[other]\nenabled = "off"\n[telemetry]\nx = 1')).toBeNull();
    expect(configTelemetryEnabled("")).toBeNull();
    expect(configTelemetryEnabled('[telemetry]\nenabled = "maybe"')).toBeNull();
  });
});

describe("consent", () => {
  it("is on once the notice was shown and nothing turned usage data off", () => {
    expect(crashConsent(machine(NOTICE))).toEqual({ allowed: true, reason: "default" });
  });

  it("waits for the first-run notice", () => {
    expect(crashConsent(machine({}))).toEqual({ allowed: false, reason: "notice_pending" });
    expect(crashConsent(machine({}, { CUA_TELEMETRY: "1" })).allowed).toBe(false);
  });

  it("follows the Settings switch in config.toml", () => {
    const off = machine({ ...NOTICE, "/Users/jane/.cua/config.toml": '[telemetry]\nenabled = "off"\n' });
    expect(crashConsent(off)).toEqual({ allowed: false, reason: "config" });
    const on = machine({ ...NOTICE, "/Users/jane/.cua/config.toml": '[telemetry]\nenabled = "on"\n' });
    expect(crashConsent(on).allowed).toBe(true);
  });

  it("honours DO_NOT_TRACK and CUA_TELEMETRY over the file", () => {
    const on = { ...NOTICE, "/Users/jane/.cua/config.toml": '[telemetry]\nenabled = "on"\n' };
    expect(crashConsent(machine(on, { DO_NOT_TRACK: "1" }))).toEqual({ allowed: false, reason: "do_not_track" });
    expect(crashConsent(machine(on, { DO_NOT_TRACK: "0" })).allowed).toBe(true);
    expect(crashConsent(machine(on, { CUA_TELEMETRY: "off" }))).toEqual({ allowed: false, reason: "env" });
    const off = { ...NOTICE, "/Users/jane/.cua/config.toml": '[telemetry]\nenabled = "off"\n' };
    expect(crashConsent(machine(off, { CUA_TELEMETRY: "1" })).allowed).toBe(true);
    expect(crashConsent(machine(NOTICE, { CUA_TELEMETRY_DISABLED: "1" })).reason).toBe("legacy_env");
    expect(crashConsent(machine(NOTICE, { CUA_TELEMETRY_ENABLED: "false" })).reason).toBe("legacy_env");
  });

  it("is off in CI unless the file or CUA_TELEMETRY says on", () => {
    expect(crashConsent(machine(NOTICE, { GITHUB_ACTIONS: "true" }))).toEqual({ allowed: false, reason: "ci" });
    expect(crashConsent(machine(NOTICE, { CI: "false" })).allowed).toBe(true);
    expect(crashConsent(machine(NOTICE, { CI: "1", CUA_TELEMETRY: "1" })).allowed).toBe(true);
  });

  it("reads $CUA_HOME, and an unreadable file is not consent to turn off", () => {
    const m = machine({ "/tmp/cua/telemetry/notice_shown": "", "/tmp/cua/config.toml": '[telemetry]\nenabled = "off"' }, { CUA_HOME: "/tmp/cua" });
    expect(crashConsent(m).allowed).toBe(false);
    const broken: ConsentSource = { ...machine(NOTICE), read: () => { throw new Error("EACCES"); } };
    expect(crashConsent(broken).allowed).toBe(true);
  });
});

describe("the runtime switch", () => {
  it("tells the listeners, and one that throws does not stop the rest", () => {
    const a = vi.fn(() => { throw new Error("x"); });
    const b = vi.fn();
    const offA = onConsentMayHaveChanged(a);
    const offB = onConsentMayHaveChanged(b);
    consentMayHaveChanged();
    expect([a.mock.calls.length, b.mock.calls.length]).toEqual([1, 1]);
    offA();
    offB();
    consentMayHaveChanged();
    expect([a.mock.calls.length, b.mock.calls.length]).toEqual([1, 1]);
  });
});

describe("what the page is told", () => {
  it("is off without settings", () => {
    setCrashSettings(null);
    expect(crashReportingAnswer(true)).toMatchObject({ allowed: false, dsn: "" });
  });

  it("follows the switch, and offers the UI test only on a local build", () => {
    setCrashSettings({ dsn: "d", release: "cua-spaces-desktop@1.0.0", environment: "stable", uiTest: true });
    expect(crashReportingAnswer(true)).toEqual({ allowed: true, dsn: "d", release: "cua-spaces-desktop@1.0.0", environment: "stable", shell: "electron" });
    expect(crashReportingAnswer(false).allowed).toBe(false);
    setCrashSettings({ dsn: "d", release: "r", environment: "local", uiTest: true });
    expect(crashReportingAnswer(true).crashTest).toBe("ui");
    setCrashSettings(null);
  });

  it("runs a forced crash on a local build only", () => {
    expect(crashTestKind("main-js", "local")).toBe("main-js");
    expect(crashTestKind("main-native", "local")).toBe("main-native");
    expect(crashTestKind("ui", "local")).toBe("ui");
    expect(crashTestKind("main-js", "stable")).toBeNull();
    expect(crashTestKind("main-js", "beta")).toBeNull();
    expect(crashTestKind("nope", "local")).toBeNull();
    expect(crashTestKind(undefined, "local")).toBeNull();
  });
});

describe("scrubbing", () => {
  it("shortens home folders and hides emails", () => {
    expect(scrubString("/Users/jane/Library/Cua/app.asar/main.cjs")).toBe("~/Library/Cua/app.asar/main.cjs");
    expect(scrubString("at /home/bob/.cua/x.js:1")).toBe("at ~/.cua/x.js:1");
    expect(scrubString("C:\\Users\\Jane Doe\\AppData\\x.js")).toBe("~\\AppData\\x.js");
    expect(scrubString("D:/Users/jane/x")).toBe("~/x");
    expect(scrubString("mail jane.doe+x@example.co.uk now")).toBe("mail [email] now");
    expect(scrubString("/opt/jane/x", "/opt/jane")).toBe("~/x");
    expect(scrubString("/Volumes/data/me/x", "/Volumes/data/me/")).toBe("~/x");
  });

  const event = () => ({
    message: "could not open Space 'Jane's Secret'",
    logentry: { message: "x %s", params: ["a@b.co"] },
    user: { id: "1", ip_address: "{{auto}}", email: "a@b.co" },
    server_name: "janes-mbp.local",
    breadcrumbs: [{ message: "click" }],
    attachments: [{ filename: "screen.png" }],
    exception: {
      values: [
        {
          type: "TypeError",
          value: "ENOENT: /Users/jane/Documents/taxes.pdf for jane@example.com",
          module: "main",
          stacktrace: {
            frames: [
              { filename: "/Users/jane/Cua Spaces.app/Contents/Resources/app.asar/dist-electron/main.cjs", function: "open", lineno: 3, vars: { secret: "x" }, context_line: "const secret = 1" },
              { abs_path: "file:///C:/Users/Jane/app/main.cjs?x=1#y", function: "Foo#bar" },
            ],
          },
        },
      ],
    },
    request: { url: "https://cua.ai/a?token=abc#frag", headers: { cookie: "c" }, data: "body" },
    tags: { shell: "electron", where: "/Users/jane/x", who: "a@b.co" },
    extra: { nested: { path: "/home/bob/y", list: ["/Users/jane/z"] } },
    contexts: { app: { app_name: "Cua Spaces", app_version: "1" }, device: { name: "janes-mbp", family: "Desktop" }, runtime: { name: "Electron" } },
  });

  it("clears messages and values, keeps type, module and frames", () => {
    const out = scrubEvent(event(), HOME);
    expect(out.message).toBeUndefined();
    expect(out.logentry).toBeUndefined();
    const v = out.exception!.values![0]!;
    expect(v).toMatchObject({ type: "TypeError", value: "", module: "main" });
    expect(v.stacktrace!.frames![0]).toEqual({ filename: "~/Cua Spaces.app/Contents/Resources/app.asar/dist-electron/main.cjs", function: "open", lineno: 3 });
    expect(v.stacktrace!.frames![1]).toEqual({ abs_path: "file:///~/app/main.cjs", function: "Foo#bar" });
  });

  it("drops the user, breadcrumbs, attachments and the host name", () => {
    const out = scrubEvent(event(), HOME);
    expect(out.user).toBeUndefined();
    expect(out.breadcrumbs).toBeUndefined();
    expect(out.attachments).toBeUndefined();
    expect(out.server_name).toBe("");
    expect(out.contexts).toEqual({ app: { app_version: "1" }, device: { family: "Desktop" }, runtime: { name: "Electron" } });
  });

  it("keeps a URL without its query, and no headers or body", () => {
    expect(scrubEvent(event(), HOME).request).toEqual({ url: "https://cua.ai/a" });
  });

  it("scrubs what is left", () => {
    const out = scrubEvent(event(), HOME);
    expect(out.tags).toEqual({ shell: "electron", where: "~/x", who: "[email]" });
    expect(out.extra).toEqual({ nested: { path: "~/y", list: ["~/z"] } });
  });

  it("leaves nothing identifying anywhere in the result", () => {
    const text = JSON.stringify(scrubEvent(event(), HOME));
    for (const needle of ["jane", "Jane", "bob", "@", "janes-mbp", "taxes", "Secret", "token=abc", "frag"]) expect(text).not.toContain(needle);
  });

  it("does not change the event it was given", () => {
    const e = event();
    scrubEvent(e, HOME);
    expect(e).toEqual(event());
  });
});
