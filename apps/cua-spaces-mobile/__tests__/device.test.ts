// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { registerMessage, sessionMessage } from "../src/data/live/device";
import { DeviceManager, realDeviceName } from "../src/data/live/enroll";
import { RelayClient } from "../src/data/live/relay";
import { testSigner } from "./helpers/signer";

describe("the device messages (shared with the Rust test that reads cua_host::device)", () => {
  const shared = JSON.parse(readFileSync(join(__dirname, "../core/fixtures/device-messages.json"), "utf8"));
  it("are built from the one shared file", () => {
    expect(registerMessage("dev_abc", 7)).toBe("cua-device-register/v1\ndev_abc\n7");
    expect(sessionMessage("dev_abc", 7)).toBe("cua-device-session/v1\ndev_abc\n7");
    expect(shared.register).toBe("cua-device-register/v1\n{device}\n{ts}");
  });
});

describe("enrollment without a device key (the core not linked on this platform)", () => {
  it("says so plainly and never pretends", async () => {
    const m = new DeviceManager(
      new RelayClient("https://relay.test", async () => new Response("{}")),
      async () => "tok",
      () => null,
    );
    expect(await m.enroll()).toMatchObject({ state: "refused", message: expect.stringMatching(/cannot hold this phone's key/) });
    expect(await m.check()).toMatchObject({ state: "refused" });
    expect(await m.session()).toBeNull();
  });

  it("with a signer it registers and then opens a session", async () => {
    const signer = testSigner();
    const calls: string[] = [];
    const relay = new RelayClient("https://relay.test", async (url) => {
      calls.push(new URL(url).pathname);
      return new Response(
        JSON.stringify(
          url.endsWith("/session") ? { session: "s1", expires_at: 9_999_999_999 } : { device: { id: signer.id, name: "This phone", state: "enrolled" } },
        ),
      );
    });
    const m = new DeviceManager(
      relay,
      async () => "tok",
      () => signer,
      "Dillon's iPhone",
      () => 1_700_000_000,
      "ios",
    );
    expect(await m.enroll()).toEqual({ state: "enrolled" });
    expect(await m.session()).toBe("s1");
    expect(calls).toEqual(["/v1/devices", "/v1/devices/session"]);
  });

  it("enrolls under the phone's real name and what it can host, never the first-person label", async () => {
    const signer = testSigner();
    const bodies: Record<string, unknown>[] = [];
    const relay = new RelayClient("https://relay.test", async (_url, init) => {
      bodies.push(JSON.parse(String(init?.body ?? "{}")));
      return new Response(JSON.stringify({ device: { id: signer.id, name: "x", state: "enrolled" } }));
    });
    for (const given of ["Dillon's iPhone", "This phone", "this device", ""]) {
      bodies.length = 0;
      await new DeviceManager(
        relay,
        async () => "tok",
        () => signer,
        given,
        () => 1_700_000_000,
        "ios",
      ).enroll();
      expect(bodies[0]!.name).not.toMatch(/^this (phone|device)$/i);
      expect(String(bodies[0]!.name).length).toBeGreaterThan(0);
      // It states what it can host: nothing today, versioned.
      expect(bodies[0]!.capabilities).toEqual({ v: 1, can_host_spaces: false, images: [], kinds: [], desktop: false });
    }
    bodies.length = 0;
    await new DeviceManager(
      relay,
      async () => "tok",
      () => signer,
      "Dillon's iPhone",
      () => 1,
      "ios",
    ).enroll();
    expect(bodies[0]!.name).toBe("Dillon's iPhone");
  });

  it("names a phone from the system's name, else its model", () => {
    expect(realDeviceName("Dillon's iPhone", "ios", false)).toBe("Dillon's iPhone");
    expect(realDeviceName("This phone", "ios", false)).toBe("iPhone");
    expect(realDeviceName(null, "ios", true)).toBe("iPad");
    expect(realDeviceName("  ", "android", false)).toBe("Android phone");
    expect(realDeviceName("x".repeat(200), "ios", false)).toHaveLength(64);
  });
});
