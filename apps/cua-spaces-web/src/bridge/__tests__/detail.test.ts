// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { HostError } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import type { TelemetryView } from "../contracts/host";
import { sanitizeSignals } from "../ops/telemetry";
import { TelemetryForwarder, telemetryAllowed } from "../telemetry";
import { toMachineRows } from "../adapters/webkit";
import { rowsToSpaces, toMachines } from "../derive";
import { agentRunLines, desktopCover, spaceDetail } from "../space-detail";
import { parseDropped, sendFiles, splitDrop } from "../space-files";
import type { MachineAccessNotice } from "../contracts/devices";
import type { SpaceRow } from "../contracts/spaces";
import { testCore, wasmBuilt } from "./testCore";

const view = (enabled: boolean, noticeShown = true): TelemetryView => ({
  enabled,
  noticeShown,
  source: "config",
  sourceKind: "config",
  noticeText: "",
  docsUrl: "",
});

describe("usage events", () => {
  it("lets only schema fields and fixed words through", () => {
    expect(
      sanitizeSignals([
        { type: "step", step: "app_launched", ok: true, email: "ada@example.com" },
        { type: "feature", feature: "/Users/ada" },
        { type: "space-create-started", location: "local", guestOs: "linux", kind: "container", gpu: false },
        { type: "space-create", location: "local", guestOs: "linux", kind: "vm", outcome: "ok", failedPhase: "none", stalled: false, elapsedMs: -1, gpu: false },
        { type: "made-up", word: "x" },
        "step",
      ]),
    ).toEqual([
      { type: "step", step: "app_launched", ok: true },
      { type: "space-create-started", location: "local", guestOs: "linux", kind: "container", gpu: false },
    ]);
  });

  it("sends only while the setting is on and the notice was shown", async () => {
    expect(telemetryAllowed(null)).toBe(false);
    expect(telemetryAllowed(view(true, false))).toBe(false);
    const demo = createDemoAdapter({ latencyMs: 0 });
    let current = view(true);
    const f = new TelemetryForwarder(demo, async () => current);
    await f.track([{ type: "step", step: "signed_in", ok: true }]);
    current = view(false);
    await f.track([{ type: "step", step: "first_space", ok: true }]);
    expect(demo.state.telemetry).toEqual([{ type: "step", step: "signed_in", ok: true }]);
    demo.dispose?.();
  });

  it("stops asking a host with no telemetry route", async () => {
    let calls = 0;
    const host = {
      mode: "webkit" as const,
      call: async () => {
        calls++;
        throw new HostError("no route", "unsupported");
      },
      subscribe: () => () => {},
    };
    const f = new TelemetryForwarder(host as never, async () => view(true));
    await f.track([{ type: "step", step: "a", ok: true }]);
    await f.track([{ type: "step", step: "b", ok: true }]);
    expect(calls).toBe(1);
  });
});

describe("demo host: Space detail and This machine", () => {
  it("reads usage and windows for a running Space, and toggles picture in picture", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const usage = await demo.call("spaces.usage", { spaceId: "local:design-review" });
    expect(usage?.memoryLimited).toBe(true);
    const { windows, display } = await demo.call("spaces.windows", { spaceId: "local:design-review" });
    expect(windows.length).toBeGreaterThan(0);
    expect(display).not.toBeNull();
    expect(await demo.call("stream.pip", { spaceId: "local:design-review", command: { type: "open", row: "desktop" } })).toEqual(["desktop"]);
    expect(await demo.call("stream.pip", { spaceId: "local:design-review", command: { type: "close", row: "desktop" } })).toEqual([]);
    await expect(demo.call("spaces.windows", { spaceId: "local:agent-sandbox" })).rejects.toMatchObject({ code: "space_off" });
    demo.dispose?.();
  });

  it("sets this machine up, stops sharing and removes the setup", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    expect((await demo.call("host.action", { action: "stop-sharing" })).sharing).toBe(false);
    expect((await demo.call("host.action", { action: "remove" })).configured).toBe(false);
    const set = await demo.call("host.setUp", { request: { mode: "direct", direct: "0.0.0.0:3300", name: "Studio" } });
    expect(set).toMatchObject({ configured: true, mode: "direct", directUrl: "http://0.0.0.0:3300", name: "Studio" });
    const rows = await demo.call("machines.list", {});
    expect(rows.find((r) => r.current)?.host?.name).toBe("Studio");
    await expect(demo.call("host.openSettings", { url: "https://example.com" })).rejects.toMatchObject({ code: "bad_args" });
    demo.dispose?.();
  });
});

const NOTICE: MachineAccessNotice = {
  kind: "waiting",
  status: "Waiting for approval",
  text: "Waiting for approval. On an enrolled device, approve this Mac with the code K7QX-M2RP.",
  actionLabel: "Enroll This Mac\u2026",
};

describe("this device's access to its machines", () => {
  it("rides on This Mac's machine row from the SwiftUI host's machines.list", () => {
    const rows = toMachineRows({ spaces: [] } as never, { devices: null, signedIn: true, accessNotice: NOTICE });
    expect(rows.find((r) => r.current)?.accessNotice).toEqual(NOTICE);
    const machines = toMachines(rows, []);
    expect(machines.find((m) => m.current)?.accessNotice).toEqual(NOTICE);
    // Enrolled: none.
    expect(toMachineRows({ spaces: [] } as never, { devices: null, signedIn: true, accessNotice: null })[0]!.accessNotice).toBeUndefined();
  });
});

describe.skipIf(!wasmBuilt)("a Space's detail as this device sees it (wasm core)", () => {
  const row = (over: Partial<SpaceRow>): SpaceRow => ({ id: "relay:studio", name: "Studio", provider: "relay", os: "macos", spacesdVersion: "0.6.0", features: ["desktop_stream", "host_spaces"], reachable: true, ...over });

  it("greys out a machine's connection actions while this device is not enrolled, and leaves other Spaces alone", async () => {
    const core = await testCore();
    const [studio, local] = rowsToSpaces(core, [row({}), row({ id: "local:dev", name: "dev", provider: "local", os: "linux" })], 0);
    const d = spaceDetail(core, studio!, null, null, { sharing: true }, NOTICE);
    expect(d.access).toEqual(NOTICE);
    expect(d.canStream).toBe(false);
    expect(d.previewText).toBe(NOTICE.text);
    expect(d.facts.find((f) => f.label === "Status")?.value).toBe("Waiting for approval");
    expect(d.actions.filter((a) => ["teleport", "pip", "share", "open"].includes(a.id)).every((a) => !a.enabled)).toBe(true);
    expect(spaceDetail(core, local!, null, null, { sharing: true }, NOTICE).access).toBeUndefined();
  });

  it("hides Share while Settings, Experiments, Sharing is off", async () => {
    const core = await testCore();
    const [studio] = rowsToSpaces(core, [row({})], 0);
    expect(spaceDetail(core, studio!, null, null, {}).actions.some((a) => a.id === "share")).toBe(false);
    expect(spaceDetail(core, studio!, null, null, { sharing: true }).actions.some((a) => a.id === "share")).toBe(true);
  });

  it("explains a machine that keeps its desktop private, with no button beside the note", async () => {
    const core = await testCore();
    const [studio] = rowsToSpaces(core, [row({ features: ["host_spaces", "files"] })], 0);
    const d = spaceDetail(core, studio!, null, null, {});
    expect(d.desktopNote).toBe("Studio isn\u2019t sharing its desktop. You can still create Spaces on it.");
    expect(d.newSpace).toEqual({ label: "New Space on Studio\u2026", on: "host:studio" });
    expect(d.sections).toEqual([]);
  });

  it("draws the preview's cover from the core", async () => {
    const core = await testCore();
    const input = { canStream: true, previewText: "", autoConnect: true, connectRequested: false, stream: "nosession" as const };
    expect(desktopCover(core, input)).toMatchObject({ kind: "connecting", text: "Connecting\u2026", openStream: true });
    expect(desktopCover(core, { ...input, autoConnect: false })).toMatchObject({ kind: "connect", button: "Connect", openStream: false });
    expect(desktopCover(core, { ...input, stream: "failed" })).toMatchObject({ kind: "status", text: "Could not connect to the desktop", button: "Try again" });
    expect(desktopCover(core, { ...input, access: NOTICE })).toMatchObject({ kind: "connect", buttonDisabled: true, action: NOTICE.actionLabel, text: NOTICE.text });
  });
});

describe("a Space's drop well", () => {
  it("tells an app dropped on the well from the files beside it", () => {
    expect(splitDrop(["/a/notes.txt", "/Applications/Slack.app", "/a/b.png"])).toEqual({ app: "/Applications/Slack.app", files: ["/a/notes.txt", "/a/b.png"] });
    expect(splitDrop(["/a/notes.txt"])).toEqual({ app: null, files: ["/a/notes.txt"] });
    // The names the notch leaves on a Space's address are a drop, and nothing else is.
    expect(parseDropped(["Slack.app", "notes.txt"])).toEqual(["Slack.app", "notes.txt"]);
    for (const bad of [undefined, null, "Slack.app", [], [""], ["a", 3], Array.from({ length: 201 }, () => "x")]) expect(parseDropped(bad)).toBeUndefined();
  });

  it("says what it sends and what landed, or why it failed", async () => {
    const core = await testCore();
    const demo = createDemoAdapter({ latencyMs: 0 });
    const seen: string[] = [];
    await sendFiles(demo, core, "local:design-review", ["/Users/ada/notes.txt"], (s) => seen.push(`${s?.kind}: ${s?.text}`));
    expect(seen[0]).toMatch(/^working: Sending notes\.txt/);
    expect(seen[1]).toMatch(/^done: notes\.txt .* verified in /);
    seen.length = 0;
    await sendFiles(demo, core, "local:agent-sandbox", ["/Users/ada/notes.txt"], (s) => seen.push(s!.kind));
    expect(seen).toEqual(["working", "failed"]);
    await sendFiles(demo, core, "local:design-review", [], (s) => seen.push(s!.kind));
    expect(seen).toHaveLength(2);
    demo.dispose?.();
  });
});

describe.skipIf(!wasmBuilt)("a Space's Agents rows (wasm core)", () => {
  it("are the agent, what it was asked, and its status word", async () => {
    const core = await testCore();
    const [row] = agentRunLines(core, [{ runId: "r1", agent: "claude-code", status: "idle", reason: "waiting", summary: "Fix the tests", createdAt: null, phase: "", turn: 1 }]);
    expect(row).toMatchObject({ runId: "r1", status: "Idle", reason: "waiting" });
    expect(row!.line).toMatch(/^Claude Code · /);
  });
});
