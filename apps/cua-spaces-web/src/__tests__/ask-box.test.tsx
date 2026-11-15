// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The new prompt box on Home: pick a model, optionally a repo, type, Enter.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { askTasks, resetAskTasks } from "@/lib/ask-first";
import { resetPickedAgent } from "@/lib/ask-first-model";
import { resetRepos } from "@/lib/ask-first-repo";
import { mountApp, pickNewLinux, stubBrowserApis } from "./app-harness";

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
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
// Three Claude sign-ins and one ChatGPT sign-in; nothing for Hermes or OpenCode.
const ACCOUNTS = { demo: { accounts: true } };

async function openAgents() {
  await act(async () => {
    fireEvent.click(await waitFor(() => q('[data-picker="agent"]')!));
  });
  await waitFor(() => expect(q("[data-agent-menu]")).not.toBeNull());
}

describe.skipIf(!wasmBuilt)("the prompt box", () => {
  it("starts with the first connected app and its account (Codex, then Hermes, Claude Code, OpenCode) and only the agent and a repo to pick", async () => {
    await mountApp("/home/ask", { failsCreates: false, listsGhost: false, ...ACCOUNTS });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toBe("GPT-5.5Medium"));
    expect(q('[data-picker="repo"]')!.textContent).toContain("Add a repo");
    // No model chip and no separate coding app picker.
    expect(q('[data-picker="model"]')).toBeNull();
    expect(q('[data-picker="who"]')).toBeNull();
  });

  it("groups by coding app: the top four first, their accounts as rows, Sign in where there is none", async () => {
    await mountApp("/home/ask", { failsCreates: false, listsGhost: false, ...ACCOUNTS });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    await openAgents();
    const groups = [...document.querySelectorAll<HTMLElement>("[data-agent-group]")].map((g) => g.dataset.agentGroup);
    expect(groups).toEqual(["codex", "hermes", "claude-code", "opencode"]);
    const rows = (app: string) => [...document.querySelectorAll<HTMLElement>(`[data-agent-group="${app}"] [role="option"]`)].map((r) => r.textContent);
    expect(rows("claude-code")).toEqual(["ada@example.com (Max)", "work Max", "side project Max"]);
    expect(rows("codex")).toEqual(["ChatGPT"]);
    expect(q('[data-agent-sign-in="hermes"]')!.textContent).toBe("Sign in");
    expect(q('[data-agent-sign-in="opencode"]')!.textContent).toBe("Sign in");
    expect(q('[data-agent-sign-in="claude-code"]')).toBeNull();
    // The model is not a group: it is one quiet line, and only for an account that offers a choice.
    expect(q("[data-model-group]")).toBeNull();
    // An account with one model shows no model line at all.
    expect(q("[data-agent-model-row]")).toBeNull();
  });

  it("picks an app and an account, shows both on the chip, and remembers them across a reload", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    const host = {
      failsCreates: false,
      listsGhost: false,
      ...ACCOUNTS,
      intercept: (op: string, args: unknown) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    };
    await mountApp("/home/ask", host);
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    await openAgents();
    await act(async () => {
      fireEvent.click(q('[data-agent-group="claude-code"] [data-agent-account="kwork"]')!);
    });
    // The chip is the app alone; the account is the ticked row inside the menu.
    expect(q("[data-ask-agent]")!.textContent).toBe("Sonnet 5.5High");
    await openAgents();
    expect(q('[data-agent-group="claude-code"] [aria-selected="true"]')!.textContent).toBe("work Max");
    await act(async () => {
      fireEvent.keyDown(q("[data-agent-menu]")!, { key: "Escape" });
    });
    // The host keeps which account the app runs with, so a run uses the one shown.
    await waitFor(() =>
      expect(
        calls.some((c) => c.op === "agentKeys.use" && c.args.app === "claude-code" && c.args.env === "CLAUDE_CODE_OAUTH_TOKEN" && c.args.slot === "kwork"),
      ).toBe(true),
    );
    cleanup();
    // A new launch: the choice is still there (the app and model here, the account from the host's store).
    await mountApp("/home/ask", host);
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toBe("Sonnet 5.5High"));
    await openAgents();
    expect(q('[data-agent-group="claude-code"] [aria-selected="true"]')!.textContent).toBe("work Max");
  });

  it("says in one line what signing in is for, and leaves managing sign-ins to Settings", async () => {
    await mountApp("/home/ask", { failsCreates: false, listsGhost: false, ...ACCOUNTS });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    await openAgents();
    expect(q("[data-agent-menu-why]")!.textContent).toBe("Pick the agent for this job. Sign in once, and it runs on any of your computers.");
    // No sign-in management in the job box: no chevrons, no rename or remove.
    expect(q("[data-agent-manage]")).toBeNull();
    expect(q("[data-manage-rename]")).toBeNull();
    expect(q("[data-agent-configure]")!.textContent).toBe("Manage agents and sign-ins");
  });

  it("an app's sign-ins live on its Settings page: why, which one is in use, use another, rename and remove", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    await mountApp("/settings/connections/claude-code", {
      failsCreates: false,
      listsGhost: false,
      ...ACCOUNTS,
      intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    });
    const page = await waitFor(() => {
      const el = q('[data-agent-sign-ins="claude-code"]');
      expect(el).not.toBeNull();
      return el!;
    });
    expect(document.body.textContent).toContain("Sign in once so your agents can run on your other computers without asking again.");
    expect(page.textContent).toContain("ada@example.com");
    expect(q("[data-manage-default]")!.closest("[data-manage-account]")!.textContent).toContain("ada@example.com");
    await act(async () => {
      fireEvent.click(q('[data-manage-account="kwork"] [data-manage-use]')!);
    });
    await waitFor(() => expect(calls.some((c) => c.op === "agentKeys.use" && c.args.slot === "kwork")).toBe(true));
    await act(async () => {
      fireEvent.click(q('[data-manage-account="kside"] [data-manage-rename]')!);
    });
    const name = screen.getByLabelText("Name") as HTMLInputElement;
    fireEvent.change(name, { target: { value: "client Max" } });
    await act(async () => {
      fireEvent.keyDown(name, { key: "Enter" });
    });
    await waitFor(() => expect(calls.some((c) => c.op === "agentKeys.rename" && c.args.slot === "kside" && c.args.label === "client Max")).toBe(true));
    await waitFor(() => expect(q('[data-manage-account="kside"]')!.textContent).toContain("client Max"));
    await act(async () => {
      fireEvent.click(q('[data-manage-account="kside"] [data-manage-remove]')!);
    });
    await act(async () => {
      fireEvent.click(q("[data-manage-confirm-remove]")!);
    });
    await waitFor(() => expect(calls.some((c) => c.op === "agentKeys.remove" && c.args.slot === "kside")).toBe(true));
    await waitFor(() => expect(q('[data-manage-account="kside"]')).toBeNull());
  });

  it("the model is a secondary line: it lists the account's models and keeps the pick", async () => {
    await mountApp("/home/ask", { failsCreates: false, listsGhost: false, ...ACCOUNTS });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    await openAgents();
    await act(async () => {
      fireEvent.click(q('[data-agent-group="claude-code"] [data-agent-account="kwork"]')!);
    });
    await openAgents();
    await act(async () => {
      fireEvent.click(q("[data-agent-model-row]")!);
    });
    // Sonnet 5.5 is the default (checked); Haiku 5.5 is one tap away, tagged Fast; older ids are not listed.
    expect(q('[data-model-row="claude-sonnet-5-5"]')!.getAttribute("aria-selected")).toBe("true");
    expect(q('[data-model-row="claude-haiku-5-5"]')!.textContent).toContain("Fast");
    expect(q('[data-model-row="claude-haiku-4-5"]')).toBeNull();
    await act(async () => {
      fireEvent.click(q('[data-model-row="claude-haiku-5-5"]')!);
    });
    await openAgents();
    expect(q("[data-agent-model-row]")!.textContent).toContain("Claude Haiku 5.5");
  });

  it("Enter sends the job with the picked app, account and model; Shift+Enter is a new line", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    await mountApp("/home/ask", {
      failsCreates: false,
      listsGhost: false,
      ...ACCOUNTS,
      intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    });
    const box = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    await openAgents();
    await act(async () => {
      fireEvent.click(q('[data-agent-group="claude-code"] [data-agent-account="kwork"]')!);
    });
    await pickNewLinux();
    fireEvent.change(box, { target: { value: "fix the build" } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    });
    expect(askTasks()).toHaveLength(0);
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    await waitFor(() => expect(askTasks()[0]).toMatchObject({ prompt: "fix the build", agent: "claude-code", model: "claude-sonnet-5-5" }));
    await waitFor(() => expect(calls.some((c) => c.op === "thread.post" && c.args.text === "@claude fix the build")).toBe(true), { timeout: 15000 });
  });

  it("with nothing signed in the chip says to sign in, every app has a Sign in row and Start waits", async () => {
    await mountApp("/home/ask", { failsCreates: false, listsGhost: false, demo: { keys: "none" } });
    await screen.findByLabelText("What should it do?");
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toBe("Choose an agent"));
    expect((screen.getByRole("button", { name: "Start" }) as HTMLButtonElement).disabled).toBe(true);
    await openAgents();
    expect(document.querySelectorAll("[data-agent-sign-in]")).toHaveLength(4);
  });

  it("adds a repo from a link, refuses what is not one, keeps it on the task, and removes it with the x", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    const { router } = await mountApp("/home/ask", {
      failsCreates: false,
      listsGhost: false,
      ...ACCOUNTS,
      intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    });
    const box = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    await act(async () => {
      fireEvent.click(
        await waitFor(() => {
          const el = q('[data-picker="repo"]');
          expect(el).not.toBeNull();
          return el!;
        }),
      );
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
    await pickNewLinux();
    fireEvent.change(box, { target: { value: "find out why the tests fail" } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    await waitFor(() => expect(askTasks()[0]!.repo).toEqual({ url: "https://github.com/trycua/demo-failing-tests", label: "trycua/demo-failing-tests" }));
    // The agent is told the repo in the job's own message.
    await waitFor(
      () =>
        expect(
          calls.some(
            (c) => c.op === "thread.post" && String(c.args.text).includes("find out why the tests fail\n\nRepo: https://github.com/trycua/demo-failing-tests"),
          ),
        ).toBe(true),
      { timeout: 15000 },
    );
    // It stays for the next job until removed (back on Home, the job's Space is open now).
    await act(async () => void router.navigate({ to: "/home/ask" }));
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
    for (const f of ["ask-box.tsx", "agent-picker.tsx", "repo-chip.tsx", "home.tsx"]) {
      const text = readFileSync(join(dir, f), "utf8");
      expect(text, f).not.toContain("·");
      expect(text.toLowerCase(), f).not.toMatch(/\bdevices?\b/);
    }
  });
});
