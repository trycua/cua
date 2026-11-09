// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { exitDescription, exitLabel, validProxy } from "@/components/network/presets";

import { blockedFromWire, isBackgroundNoise, networkFromWire, reasonWords, topologyFromJson } from "../network";

const wire = {
  preset: "internet",
  summary: "Internet only, through This machine",
  enforcement: "enforced",
  point: "the Cua network gateway",
  noticePending: false,
  clipboard: "viewing",
  volume: "own",
};

describe("Exit through", () => {
  it("reads the exit from the host, and its own network as none", () => {
    expect(networkFromWire({ ...wire, via: "this-machine" }).via).toBe("this-machine");
    expect(networkFromWire({ ...wire, via: "internet" }).via).toBeUndefined();
    expect(networkFromWire({ ...wire, via: null }).via).toBeUndefined();
  });

  it("reads the topology: name, exit, this computer's ports, members and paths", () => {
    const t = topologyFromJson(
      JSON.stringify({
        space: { id: "local:w1", name: "w1.swarm" },
        tree: {
          members: [
            { id: "local:swarm", name: "swarm.swarm", relation: "parent" },
            { id: "local:w1", name: "w1.swarm", relation: "self" },
          ],
        },
        via: { kind: "space", label: "swarm" },
        exit: { state: "unreachable", note: "It's stopped." },
        host: { reach: ["localhost:3000"] },
        host_ports: { at_localhost: [3000], at_host_only: [[5432, "in use"]] },
        paths: { machines: [{ name: "studio-mac", kind: "lan", why: "Fastest path: the local network, 2 ms" }] },
      }),
    );
    expect(t?.name).toBe("w1.swarm");
    expect(t?.exit).toEqual({ label: "swarm", state: "unreachable", note: "It's stopped." });
    expect(t?.hostReach).toEqual(["localhost:3000"]);
    expect(t?.hostOnly).toEqual([{ port: 5432, why: "in use" }]);
    expect(t?.members.map((m) => m.relation)).toEqual(["parent", "self"]);
    expect(t?.paths[0]?.kind).toBe("lan");
    expect(topologyFromJson("")).toBeUndefined();
    expect(topologyFromJson("{nope")).toBeUndefined();
  });

  it("leaves members that run on other computers out of the exit choices", () => {
    const t = topologyFromJson(
      JSON.stringify({
        tree: {
          members: [
            { id: "local:gw", name: "gw.swarm", relation: "member" },
            { id: "remote:ab12:db", name: "db.swarm", relation: "member", machine: "studio-mac" },
          ],
        },
        overlay: { machines: [{ name: "studio-mac", path: "lan", why: "Fastest path: the local network, 2 ms" }] },
        paths: { machines: [{ name: "studio-mac", kind: "lan", why: "Fastest path: the local network, 2 ms" }] },
      }),
    );
    expect(t?.members.map((m) => m.name)).toEqual(["gw.swarm"]);
    expect(t?.paths[0]?.name).toBe("studio-mac");
  });

  it("names exits plainly, never with a middle dot or a proxy's password", () => {
    const members = [{ id: "local:gw", name: "gw.swarm", relation: "member" }];
    expect(exitLabel(undefined)).toBe("Its own network");
    expect(exitLabel("this-machine")).toBe("This computer");
    expect(exitLabel("space:local:gw", members)).toBe("gw");
    expect(exitLabel("socks5://me:secret@proxy.example:1080")).toBe("socks5://proxy.example:1080");
    for (const v of [undefined, "this-machine", "space:local:gw", "socks5://p:1"]) {
      expect(exitLabel(v, members)).not.toContain("·");
      expect(exitDescription(v)).not.toContain("·");
    }
    expect(validProxy("socks5://proxy.example:1080")).toBe(true);
    expect(validProxy("http://proxy.example:8080")).toBe(false);
    expect(validProxy("socks5://proxy.example")).toBe(false);
  });
});

describe("The blocked-connection line", () => {
  it("reads as a sentence for a reason the page has no words for", () => {
    // "Blocked: this Space can't reach {what}": an unknown reason used to say "can't reach not allowed by ...".
    expect(reasonWords("default:something-new")).toBe("a destination its settings don't allow");
    expect(blockedFromWire({ dst: "10.0.3.255:137", reason: "x", lastUnix: 1 } as never).what).toBe("a destination its settings don't allow");
  });

  it("leaves the guest's own name-service broadcast out of the notice", () => {
    expect(isBackgroundNoise("10.0.3.255:137")).toBe(true);
    expect(isBackgroundNoise("192.168.1.255:138")).toBe(true);
    expect(isBackgroundNoise("10.0.2.2:5432")).toBe(false);
    expect(isBackgroundNoise("10.0.3.255:5353")).toBe(false);
  });
});
