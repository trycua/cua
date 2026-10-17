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
    // Another computer can run them: Add a machine is right here.
    expect(await screen.findByRole("button", { name: "Add a machine" })).toBeTruthy();
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

  // A10 (15 Oct clean Windows 11): no Docker Desktop, and nothing to do
  // about it from the app. The host now offers the install; Windows' words.
  it("offers the host's runtime fix under the cards, and runs it", async () => {
    const fixes: unknown[] = [];
    await mountDone(
      (local) => ({
        ...local,
        available: false,
        backends: [],
        error: "Local Spaces on Windows run in Docker Desktop, which isn't installed yet.",
        fix: { action: "install-docker", label: "Install Docker Desktop", note: "About 600 MB.", busy: false, termsUrl: "https://www.docker.com/legal/" },
      }),
      (op, args) => {
        if (op === "spaces.fixRuntime") {
          fixes.push(args);
          return null;
        }
        return undefined;
      },
    );
    await screen.findByText("Create your first Space");
    const install = await screen.findByRole("button", { name: "Install Docker Desktop" });
    expect(screen.getByRole("button", { name: "Docker's terms" })).toBeTruthy();
    for (const os of ["macos", "linux", "windows"]) expect(createOf(os).disabled, os).toBe(true);
    fireEvent.click(install);
    await waitFor(() => expect(fixes).toEqual([{ action: "install-docker" }]));
  });

  it("shows a running fix as busy, with no action to press", async () => {
    await mountDone((local) => ({
      ...local,
      available: false,
      backends: [],
      error: "Docker Desktop is starting.",
      fix: { action: null, label: "Waiting for Docker Desktop…", note: null, busy: true, termsUrl: null },
    }));
    const waiting = (await screen.findByRole("button", { name: "Waiting for Docker Desktop…" })) as HTMLButtonElement;
    expect(waiting.disabled).toBe(true);
  });

  it("offers no fix once a runtime is ready", async () => {
    await mountDone((local) => ({
      ...local,
      backends: ["docker"],
      macosImage: null,
      hostArch: "amd64",
      fix: { action: "start-docker", label: "Start Docker Desktop", note: null, busy: false, termsUrl: null },
    }));
    await screen.findByText("Create your first Space");
    await waitFor(() => expect(createOf("linux").disabled).toBe(false));
    expect(screen.queryByRole("button", { name: "Start Docker Desktop" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Add a machine" })).toBeNull();
  });
});
