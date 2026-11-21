// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ComputerRow, DirectorySpace } from "../src/core/views";
import { mapDirectory } from "../src/data/live/spaces";
import type { RelayMachine } from "../src/data/live/relay";

const row = (id: string, over: Partial<RelayMachine> = {}): RelayMachine => ({
  id,
  name: id,
  owner: { id: "acct:you" },
  role: "owner",
  online: true,
  sharing: true,
  version: "1",
  url: "",
  host: null,
  meta: {},
  confirmed: true,
  ...over,
});
const computer = (id: string, over: Partial<ComputerRow> = {}): ComputerRow => ({
  id,
  name: id,
  kind: "mac",
  state: "Online",
  online: true,
  line: "",
  note: "",
  hostsSpaces: false,
  spaces: 0,
  isPhone: false,
  current: false,
  unconfirmed: false,
  capabilities: { v: 1, canHostSpaces: false, images: [], kinds: [], desktop: false },
  ...over,
});
const space = (id: string, over: Partial<DirectorySpace> = {}): DirectorySpace => ({
  id: `relay:${id}`,
  machine: id,
  name: id,
  host: "",
  hostName: "",
  state: "Running",
  online: true,
  os: "macos",
  kindLabel: "macOS VM",
  role: "owner",
  detail: "",
  ...over,
});

describe("the core's pool becomes the computers a call can go to and the Spaces under them", () => {
  it("a computer's Spaces sit under it, and a computer is never one of them", () => {
    const rows = [row("studio", { name: "Studio Mac" }), row("space-1", { host: "studio" }), row("plain", { name: "Dillons-MacBook-Pro-2.local" })];
    const r = mapDirectory(
      rows,
      [computer("studio", { name: "Studio Mac", hostsSpaces: true }), computer("plain", { name: "Dillons-MacBook-Pro-2.local" })],
      [space("space-1", { host: "studio", hostName: "Studio Mac" })],
    );
    expect(r.spaces).toHaveLength(1);
    expect(r.spaces[0]).toMatchObject({ hostId: "studio", threadOnHost: true, kindLabel: "macOS VM", role: "owner", state: "Running" });
    expect(r.spaces.map((s) => s.machine.id)).toEqual(["space-1"]);
    expect(r.machines.map((m) => m.id).sort()).toEqual(["plain", "studio"]);
  });

  it("a Space that runs on its own has no thread a phone can reach", () => {
    const r = mapDirectory([row("space-solo")], [], [space("space-solo")]);
    expect(r.spaces[0]).toMatchObject({ hostId: "space-solo", threadOnHost: false });
  });

  it("phones are in the pool and never a computer a call goes to", () => {
    const r = mapDirectory([row("studio")], [computer("studio"), computer("dev_ph", { kind: "iphone", isPhone: true })], []);
    expect(r.machines.map((m) => m.id)).toEqual(["studio"]);
  });

  it("nothing is sent to a computer nobody confirmed, nor to a Space it runs", () => {
    const rows = [row("planted", { confirmed: false }), row("space-2", { host: "planted" }), row("space-3", { confirmed: false })];
    const r = mapDirectory(rows, [computer("planted", { unconfirmed: true })], [space("space-2", { host: "planted" }), space("space-3")]);
    expect(r.spaces).toEqual([]);
    expect(r.machines).toEqual([]);
  });

  it("a Space shared while its computer is not still lists, behind a neutral computer", () => {
    const r = mapDirectory([row("space-x", { host: "hidden-host", role: "shared" })], [], [space("space-x", { host: "hidden-host", role: "editor" })]);
    expect(r.spaces[0]!.hostId).toBe("hidden-host");
    expect(r.machines[0]!.name).toBe("Shared with you");
  });
});
