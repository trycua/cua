// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import { demoPlatformOf, type DemoPlatform } from "../adapters/demo/platform";
import { wizardEnv, wizardInitial, wizardReduce, wizardView } from "../new-space";
import { storageInitial, storageSection } from "../settings-derive";
import { testCore, wasmBuilt } from "./testCore";

const adapters: ReturnType<typeof createDemoAdapter>[] = [];
const demo = (platform?: DemoPlatform) => {
  const a = createDemoAdapter({ latencyMs: 0, stepMs: 4, platform });
  adapters.push(a);
  return a;
};
afterEach(() => adapters.splice(0).forEach((a) => a.dispose?.()));

const MAC_WORDS = /This Mac|this Mac|Finder|\/Volumes|Macintosh HD/;

describe("the demo host on Windows and Linux", () => {
  it("reads Node's platform and arch", () => {
    expect(demoPlatformOf("win32", "x64")).toEqual({ os: "windows", arch: "amd64" });
    expect(demoPlatformOf("linux", "arm64")).toEqual({ os: "linux", arch: "arm64" });
    expect(demoPlatformOf("darwin", "arm64")).toEqual({ os: "macos", arch: "arm64" });
  });

  it("keeps the Mac's data on a Mac", async () => {
    const a = demo();
    const here = (await a.call("machines.list", {})).find((m) => m.current)!;
    expect([here.name, here.os, here.model]).toEqual(["This Mac", "macos", "MacBook Pro"]);
    const storage = await a.call("storage.get", {});
    expect(storage.mount?.path).toBe("/Volumes/Cua Volume");
    expect((await a.call("spaces.createOptions", {})).local?.backends).toContain("lume");
  });

  it("names this machine after the system it runs on", async () => {
    for (const [p, name] of [
      [demoPlatformOf("win32", "x64"), "This PC"],
      [demoPlatformOf("linux", "x64"), "This computer"],
    ] as const) {
      const a = demo(p);
      const here = (await a.call("machines.list", {})).find((m) => m.current)!;
      expect([here.name, here.os, here.arch]).toEqual([name, p.os, "x86_64"]);
      expect((await a.call("host.status", {})).name).toBe(name);
      const options = await a.call("spaces.createOptions", {});
      expect(options.local?.hostArch).toBe("amd64");
      expect(options.local?.backends).not.toContain("lume");
      expect(JSON.stringify(options)).not.toMatch(MAC_WORDS);
      const storage = await a.call("storage.get", {});
      expect(storage.os).toBe(p.os);
      expect(JSON.stringify(storage)).not.toMatch(MAC_WORDS);
      const volume = await a.call("volume.overview", {});
      expect(JSON.stringify(volume)).not.toMatch(MAC_WORDS);
    }
  });

  it.skipIf(!wasmBuilt)("draws Storage and the wizard without Mac words", async () => {
    const core = await testCore();
    for (const p of [demoPlatformOf("win32", "x64"), demoPlatformOf("linux", "x64")]) {
      const a = demo(p);
      const section = storageSection(core, await a.call("storage.get", {}), storageInitial(core)!)!;
      const words = JSON.stringify(section);
      expect(words).not.toMatch(MAC_WORDS);
      expect(words).toContain(p.os === "windows" ? "This PC" : "This computer");
      if (p.os === "linux") expect(words).toContain("Mounted at");

      const env = wizardEnv(core, {
        options: await a.call("spaces.createOptions", {}),
        defaultLocation: "local",
        cloudAvailable: true,
        clouds: null,
        machines: (await a.call("machines.list", {})).map((m) => ({ ...m, current: Boolean(m.current) })),
      });
      let state = wizardInitial(core, env);
      const system = wizardView(core, state, env, p.os);
      expect(system.placements.find((o) => o.id === "local")?.label).toBe(p.os === "windows" ? "This PC" : "This computer");
      state = wizardReduce(core, wizardReduce(core, wizardReduce(core, state, { type: "next" }, env), { type: "next" }, env), { type: "next" }, env);
      const summary = wizardView(core, state, env, p.os).summary;
      expect(JSON.stringify(summary)).not.toMatch(MAC_WORDS);
      expect(summary.find((f) => f.label === "Architecture")?.value).toBe("x64");
    }
  });
});
