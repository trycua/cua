// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Connect for Claude Code imports the sign-in Claude Code already has on this
// machine, once the person agrees: the consent says what is read, kept and sent
// (the core's words); nothing is read before it; "Signed in as ..." is the one
// wording on every surface; a sign-in that ran out reads "Needs reconnect" with
// Reconnect everywhere; and when there is nothing to import the ways on are
// offered in order (sign in with Claude Code, paste a token, use an API key).

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetConnectedAppsState();
  resetConnectState();
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const found = (s: string) =>
  waitFor(() => {
    const el = q(s);
    expect(el).not.toBeNull();
    return el!;
  });
const click = (el: Element) =>
  act(async () => {
    fireEvent.click(el);
  });

type Call = { op: string; args: unknown };
function host(calls: Call[], demo: { claudeLogin?: "signed-in" | "expired" | "none" } = {}) {
  return { failsCreates: false, listsGhost: false, demo, intercept: (op: string, args: unknown) => (calls.push({ op, args }), undefined) };
}
const connects = (calls: Call[]) => calls.filter((c) => c.op === "agents.connect");

const WHO = "Signed in as ada@example.com (Claude Max)";

describe.skipIf(!wasmBuilt)("importing the Claude Code sign-in", () => {
  it("asks first, in plain words, and reads nothing until you agree", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connectors", host(calls));
    await click(await found("[data-connector-app=claude-code] [data-connector-connect]"));
    const sheet = await found("[data-import-sheet=consent]");
    const text = sheet.textContent ?? "";
    expect(q("[data-import-title]")!.textContent).toBe("Use your Claude Code sign-in");
    expect(text).toContain("Claude Code-credentials");
    expect(text).toContain("~/.claude/.credentials.json");
    expect(text).toContain("never your refresh token");
    expect(text).toContain("You stay signed in to Claude Code");
    expect(text).toContain("Your Spaces get that access token");
    // Nothing is read, and nothing is added anywhere, while the question is open.
    expect(connects(calls)).toEqual([]);
    expect(calls.some((c) => c.op === "agents.setupDriver")).toBe(false);
    // The words of this feature: no middle dot, no "devices", no setup-token promise.
    expect(text).not.toContain("·");
    expect(text.toLowerCase()).not.toContain("device");
    expect(document.body.textContent).not.toContain("claude setup-token");
  });

  it("Cancel leaves everything as it was", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connectors", host(calls));
    await click(await found("[data-connector-app=claude-code] [data-connector-connect]"));
    await found("[data-import-sheet=consent]");
    await click([...document.querySelectorAll("button")].find((b) => b.textContent === "Cancel")!);
    await waitFor(() => expect(q("[data-import-sheet]")).toBeNull());
    expect(connects(calls)).toEqual([]);
    expect(q("[data-connector-app=claude-code]")!.dataset.state).toBe("not-connected");
  });

  it("Use my sign-in imports it for Claude Code alone, then every surface says who", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connectors", host(calls));
    await click(await found("[data-connector-app=claude-code] [data-connector-connect]"));
    await click(await found("[data-import-confirm]"));
    await waitFor(() => expect(q("[data-import-sheet]")).toBeNull());
    expect(connects(calls).map((c) => c.args)).toEqual([{ agents: ["claude-code"], signIn: true }]);
    await waitFor(() => expect(q("[data-connector-app=claude-code]")!.dataset.state).toBe("connected"));
    // The row, the Anthropic provider row and the app's own page.
    expect(q("[data-connector-app=claude-code]")!.textContent).toContain(WHO);
    expect(q("[data-connector-provider=anthropic]")!.textContent).toContain(WHO);
    expect(q("[data-connector-provider=anthropic]")!.textContent).toContain("Sign out");
    // Hermes does not count it: a Claude sign-in is Claude Code's alone.
    expect(q("[data-connector-app=hermes]")!.dataset.state).not.toBe("connected");
  });

  it("a sign-in that ran out reads Needs reconnect with Reconnect, never the name", async () => {
    await mountApp("/settings/connectors", host([], { claudeLogin: "expired" }));
    await waitFor(() => expect(q("[data-connector-provider=anthropic]")?.textContent).toContain("Needs reconnect"));
    const app = q("[data-connector-app=claude-code]")!;
    expect(app.dataset.state).toBe("not-connected");
    expect(app.textContent).toContain("Needs reconnect");
    expect(app.querySelector("[data-connector-connect]")?.textContent).toBe("Reconnect");
    expect(q("[data-connector-provider=anthropic] [data-connector-provider-action]")?.textContent).toBe("Reconnect");
    expect(app.textContent).not.toContain("ada@example.com");
    // Hermes reads an API key, not Claude Code's sign-in: its row does not say Reconnect.
    expect(q("[data-connector-app=hermes] [data-connector-connect]")?.textContent).toBe("Connect");
  });

  it("Reconnect asks again and imports the newer sign-in", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connectors", host(calls, { claudeLogin: "expired" }));
    await click(await found("[data-connector-app=claude-code] [data-connector-connect]"));
    await click(await found("[data-import-confirm]"));
    await waitFor(() => expect(q("[data-connector-app=claude-code]")!.dataset.state).toBe("connected"));
    expect(q("[data-connector-provider=anthropic]")!.textContent).toContain(WHO);
    expect(q("[data-connector-provider=anthropic]")!.textContent).not.toContain("Needs reconnect");
  });

  it("with nothing to import it offers, in order: sign in with Claude Code, paste a token, use an API key", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connectors", host(calls, { claudeLogin: "none" }));
    await click(await found("[data-connector-app=claude-code] [data-connector-connect]"));
    await click(await found("[data-import-confirm]"));
    const sheet = await found("[data-import-sheet=not-found]");
    expect(sheet.textContent).toContain("No Claude Code sign-in found");
    expect(qa("[data-import-option]").map((o) => o.dataset.importOption)).toEqual(["login", "token", "key"]);
    expect(q("[data-import-command]")!.textContent).toBe("claude auth login");
    // No error banner, and nothing saved.
    expect(q("[data-connector-app=claude-code]")!.dataset.state).not.toBe("connected");
    // Paste a token opens the Anthropic sheet straight on the token field.
    await click(q("[data-import-option-action=token]")!);
    const field = await found("[data-provider-key]");
    expect(document.body.textContent).toContain("Claude token");
    fireEvent.change(field, { target: { value: `sk-ant-oat01-${"aB3_".repeat(24)}` } });
    await click(q("[data-provider-save]")!);
    await waitFor(() => expect(q("[data-connector-provider=anthropic]")!.dataset.state).toBe("connected"));
  });

  it("Try again after signing in with Claude Code reads it again", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connectors", host(calls, { claudeLogin: "none" }));
    await click(await found("[data-connector-app=claude-code] [data-connector-connect]"));
    await click(await found("[data-import-confirm]"));
    await found("[data-import-sheet=not-found]");
    await click(q("[data-import-option-action=login]")!);
    await waitFor(() => expect(connects(calls)).toHaveLength(2));
    await found("[data-import-sheet=not-found]");
  });
});

describe.skipIf(!wasmBuilt)("the Space thread's Connect card", () => {
  const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;
  const failureReply = () => qa("[data-entry]").find((e) => e.querySelector("[data-failure-connect]") && e.textContent?.includes("Claude"));
  const reply = (demo: object) =>
    mountApp(path, { failsCreates: false, listsGhost: false, demo: { threadAuthFailed: true, ...demo } }).then(() =>
      waitFor(
        () => {
          const r = failureReply();
          expect(r).toBeTruthy();
          return r as HTMLElement;
        },
        { timeout: 5000 },
      ),
    );

  it("says Reconnect when the saved sign-in ran out, with the same word as Settings", async () => {
    const r = await reply({ claudeLogin: "expired" });
    await waitFor(() => expect(r.querySelector("[data-failure-connect]")?.textContent).toContain("Reconnect"));
    expect(r.querySelector<HTMLAnchorElement>("[data-failure-connect]")!.getAttribute("href")).toBe("/settings/connectors/claude-code");
    expect(r.textContent).not.toContain("\u00b7");
  });

  it("says Connect when there is no sign-in saved", async () => {
    const r = await reply({});
    expect(r.querySelector("[data-failure-connect]")?.textContent).toContain("Connect");
    expect(r.querySelector("[data-failure-connect]")?.textContent).not.toContain("Reconnect");
  });
});
