// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The new prompt box on Home: pick a model, optionally a repo, type, Enter.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { askTasks, resetAskTasks } from "@/lib/ask-first";
import { resetPickedModel } from "@/lib/ask-first-model";
import { resetRepos } from "@/lib/ask-first-repo";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetPickedModel();
  resetRepos();
  resetConnectState();
  resetConnectedAppsState();
  setFlagOverride("spaces_ask_first_home", true);
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
const SEVERAL = { demo: { keys: "several" as const, rejectedKeys: ["DEEPSEEK_API_KEY"] } };

async function openModels() {
  await act(async () => {
    fireEvent.click(await waitFor(() => q('[data-picker="model"]')!));
  });
  await waitFor(() => expect(q("[data-model-menu]")).not.toBeNull());
}

describe.skipIf(!wasmBuilt)("the prompt box", () => {
  it("has a model to start with (the quick one of what is connected) and only the model and a repo to pick", async () => {
    await mountApp("/home", { failsCreates: false, listsGhost: false, ...SEVERAL });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-model]")!.textContent).toBe("Claude Haiku 4.5"));
    expect(q('[data-picker="repo"]')!.textContent).toContain("Add a repo");
    // No coding app picker: the app follows the model.
    expect(q('[data-picker="who"]')).toBeNull();
    expect(q("[data-ask-mention]")).toBeNull();
  });

  it("lists connected models by provider with Fast on the quick ones, and picks one with a click or a digit", async () => {
    await mountApp("/home", { failsCreates: false, listsGhost: false, ...SEVERAL });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-model]")!.textContent).toBe("Claude Haiku 4.5"));
    await openModels();
    const groups = [...document.querySelectorAll<HTMLElement>("[data-model-group]")].map((g) => g.dataset.modelGroup);
    expect(groups).toEqual(expect.arrayContaining(["anthropic", "openrouter"]));
    expect(q('[data-model-row="claude-haiku-4-5"]')!.textContent).toContain("Fast");
    expect(q('[data-model-row="claude-sonnet-5"]')!.textContent).not.toContain("Fast");
    await act(async () => {
      fireEvent.click(q('[data-model-row="claude-sonnet-5"]')!);
    });
    expect(q("[data-ask-model]")!.textContent).toBe("Claude Sonnet 5");
    // Picked once, kept: a reopened menu ticks it, and the digits pick rows.
    await openModels();
    expect(q('[data-model-row="claude-sonnet-5"]')!.getAttribute("aria-selected")).toBe("true");
    await act(async () => {
      fireEvent.keyDown(q("[data-model-menu]")!, { key: "1" });
    });
    expect(q("[data-ask-model]")!.textContent).toBe("Claude Haiku 4.5");
  });

  it("Enter sends the job with the picked model and the app that runs it; Shift+Enter is a new line", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    await mountApp("/home", {
      failsCreates: false,
      listsGhost: false,
      ...SEVERAL,
      intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    });
    const box = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    await waitFor(() => expect(q("[data-ask-model]")!.textContent).toBe("Claude Haiku 4.5"));
    fireEvent.change(box, { target: { value: "fix the build" } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    });
    expect(askTasks()).toHaveLength(0);
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    await waitFor(() => expect(askTasks()[0]).toMatchObject({ prompt: "fix the build", agent: "claude-code", model: "claude-haiku-4-5" }));
    await waitFor(() => expect(calls.some((c) => c.op === "thread.post" && c.args.text === "@claude fix the build")).toBe(true), { timeout: 15000 });
  });

  it("Connect a provider opens Connections, and with nothing connected the model says so and Start waits", async () => {
    const { router } = await mountApp("/home", { failsCreates: false, listsGhost: false, demo: { keys: "none" } });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-model]")!.textContent).toBe("Connect a provider"));
    expect((screen.getByRole("button", { name: "Start" }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => {
      fireEvent.click(q('[data-picker="model"]')!);
    });
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/connections"));
  });

  it("adds a repo from a link, refuses what is not one, keeps it on the task, and removes it with the x", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    const { router } = await mountApp("/home", { failsCreates: false, listsGhost: false, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    const box = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    await act(async () => {
      fireEvent.click(await waitFor(() => {
        const el = q('[data-picker="repo"]');
        expect(el).not.toBeNull();
        return el!;
      }));
    });
    const field = await screen.findByLabelText("Repo link or owner/name");
    fireEvent.change(field, { target: { value: "what is this" } });
    await act(async () => {
      fireEvent.keyDown(field, { key: "Enter" });
    });
    expect(field.getAttribute("aria-invalid")).toBe("true");
    fireEvent.change(field, { target: { value: "https://github.com/trycua/demo-failing-tests" } });
    await act(async () => {
      fireEvent.keyDown(field, { key: "Enter" });
    });
    await waitFor(() => expect(q("[data-ask-repo]")!.dataset.askRepo).toBe("trycua/demo-failing-tests"));
    fireEvent.change(box, { target: { value: "find out why the tests fail" } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    await waitFor(() => expect(askTasks()[0]!.repo).toEqual({ url: "https://github.com/trycua/demo-failing-tests", label: "trycua/demo-failing-tests" }));
    // The agent is told the repo in the job's own message.
    await waitFor(() => expect(calls.some((c) => c.op === "thread.post" && String(c.args.text).includes("find out why the tests fail\n\nRepo: https://github.com/trycua/demo-failing-tests"))).toBe(true), { timeout: 15000 });
    // It stays for the next job until removed (back on Home, the job's Space is open now).
    await act(async () => void router.navigate({ to: "/home" }));
    await act(async () => {
      fireEvent.click(
        await waitFor(() => {
          const el = q("[data-ask-repo-remove]");
          expect(el).not.toBeNull();
          return el!;
        }),
      );
    });
    expect(q("[data-ask-repo]")).toBeNull();
    expect(q('[data-picker="repo"]')).not.toBeNull();
  });
});

describe("the box's words", () => {
  it("never join two things with a middle dot, and never say devices", () => {
    const dir = join(__dirname, "..", "components", "ask-first");
    for (const f of ["ask-box.tsx", "model-picker.tsx", "repo-chip.tsx", "home.tsx"]) {
      const text = readFileSync(join(dir, f), "utf8");
      expect(text, f).not.toContain("·");
      expect(text.toLowerCase(), f).not.toMatch(/\bdevices?\b/);
    }
  });
});
