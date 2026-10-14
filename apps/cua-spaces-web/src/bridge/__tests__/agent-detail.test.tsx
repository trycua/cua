// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import { routineLabel } from "../adapters/demo/agent-detail";
import type { PersistentAgent } from "../contracts/agents";
import type { CoreClient } from "../core";
import { BridgeProvider, useAgentDetail, type AgentDetailHook } from "../index";
import { agentDetailFromTools, agentFileFromTools, routineAddArgs, tauriAgentDetailOps, webkitAgentDetailOps } from "../ops/agent-detail";
import { noCore, testCore, wasmBuilt } from "./testCore";

afterEach(cleanup);

/* ---- The ops ----------------------------------------------------------------- */

describe("agent detail ops", () => {
  it("reads a home recursively and maps the tools' JSON onto the core's input", async () => {
    const calls: [string, Record<string, unknown>][] = [];
    const tool = async (name: string, args: Record<string, unknown>) => {
      calls.push([name, args]);
      switch (name) {
        case "volume_ls":
          return args.path === "agents/ada/"
            ? { entries: [{ path: "agents/ada/notes/", name: "notes", folder: true }, { path: "agents/ada/MEMORY.md", name: "MEMORY.md", size: 12 }] }
            : { entries: [{ path: "agents/ada/notes/a.md", name: "a.md", size: 3 }] };
        case "routine_list":
          return { routines: [{ id: "R1", title: "Morning", label: "Every day at 8:00 AM", isEnabled: false }, { id: "R2", title: "Noon", label: "x" }] };
        case "computer_access_list":
          return { grants: [{ agent: "ada", machine: "relay:m1", revoked: false }], audit: [{ ts_ms: 5, action: "use", principal: "agent:ada", path: "relay:m1", detail: "" }] };
        default:
          throw new Error(name);
      }
    };
    const d = await agentDetailFromTools(tool, "ada", "relay:m1");
    expect(d.home.map((f) => f.path)).toEqual(["agents/ada/MEMORY.md", "agents/ada/notes/a.md"]);
    expect(d.routines.map((r) => r.enabled)).toEqual([false, true]);
    expect(d.audit).toEqual([{ tsMs: 5, action: "use", principal: "agent:ada", path: "relay:m1", detail: "" }]);
    expect(d.thisMachine).toBe("relay:m1");
    expect(calls.find(([n]) => n === "computer_access_list")?.[1]).toEqual({ audit: 20 });
  });

  it("reads a file with its versions, and shows no bytes of a binary one", async () => {
    const tool = async (name: string) =>
      name === "volume_read"
        ? { content: "AAEC", encoding: "base64" }
        : { versions: [{ version: "v2", modified_ms: 9, latest: true }, { version: "v1", modified_ms: 1, deleted: true }] };
    expect(await agentFileFromTools(tool, "agents/ada/x.bin")).toEqual({
      path: "agents/ada/x.bin",
      text: "",
      binary: true,
      versions: [
        { version: "v2", modifiedMs: 9, deleted: false, latest: true },
        { version: "v1", modifiedMs: 1, deleted: true, latest: false },
      ],
    });
  });

  it("sends routine_add only the schedule that is set", () => {
    const base = { agent: "ada", title: "T", prompt: "P", everyMinutes: null, dailyAt: null, weeklyOn: null };
    expect(routineAddArgs({ ...base, everyMinutes: 30 })).toEqual({ agent: "ada", title: "T", prompt: "P", every_minutes: 30 });
    expect(routineAddArgs({ ...base, weeklyOn: "fri 16:00" })).toEqual({ agent: "ada", title: "T", prompt: "P", weekly_on: "fri 16:00" });
  });

  it("Tauri runs each change through agents_tool", async () => {
    const seen: unknown[] = [];
    const ops = tauriAgentDetailOps(async <T,>(cmd: string, args?: Record<string, unknown>) => {
      seen.push([cmd, args]);
      return {} as T;
    });
    await ops["agents.setRoutine"]({ id: "R1", enabled: true });
    await ops["agents.allowComputer"]({ agent: "ada", machine: "relay:m1" });
    expect(seen).toEqual([
      ["agents_tool", { tool: "routine_set_enabled", args: { id: "R1", enabled: true } }],
      ["agents_tool", { tool: "computer_access_grant", args: { agent: "ada", machine: "relay:m1" } }],
    ]);
  });

  it("the native hosts get each operation under its own name, and a sparse detail reads as empty", async () => {
    const seen: unknown[] = [];
    const ops = webkitAgentDetailOps(async <T,>(method: string, args?: Record<string, unknown>) => {
      seen.push([method, args]);
      return (method === "agents.detail" ? { home: [{ path: "agents/ada/a", name: "a", size: 1 }] } : null) as T;
    });
    expect(await ops["agents.detail"]({ name: "ada" })).toEqual({ home: [{ path: "agents/ada/a", name: "a", size: 1 }], routines: [], grants: [], audit: [], thisMachine: null });
    expect(await ops["agents.removeRoutine"]({ id: "R1" })).toBeNull();
    expect(seen).toEqual([
      ["agents.detail", { name: "ada" }],
      ["agents.removeRoutine", { id: "R1" }],
    ]);
  });

  it("the demo labels routines as the daemon does", () => {
    expect(routineLabel(60, null, null)).toBe("Every hour");
    expect(routineLabel(null, "08:05", null)).toBe("Every day at 8:05 AM");
    expect(routineLabel(null, null, "fri 16:00")).toBe("Every Friday at 4:00 PM");
  });
});

/* ---- The hook, with the core, on the demo host ------------------------------------ */

const ADA: PersistentAgent = { name: "ada", harness: "claude-code", space: "local:design-review", paused: false, spaceState: "running", runId: "run-ada-7", savedMs: Date.UTC(2026, 9, 3, 12) };

function mount(core: CoreClient, agent: PersistentAgent = ADA) {
  const adapter = createDemoAdapter({ latencyMs: 0, stepMs: 4 });
  const got: { d?: AgentDetailHook } = {};
  function Probe() {
    got.d = useAgentDetail(agent);
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core}>
      <Probe />
    </BridgeProvider>,
  );
  return got;
}

const idle = async (got: { d?: AgentDetailHook }) => waitFor(() => expect(got.d?.busy).toBe(false));

describe.skipIf(!wasmBuilt)("useAgentDetail with the core", () => {
  it("draws the agent's memory, opens a file and restores an older version", async () => {
    const got = mount(await testCore());
    await waitFor(() => expect(got.d?.view?.memory.length).toBeGreaterThan(0));
    const v = got.d!.view!;
    expect(v.tabs.map((t) => t.label)).toEqual(["Memory", "Routines", "Access to this computer"]);
    expect(v.subtitle).toMatch(/^Home agents\/ada\/, saved /);
    expect(v.memory.map((l) => l.text)).toEqual(["MEMORY.md", "notes/review-checklist.md", "notes/open-questions.md"]);

    act(() => got.d!.act({ type: "open-file", path: "agents/ada/MEMORY.md" }));
    await waitFor(() => expect(got.d?.view?.file).toBeTruthy());
    const file = got.d!.view!.file!;
    expect(file.path).toBe("MEMORY.md");
    expect(file.text).toContain("light and dark");
    expect(file.versions.map((l) => [l.text === "Current", l.actionLabel ?? null])).toEqual([
      [true, null],
      [false, "Restore"],
      [false, "Restore"],
    ]);

    act(() => got.d!.act({ type: "restore", version: file.versions[1]!.id }));
    await idle(got);
    // Restored: the file closes, as in the native page.
    expect(got.d!.view!.file ?? null).toBeNull();
    act(() => got.d!.act({ type: "open-file", path: "agents/ada/MEMORY.md" }));
    await waitFor(() => expect(got.d?.view?.file?.text).not.toContain("light and dark"));
  });

  it("adds, turns off and removes a routine; the form checks what it sends", async () => {
    const got = mount(await testCore());
    await waitFor(() => expect(got.d?.view?.routines.length).toBe(2));
    const send = (a: Parameters<AgentDetailHook["act"]>[0]) => act(() => got.d!.act(a));
    send({ type: "set-tab", tab: "routines" });
    expect(got.d!.view!.canAddRoutine).toBe(false);
    send({ type: "set-title", title: "Inbox" });
    send({ type: "set-prompt", prompt: "Sort my inbox" });
    send({ type: "set-schedule", schedule: "weekly" });
    send({ type: "set-time", time: "25:00" });
    expect(got.d!.view!.canAddRoutine).toBe(false);
    send({ type: "set-time", time: "16:30" });
    send({ type: "set-weekday", weekday: "fri" });
    expect(got.d!.view!.canAddRoutine).toBe(true);
    send({ type: "add-routine" });
    await waitFor(() => expect(got.d!.view!.routines.map((l) => l.text)).toContain("Inbox"));
    await idle(got);
    const added = got.d!.view!.routines.find((l) => l.text === "Inbox")!;
    expect(added).toMatchObject({ trailing: "Every Friday at 4:30 PM", on: true, actionLabel: "Remove" });
    // The form starts over.
    expect(got.d!.view!.form.title).toBe("");

    send({ type: "toggle-routine", id: added.id });
    await waitFor(() => expect(got.d!.view!.routines.find((l) => l.id === added.id)?.on).toBe(false));
    await idle(got);
    send({ type: "remove-routine", id: added.id });
    await waitFor(() => expect(got.d!.view!.routines.map((l) => l.id)).not.toContain(added.id));
  });

  it("revokes and allows this computer, and shows the agent's audit", async () => {
    const got = mount(await testCore());
    await waitFor(() => expect(got.d?.view?.access.length).toBe(1));
    expect(got.d!.view!.access[0]).toMatchObject({ text: "This computer", actionLabel: "Revoke" });
    expect(got.d!.view!.allowThisMachineLabel ?? null).toBeNull();
    // Ada's own lines only (Claw's refusal and Atlas's use are not hers).
    expect(got.d!.view!.audit.map((l) => l.text)).toEqual(["Used relay:this-mac", "Allowed relay:this-mac"]);

    act(() => got.d!.act({ type: "revoke-computer", machine: "relay:this-mac" }));
    await waitFor(() => expect(got.d!.view!.access).toEqual([]));
    await idle(got);
    expect(got.d!.view!.accessEmpty).toBe("This agent cannot use your computers.");
    expect(got.d!.view!.allowThisMachineLabel).toBe("Allow on this computer");
    expect(got.d!.thisMachine).toBe("relay:this-mac");
    expect(got.d!.view!.audit[0]!.text).toBe("Revoked relay:this-mac");

    act(() => got.d!.act({ type: "allow-computer", machine: got.d!.thisMachine! }));
    await waitFor(() => expect(got.d!.view!.access.map((l) => l.text)).toEqual(["This computer"]));
  });

  it("says why a change failed", async () => {
    const got = mount(await testCore());
    await waitFor(() => expect(got.d?.view?.memory.length).toBeGreaterThan(0));
    await idle(got);
    act(() => got.d!.act({ type: "open-file", path: "agents/ada/missing.md" }));
    await waitFor(() => expect(got.d!.error).toMatch(/not found/));
  });
});

describe("useAgentDetail without the core", () => {
  it("draws nothing (the page shows the run only)", async () => {
    const got = mount(noCore);
    await act(async () => {});
    expect(got.d!.view).toBeNull();
  });
});
