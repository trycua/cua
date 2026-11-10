// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `network.*` and `spaces.setAccess`: the SDK's network, clipboard and Volume
// settings through the backend, answering the web contract's shapes.
import { describe, expect, it, vi } from "vitest";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import { LiveSpacesBackend, networkSettingsOf } from "../src/model/backend";

function backend(spaces: Record<string, unknown>) {
  return new LiveSpacesBackend({} as never, { spaces: () => spaces } as never);
}

const INFO = {
  settings: {
    preset: "localhost",
    localhostPorts: [5432, 3000],
    allLocalhostPorts: false,
    allow: [],
    deny: [],
    customBase: "",
    widenedBy: undefined,
    via: "this-machine",
  },
  summary: "Allow this computer's localhost (ports 5432, 3000)",
  widenings: [],
  enforcement: "enforced",
  point: "the host firewall",
  note: undefined,
  parent: undefined,
  root: undefined,
  alwaysBlocked: ["cloud metadata"],
  noticePending: true,
  topologyJson: "",
};

describe("network.*", () => {
  it("answers a Space's network with its clipboard and Volume, in the page's shape", async () => {
    const b = backend({ network: vi.fn(async () => INFO), list: vi.fn(async () => [{ id: "local:aurora", clipboard: "ask", volume: "off" }]) });
    const r = await b.network("local:aurora");
    expect(r).toMatchObject({ preset: "localhost", localhostPorts: [5432, 3000], via: "this-machine", noticePending: true, clipboard: "ask", volume: "off" });
    expect(validate(OP_SHAPES["network.get"], r, SHAPE_DEFS)).toEqual([]);
  });

  it("sets a network from the page's preset word", async () => {
    const setNetwork = vi.fn(async () => ({ applied: "next_start" }));
    const b = backend({ setNetwork });
    expect(await b.setNetwork("local:aurora", "localhost:5432,3000", [], [], false, false, "this-machine")).toBe("next_start");
    expect(setNetwork).toHaveBeenCalledWith(
      "local:aurora",
      expect.objectContaining({ preset: "localhost", localhostPorts: [5432, 3000], via: "this-machine" }),
      false,
      false,
    );
  });

  it("reads the words of a preset", () => {
    expect(networkSettingsOf("internet", [], [])).toMatchObject({ preset: "internet", localhostPorts: [], allLocalhostPorts: false });
    expect(networkSettingsOf("localhost:all", [], [])).toMatchObject({ preset: "localhost", allLocalhostPorts: true });
    expect(networkSettingsOf("localhost+lan:5432,3000", [], [])).toMatchObject({
      preset: "localhost+lan",
      localhostPorts: [5432, 3000],
      allLocalhostPorts: false,
    });
    expect(networkSettingsOf("localhost+lan:all", [], [])).toMatchObject({ preset: "localhost+lan", localhostPorts: [], allLocalhostPorts: true });
    expect(networkSettingsOf("custom:none", ["github.com"], ["10.0.0.0/8"])).toMatchObject({ customBase: "none", allow: ["github.com"], deny: ["10.0.0.0/8"] });
    expect(networkSettingsOf("localhost:99999,abc,22", [], []).localhostPorts).toEqual([22]);
  });

  it("answers the blocked connections as numbers", async () => {
    const b = backend({
      networkLog: vi.fn(async () => [{ reason: "default:lan", dst: "192.168.1.20:445", proto: "tcp", count: 3n, lastUnix: 1_700_000_000n }]),
    });
    const r = await b.networkLog("local:aurora");
    expect(r).toEqual([{ reason: "default:lan", dst: "192.168.1.20:445", proto: "tcp", count: 3, lastUnix: 1_700_000_000 }]);
    expect(validate(OP_SHAPES["network.log"], r, SHAPE_DEFS)).toEqual([]);
  });

  it("sets the clipboard and the Volume, each only when asked", async () => {
    const setClipboard = vi.fn(async () => undefined);
    const setVolume = vi.fn(async () => undefined);
    const b = backend({ setClipboard, setVolume });
    await b.setAccess("local:aurora", "off", null);
    expect(setClipboard).toHaveBeenCalledWith("local:aurora", "off");
    expect(setVolume).not.toHaveBeenCalled();
    await b.setAccess("local:aurora", null, "own");
    expect(setVolume).toHaveBeenCalledWith("local:aurora", "own");
  });

  it("lists the Spaces on this computer whose exit is this computer", async () => {
    const network = vi.fn(async (id: string) => ({ settings: { via: id === "local:b" ? "this-machine" : null } }));
    const b = backend({ list: vi.fn(async () => [{ id: "local:a" }, { id: "local:b" }, { id: "relay:m/c" }]), network });
    expect(await b.exitsThroughThisMachine()).toEqual(["local:b"]);
    expect(network).not.toHaveBeenCalledWith("relay:m/c");
  });
});
