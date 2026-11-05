// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// CUA-3193: on a Mac that can't run Spaces itself, the new setup stopped on
// "Preparing stopped: local runtime: This Mac can't run Spaces itself, ..." with Try again.
// No raw runtime error reaches Home: the core's sentence shows, with "Add a computer".

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectState } from "@/hooks/use-connectors";
import { resetAskTasks } from "@/lib/ask-first";
import { resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetConnectState();
  resetPrepare();
  setFlagOverride("spaces_ask_first_home", true);
  setFlagOverride("spaces_new_onboarding", true);
});
afterEach(() => {
  cleanup();
  resetPrepare();
  vi.unstubAllEnvs();
});

/** `cua_create_failure::preflight::MAC_UNSUPPORTED`, the daemon's refusal (#6323). */
const MAC_UNSUPPORTED =
  "This Mac can't run Spaces itself, because it can't run virtual computers. That happens when the Mac is itself a virtual machine, or is an older Intel Mac. Add another computer to your account, or link your own cloud, and create the Space there.";

/** What the SDK's errors start with: internal words a person never reads. */
const RAW = [/local runtime:/i, /\[local\]/i, /config\.\w+:/, /os error/i, /exit(ing)?,? status/i];

describe.skipIf(!wasmBuilt)("preparing on a Mac that can't run Spaces", () => {
  for (const raw of [`local runtime: ${MAC_UNSUPPORTED}`, `local runtime: [local] ${MAC_UNSUPPORTED}`]) {
    it(`says so in plain words with Add a computer (${raw.slice(0, 22)})`, { timeout: 40000 }, async () => {
      const { router } = await mountApp("/home", {
        failsCreates: false,
        listsGhost: false,
        demo: { onboarded: true },
        intercept: (op) => {
          if (op === "spaces.create") throw new Error(raw);
          return undefined;
        },
      });
      const line = await waitFor(
        () => {
          const el = document.querySelector<HTMLElement>("[data-prepare-line]");
          expect(el).not.toBeNull();
          return el!;
        },
        { timeout: 20000 },
      );
      const text = line.textContent ?? "";
      expect(text).toContain("This Mac can't run Spaces itself");
      for (const r of RAW) expect(text).not.toMatch(r);
      const button = line.querySelector<HTMLElement>("[data-prepare-action]")!;
      expect(button.dataset.prepareAction).toBe("add-computer");
      expect(button.textContent).toBe("Add a computer");
      await act(async () => {
        fireEvent.click(button);
        await new Promise((r) => setTimeout(r, 20));
      });
      await waitFor(() => expect(router.state.location.pathname).toBe("/machines"));
      expect(router.state.location.search).toMatchObject({ add: true });
    });
  }
});
