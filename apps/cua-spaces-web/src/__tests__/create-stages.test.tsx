// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A create says which step it is on and, while it downloads, how far, how
// fast and how long is left: on its tile, in the core's words. The same for
// a macOS Space created on another of your machines, whose host reports
// its download through the daemon (CUA-1635: it used to sit at
// "Starting… 1%").

import { act, cleanup, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { stageLabelForTest, transferText } from "@/bridge/derive";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const GB = 2 ** 30;

describe.skipIf(!wasmBuilt)("a create's stages (wasm core)", () => {
  it("shows a macOS create on another machine downloading, then each step", async () => {
    let pendingId = "";
    let finish!: () => void;
    const done = new Promise<void>((r) => (finish = r));
    const { hooks, emit } = await mountApp("/spaces", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        if (op !== "spaces.create") return undefined;
        pendingId = (args as { pendingId: string }).pendingId;
        return done.then(() => {
          throw new Error("cancelled: test over");
        });
      },
    });
    await waitFor(() => expect(hooks.spaces.data?.length).toBeGreaterThan(0));
    act(() => void hooks.spaces.createSpace({ image: "macos", os: "macos", on: "host:mac-mini" }).catch(() => {}));
    await waitFor(() => expect(pendingId).not.toBe(""));
    const progress = (phase: string, bytes?: [number, number, number]) =>
      act(() =>
        emit({
          type: "spaces.createProgress",
          progress: {
            pendingId,
            phase,
            fraction: bytes ? bytes[0] / bytes[1] : null,
            detail: "ghcr.io/trycua/macos:26",
            space: "",
            ...(bytes ? { bytesDone: bytes[0], bytesTotal: bytes[1], bytesPerSecond: bytes[2] } : {}),
          },
        } as never),
      );
    const tile = () => document.querySelector<HTMLElement>(`[data-space-id="${pendingId}"]`);

    progress("pulling", [Math.round(12.4 * GB), 30 * GB, 85 * 2 ** 20]);
    await waitFor(() => expect(tile()?.textContent).toContain("Downloading macOS (first time only)…"));
    expect(tile()!.textContent).toContain("Another computer");
    expect(tile()!.querySelector("[data-create-transfer]")?.textContent).toBe("12.4 of 30.0 GB · 85 MB/s · about 4 min");
    const row = hooks.spaces.data!.find((s) => s.id === pendingId)!;
    expect(row.progress!.permille).toBeGreaterThan(10);

    for (const [phase, words] of [
      ["creating", "Preparing the disk…"],
      ["booting", "Booting macOS…"],
      ["waiting_for_services", "Starting Cua inside the Space…"],
    ] as const) {
      progress(phase);
      await waitFor(() => expect(tile()?.textContent).toContain(words));
      expect(tile()!.querySelector("[data-create-transfer]")).toBeNull();
    }
    finish();
  });
});

describe("a create's stages without the core", () => {
  it("names the steps and the download as the core does", () => {
    const p = { os: "macos", provider: "relay", kind: null } as const;
    expect(stageLabelForTest({ ...p, phase: "pulling" })).toBe("Downloading macOS (first time only)…");
    expect(stageLabelForTest({ ...p, phase: "creating" })).toBe("Preparing the disk…");
    expect(stageLabelForTest({ ...p, phase: "booting" })).toBe("Booting macOS…");
    expect(stageLabelForTest({ ...p, phase: "waiting_for_services" })).toBe("Starting Cua inside the Space…");
    const container = { os: "linux", provider: "local", kind: "container" } as const;
    expect(stageLabelForTest({ ...container, phase: "booting" })).toBe("Starting the container…");
    expect(transferText({ bytesDone: 12.4 * GB, bytesTotal: 30 * GB, bytesPerSecond: 85 * 2 ** 20 })).toBe("12.4 of 30.0 GB · 85 MB/s · about 4 min");
    expect(transferText({ bytesDone: 420 * 2 ** 20, bytesTotal: 900 * 2 ** 20 })).toBe("420 of 900 MB");
    expect(transferText({})).toBeNull();
    // Nothing is promised without a rate; work after the download counts in the time left.
    const MB = 2 ** 20;
    expect(transferText({ bytesDone: 440 * MB, bytesTotal: 456 * MB, bytesPerSecond: 20 * MB })).toMatch(/less than a minute$/);
    expect(
      transferText({ bytesDone: 440 * MB, bytesTotal: 456 * MB, bytesPerSecond: 20 * MB }, { bytes: 1100 * MB, secs: 26 }),
    ).toMatch(/about 1 min$/);
    expect(transferText({ bytesDone: 0, bytesTotal: 1100 * MB }, { bytes: 0, secs: 26, pace: 9_100_000 })).toMatch(/about 3 min$/);
    expect(transferText({ bytesDone: MB, bytesTotal: 10 * MB })).toBe("1.0 of 10.0 MB");
  });
});
