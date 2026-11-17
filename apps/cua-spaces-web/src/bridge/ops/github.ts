// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Connections → GitHub: the sign-in the host uses when an agent
 * asks to push a branch and open a pull request. The cua daemon decides it
 * (`cua_spaces::agents::github_auth`): a token saved in Connections, `GH_TOKEN`
 * in the daemon's environment, then the GitHub CLI (`gh auth login`), which
 * can hold several accounts and several hosts (GitHub Enterprise). Each
 * operation is one of the daemon's app methods over `SpaceService.CallSpaceTool`
 * (`github.status`, `github.rescan`, `github.use`, `github.enable`). They are
 * not MCP tools: no agent can read or change the connection.
 *
 * Nothing here ever receives a token: an answer names the hosts, the
 * accounts, the source in use and the account's scopes. A token that is
 * pasted goes in through `agentKeys.set` (`GITHUB_TOKEN`), as before.
 *
 * | Operation | Tauri (`agents_tool`) | SwiftUI (`WebUIBridge+Pages.swift`) |
 * |---|---|---|
 * | `github.status` | `github.status` | `github.status` → `PersistentModel.daemonTool` |
 * | `github.rescan` | `github.rescan` | `github.rescan` |
 * | `github.use {host, login?}` | `github.use` | `github.use` |
 * | `github.enable {host, enabled}` | `github.enable` | `github.enable` |
 *
 * The Electron shell answers the SwiftUI host's methods.
 */

import type { OpCoverage } from "../coverage";
import type { OpArgs, OpResult } from "../protocol";

/** Which source of the chain gave the sign-in. */
export type GithubSource = "saved" | "env" | "gh" | "git";

export interface GithubAccount {
  login: string;
  /** gh's own account for the host. */
  active: boolean;
  /** The one that will be used. */
  selected: boolean;
  /** You chose it. */
  chosen: boolean;
  /** GitHub still accepts the sign-in. */
  valid: boolean;
  scopes: string[];
}

export interface GithubSourceRow {
  id: GithubSource;
  used: boolean;
  present: boolean;
  line: string;
}

export interface GithubHost {
  host: string;
  enabled: boolean;
  source?: GithubSource;
  login?: string;
  envName?: string;
  /** The state in plain words ("Signed in as ada with gh"). */
  line: string;
  problem: boolean;
  /** The token can do more than a push and a pull request need. */
  warning?: string;
  accounts: GithubAccount[];
  sources: GithubSourceRow[];
}

/** `github.status`, as the page reads it (and as the core's `githubConnection.view` takes it). */
export interface GithubReport {
  hosts: GithubHost[];
  ghFound: boolean;
  /** gh is older than 2.81 (no account list as JSON). */
  ghOld: boolean;
  /** What you chose can be kept on this computer. */
  canSave: boolean;
  scannedMs: number;
}

declare module "../protocol" {
  interface HostOperations {
    /** What is signed in on each host and which account will be used; no network, no token. */
    "github.status": { args: Record<string, never>; result: GithubReport };
    /** The same after looking again (gh's accounts, and what a saved or environment token may do). */
    "github.rescan": { args: Record<string, never>; result: GithubReport };
    /** Uses this gh account on a host; without `login`, gh's own. Answers the status after. */
    "github.use": { args: { host: string; login?: string | null }; result: GithubReport };
    /** Switches a host on or off: off, Cua uses no GitHub sign-in there. Answers the status after. */
    "github.enable": { args: { host: string; enabled: boolean }; result: GithubReport };
  }
}

export const GITHUB_OPERATIONS = ["github.status", "github.rescan", "github.use", "github.enable"] as const;
export type GithubOp = (typeof GITHUB_OPERATIONS)[number];

/** The SwiftUI host routes each under its own name. */
export const GITHUB_WEBKIT_METHODS = GITHUB_OPERATIONS;

export const GITHUB_COVERAGE = {
  "github.status": { webkit: { methods: ["github.status"] }, tauri: ["agents_tool"] },
  "github.rescan": { webkit: { methods: ["github.rescan"] }, tauri: ["agents_tool"] },
  "github.use": { webkit: { methods: ["github.use"] }, tauri: ["agents_tool"] },
  "github.enable": { webkit: { methods: ["github.enable"] }, tauri: ["agents_tool"] },
} as const satisfies Record<GithubOp, OpCoverage>;

/** Arguments that take each operation down its usual path (the coverage test). */
export const GITHUB_ARGS: { [K in GithubOp]: OpArgs<K> } = {
  "github.status": {},
  "github.rescan": {},
  "github.use": { host: "github.com", login: "ada" },
  "github.enable": { host: "github.com", enabled: false },
};

/* ---- The daemon's answer ---------------------------------------------------------- */

const SOURCES = ["saved", "env", "gh", "git"] as const;
const sourceOf = (v: unknown): GithubSource | undefined => SOURCES.find((s) => s === v);
/** One short plain line, else none. */
const line = (v: unknown): string => (typeof v === "string" && v.length <= 700 && !/[\u0000-\u001f]/.test(v) ? v.trim() : "");
const name = (v: unknown): string => (typeof v === "string" && /^[A-Za-z0-9._\-[\]]{1,100}$/.test(v) ? v : "");
const hostName = (v: unknown): string => (typeof v === "string" && /^[A-Za-z0-9.-]{1,253}$/.test(v) ? v : "");
const rec = (v: unknown): Record<string, unknown> => (v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/** The daemon's `Report` (snake_case) as the page reads it. Anything that is not a
 * name or a plain line is dropped, so a host that answered a token would not show one. */
export function githubFromWire(raw: unknown): GithubReport {
  const o = rec(raw);
  const gh = rec(o.gh);
  const hosts = arr(o.hosts).flatMap((h): GithubHost[] => {
    const r = rec(h);
    const host = hostName(r.host);
    if (!host) return [];
    const source = sourceOf(r.source);
    const login = name(r.login);
    const envName = typeof r.env_name === "string" && /^[A-Z_]{1,40}$/.test(r.env_name) ? r.env_name : "";
    const warning = line(r.warning);
    return [
      {
        host,
        enabled: r.enabled !== false,
        ...(source ? { source } : {}),
        ...(login ? { login } : {}),
        ...(envName ? { envName } : {}),
        line: line(r.line),
        problem: r.problem === true,
        ...(warning ? { warning } : {}),
        accounts: arr(r.accounts).flatMap((a): GithubAccount[] => {
          const x = rec(a);
          const l = name(x.login);
          return l
            ? [
                {
                  login: l,
                  active: x.active === true,
                  selected: x.selected === true,
                  chosen: x.chosen === true,
                  valid: x.valid !== false,
                  scopes: arr(x.scopes).filter((s): s is string => typeof s === "string" && /^[A-Za-z0-9:_.-]{1,40}$/.test(s)),
                },
              ]
            : [];
        }),
        sources: arr(r.sources).flatMap((s): GithubSourceRow[] => {
          const x = rec(s);
          const id = sourceOf(x.id);
          return id ? [{ id, used: x.used === true, present: x.present === true, line: line(x.line) }] : [];
        }),
      },
    ];
  });
  return {
    hosts,
    ghFound: gh.found === true,
    ghOld: gh.old === true,
    canSave: o.can_save === true,
    scannedMs: typeof o.scanned_ms === "number" ? o.scanned_ms : 0,
  };
}

type Handlers = { [K in GithubOp]: (args: OpArgs<K>) => Promise<OpResult<K>> };
type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Request = <T>(method: GithubOp, args?: Record<string, unknown>) => Promise<T>;

const useArgs = ({ host, login }: OpArgs<"github.use">) => ({ host, ...(login ? { login } : {}) });

/** Tauri: the daemon's methods through `agents_tool` (the shell allows them). */
export function tauriGithubOps(invoke: Invoke): Handlers {
  const tool = async (op: GithubOp, args: Record<string, unknown> = {}) => githubFromWire(await invoke<unknown>("agents_tool", { tool: op, args }));
  return {
    "github.status": () => tool("github.status"),
    "github.rescan": () => tool("github.rescan"),
    "github.use": (a) => tool("github.use", useArgs(a)),
    "github.enable": ({ host, enabled }) => tool("github.enable", { host, enabled }),
  };
}

/** The SwiftUI host: each method answers the daemon's JSON as is. */
export function webkitGithubOps(request: Request): Handlers {
  const call = async (op: GithubOp, args: Record<string, unknown> = {}) => githubFromWire(await request<unknown>(op, args));
  return {
    "github.status": () => call("github.status"),
    "github.rescan": () => call("github.rescan"),
    "github.use": (a) => call("github.use", useArgs(a)),
    "github.enable": ({ host, enabled }) => call("github.enable", { host, enabled }),
  };
}
