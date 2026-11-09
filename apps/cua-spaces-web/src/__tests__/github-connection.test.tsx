// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Connections, GitHub: one row per host with its state in plain
// words, a page behind each row (the switch, the account picker, how Cua signs
// in), Rescan, and a pasted token through the key sheet. The host is the demo
// host; nothing here is a real GitHub account and no token is ever shown.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetConnectedAppsState();
  resetConnectState();
});
afterEach(cleanup);

type Call = { op: string; args: unknown };
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
const host = (calls: Call[], github?: "gh" | "none" | "saved" | "warning") => ({
  failsCreates: false,
  listsGhost: false,
  demo: { ...(github ? { github } : {}) },
  intercept: (op: string, args: unknown) => (calls.push({ op, args }), undefined),
});
const line = (h: string) => q(`[data-github-host="${h}"] [data-github-line]`)?.textContent;
const openRow = async (h: string) => {
  await click(await found(`[data-github-host="${h}"]`));
  return found(`[data-github-detail="${h}"]`);
};

describe.skipIf(!wasmBuilt)("Connections, GitHub (real routes, wasm core)", () => {
  it("lists each host with its state in plain words, github.com first, with no middle dots", async () => {
    await mountApp("/settings/connections", host([]));
    await found('[data-github-host="github.com"]');
    expect(qa("[data-connectors-github] [data-github-host]").map((r) => r.dataset.githubHost)).toEqual(["github.com", "ghe.example.com"]);
    expect(line("github.com")).toBe("Signed in as ada with gh");
    expect(line("ghe.example.com")).toBe("Signed in as ada-corp with gh");
    expect(q("[data-connectors-github]")!.textContent).not.toContain("·");
    expect(document.body.textContent).toContain("Spaces never get it");
  });

  it("picks one of several gh accounts, warns about broader scopes, and goes back to gh's own", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections", host(calls));
    const sheet = await openRow("github.com");
    // The picker is there because gh has two accounts on this host.
    expect(sheet.querySelector("[data-github-accounts]")!.textContent).toContain("Account");
    expect(q("[data-github-account=ada]")!.getAttribute("aria-checked")).toBe("true");
    expect(q("[data-github-warning]")).toBeNull();
    await click(q("[data-github-account=bob]")!);
    await waitFor(() => expect(line("github.com")).toBe("Signed in as bob with gh"));
    expect(calls.some((c) => c.op === "github.use" && JSON.stringify(c.args) === JSON.stringify({ host: "github.com", login: "bob" }))).toBe(true);
    // bob's token can change workflows and administer organizations: said plainly, on the page and as a mark on the row.
    await waitFor(() => expect(q("[data-github-warning]")?.textContent).toContain("change GitHub Actions workflows and administer organizations and settings"));
    expect(q('[data-github-host="github.com"] [data-github-warning-mark]')).not.toBeNull();
    await click(await found("[data-github-auto]"));
    await waitFor(() => expect(line("github.com")).toBe("Signed in as ada with gh"));
    expect(q("[data-github-warning]")).toBeNull();
    expect(calls.filter((c) => c.op === "github.use").pop()!.args).toEqual({ host: "github.com", login: null });
  });

  it("switches a host off per host and says so on the row", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections", host(calls));
    await openRow("github.com");
    await click(await found("[data-github-switch]"));
    await waitFor(() => expect(line("github.com")).toBe("Turned off: Cua will not use GitHub on this host"));
    expect(line("ghe.example.com")).toBe("Signed in as ada-corp with gh");
    expect(calls.find((c) => c.op === "github.enable")!.args).toEqual({ host: "github.com", enabled: false });
  });

  it("says what to do when nothing is signed in and Rescan asks the daemon again", async () => {
    const calls: Call[] = [];
    await mountApp("/settings/connections", host(calls, "none"));
    await found('[data-github-host="github.com"]');
    expect(line("github.com")).toBe("No GitHub sign-in found: run gh auth login or paste a token");
    expect(qa("[data-github-host]").length).toBe(1);
    await click(await found("[data-github-rescan]"));
    await waitFor(() => expect(calls.some((c) => c.op === "github.rescan")).toBe(true));
    const sheet = await openRow("github.com");
    expect(sheet.querySelector("[data-github-help]")!.textContent).toBe("Run gh auth login in a terminal, or paste a token, then Rescan.");
    // Paste a token opens the existing key sheet for GITHUB_TOKEN.
    await click(sheet.querySelector("[data-github-paste]")!);
    await waitFor(() => expect(document.body.textContent).toContain("GITHUB_TOKEN"));
  });

  it("shows a saved token as the source in use, ahead of gh", async () => {
    await mountApp("/settings/connections", host([], "saved"));
    await found('[data-github-host="github.com"]');
    expect(line("github.com")).toBe("Signed in as carol with a saved token");
    const sheet = await openRow("github.com");
    expect(sheet.querySelector('[data-github-source="saved"]')!.textContent).toContain("In use");
    expect(sheet.querySelector('[data-github-source="gh"]')!.textContent).not.toContain("In use");
  });
});
