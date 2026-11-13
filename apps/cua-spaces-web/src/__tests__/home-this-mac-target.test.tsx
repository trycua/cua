// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Home's box starts on this computer once it's in the account (5 Nov 2026), shown as a chip with a
// switcher; when this computer isn't set up yet, the chip says so and offers one Set up.

import { cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { resetAskTasks } from "@/lib/ask-first";
import { resetPickedAgent } from "@/lib/ask-first-model";
import { resetRepos } from "@/lib/ask-first-repo";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetPickedAgent();
  resetRepos();
  resetConnectState();
  resetConnectedAppsState();
});
afterEach(() => {
  cleanup();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);

describe.skipIf(!wasmBuilt)("Home's box, where it runs", () => {
  it("starts on This Mac once it's in the account, as a chip, with no Space made first", { timeout: 30_000 }, async () => {
    await mountApp("/home/ask", { failsCreates: false, listsGhost: false, demo: { noSpaces: true } });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-where-chip]")?.dataset.askWhereChip).toBe("this-mac"));
    expect(q("[data-ask-where]")!.textContent).toBe("This Mac");
    expect(q("[data-ask-where-setup]")).toBeNull();
  });

  it("signed in on a Mac that isn't set up yet: the chip says so, with one Set up that sets it up", { timeout: 30_000 }, async () => {
    const calls: string[] = [];
    // This Mac is in the account but doesn't share its desktop yet (what signing in sets up).
    let shared = false;
    await mountApp("/home/ask", {
      failsCreates: false,
      listsGhost: false,
      demo: { noSpaces: true },
      intercept: (op, args) => {
        calls.push(op);
        if (op === "host.action" && (args as { action: string }).action === "share-desktop") shared = true;
        return undefined;
      },
      answer: (op, out) => (op === "host.status" ? { ...(out as object), shareDesktop: shared } : out),
    });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-where-chip]")?.dataset.askWhereChip).toBe("needs-setup"));
    expect(q("[data-ask-where]")!.textContent).toContain("not set up for agents yet");
    const setUp = screen.getByRole("button", { name: "Set up This Mac so agents can work on it" });
    setUp.click();
    await waitFor(() => expect(calls).toContain("host.action"), { timeout: 15_000 });
    await waitFor(() => expect(q("[data-ask-where-chip]")?.dataset.askWhereChip).toBe("this-mac"), { timeout: 15_000 });
    expect(calls).not.toContain("spaces.create");
  });

  it("signed out: starts on a new computer, as before", { timeout: 30_000 }, async () => {
    await mountApp("/home/ask", { failsCreates: false, listsGhost: false, demo: { signedIn: false, onboarded: true, noSpaces: true } });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-where-chip]")?.dataset.askWhereChip).toBe("new"));
  });
});
