// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { screen } from "@testing-library/react-native";

import { createLiveBackend } from "../src/data/live/backend";
import { DeviceManager } from "../src/data/live/enroll";
import { NOT_PROVIDING_NOTE, approveCommand, problemLine, reachKind } from "../src/data/live/reach";
import { RelayClient } from "../src/data/live/relay";
import { makeCore, setCoreForTests } from "../src/core";
import { HomeScreen } from "../src/features/home/HomeScreen";
import type { Backend, SpaceList } from "../src/data/types";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { fakeRelay } from "./helpers/fakeDaemon";
import { renderWithApp } from "./helpers/render";
import { testSigner } from "./helpers/signer";

jest.mock("expo-router", () => ({ useRouter: () => ({ push: jest.fn() }) }));

const row = (id: string, name: string, extra: Record<string, unknown> = {}) => ({ id, name, owner: { id: "acct:you" }, role: "owner", online: true, sharing: true, version: "1", url: "", confirmed: true, meta: {}, ...extra });
const machines = [
  row("studio", "Studio Mac", { meta: { "cua.host.spaces": "on", "cua.host.phone": "on" } }),
  row("desk", "Desk PC", { meta: { "cua.host.spaces": "on", "cua.host.phone": "on" } }),
  row("den", "Den Mac", { meta: {} }),
  row("space-aaaa1111", "checkout-api", { host: "studio" }),
  row("space-bbbb2222", "ledger", { host: "desk" }),
];

/** A stand in for the app core's `directory.view` (the real one is Rust, covered by its own tests): computers and Spaces from the rows. */
function installDirectoryCore() {
  setCoreForTests(
    makeCore((method, args) => {
      if (method !== "directory.view") throw new Error(`no fixture for ${method}`);
      const rows = (JSON.parse(args).input.machines ?? []) as ReturnType<typeof row>[];
      const isSpace = (r: ReturnType<typeof row> & { host?: string }) => !!r.host;
      const computers = rows
        .filter((r) => !isSpace(r))
        .map((r) => {
          const hosts = (r.meta as Record<string, string>)["cua.host.spaces"] === "on";
          return { id: r.id, name: r.name, kind: "mac", state: "Online", online: r.online, line: "", note: "", hostsSpaces: hosts, spaces: 0, isPhone: false, current: false, unconfirmed: false, capabilities: { v: 1, canHostSpaces: hosts, images: hosts ? ["*"] : [], kinds: [], desktop: true } };
        });
      const spaces = rows.filter(isSpace).map((r) => ({ id: `relay:${r.id}`, machine: r.id, name: r.name, host: (r as { host?: string }).host, hostName: "", state: "Running", online: true, os: "macos", kindLabel: "macOS VM", role: "owner", detail: "" }));
      return JSON.stringify({ computers, spaces, reachingLine: null, empty: null, errorLine: null });
    }),
  );
}

const entry = (seq: number) => ({ seq, id: `e${seq}`, ts_ms: 1000 * seq, author: { kind: "person", id: "acct:sam", name: "Sam" }, kind: "message", text: "hi", mentions: [], reply_to: null, attachments: [] });

function setup(threadFails: Record<string, { status: number; message: string }> = {}, approval?: () => { code: string | null } | null) {
  const fake = fakeRelay({
    machines,
    threadFails,
    tools: (name) => (name === "agent_members" ? { members: [] } : name === "thread_read" ? { entries: [entry(1)], cursor: 1, unread: 0, me: { id: "acct:you" }, role: "owner", live: { key: "", agents: [] } } : {}),
  });
  const backend = createLiveBackend({ relay: new RelayClient("https://relay.test", fake.fetcher as never), token: async () => "t", deviceSession: async () => "s", approval, me: () => "acct:you", fetcher: fake.fetcher as never, now: () => 2_000_000 });
  return { backend, fake };
}

beforeEach(installDirectoryCore);
afterAll(uninstallCore);

describe("which computer did not answer, and why", () => {
  it("keeps a computer that needs an update apart from one that timed out or cannot be reached", async () => {
    const { backend } = setup({
      studio: { status: 7, message: "this computer needs the latest Cua Spaces to be used from a phone: update it" },
      desk: { status: 4, message: "the computer did not answer in time" },
    });
    const list = await backend.listSpaces();
    expect(list.waiting).toBeNull();
    expect(list.incomplete).toBe(true);
    expect(list.problems).toEqual([
      { computerId: "studio", computer: "Studio Mac", kind: "needs-update" },
      { computerId: "desk", computer: "Desk PC", kind: "timeout" },
    ]);
    const unreachable = await setup({ desk: { status: 14, message: "connection refused" } }).backend.listSpaces();
    expect(unreachable.problems).toEqual([{ computerId: "desk", computer: "Desk PC", kind: "unreachable" }]);
  });

  it("words each reason plainly with the computer's name", () => {
    expect(problemLine({ computerId: "a", computer: "Studio Mac", kind: "needs-update" })).toBe("Studio Mac needs an update. Update Cua Spaces on it, then pull down to try again.");
    expect(problemLine({ computerId: "a", computer: "Desk PC", kind: "timeout" })).toBe("Desk PC took too long to answer. Pull down to try again.");
    expect(problemLine({ computerId: "a", computer: "Den Mac", kind: "unreachable" })).toBe("Den Mac could not be reached. Check that it is on and online, then pull down to try again.");
    expect(reachKind(new Error("something else broke"))).toBe("unreachable");
    expect(reachKind(new Error("[deadline_exceeded] timed out"))).toBe("timeout");
  });

  it("says a phone waiting for approval once, not as every computer failing", async () => {
    const refused = { status: 7, message: "this device is not enrolled for your cua.ai account" };
    const { backend } = setup({ studio: refused, desk: refused });
    const list = await backend.listSpaces();
    expect(list.waiting).toEqual({ code: null });
    expect(list.problems).toEqual([]);
  });

  it("makes no per Space call while the device manager says the phone is waiting, and passes its code on", async () => {
    const { backend, fake } = setup({}, () => ({ code: "K7Q2-M9XP" }));
    const list = await backend.listSpaces();
    expect(list.waiting).toEqual({ code: "K7Q2-M9XP" });
    expect(fake.seen.filter((s) => s.tool)).toHaveLength(0);
  });

  it("notes an online computer that is not providing Spaces, with how to turn it on", async () => {
    const { backend } = setup();
    const list = await backend.listSpaces();
    const den = list.computers!.find((c) => c.id === "den")!;
    expect(den.note).toBe(NOT_PROVIDING_NOTE);
    expect(NOT_PROVIDING_NOTE).toMatch(/^Not providing Spaces to your phone\./);
    expect(NOT_PROVIDING_NOTE).toContain("Provide Spaces");
    expect(NOT_PROVIDING_NOTE).toContain("cua host setup --share spaces");
    expect(list.computers!.find((c) => c.id === "studio")!.note).toBe("");
  });
});

describe("a pending phone, from the device manager", () => {
  const body = (url: string, state: "pending" | "enrolled") => (url.endsWith("/session") ? null : { device: { id: "dev_x", name: "iPhone", state }, code: "K7Q2-M9XP" });

  it("remembers the relay's pending refusal with a code, retries on the next call, and clears when approved", async () => {
    const signer = testSigner();
    let approved = false;
    let t = 1_700_000_000;
    const relay = new RelayClient("https://relay.test", async (url) => {
      if (url.endsWith("/v1/devices/session")) {
        return approved ? new Response(JSON.stringify({ session: "s1", expires_at: 9_999_999_999 })) : new Response(JSON.stringify({ error: "this device is waiting for approval" }), { status: 403 });
      }
      return new Response(JSON.stringify(body(url, "pending")));
    });
    const m = new DeviceManager(relay, async () => "tok", () => signer, "iPhone", () => t, "ios");
    expect(m.approval()).toBeNull();
    expect(await m.session()).toBeNull();
    expect(m.approval()).toEqual({ code: "K7Q2-M9XP" });
    // Asked again only after a few seconds, so a busy list does not hammer the relay.
    approved = true;
    expect(await m.session()).toBeNull();
    t += 4;
    expect(await m.session()).toBe("s1");
    expect(m.approval()).toBeNull();
  });

  it("does not call a network failure a pending approval", async () => {
    const relay = new RelayClient("https://relay.test", async () => {
      throw new TypeError("Network request failed");
    });
    const m = new DeviceManager(relay, async () => "tok", () => testSigner(), "iPhone", () => 1, "ios");
    expect(await m.session()).toBeNull();
    expect(m.approval()).toBeNull();
  });
});

describe("Home", () => {
  const home = (patch: (l: SpaceList) => Partial<SpaceList>) =>
    renderWithApp(<HomeScreen onOpen={() => {}} onSettings={() => {}} />, { wrap: (b: Backend) => ({ ...b, listSpaces: async () => ({ ...(await b.listSpaces()), ...patch({} as SpaceList) }) }) });

  beforeEach(installFixtureCore);

  it("shows the waiting for approval card with the code and the exact command", async () => {
    await home(() => ({ waiting: { code: "K7Q2-M9XP" }, incomplete: true, problems: [] }));
    expect(await screen.findByText("This phone is waiting for approval")).toBeTruthy();
    expect(screen.getByText("Code K7Q2-M9XP")).toBeTruthy();
    expect(screen.getByTestId("home-waiting-command").props.children).toBe("cua devices approve K7Q2-M9XP");
    expect(screen.queryByText(/did not answer/)).toBeNull();
    expect(approveCommand(null)).toBe("cua devices approve <CODE>");
  });

  it("names the computers that did not answer, one line each, and not the generic banner", async () => {
    await home(() => ({
      incomplete: true,
      waiting: null,
      problems: [
        { computerId: "a", computer: "Studio Mac", kind: "needs-update" },
        { computerId: "b", computer: "Desk PC", kind: "timeout" },
      ],
    }));
    expect(await screen.findByText("Studio Mac needs an update. Update Cua Spaces on it, then pull down to try again.")).toBeTruthy();
    expect(screen.getByText("Desk PC took too long to answer. Pull down to try again.")).toBeTruthy();
    expect(screen.queryByText("Some computers did not answer. Pull down to try again.")).toBeNull();
  });

  it("falls back to the generic line only when nothing says which computer", async () => {
    await home(() => ({ incomplete: true, waiting: null, problems: [] }));
    expect(await screen.findByText("Some computers did not answer. Pull down to try again.")).toBeTruthy();
  });
});
