// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * SAMPLE DATA. Everything in this file is made up: the people, machines,
 * Spaces, messages and the push card. It is what the developer switch
 * "Sample data" shows (and what the screenshots use); every screen labels it.
 * The shapes are the real wire records (`data/thread/protocol.ts`), so the
 * same screens draw live data.
 */

import type { AgentMember, MachineInfo, Scene, SpaceMemberRow, ThreadEntry } from "../types";

export const MIN = 60_000;
export const HOUR = 60 * MIN;

export const ME = { kind: "person" as const, id: "acct:you", name: "You" };
export const SAM = { kind: "person" as const, id: "acct:sam", name: "Sam" };
export const JORDAN = { kind: "person" as const, id: "acct:jordan", name: "Jordan" };
export const CUA = { kind: "system" as const, id: "system:host", name: "Cua" };
const agentAuthor = (handle: string, name: string) => ({ kind: "agent" as const, id: `agent:${handle}`, name });

export const CLAUDE = agentAuthor("claude", "Claude Code");
export const REVIEWER = agentAuthor("reviewer", "Reviewer");
export const BOOKKEEPER = agentAuthor("bookkeeper", "Bookkeeper");
export const OPERATOR = agentAuthor("operator", "Operator");
export const RESEARCHER = agentAuthor("researcher", "Researcher");

export function machines(now: number): MachineInfo[] {
  return [
    { id: "studio-mac", name: "Studio Mac", kind: "mac", online: true, lastSeenMs: null },
    { id: "desktop-pc", name: "Desktop PC", kind: "pc", online: true, lastSeenMs: null },
    { id: "cloud-eu", name: "Linked cloud", kind: "cloud", online: true, lastSeenMs: null },
    { id: "old-macbook", name: "Old MacBook", kind: "mac", online: false, lastSeenMs: now - 26 * HOUR },
  ];
}

export interface SampleSpace {
  id: string;
  name: string;
  machineId: string;
  scope: "app" | "machine";
  kindLabel: string;
  scene: Scene;
  role: "owner" | "editor" | "viewer";
  members: AgentMember[];
  people: SpaceMemberRow[];
}

const identity = (handle: string, displayName: string, harness: string) => ({
  origin: "space" as const,
  handle,
  displayName,
  collides: false,
  icon: null,
  harness: { id: harness, version: null, reported: true },
  owner: { id: ME.id, name: ME.name },
  device: null,
  joined: { method: "started" as const, atMs: null, by: null },
  state: "active" as const,
  lastSeenMs: null,
});

const member = (
  handle: string,
  displayName: string,
  harness: string,
  status: string,
  activity: string | null,
  needsYou: AgentMember["needsYou"] = [],
): AgentMember => ({
  name: handle,
  handle,
  harness,
  status,
  activity,
  sessions: 1,
  rights: null,
  youCan: null,
  heldBy: null,
  identity: identity(handle, displayName, harness),
  dm: { unread: 0, last: null },
  needsYou,
});

const person = (a: { id: string; name: string }, role: string, here: boolean): SpaceMemberRow => ({
  id: a.id,
  name: a.name,
  kind: "person",
  harness: null,
  status: here ? "here" : "away",
  activity: null,
  role,
  here,
});

export function spaces(now: number): SampleSpace[] {
  const you = person(ME, "Owner", true);
  return [
    {
      id: "relay:sample-checkout-api",
      name: "checkout-api",
      machineId: "studio-mac",
      scope: "machine",
      kindLabel: "macOS VM",
      scene: "code",
      role: "owner",
      members: [
        member("claude", "Claude Code", "claude-code", "waiting", "waiting for your approval", [
          { seq: 7, kind: "approval", text: "Claude Code wants to push a branch and open a pull request", tsMs: now - 2 * MIN },
          { seq: 8, kind: "approval", text: "Claude Code wants to download packages from registry.npmjs.org", tsMs: now - 1 * MIN },
        ]),
      ],
      people: [you, person(SAM, "Editor", true), person(JORDAN, "Viewer", false)],
    },
    {
      id: "relay:sample-design-review",
      name: "design-review",
      machineId: "studio-mac",
      scope: "machine",
      kindLabel: "macOS VM",
      scene: "design",
      role: "owner",
      members: [member("reviewer", "Reviewer", "hermes", "asleep", null)],
      people: [you, person(SAM, "Editor", false)],
    },
    {
      id: "relay:sample-ledger",
      name: "Ledger",
      machineId: "desktop-pc",
      scope: "app",
      kindLabel: "App",
      scene: "browser",
      role: "owner",
      members: [member("bookkeeper", "Bookkeeper", "openclaw", "working", "reconciling March invoices")],
      people: [you],
    },
    {
      id: "relay:sample-desktop-pc",
      name: "Desktop PC",
      machineId: "desktop-pc",
      scope: "machine",
      kindLabel: "Whole computer",
      scene: "desktop",
      role: "owner",
      members: [member("operator", "Operator", "codex", "asleep", null)],
      people: [you],
    },
    {
      id: "relay:sample-web-research",
      name: "web-research",
      machineId: "cloud-eu",
      scope: "machine",
      kindLabel: "Linux",
      scene: "terminal",
      role: "editor",
      members: [
        member("researcher", "Researcher", "claude-code", "working", "reading 3 reports"),
        member("researcher-2", "Researcher 2", "claude-code", "working", "summarising"),
        member("researcher-3", "Researcher 3", "claude-code", "idle", null),
      ],
      people: [you, person(JORDAN, "Owner", true)],
    },
    {
      id: "relay:sample-ci-runner",
      name: "ci-runner",
      machineId: "old-macbook",
      scope: "machine",
      kindLabel: "macOS VM",
      scene: "notes",
      role: "owner",
      members: [member("claude", "Claude Code", "claude-code", "asleep", null)],
      people: [you],
    },
  ];
}

type Extra = Partial<Omit<ThreadEntry, "seq" | "id" | "tsMs" | "author" | "text">>;
export function entry(seq: number, tsMs: number, author: ThreadEntry["author"], text: string, extra: Extra = {}): ThreadEntry {
  return {
    seq,
    id: `e${seq}`,
    tsMs,
    author,
    kind: "message",
    text,
    mentions: [],
    replyTo: null,
    task: null,
    attachments: [],
    approval: null,
    answer: null,
    noteFor: null,
    update: null,
    runsOnKeysOf: null,
    event: null,
    ...extra,
  };
}

/**
 * The push card's data as the host measures it (`ExportCardInput`; the app core's `threadCards.exportCard` makes every
 * word and number of the card from it). Made up, like everything in this file.
 */
export const SAMPLE_CARD_INPUT = {
  agent: "Claude Code",
  repo: "acme/checkout-api",
  branch: "fix/retry-backoff",
  base: "main",
  head: "0a1b2c3d4e5f60718293",
  files: [
    { path: "src/net/ws_reconnect.py", status: "modified", additions: 9, deletions: 3, binary: false },
    { path: "tests/test_ws_reconnect.py", status: "added", additions: 21, deletions: 0, binary: false },
    { path: "docs/retry-notes.md", status: "deleted", additions: 0, deletions: 14, binary: false },
    { path: "src/net/transport.py", oldPath: "src/net/http.py", status: "renamed", additions: 2, deletions: 2, binary: false },
    { path: "assets/status.png", status: "modified", additions: null, deletions: null, binary: true },
  ],
  filesTotal: 5,
  additionsTotal: 32,
  deletionsTotal: 19,
  commits: [
    { sha: "0a1b2c3d4e5f60718293", author: "Claude Code", subject: "Back off between reconnect retries" },
    { sha: "9f8e7d6c5b4a39281706", author: "Claude Code", subject: "Test the reconnect delays" },
  ],
  commitsTotal: 2,
  pr: { title: "Back off between reconnect retries", body: "Reconnects now wait 1s, 2s, 4s.\n\nFixes the flaky sync test.\nAdds tests/test_ws_reconnect.py." },
  linkOnly: false,
  account: "ada, signed in with gh",
  from: "checkout-api",
};

export function checkoutThread(now: number): ThreadEntry[] {
  const t = (m: number) => now - m * MIN;
  return [
    entry(1, t(95), CLAUDE, "Morning. The nightly run has 1 failing test in partial refunds. I can take it when someone says go."),
    entry(2, t(88), SAM, "I'm on the tax one. @claude take partial refunds?", { mentions: ["claude"], task: { to: "claude", session: null } }),
    entry(3, t(86), ME, "@claude yes, fix it and open a PR", { mentions: ["claude"], task: { to: "claude", session: null } }),
    entry(4, t(85), CLAUDE, "On it. Shipping was dropped on partial refunds; fixing that in refunds.ts and adding a test.", { replyTo: 3 }),
    entry(
      5,
      t(42),
      CLAUDE,
      "Fixed: partial refunds keep the shipping line. **149 passed**, 0 failed.\n\n```\nnpm test\n149 passed, 0 failed\n```\nDiff in `refunds.ts`.",
      {
        replyTo: 3,
        attachments: [
          { path: "refunds.diff", name: "refunds.diff", size: 1312, mime: "text/x-diff" },
          { path: "test.log", name: "test log", size: 9120, mime: "text/plain" },
        ],
      },
    ),
    entry(6, t(41), ME, "@claude open the PR", { mentions: ["claude"], task: { to: "claude", session: null } }),
    entry(7, t(2), CUA, "Claude Code wants to push a branch and open a pull request on acme/checkout-api.", {
      kind: "approval",
      approval: {
        action: "export:repo.push",
        details: "Branch fix/retry-backoff on acme/checkout-api.",
        state: "open",
        card: SAMPLE_CARD_INPUT,
        kind: "push",
        scopeChoices: ["once"],
      },
    }),
    // A second approval that waits, from the agent itself: plain text and exact detail, never a card.
    entry(8, t(1), CLAUDE, "Allow reaching registry.npmjs.org to install the test dependencies?", {
      kind: "approval",
      approval: {
        action: "network:registry.npmjs.org",
        details: "GET https://registry.npmjs.org/ (npm install)",
        state: "open",
        card: null,
        kind: "request",
        scopeChoices: ["once", "space"],
      },
    }),
  ];
}

/**
 * Tool calls the sample agent made, in the daemon's wire shape (`thread_tool_rows`, snake_case): the sample backend
 * answers with this JSON and the app reads it through the same mapper a live daemon's answer goes through. Rows are
 * as `cua-agents` tool_row words them.
 */
export const CHECKOUT_TOOL_ROWS_WIRE = {
  groups: [
    {
      after_seq: 4,
      agent: "claude",
      calls: [
        { id: "tc_1", cat: "read", line: "Read refunds.ts", detail: "src/orders/refunds.ts, 212 lines", state: "done", at_ms: 1 },
        { id: "tc_2", cat: "read", line: "Read refunds.test.ts", detail: "src/orders/refunds.test.ts, 96 lines", state: "done", at_ms: 2 },
        {
          id: "tc_3",
          cat: "edited",
          line: "Edited refunds.ts",
          detail: "+6 -2 in applyPartialRefund(): keep the shipping line when the refund is partial.",
          state: "done",
          at_ms: 3,
        },
        { id: "tc_4", cat: "edited", line: "Edited refunds.test.ts", detail: '+18 -0: adds "keeps shipping on a partial refund".', state: "done", at_ms: 4 },
        { id: "tc_5", cat: "ran", line: "Ran npm test", detail: "$ npm test\n149 passed, 0 failed (14.2s)", state: "done", at_ms: 5 },
      ],
    },
  ],
  cursor: 5,
  caught_up: true,
};

/** What a sample agent that is asked something does: two quick reads, then a command that is still running. */
export const replyToolCalls = (n: number) => [
  { id: `live_${n}_1`, cat: "read", line: "Read README.md", detail: "README.md, 58 lines", state: "done", at_ms: n },
  { id: `live_${n}_2`, cat: "ran", line: "Ran git status", detail: "$ git status\non branch main, working tree clean", state: "done", at_ms: n },
  { id: `live_${n}_3`, cat: "ran", line: "Ran npm test", detail: "$ npm test", state: "running", at_ms: n },
];

export function designThread(now: number): ThreadEntry[] {
  const t = (m: number) => now - m * MIN;
  return [
    entry(1, t(1440 + 40), REVIEWER, "Reviewed 3 PRs. Two are fine, one changes the checkout copy: should I block it?", { replyTo: null }),
    entry(2, t(1440 + 20), SAM, "Copy change is mine, it's approved by design."),
    entry(3, t(1440 + 19), REVIEWER, "Done for today. Message me here when there is more to review."),
    entry(4, t(1440 + 19), CUA, "Reviewer is asleep", { kind: "system" }),
  ];
}

export function simpleThread(now: number, who: ThreadEntry["author"], lines: string[]): ThreadEntry[] {
  return lines.map((text, i) => entry(i + 1, now - (lines.length - i) * 17 * MIN, who, text));
}

export const SAMPLE_BANNER = "Sample data";

/** The agents a sample machine has connected, in the shape a daemon's `task_options` answers (`agents`). */
const sampleAgents = (ready: string[]) =>
  [
    { harness: "claude-code", handle: "claude", label: "Claude Code", why: "Sign in to Claude Code in Settings, Connections" },
    { harness: "openai-codex", handle: "codex", label: "Codex", why: "Sign in to Codex in Settings, Connections" },
    { harness: "hermes", handle: "hermes", label: "Hermes", why: "Sign in to Hermes in Settings, Connections" },
    { harness: "openclaw", handle: "openclaw", label: "OpenClaw", why: "Sign in to OpenClaw in Settings, Connections" },
  ].map((a) => ({ ...a, ready: ready.includes(a.harness), why: ready.includes(a.harness) ? null : a.why }));

/**
 * What each online sample machine answers to `task_options` (the daemon's own shape: `{machine, agents}`). The sample
 * backend asks every online machine and merges the answers through the same mapper a live relay's answers go through.
 */
export const SAMPLE_TASK_OPTIONS_BY_HOST: Record<string, unknown> = {
  "studio-mac": {
    machine: { id: "studio-mac", os: ["macos"], online: true, can_create: true, why: null },
    agents: sampleAgents(["claude-code", "hermes", "openclaw"]),
  },
  "desktop-pc": { machine: { id: "desktop-pc", os: ["windows"], online: true, can_create: true, why: null }, agents: sampleAgents(["claude-code", "hermes"]) },
  "cloud-eu": {
    machine: { id: "cloud-eu", os: ["linux"], online: true, can_create: false, why: "Budget reached this month" },
    agents: sampleAgents(["claude-code"]),
  },
};

/** The checkout-api change set, from the same files the push card lists (`space_changes`, wire shape). */
export const SAMPLE_CHANGES_WIRE = {
  repo: "acme/checkout-api",
  branch: "fix/retry-backoff",
  base: "main",
  head_sha: "0a1b2c3",
  verified: true,
  files: SAMPLE_CARD_INPUT.files.map((f) => ({
    path: f.path,
    old_path: (f as { oldPath?: string }).oldPath,
    status: f.status,
    additions: f.additions,
    deletions: f.deletions,
    binary: f.binary,
    size: f.binary ? 283 : 1200,
    mime: f.binary ? "image/png" : f.path.endsWith(".md") ? "text/markdown" : "text/x-python",
  })),
  truncated: false,
};

/** One file's diff (`space_file_diff`, wire shape): plain `git diff` text, made up. */
export const SAMPLE_PATCHES: Record<string, string> = {
  "src/net/ws_reconnect.py": `diff --git a/src/net/ws_reconnect.py b/src/net/ws_reconnect.py
index 3b1c2d4..9e8f7a6 100644
--- a/src/net/ws_reconnect.py
+++ b/src/net/ws_reconnect.py
@@ -18,10 +18,17 @@ class Reconnector:
-    def __init__(self, transport, base=1.0, cap=30.0):
+    def __init__(self, transport, base=1.0, cap=30.0, jitter=0.1):
         self.transport = transport
         self.attempt = 0
+        self.base = base
+        self.cap = cap
+        self.jitter = jitter
+        log.debug("reconnector ready")

     async def run(self):
         while True:
-            await asyncio.sleep(1.0)
-            if await self.transport.connect():
+            delay = min(self.base * 2 ** self.attempt, self.cap)
+            await asyncio.sleep(delay)
+            if await self.transport.connect(timeout=delay):
                 self.attempt = 0
                 return
+            log.info("reconnect attempt %d failed", self.attempt)
             self.attempt += 1
`,
  "tests/test_ws_reconnect.py": `diff --git a/tests/test_ws_reconnect.py b/tests/test_ws_reconnect.py
new file mode 100644
index 0000000..4c5d6e7
--- /dev/null
+++ b/tests/test_ws_reconnect.py
@@ -0,0 +1,12 @@
+import pytest
+from net.ws_reconnect import Reconnector
+
+
+@pytest.mark.asyncio
+async def test_delays_double_up_to_the_cap(fake_transport):
+    r = Reconnector(fake_transport, base=1.0, cap=4.0)
+    delays = []
+    fake_transport.on_sleep = delays.append
+    fake_transport.fail_times = 4
+    await r.run()
+    assert delays == [1.0, 2.0, 4.0, 4.0]
`,
  "docs/retry-notes.md": `diff --git a/docs/retry-notes.md b/docs/retry-notes.md
deleted file mode 100644
index 7a8b9c0..0000000
--- a/docs/retry-notes.md
+++ /dev/null
@@ -1,4 +0,0 @@
-# Retry notes
-
-Reconnects wait one second. This is a placeholder until backoff lands.
-See the sync test for the flaky case.
`,
  "src/net/transport.py": `diff --git a/src/net/http.py b/src/net/transport.py
similarity index 92%
rename from src/net/http.py
rename to src/net/transport.py
index 1d2e3f4..5a6b7c8 100644
--- a/src/net/http.py
+++ b/src/net/transport.py
@@ -3,5 +3,5 @@ import asyncio
 
 class Transport:
-    timeout = 10
-    retries = 0
+    timeout = 15
+    retries = 3
     def __init__(self, url):
`,
};

/** A file's content for the preview (`space_file_preview`, wire shape). The image is a made up 160 by 100 status panel. */
export const SAMPLE_PREVIEWS: Record<string, { mime: string; encoding: "base64" | "utf8"; content: string; size: number }> = {
  "assets/status.png": {
    mime: "image/png",
    encoding: "base64",
    size: 283,
    content:
      "iVBORw0KGgoAAAANSUhEUgAAAKAAAABkCAIAAACO1KzYAAAA4klEQVR42u3ZsQ2AIBCGUUaxsKZiE5d0FApKRsHYuoINxJwv+Sa41/2Xtj0rcMkJAAuwAAuwAAuwAAMWYAEWYAEWYAEGLMACLMACLMB6CTyuW4EDDFiABViABViABRiwAAuwAAuwVgGX8wgfYMCAAQMGDBgwYMCAAQMGDBgwYMCAAQP2bBBgAQYswAIswAKsVcC1dU0KMGDAgAEDBgxYgAVY3wOWJUuABViABViAAcs/2BwBGDBgwIABAwYMGDBgwIANHQIswAIswIAFWIAFWIAFWIAFGLAAC7AAC7AAC/AvewAptVN4n4p4lwAAAABJRU5ErkJggg==",
  },
  "src/net/ws_reconnect.py": {
    mime: "text/x-python",
    encoding: "utf8",
    size: 612,
    content:
      "import asyncio\nimport logging\n\nlog = logging.getLogger(__name__)\n\n\nclass Reconnector:\n    def __init__(self, transport, base=1.0, cap=30.0):\n        self.transport = transport\n        self.attempt = 0\n        self.base = base\n        self.cap = cap\n",
  },
  "docs/retry-notes.md": {
    mime: "text/markdown",
    encoding: "utf8",
    size: 108,
    content: "# Retry notes\n\nReconnects wait one second. This is a placeholder until backoff lands.\nSee the sync test for the flaky case.\n",
  },
};
