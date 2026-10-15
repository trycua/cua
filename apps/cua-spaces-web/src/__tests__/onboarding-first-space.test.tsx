// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The last onboarding page's "Create your first Space" offers only what
// this machine can create, by the Spaces page's one-click rule: on Windows
// and Linux every macOS Create was enabled, and on a Windows box with no
// runtime every Create was.

import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { NewSpaceOptions } from "@/bridge/contracts/new-space";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { CLOSED_NEW_SPACE, updateNewSpaceSession } from "@/bridge/new-space";
import { onboardingInitial, resetSavedOnboardingCache, writeSavedOnboarding } from "@/bridge/onboarding";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetSavedOnboardingCache();
  // The host's options are kept for the page's life: each test reads its own host's.
  updateNewSpaceSession(() => CLOSED_NEW_SPACE);
});
afterEach(cleanup);

/** The demo host with `local` changed as `edit` says. */
async function mountDone(edit: (local: NonNullable<NewSpaceOptions["local"]>) => NewSpaceOptions["local"], intercept?: (op: string, args: unknown) => unknown) {
  const core = await testCore();
  writeSavedOnboarding({ state: { ...onboardingInitial(core, null), step: "done" }, skipped: false });
  return mountApp("/onboarding", {
    failsCreates: false,
    listsGhost: false,
    intercept,
    answer: (op, out) => {
      if (op !== "spaces.createOptions") return out;
      const o = out as NewSpaceOptions;
      return { ...o, local: o.local ? edit(o.local) : o.local };
    },
  });
}

const card = (os: string) => document.querySelector<HTMLElement>(`[data-first-space="${os}"]`);
const createOf = (os: string) => within(card(os)!).getByRole("button") as HTMLButtonElement;

describe.skipIf(!wasmBuilt)("the first Space on the last onboarding page (real routes, wasm core)", () => {
  it("offers no macOS off an Apple silicon Mac, and says why", async () => {
    await mountDone((local) => ({ ...local, backends: ["docker"], macosImage: null, hostArch: "amd64" }));
    await screen.findByText("Create your first Space");
    await waitFor(() => expect(card("macos")?.querySelector("[data-first-space-blocked]")).toBeTruthy());
    expect(createOf("macos").disabled).toBe(true);
    expect(createOf("linux").disabled).toBe(false);
    expect(card("linux")!.querySelector("[data-first-space-blocked]")).toBeNull();
  });

  it("offers nothing on a machine with no runtime", async () => {
    await mountDone((local) => ({ ...local, available: false, backends: [], error: "No local runtime found (Docker or Lume)." }));
    await screen.findByText("Create your first Space");
    await waitFor(() => expect(card("linux")?.querySelector("[data-first-space-blocked]")).toBeTruthy());
    for (const os of ["macos", "linux", "windows"]) expect(createOf(os).disabled, os).toBe(true);
  });

  // CUA-1712: Create sent no `kind`, and the Electron host (Linux, Windows)
  // refused it with "config.kind: container | vm".
  it("Create sends the image's kind, as the hosts' create needs", async () => {
    const creates: { config: Record<string, unknown> }[] = [];
    await mountDone(
      (local) => ({ ...local, backends: ["docker"], macosImage: null, hostArch: "amd64" }),
      (op, args) => {
        if (op === "spaces.create") creates.push(args as { config: Record<string, unknown> });
        return undefined;
      },
    );
    await screen.findByText("Create your first Space");
    await waitFor(() => expect(createOf("linux").disabled).toBe(false));
    fireEvent.click(createOf("linux"));
    await waitFor(() => expect(creates).toHaveLength(1));
    expect(creates[0]!.config.kind).toBe("container");
    expect(creates[0]!.config.image).toBeTruthy();
  });
});
