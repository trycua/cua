// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider, expiresInText, useMachines } from "@/bridge";
import { HostError, type DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter, type DemoOptions } from "@/bridge/adapters/demo";
import { unavailableCore } from "@/bridge/core";
import type { PairingCode } from "@/bridge/contracts/devices";
import { AddComputerSheet, agentPrompt, METHODS, type AddView } from "@/components/add-computer-sheet";

afterEach(cleanup);

type Override = Record<string, (args: Record<string, unknown>) => Promise<unknown>>;

/** The demo host, with some operations answered differently, recording the pairing calls. */
function host(options: DemoOptions = {}, overrides: Override | ((base: DataAdapter) => Override) = {}) {
  const base = createDemoAdapter({ latencyMs: 0, stepMs: 0, signedIn: true, onboarded: true, ...options });
  const override = typeof overrides === "function" ? overrides(base) : overrides;
  const calls: string[] = [];
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: Record<string, unknown>) => {
      if (op.startsWith("devices.") && op.includes("airing")) calls.push(op);
      const f = override[op];
      if (f) return f(args);
      return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
    }) as DataAdapter["call"],
  });
  return { adapter, calls, base };
}

/** A host whose other computer never uses the code (to look at a page while it waits). */
const neverUsed = (base: DataAdapter): Override => ({
  "machines.list": async () => ((await base.call("machines.list", {})) as { id: string }[]).filter((r) => r.id !== "m_pair_demo"),
  "devices.pairingCodes": async () => ((await base.call("devices.pairingCodes", {})) as PairingCode[]).map((c) => ({ ...c, state: c.state === "used" ? "active" : c.state, machine: null })),
});

function Harness(props: { team?: string | null; initialView?: AddView; initialCode?: string; pollMs?: number; settleMs?: number }) {
  const { data: machines = [] } = useMachines();
  return (
    <>
      <ul aria-label="Machines">
        {machines.map((m) => (
          <li key={m.id}>{m.name}</li>
        ))}
      </ul>
      <AddComputerSheet open onOpenChange={() => {}} pollMs={props.pollMs ?? 20} newComputerPollMs={20} settleMs={props.settleMs ?? 0} {...props} />
    </>
  );
}

async function mount(adapter: DataAdapter, props: Parameters<typeof Harness>[0] = {}) {
  render(
    <BridgeProvider adapter={adapter} core={unavailableCore("test: no core")}>
      <Harness {...props} />
    </BridgeProvider>,
  );
  await act(() => new Promise((r) => setTimeout(r, 10)));
}

const sheet = () => document.querySelector<HTMLElement>("[data-add-computer]")!;
const view = () => sheet()?.getAttribute("data-add-computer");
const phase = () => sheet()?.getAttribute("data-add-machine");
const method = (v: string) => document.querySelector<HTMLButtonElement>(`[data-add-method="${v}"]`)!;
const status = () => document.querySelector("[data-add-status]")?.textContent ?? "";
const prompt = () => document.querySelector<HTMLTextAreaElement>("#add-agent-prompt")!;
const back = () => fireEvent.click(document.querySelector("[data-add-back]")!);

describe("Add a machine: the methods", () => {
  it("lists one row per method, Sign in there first, and a prompt for an agent with the live code", async () => {
    const { adapter, calls } = host({}, neverUsed);
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    expect(view()).toBe("pick");
    expect([...document.querySelectorAll("[data-add-method]")].map((b) => b.textContent)).toEqual(["Sign in there", "Open a link", "Scan a QR code", "Run a command", "Enter a code"]);
    expect(METHODS[0]!.view).toBe("signin");
    expect(prompt().readOnly).toBe(true);
    expect(prompt().value).toMatch(/^Add this machine to my Cua Spaces: https:\/\/cua\.ai\/add\/K7QM-4XPB-[0-9A-Z]{4}$/);
    expect(screen.getByLabelText("Copy prompt")).toBeTruthy();
    expect(screen.getByText("Enter it")).toBeTruthy();
    // No desktop-or-private choice, no by-hand steps on the sheet.
    expect(document.body.textContent).not.toMatch(/keep its screen private|Set it up by hand instead/);
    expect(calls).toEqual(["devices.createPairingCode"].concat(calls.slice(1)));
  });

  it("Sign in there: the install link and this account, then waits for any new machine", async () => {
    let extra = false;
    const { adapter } = host({}, (base) => ({
      ...neverUsed(base),
      "machines.list": async () => {
        // Only the computer that signs in is new (not the demo's paired one).
        const rows = ((await base.call("machines.list", {})) as { id: string }[]).filter((r) => r.id !== "m_pair_demo");
        return extra ? [...rows, { id: "m_new", name: "Office Mac", via: "relay", online: true, os: "macos", limits: [] }] : rows;
      },
    }));
    await mount(adapter);
    fireEvent.click(method("signin"));
    expect(view()).toBe("signin");
    expect(screen.getByText("https://cua.ai/install")).toBeTruthy();
    expect(document.querySelector("[data-add-signin]")?.textContent).toMatch(/sign in as ada@example\.com/);
    expect(status()).toBe("Waiting for a new machine…");
    // No code here, so no menu.
    expect(document.querySelector("[data-add-more]")).toBeNull();
    // The computers known on opening are read first (they are not new).
    await waitFor(() => expect(document.querySelectorAll('ul[aria-label="Machines"] li').length).toBeGreaterThan(0));
    // The other computer signs in: it shows up, and the sheet says so.
    extra = true;
    await waitFor(() => expect(status()).toBe("Added: Office Mac"), { timeout: 4000 });
    expect(document.querySelector("[data-add-done]")?.className).toContain("bg-primary");
  });

  it("machines listed in the first moments after opening are the account's own, not new", async () => {
    let late = false;
    const { adapter } = host({}, (base) => ({
      ...neverUsed(base),
      "machines.list": async () => {
        const rows = ((await base.call("machines.list", {})) as { id: string }[]).filter((r) => r.id !== "m_pair_demo");
        return late ? [...rows, { id: "m_late", name: "Late relay row", via: "relay", online: false, os: "linux", limits: [] }] : rows;
      },
    }));
    await mount(adapter, { settleMs: 300 });
    fireEvent.click(method("signin"));
    late = true;
    await act(() => new Promise((r) => setTimeout(r, 600)));
    expect(status()).toBe("Waiting for a new machine…");
  });

  it("Open a link: the one-time link to copy", async () => {
    const { adapter } = host({}, neverUsed);
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    fireEvent.click(method("link"));
    expect(document.querySelector("[data-add-link] code")?.textContent).toMatch(/^https:\/\/cua\.ai\/add\/K7QM-4XPB-[0-9A-Z]{4}$/);
    expect(screen.getByLabelText("Copy link")).toBeTruthy();
    expect(status()).toBe("Waiting for the machine…");
  });

  it("Scan a QR code: the same link, big", async () => {
    const { adapter } = host({}, neverUsed);
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    fireEvent.click(method("qr"));
    const qr = document.querySelector("[data-add-qr] [data-pairing-qr]")!;
    expect(qr.getAttribute("width")).toBe("208");
    expect(qr.querySelector("path")?.getAttribute("d")).toMatch(/^M\d/);
    expect(document.body.textContent).not.toMatch(/phone/i);
  });

  it("Run a command: one command for macOS and Linux, and Windows behind a quiet switch", async () => {
    const { adapter } = host({}, neverUsed);
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    fireEvent.click(method("command"));
    const command = () => document.querySelector("[data-add-command] code")?.textContent ?? "";
    expect(command()).toMatch(/^curl -fsSL https:\/\/cua\.ai\/install\.sh \| sh -s -- --pair K7QM-4XPB-[0-9A-Z]{4}$/);
    // Wrapped only between words: `--pair` stays whole.
    expect([...document.querySelectorAll("[data-add-command] code span")].map((w) => w.textContent?.trim())).toContain("--pair");
    fireEvent.click(screen.getByText("Windows (preview)"));
    await waitFor(() => expect(command()).toMatch(/install\.ps1\)\)\) -Pair K7QM-4XPB-/));
    back();
    expect(view()).toBe("pick");
  });

  it("Enter a code: the code big, with how long it works", async () => {
    const { adapter } = host({}, neverUsed);
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    fireEvent.click(method("code"));
    expect(document.querySelector("[data-pairing-code] code")?.textContent).toMatch(/^K7QM-4XPB-[0-9A-Z]{4}$/);
    expect(document.querySelector("[data-pairing-expires]")?.textContent).toBe("Works once. Expires in 30 min.");
    expect(screen.getByLabelText("Copy code")).toBeTruthy();
  });
});

describe("Add a machine: states", () => {
  it("waits, then says which machine was added, however it was added", async () => {
    const { adapter, calls } = host();
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("used"), { timeout: 3000 });
    expect(calls.filter((c) => c === "devices.pairingCodes").length).toBeGreaterThanOrEqual(3);
    await waitFor(() => expect(status()).toBe("Added: Office PC"));
    await waitFor(() => expect(document.querySelector('ul[aria-label="Machines"]')?.textContent).toContain("Office PC"));
    expect(screen.getByText("Done")).toBeTruthy();
    expect(screen.queryByText("Cancel")).toBeNull();
  });

  it("names the machine once it is listed, when the code was used before that", async () => {
    let hide = true;
    const { adapter } = host({}, (base) => ({
      "machines.list": async () => ((await base.call("machines.list", {})) as { id: string }[]).filter((r) => !hide || r.id !== "m_pair_demo"),
    }));
    await mount(adapter);
    await waitFor(() => expect(status()).toBe("Added: the machine"), { timeout: 3000 });
    hide = false;
    await waitFor(() => expect(status()).toBe("Added: Office PC"));
  });

  it("renews the code, the link, the command and the prompt by itself when it expires", async () => {
    let made = 0;
    const { adapter, calls } = host({}, (base) => ({
      ...neverUsed(base),
      // The first code lapses a moment after it is shown.
      "devices.createPairingCode": async (args) => {
        const code = (await base.call("devices.createPairingCode", args as never)) as PairingCode;
        made += 1;
        return made === 1 ? { ...code, expiresAt: Math.floor(Date.now() / 1000) + 1 } : code;
      },
    }));
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    const first = prompt().value;
    expect(first).toContain("K7QM-4XPB-2RHD");
    await waitFor(() => expect(prompt().value).toContain("K7QM-4XPB-9TCW"), { timeout: 4000 });
    expect(phase()).toBe("ready");
    expect(calls.filter((c) => c === "devices.createPairingCode")).toHaveLength(2);
  });

  it("does not renew a code that arrives already expired (a clock that is off), and offers New code", async () => {
    const { adapter, calls } = host({ now: () => Date.now() - 31 * 60_000 });
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("expired"));
    fireEvent.click(method("code"));
    await act(() => new Promise((r) => setTimeout(r, 100)));
    expect(calls.filter((c) => c === "devices.createPairingCode")).toHaveLength(1);
    expect(status()).toMatch(/^This code expired\.\s*New code$/);
  });

  it("revokes from the menu, says so, and makes a new code on request", async () => {
    const { adapter, calls } = host({}, neverUsed);
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    fireEvent.click(method("code"));
    fireEvent.click(document.querySelector("[data-add-more]")!);
    await waitFor(() => expect(screen.getByText("Revoke")).toBeTruthy());
    expect(document.querySelector("[data-pairing-menu-expires]")?.textContent).toBe("Works once. Expires in 30 min.");
    expect(screen.getByText("New code")).toBeTruthy();
    expect(screen.getByText("Set it up by hand")).toBeTruthy();
    fireEvent.click(screen.getByText("Revoke"));
    await waitFor(() => expect(phase()).toBe("revoked"));
    expect(calls).toContain("devices.revokePairingCode");
    expect(status()).toMatch(/^You revoked this code\./);
    fireEvent.click(document.querySelector("[data-pairing-renew]")!);
    await waitFor(() => expect(phase()).toBe("ready"));
    expect(calls.filter((c) => c === "devices.createPairingCode")).toHaveLength(2);
  });

  it("Set it up by hand goes to Sign in there", async () => {
    const { adapter } = host({}, neverUsed);
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    fireEvent.click(method("command"));
    fireEvent.click(document.querySelector("[data-add-more]")!);
    await waitFor(() => expect(screen.getByText("Set it up by hand")).toBeTruthy());
    fireEvent.click(screen.getByText("Set it up by hand"));
    await waitFor(() => expect(view()).toBe("signin"));
  });

  it("shows that it is making a code", async () => {
    const { adapter } = host({}, { "devices.createPairingCode": () => new Promise(() => {}) });
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("minting"));
    expect(prompt().value).toBe("Making a code…");
    fireEvent.click(method("link"));
    expect(screen.getByText("Making a code…")).toBeTruthy();
  });
});

describe("Add a machine: without a code", () => {
  it("signed out: only Sign in there, and a way to sign in", async () => {
    const { adapter, calls } = host({ signedIn: false });
    await mount(adapter);
    expect(method("signin").disabled).toBe(false);
    for (const m of ["link", "qr", "command", "code"]) expect(method(m).disabled).toBe(true);
    expect(document.querySelector("[data-add-signed-out]")?.textContent).toBe("Sign in to make a code.");
    expect(document.querySelector("[data-add-prompt]")).toBeNull();
    expect(calls).toEqual([]);
  });

  it("not enrolled: This Mac can't make codes yet", async () => {
    const notice = { kind: "needs-enrollment", status: "Needs enrollment", text: "Enroll this Mac to open your machines.", actionLabel: "Enroll This Mac…" };
    const { adapter, calls } = host(
      {},
      {
        "machines.list": async () => {
          const rows = (await createDemoAdapter({ latencyMs: 0 }).call("machines.list", {})) as { current?: boolean }[];
          return rows.map((r) => (r.current ? { ...r, accessNotice: notice } : r));
        },
      },
    );
    await mount(adapter);
    await waitFor(() => expect(document.querySelector("[data-add-no-code-note]")?.textContent).toBe("This Mac can't make codes yet."));
    expect(method("code").disabled).toBe(true);
    expect(calls).toEqual([]);
  });

  it("a relay that can't make codes: says so, Sign in there still works", async () => {
    const { adapter } = host({}, { "devices.createPairingCode": () => Promise.reject(new HostError("relay: 404 Not Found", "failed")) });
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("error"));
    expect(document.querySelector("[data-add-code-error]")?.textContent).toMatch(/Your Cua relay can't make codes yet/);
    expect(method("signin").disabled).toBe(false);
    expect(method("link").disabled).toBe(true);
  });

  it("another failure: says it in words and tries again", async () => {
    let fail = true;
    const { adapter, calls } = host();
    const base = adapter.call.bind(adapter);
    const flaky = Object.assign(Object.create(adapter) as DataAdapter, {
      call: ((op: string, args: never) => (op === "devices.createPairingCode" && fail ? Promise.reject(new Error("not found: relay: no such device")) : base(op as never, args))) as DataAdapter["call"],
    });
    await mount(flaky);
    await waitFor(() => expect(phase()).toBe("error"));
    expect(document.querySelector("[data-add-code-error]")?.textContent).toMatch(/This machine isn't set up for access yet\./);
    expect(document.body.textContent).not.toMatch(/relay: no such device|not found:/);
    fail = false;
    fireEvent.click(document.querySelector("[data-add-retry]")!);
    await waitFor(() => expect(phase()).toBe("ready"));
    expect(calls).toContain("devices.createPairingCode");
  });
});

describe("Add a machine: for a team", () => {
  const acme = "0f1e2d3c-0000-4000-8000-0000000000a1";
  const teams = (currentTeamId: string | null) => () =>
    Promise.resolve({
      teams: [
        { id: acme, alias: "acme", name: "Acme", role: "owner", plan: "teams" },
        { id: "0f1e2d3c-0000-4000-8000-0000000000b2", alias: "side", name: "", role: "member", plan: "teams" },
        { id: "0f1e2d3c-0000-4000-8000-0000000000c3", alias: "free", name: "Free", role: "owner", plan: "free" },
      ],
      currentTeamId,
    });

  it("offers only teams on the Teams plan this account owns or runs, and adds the machine for it", async () => {
    const shared: Record<string, unknown>[] = [];
    const made: unknown[] = [];
    const { adapter } = host({}, (base) => ({
      "teams.list": teams(acme),
      "devices.createPairingCode": (args) => {
        made.push(args.team);
        return base.call("devices.createPairingCode", args as never);
      },
      "sharing.share": (args) => {
        shared.push(args);
        return Promise.resolve([]);
      },
    }));
    await mount(adapter);
    const picker = await waitFor(() => screen.getByLabelText("Add it for"));
    expect(picker.textContent).toContain("For Acme");
    await waitFor(() => expect(status()).toBe("Added: Office PC, for Acme"), { timeout: 3000 });
    expect(shared[0]).toMatchObject({ who: `team:${acme}`, role: "editor" });
    expect(made).toContain(acme);
  });

  it("shows no picker without such a team, and adds to no team", async () => {
    const shared: unknown[] = [];
    const { adapter } = host({}, {
      "teams.list": () => Promise.resolve({ teams: [{ id: "x", alias: "side", name: "", role: "member", plan: "teams" }], currentTeamId: null }),
      "sharing.share": (args) => {
        shared.push(args);
        return Promise.resolve([]);
      },
    });
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("used"), { timeout: 3000 });
    expect(screen.queryByLabelText("Add it for")).toBeNull();
    expect(shared).toHaveLength(0);
  });

  it("a team that never paid: says so in words, with Upgrade for the owner", async () => {
    const opened: unknown[] = [];
    const { adapter } = host({}, {
      "teams.list": teams(acme),
      "sharing.share": () => Promise.reject(new Error("Team machines need the Teams plan. https://app.cua.ai/buy/teams")),
      "session.openExternal": (args) => {
        opened.push(args.url);
        return Promise.resolve(undefined);
      },
    });
    await mount(adapter);
    await waitFor(() => expect(document.querySelector('[data-pairing-team="failed"]')).toBeTruthy(), { timeout: 3000 });
    const text = document.querySelector("[data-pairing-added]")?.textContent ?? "";
    expect(text).toContain("Added: Office PC. It isn't on Acme yet. Team machines need the Teams plan.");
    expect(text).not.toContain("https://");
    fireEvent.click(document.querySelector("[data-pairing-upgrade]")!);
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/teams"]));
  });

  it("finishing setup while the machine joins the team", async () => {
    let finish: () => void = () => {};
    const { adapter } = host({}, {
      "teams.list": teams(acme),
      "sharing.share": () => new Promise((r) => (finish = () => r([]))),
    });
    await mount(adapter);
    await waitFor(() => expect(document.querySelector('[data-pairing-team="adding"]')?.textContent).toContain("Finishing setup…"), { timeout: 3000 });
    finish();
    await waitFor(() => expect(status()).toBe("Added: Office PC, for Acme"));
  });
});

describe("Have a code? Enter it (this machine joins)", () => {
  it("opens from the footer, waits for a whole code, then adds this machine", async () => {
    const joined: Record<string, unknown>[] = [];
    const { adapter } = host({}, (base) => ({
      ...neverUsed(base),
      "host.joinWithCode": (args) => {
        joined.push(args);
        return base.call("host.joinWithCode", args as never);
      },
    }));
    await mount(adapter);
    fireEvent.click(document.querySelector("[data-add-enter]")!);
    expect(view()).toBe("enter");
    const submit = document.querySelector<HTMLButtonElement>("[data-join-submit]")!;
    expect(submit.textContent).toBe("Add this machine");
    fireEvent.change(screen.getByLabelText("Code"), { target: { value: "k7qm 4xpb" } });
    expect(submit.disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Code"), { target: { value: "https://cua.ai/add/k7qm-4xpb-2rhd?relay=https://relay.example.com" } });
    expect(submit.disabled).toBe(false);
    fireEvent.click(submit);
    await waitFor(() => expect(document.querySelector("[data-join-joined]")?.textContent).toBe("Added this machine"));
    expect(joined[0]).toMatchObject({ code: "K7QM4XPB2RHD", relay: "https://relay.example.com" });
  });

  it("a cua://add link opens it with the code filled in, and nothing joins until Add", async () => {
    const joined: unknown[] = [];
    const { adapter } = host({}, { "host.joinWithCode": (args) => (joined.push(args), Promise.resolve({})) });
    await mount(adapter, { initialView: "enter", initialCode: "K7QM4XPB2RHD" });
    expect(view()).toBe("enter");
    expect((screen.getByLabelText("Code") as HTMLInputElement).value).toBe("K7QM-4XPB-2RHD");
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(joined).toEqual([]);
  });

  it("a code that fails says why, in words", async () => {
    const { adapter } = host({}, { "host.joinWithCode": () => Promise.reject(new Error("pairing code expired")) });
    await mount(adapter, { initialView: "enter", initialCode: "K7QM4XPB2RHD" });
    fireEvent.click(document.querySelector("[data-join-submit]")!);
    await waitFor(() => expect(document.querySelector("[data-join-error]")).toBeTruthy());
  });
});

describe("agentPrompt and expiresInText", () => {
  it("words the prompt as one line with the add link", () => {
    expect(agentPrompt("K7QM-4XPB-2RHD")).toBe("Add this machine to my Cua Spaces: https://cua.ai/add/K7QM-4XPB-2RHD");
    expect(agentPrompt("K7QM-4XPB-2RHD", "https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fr.example")).toBe(
      "Add this machine to my Cua Spaces: https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fr.example",
    );
  });
  it("rounds minutes up, and counts seconds in the last minute", () => {
    expect(expiresInText(1800, 0)).toBe("Expires in 30 min");
    expect(expiresInText(100, 50_000)).toBe("Expires in 50 s");
  });
});
