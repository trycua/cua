// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's CI Runners: made-up data in `cua ci overview`'s own shape
 * (an organization called acme, its repositories, a week of jobs on a cloud
 * and on the Mac itself, a budget and an autopilot). `?demo=ci-fresh` starts
 * with nothing connected and `?demo=ci-connected` with GitHub connected and
 * no job yet. Connecting plays the hosted flow: a GitHub page to open, then
 * connected a few polls later.
 */

import type { CiConnectAnswer, CiOverview } from "../../contracts/ci-runners";
import type { OpArgs, OpResult } from "../../protocol";
import type { CiOp } from "./ci-types";

export type DemoCiStart = "fresh" | "connected" | "busy";

const MIN = 60_000;

const SETUP = (connected: boolean, permitted: number, used: number, passed: boolean): CiOverview["setup"] => [
  { id: "connect_github", state: connected ? "done" : "todo", detail: connected ? "Connected" : "" },
  { id: "repositories", state: connected ? (permitted > 0 ? "done" : "todo") : "todo", detail: permitted > 0 ? `${permitted} repositories can run jobs` : "" },
  { id: "runner_label", state: used > 0 ? "done" : "todo", detail: `${used} repositories used it this week` },
  { id: "first_job_passed", state: passed ? "done" : "todo", detail: passed ? "A job passed" : "No job has passed yet" },
];

const REPOS = [
  { name: "acme/app", allowed: true },
  { name: "acme/api", allowed: true },
  { name: "acme/docs", allowed: true },
  { name: "acme/website", allowed: false },
  { name: "acme/infra", allowed: false },
];

const ORG = "acme";

export interface DemoCiState {
  connected: boolean;
  busy: boolean;
  repos: { name: string; allowed: boolean }[];
  goal: string;
  /** Polls of `ci.connect` since it started (the page opens after the first). */
  polls: number;
}

export function demoCiState(start: DemoCiStart): DemoCiState {
  return { connected: start !== "fresh", busy: start === "busy", repos: REPOS.map((r) => ({ ...r })), goal: "balanced", polls: 0 };
}

function jobs(now: number): CiOverview["jobs"] {
  const base = { start_mode: "", machine: "", machine_type: "", cloud: "", spot: false, log_pool: "hosted", log_slot: "" };
  return [
    {
      ...base,
      job_key: "9912/301",
      name: "test",
      repo: "acme/app",
      state: "running",
      queued_ms: now - 3 * MIN,
      wait_ms: 4_000,
      run_ms: null,
      cost_micros: null,
      all_in_micros: null,
      start_mode: "warm_pool",
      machine: "cua-ci-7",
      machine_type: "c7i.xlarge",
      cloud: "aws",
      spot: true,
      log_slot: "s7",
    },
    {
      ...base,
      job_key: "9911/298",
      name: "build",
      repo: "acme/app",
      state: "passed",
      queued_ms: now - 16 * MIN,
      wait_ms: 6_000,
      run_ms: 72_000,
      cost_micros: 41_000,
      all_in_micros: 52_000,
      start_mode: "fork",
      machine: "cua-ci-7",
      machine_type: "c7i.xlarge",
      cloud: "aws",
      spot: true,
      log_slot: "s7",
    },
    {
      ...base,
      job_key: "9910/295",
      name: "lint",
      repo: "acme/api",
      state: "failed",
      queued_ms: now - 2 * 60 * MIN,
      wait_ms: 21_000,
      run_ms: 38_000,
      cost_micros: 27_000,
      all_in_micros: 27_000,
      start_mode: "cold_create",
      machine: "cua-ci-4",
      machine_type: "c7i.large",
      cloud: "aws",
      log_slot: "s4",
    },
    {
      ...base,
      job_key: "9905/280",
      name: "build",
      repo: "acme/docs",
      state: "passed",
      queued_ms: now - 5 * 60 * MIN,
      wait_ms: 1_800,
      run_ms: 94_000,
      cost_micros: 38_000,
      all_in_micros: 61_000,
      start_mode: "warm_pool",
      machine: "cua-ci-7",
      machine_type: "c7i.xlarge",
      cloud: "aws",
      spot: true,
      log_slot: "s7",
    },
    {
      ...base,
      job_key: "9800/210",
      name: "unit",
      repo: "acme/app",
      state: "passed",
      queued_ms: now - 26 * 60 * MIN,
      wait_ms: 1_200,
      run_ms: 190_000,
      start_mode: "warm_pool",
      log_slot: "s1",
    },
    {
      ...base,
      job_key: "9790/205",
      name: "ui tests",
      repo: "acme/app",
      state: "failed",
      queued_ms: now - 50 * 60 * MIN,
      wait_ms: 2_400,
      run_ms: 411_000,
      start_mode: "warm_pool",
      log_slot: "s1",
    },
    {
      ...base,
      job_key: "9700/110",
      name: "release",
      repo: "acme/api",
      state: "passed",
      queued_ms: now - 4 * 24 * 60 * MIN,
      wait_ms: 3_000,
      run_ms: 133_000,
      cost_micros: 61_000,
      all_in_micros: 70_000,
      start_mode: "warm_pool",
      machine: "cua-ci-2",
      machine_type: "c7i.xlarge",
      cloud: "aws",
      log_slot: "s2",
    },
  ];
}

export function demoCiOverview(s: DemoCiState, now: number): CiOverview {
  const allowed = s.repos.filter((r) => r.allowed).length;
  const used = s.busy
    ? [
        { name: "acme/app", jobs_7d: 31 },
        { name: "acme/api", jobs_7d: 9 },
      ]
    : [];
  return {
    generated_ms: now,
    setup: SETUP(s.connected, s.connected ? allowed : 0, used.length, s.busy),
    github: s.connected
      ? { connected: true, mode: "hosted", login: ORG, choose_url: `https://github.com/organizations/${ORG}/settings/installations` }
      : { connected: false, mode: "none", login: "", choose_url: "" },
    repositories: { used, all: s.connected ? s.repos.map((r) => ({ ...r })) : null },
    usage: s.busy
      ? { starts_24h: 18, jobs_7d: 40, pass_rate_7d: 0.925, spend_7d_micros: 3_190_000 }
      : { starts_24h: 0, jobs_7d: 0, pass_rate_7d: null, spend_7d_micros: 0 },
    jobs: s.busy ? jobs(now) : [],
    targets: s.busy
      ? [
          {
            target: "aws",
            goal: s.goal,
            warm: null,
            autopilot: {
              classes: [
                {
                  class: "linux",
                  phase: "steady",
                  jobs_seen: 212,
                  warm_now: 1,
                  idle_ttl_secs: 600,
                  spot: true,
                  reasons: [
                    "Jobs arrive on weekdays between 9 and 18, so one machine is kept ready in those hours.",
                    "Spot machines were taken back 2 times in 212 jobs, so spot is still used.",
                  ],
                },
                { class: "macos", phase: "learning", jobs_seen: 6, warm_now: 0, idle_ttl_secs: 600, spot: false, reasons: [] },
              ],
            },
          },
        ]
      : [],
    budget: s.busy
      ? { month_committed_micros: 12_400_000, monthly_micros: 50_000_000, state: "ok" }
      : { month_committed_micros: 0, monthly_micros: 0, state: "ok" },
  };
}

type Handlers = { [K in CiOp]: (args: OpArgs<K>) => Promise<OpResult<K>> | OpResult<K> };

export function demoCiHandlers(ctx: {
  state: DemoCiState;
  now: () => number;
  wait: (ms: number) => Promise<void>;
  stepMs: number;
  signedIn: () => boolean;
}): Handlers {
  const { state, now, wait, stepMs } = ctx;
  return {
    "ci.overview": async () => {
      await wait(stepMs);
      return demoCiOverview(state, now());
    },
    "ci.connect": async () => {
      await wait(stepMs);
      if (state.connected) return { state: "connected", login: ORG } satisfies CiConnectAnswer;
      if (!ctx.signedIn()) return { state: "sign_in" } satisfies CiConnectAnswer;
      state.polls += 1;
      if (state.polls < 3) return { state: "waiting", url: `https://github.com/apps/cua-ci/installations/new?state=demo` } satisfies CiConnectAnswer;
      state.connected = true;
      state.polls = 0;
      return { state: "connected", login: ORG } satisfies CiConnectAnswer;
    },
    "ci.connectToken": async ({ org, token }) => {
      await wait(stepMs * 2);
      if (token !== undefined && token.trim().length > 0 && !/^(ghp_|github_pat_)/.test(token.trim())) {
        return { state: "failed", message: "GitHub refused the token (401). Check that it is a personal access token." };
      }
      state.connected = true;
      return { state: "connected", login: org };
    },
    "ci.connectCancel": async () => {
      state.polls = 0;
      return null;
    },
    "ci.repositories": async ({ allow, remove }) => {
      await wait(stepMs);
      const changed: string[] = [];
      for (const r of state.repos) {
        if (allow?.includes(r.name) && !r.allowed) ((r.allowed = true), changed.push(`serving ${r.name}`));
        if (remove?.includes(r.name) && r.allowed) ((r.allowed = false), changed.push(`no longer serving ${r.name}`));
      }
      return { changed };
    },
    "ci.goal": async ({ goal }) => {
      await wait(stepMs);
      state.goal = goal;
      return { goal, applied: true };
    },
    "ci.log": async ({ jobKey }) => {
      await wait(stepMs);
      const at = now();
      const lines = [
        "Queued, waiting for a computer",
        "Took a Space that was already running",
        "The runner registered with GitHub",
        "GitHub gave the job to the runner",
        "The job started",
        jobKey === "9910/295" || jobKey === "9790/205" ? "The job failed after 38 s" : "The job passed after 1 min 12 s",
        "The Space was deleted and nothing of the job remains",
      ];
      return { lines: lines.map((text, i) => ({ at_ms: at - (lines.length - i) * 9_000, text })) };
    },
  };
}
