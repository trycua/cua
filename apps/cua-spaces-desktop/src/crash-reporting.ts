// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Crash reporting, the pure half (CUA-2343): the consent rule, the release
// and environment names, the Rust side's environment and the privacy scrub.
// No Electron and no Sentry here, so it all runs under test; the SDK is wired
// in crash-reporting-main.ts.
//
// Reports are allowed only when the "Share usage data" switch is on and its
// first-run notice was shown: the decision `cua-telemetry` makes
// (libs/cua/crates/cua-telemetry/src/config.rs, notice.rs). The app starts
// Sentry before the native library loads, so the switch is read here from the
// same places instead of through `telemetryStatus()`.
import * as path from "node:path";

/** `config/spaces-crash-reporting.json`, as bundled at build time. */
export interface CrashConfig {
  dsn: string;
}

/** The DSN from the config file's text; empty (reporting off) when it is missing or malformed. */
export function parseCrashConfig(text: string | null | undefined): CrashConfig {
  try {
    const dsn = (JSON.parse(text ?? "") as { dsn?: unknown }).dsn;
    return { dsn: typeof dsn === "string" ? dsn.trim() : "" };
  } catch {
    return { dsn: "" };
  }
}

/* ---- Names ------------------------------------------------------------------------ */

/** `cua-spaces-desktop@<version>`, the string the update feed carries. */
export const releaseFor = (version: string): string => `cua-spaces-desktop@${version}`;

export type CrashEnvironment = "stable" | "beta" | "local";

/** The channel: an unpackaged build is `local`, a version with a `-` suffix `beta`, anything else `stable`. */
export function environmentFor(version: string, packaged: boolean): CrashEnvironment {
  if (!packaged) return "local";
  return version.includes("-") ? "beta" : "stable";
}

/** What the Rust core and daemon read (`cua-crash`), set before either starts. */
export function crashEnv(o: { dsn: string; release: string; environment: string }): Record<string, string> {
  return {
    CUA_CRASH_DSN: o.dsn,
    CUA_CRASH_RELEASE: o.release,
    CUA_CRASH_ENVIRONMENT: o.environment,
    CUA_CRASH_SHELL: "electron",
  };
}

/* ---- Consent ---------------------------------------------------------------------- */

type Env = Readonly<Record<string, string | undefined>>;

/** `0/false/no/off/disable(d)` is false, `1/true/yes/on/enable(d)` true (config.rs `parse_bool`). */
export function parseBool(value: string): boolean | null {
  switch (value.trim().toLowerCase()) {
    case "0": case "false": case "no": case "off": case "disable": case "disabled":
      return false;
    case "1": case "true": case "yes": case "on": case "enable": case "enabled":
      return true;
    default:
      return null;
  }
}

/**
 * `[telemetry] enabled` of a config.toml: a boolean, a string such as "off"
 * or an integer. The file is the one `cua config set telemetry off` and
 * Settings write; only that key is read, so this is a line scan, not TOML.
 */
export function configTelemetryEnabled(toml: string): boolean | null {
  let table = "";
  for (const raw of toml.split(/\r?\n/)) {
    const line = raw.replace(/\s+#.*$/, "").trim();
    if (!line || line.startsWith("#")) continue;
    const header = /^\[\s*([^\]]+?)\s*\]$/.exec(line);
    if (header) {
      table = header[1] ?? "";
      continue;
    }
    const kv = /^([A-Za-z0-9_.-]+)\s*=\s*(.+)$/.exec(line);
    if (!kv) continue;
    const key = table ? `${table}.${kv[1]}` : (kv[1] ?? "");
    if (key !== "telemetry.enabled") continue;
    const value = (kv[2] ?? "").trim();
    if (value === "true") return true;
    if (value === "false") return false;
    if (/^-?\d+$/.test(value)) return Number(value) !== 0;
    const str = /^["'](.*)["']$/.exec(value);
    return str ? parseBool(str[1] ?? "") : null;
  }
  return null;
}

const CI_VARIABLES = [
  "CI", "CONTINUOUS_INTEGRATION", "GITHUB_ACTIONS", "GITLAB_CI", "JENKINS_URL", "CIRCLECI", "BUILDKITE", "TF_BUILD",
  "TEAMCITY_VERSION", "TRAVIS", "APPVEYOR", "BITBUCKET_BUILD_NUMBER", "CODEBUILD_BUILD_ID", "DRONE", "HUDSON_URL",
];

/** What the consent check reads from the machine. */
export interface ConsentSource {
  env: Env;
  /** The file's text, or null when it is missing. */
  read(file: string): string | null;
  exists(file: string): boolean;
  /** The user's home directory. */
  home: string;
}

export interface Consent {
  allowed: boolean;
  /** The first rule that decided, for logs and tests. */
  reason: "do_not_track" | "env" | "legacy_env" | "config" | "ci" | "default" | "notice_pending";
}

/** `$CUA_HOME`, else `~/.cua` (config.rs `cua_home`). */
export function cuaHome(s: Pick<ConsentSource, "env" | "home">): string {
  const set = s.env.CUA_HOME?.trim();
  return set ? set : path.join(s.env.HOME?.trim() || s.env.USERPROFILE?.trim() || s.home, ".cua");
}

/**
 * Whether crash reports may be sent now: the telemetry decision
 * (`DO_NOT_TRACK`, `CUA_TELEMETRY`, the legacy switches, config.toml, a CI
 * environment, on by default) says on, and the first-run notice marker
 * exists. Cheap enough to run for every event.
 */
export function crashConsent(s: ConsentSource): Consent {
  const { env } = s;
  const no = (reason: Consent["reason"]): Consent => ({ allowed: false, reason });
  const dnt = env.DO_NOT_TRACK?.trim();
  if (dnt && dnt !== "0" && dnt.toLowerCase() !== "false") return no("do_not_track");
  const forced = env.CUA_TELEMETRY === undefined ? null : parseBool(env.CUA_TELEMETRY);
  let enabled: boolean;
  let reason: Consent["reason"];
  if (forced !== null) {
    enabled = forced;
    reason = "env";
  } else if (parseBool(env.CUA_TELEMETRY_ENABLED ?? "") === false || parseBool(env.CUA_TELEMETRY_DISABLED ?? "") === true) {
    return no("legacy_env");
  } else {
    const home = cuaHome(s);
    let fromConfig: boolean | null = null;
    try {
      const text = s.read(path.join(home, "config.toml"));
      fromConfig = text === null ? null : configTelemetryEnabled(text);
    } catch {
      fromConfig = null;
    }
    const ci = CI_VARIABLES.some((v) => {
      const x = env[v]?.trim();
      return !!x && x.toLowerCase() !== "false" && x !== "0";
    });
    enabled = fromConfig ?? !ci;
    reason = fromConfig !== null ? "config" : ci ? "ci" : "default";
  }
  if (!enabled) return no(reason);
  let shown = false;
  try {
    shown = s.exists(path.join(cuaHome(s), "telemetry", "notice_shown"));
  } catch {
    shown = false;
  }
  return shown ? { allowed: true, reason } : no("notice_pending");
}

/* ---- Runtime switch ----------------------------------------------------------------- */

const watchers = new Set<() => void>();

/** `listener` runs whenever the usage-data switch may have changed (the Sentry wiring closes the SDK then). */
export function onConsentMayHaveChanged(listener: () => void): () => void {
  watchers.add(listener);
  return () => watchers.delete(listener);
}

/** The bridge calls this after the page changes the switch. */
export function consentMayHaveChanged(): void {
  for (const w of [...watchers]) {
    try {
      w();
    } catch {
      // Reporting never breaks Settings.
    }
  }
}

/* ---- Privacy ------------------------------------------------------------------------ */

const EMAIL = /[A-Za-z0-9._%+'-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g;
const HOME_UNIX = /(?:\/Users|\/home)\/[^/\\\s"'`:;,)\]}]+/g;
const HOME_WIN = /[A-Za-z]:[\\/]+Users[\\/]+[^\\/"'`:;,)\]}]+/gi;

const escapeRegExp = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Home folders (`/Users/x`, `/home/x`, `C:\Users\x`, the real $HOME) become `~`, emails `[email]`. */
export function scrubString(value: string, home?: string): string {
  let out = value.replace(HOME_WIN, "~");
  for (const h of new Set([home, home?.replace(/\\/g, "/")])) {
    if (h && h.length > 1) out = out.replace(new RegExp(escapeRegExp(h.replace(/[\\/]+$/, "")), "gi"), "~");
  }
  return out.replace(HOME_UNIX, "~").replace(EMAIL, "[email]");
}

/** A URL without its query string and fragment. */
export function stripUrl(url: string): string {
  return url.replace(/[?#].*$/, "");
}

/** Strings that name no one: keys and values that are URLs lose their query and fragment. */
function scrubDeep(value: unknown, home: string | undefined, depth = 0): unknown {
  if (typeof value === "string") {
    const s = /^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? stripUrl(value) : value;
    return scrubString(s, home);
  }
  if (depth > 12 || value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map((v) => scrubDeep(v, home, depth + 1));
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(value)) out[k] = scrubDeep(v, home, depth + 1);
  return out;
}

/** The parts of a Sentry event the scrub touches (the SDK's own types are wider). */
export interface ScrubbableEvent {
  message?: string;
  logentry?: unknown;
  exception?: { values?: { type?: string; value?: string; module?: string; stacktrace?: { frames?: Record<string, unknown>[] } }[] };
  [key: string]: unknown;
}

/**
 * The event as it may leave the machine. Exception values (messages), the
 * message and log entry are cleared whatever they hold: they can carry Space
 * names, file names and emails. Type, module and stack frames stay, with
 * paths scrubbed. Everything else is scrubbed or dropped: user, breadcrumbs,
 * attachments, request data beyond a query-free URL, server name, device
 * name, app name. Returns a new object.
 */
export function scrubEvent<T extends ScrubbableEvent>(event: T, home?: string): T {
  const e: ScrubbableEvent = { ...event };
  delete e.user;
  delete e.breadcrumbs;
  delete e.logentry;
  delete e.message;
  delete e.server_name;
  delete e.attachments;
  delete e.threads;
  e.server_name = "";
  if (e.exception?.values) {
    e.exception = {
      ...e.exception,
      values: e.exception.values.map((v) => ({
        ...v,
        value: "",
        ...(v.stacktrace?.frames ? { stacktrace: { ...v.stacktrace, frames: v.stacktrace.frames.map((f) => scrubFrame(f, home)) } } : {}),
      })),
    };
  }
  if (e.request && typeof e.request === "object") {
    const url = (e.request as { url?: unknown }).url;
    e.request = typeof url === "string" ? { url: scrubString(stripUrl(url), home) } : {};
  }
  for (const key of ["tags", "extra", "contexts", "debug_meta", "modules", "sdk", "fingerprint", "transaction", "culprit"]) {
    if (e[key] !== undefined) e[key] = scrubDeep(e[key], home);
  }
  const contexts = e.contexts as Record<string, Record<string, unknown>> | undefined;
  if (contexts) {
    if (contexts.app) delete contexts.app.app_name;
    if (contexts.device) {
      delete contexts.device.name;
      delete contexts.device.hostname;
    }
    delete contexts.culture;
    delete contexts.cloud_resource;
  }
  return e as T;
}

function scrubFrame(frame: Record<string, unknown>, home: string | undefined): Record<string, unknown> {
  const out: Record<string, unknown> = { ...frame };
  for (const k of ["filename", "abs_path"]) {
    if (typeof out[k] === "string") out[k] = scrubString(stripUrl(out[k] as string), home);
  }
  for (const k of ["module", "function"]) {
    if (typeof out[k] === "string") out[k] = scrubString(out[k] as string, home);
  }
  // Source lines and local variables are content, not code location.
  for (const k of ["context_line", "pre_context", "post_context", "vars"]) delete out[k];
  return out;
}

/* ---- What the page is told ------------------------------------------------------------ */

/** The settings this process started crash reporting with (null: not configured or not started). */
export interface CrashSettings {
  dsn: string;
  release: string;
  environment: CrashEnvironment;
  /** CUA_CRASH_TEST=ui, honoured on a local build only. */
  uiTest: boolean;
}

let settings: CrashSettings | null = null;

export function setCrashSettings(next: CrashSettings | null): void {
  settings = next;
}

/** The answer to the page's `crashReporting.config`: `allowed` follows the switch as it is now. */
export function crashReportingAnswer(switchOn: boolean): {
  allowed: boolean;
  dsn: string;
  release: string;
  environment: string;
  shell: "electron";
  crashTest?: "ui";
} {
  if (!settings || !settings.dsn) return { allowed: false, dsn: "", release: "", environment: "", shell: "electron" };
  return {
    allowed: switchOn,
    dsn: settings.dsn,
    release: settings.release,
    environment: settings.environment,
    shell: "electron",
    ...(settings.uiTest && settings.environment === "local" ? { crashTest: "ui" as const } : {}),
  };
}

/** The forced crash a local build runs on request (CUA_CRASH_TEST); anything else, or any other channel, is none. */
export function crashTestKind(value: string | undefined, environment: string): "main-js" | "main-native" | "ui" | null {
  if (environment !== "local") return null;
  return value === "main-js" || value === "main-native" || value === "ui" ? value : null;
}
