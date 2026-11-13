// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's Settings → Connections → GitHub: what the daemon's
 * `github.status` would say for made-up sign-ins (never a token). The default
 * is `gh` signed in with two accounts on github.com and one on a GitHub
 * Enterprise host; `?demo=github-none`, `github-saved` and `github-warning`
 * start from nothing signed in, a saved token, and a token that can do more
 * than a push and a pull request need.
 */

import { HostError } from "../../adapter";
import type { GithubAccount, GithubHost, GithubReport, GithubSource, GithubSourceRow } from "../../ops/github";
import type { DemoHandlers } from "./context";

export type DemoGithubStart = "gh" | "none" | "saved" | "warning";

interface Acct {
  login: string;
  active: boolean;
  scopes: string[];
}

export interface DemoGithub {
  start: DemoGithubStart;
  /** gh's accounts by host. */
  gh: Record<string, Acct[]>;
  /** The account you chose, by host. */
  chosen: Record<string, string>;
  off: string[];
  scannedMs: number;
}

export const demoGithubState = (start: DemoGithubStart): DemoGithub => ({
  start,
  gh:
    start === "none" || start === "saved"
      ? start === "saved"
        ? { "github.com": [{ login: "ada", active: true, scopes: ["repo"] }] }
        : {}
      : start === "warning"
        ? { "github.com": [{ login: "ada", active: true, scopes: ["repo", "workflow", "admin:org"] }] }
        : {
            "github.com": [
              { login: "ada", active: true, scopes: ["gist", "read:org", "repo"] },
              { login: "bob", active: false, scopes: ["repo", "workflow", "admin:org"] },
            ],
            "ghe.example.com": [{ login: "ada-corp", active: true, scopes: ["repo"] }],
          },
  chosen: {},
  off: [],
  scannedMs: 0,
});

const NOTHING = (host: string) =>
  host === "github.com" ? "No GitHub sign-in found: run gh auth login or paste a token" : `No sign-in found for ${host}: run gh auth login --hostname ${host}`;

const BROAD: Record<string, string> = { workflow: "change GitHub Actions workflows", delete_repo: "delete repositories" };
const broader = (scopes: string[]): string[] => [
  ...new Set(scopes.flatMap((s) => (BROAD[s] ? [BROAD[s]] : s.startsWith("admin:") ? ["administer organizations and settings"] : []))),
];
const warningFor = (scopes: string[]): string | undefined => {
  const b = broader(scopes);
  if (!b.length) return undefined;
  const words = b.length > 1 ? `${b.slice(0, -1).join(", ")} and ${b[b.length - 1]}` : b[0];
  return `This sign-in can also ${words}. Pushing and opening pull requests only needs the repo scope: use a narrower token, or sign in to gh again with only that.`;
};

function report(s: DemoGithub): GithubReport {
  const hostNames = [
    "github.com",
    ...Object.keys(s.gh)
      .filter((h) => h !== "github.com")
      .sort(),
  ];
  const hosts = hostNames.map((host): GithubHost => {
    const accts = s.gh[host] ?? [];
    const enabled = !s.off.includes(host);
    const saved = host === "github.com" && s.start === "saved";
    const pick = accts.find((a) => a.login === s.chosen[host]) ?? accts.find((a) => a.active) ?? accts[0];
    const source: GithubSource | undefined = saved ? "saved" : pick ? "gh" : undefined;
    const rows: GithubSourceRow[] = [
      ...(host === "github.com"
        ? [{ id: "saved" as const, used: saved, present: saved, line: saved ? "A token saved in Settings, Connections" : "No saved token" }]
        : []),
      { id: "env", used: false, present: false, line: "No GH_TOKEN in the Cua daemon's environment" },
      { id: "gh", used: !saved && Boolean(pick), present: Boolean(pick), line: pick ? `gh, signed in as ${pick.login}` : `gh is not signed in on ${host}` },
    ];
    const login = saved ? "carol" : pick?.login;
    const line = !enabled
      ? "Turned off: Cua will not use GitHub on this host"
      : saved
        ? "Signed in as carol with a saved token"
        : pick
          ? `Signed in as ${pick.login} with gh`
          : NOTHING(host);
    const scopes = saved ? ["repo"] : (pick?.scopes ?? []);
    const warning = enabled ? warningFor(scopes) : undefined;
    return {
      host,
      enabled,
      ...(enabled && source ? { source } : {}),
      ...(enabled && login ? { login } : {}),
      line,
      problem: false,
      ...(warning ? { warning } : {}),
      accounts: accts.map(
        (a): GithubAccount => ({
          login: a.login,
          active: a.active,
          selected: enabled && source === "gh" && a.login === pick?.login,
          chosen: s.chosen[host] === a.login,
          valid: true,
          scopes: a.scopes,
        }),
      ),
      sources: rows,
    };
  });
  return { hosts, ghFound: true, ghOld: false, canSave: true, scannedMs: s.scannedMs };
}

export function demoGithubHandlers({
  state,
  wait,
  stepMs,
  now,
}: {
  state: DemoGithub;
  wait(ms: number): Promise<void>;
  stepMs: number;
  now: () => number;
}): DemoHandlers<"github.status" | "github.rescan" | "github.use" | "github.enable"> {
  return {
    "github.status": () => report(state),
    "github.rescan": async () => {
      await wait(stepMs / 2);
      state.scannedMs = now();
      return report(state);
    },
    "github.use": ({ host, login }) => {
      if (login && !(state.gh[host] ?? []).some((a) => a.login === login))
        throw new HostError(`gh is not signed in as ${login} on ${host}: run gh auth login --hostname ${host}`);
      if (login) state.chosen[host] = login;
      else delete state.chosen[host];
      return report(state);
    },
    "github.enable": ({ host, enabled }) => {
      state.off = enabled ? state.off.filter((h) => h !== host) : [...new Set([...state.off, host])];
      return report(state);
    },
  };
}
