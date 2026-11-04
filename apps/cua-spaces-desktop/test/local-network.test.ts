// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Local Network access on macOS (the SwiftUI app's LocalNetworkPermissionTests):
// the addresses a request reaches, asked once per launch, never off macOS.
import { describe, expect, it } from "vitest";
import { LocalNetworkPermission, subnetHost, targets, VMNET_GATEWAY } from "../src/model/local-network";

describe("Local Network access", () => {
  it("reaches the subnet's first host", () => {
    expect(subnetHost("192.168.1.23", "255.255.255.0")).toBe("192.168.1.1");
    expect(subnetHost("10.0.5.9", "255.255.0.0")).toBe("10.0.0.1");
    // This machine is the first host: the next one.
    expect(subnetHost("192.168.1.1", "255.255.255.0")).toBe("192.168.1.2");
  });

  it("skips link-local and subnets with no other host", () => {
    expect(subnetHost("169.254.3.4", "255.255.0.0")).toBeNull();
    expect(subnetHost("10.0.0.1", "255.255.255.255")).toBeNull();
    expect(subnetHost("10.0.0.1", "0.0.0.0")).toBeNull();
  });

  it("asks the LAN and vmnet's gateway", () => {
    const list = targets({
      lo0: [{ address: "127.0.0.1", netmask: "255.0.0.0", family: "IPv4", internal: true, mac: "", cidr: null }],
      en0: [{ address: "192.168.7.20", netmask: "255.255.255.0", family: "IPv4", internal: false, mac: "", cidr: null }],
    });
    expect(list).toEqual(["192.168.7.1", VMNET_GATEWAY]);
    expect(targets({})).toEqual([VMNET_GATEWAY]);
  });

  it("asks once, and only on macOS", () => {
    const sent: string[] = [];
    const mac = new LocalNetworkPermission("darwin", (h) => sent.push(h));
    mac.request();
    mac.request();
    expect(sent.filter((h) => h === VMNET_GATEWAY)).toHaveLength(1);
    const other: string[] = [];
    new LocalNetworkPermission("win32", (h) => other.push(h)).request();
    new LocalNetworkPermission("linux", (h) => other.push(h)).request();
    expect(other).toEqual([]);
  });
});
