// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
// Before the redesign: public's test for this page, run with `spaces_redesign` off
// (the drift guard that keeps the default UI working).

import { vi as __vi } from "vitest";
__vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider, expiresInText, setFlagOverride, useMachines } from "@/bridge";
import { HostError, type DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter, type DemoOptions } from "@/bridge/adapters/demo";
import { unavailableCore } from "@/bridge/core";
import { AddMachineDialog } from "@/legacy/components/add-machine-dialog";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

/** The demo host, with some operations answered differently, recording the pairing calls. */
function host(options: DemoOptions = {}, override: Record<string, (args: Record<string, unknown>) => Promise<unknown>> = {}) {
  const base = createDemoAdapter({ latencyMs: 0, stepMs: 0, signedIn: true, onboarded: true, ...options });
  const calls: string[] = [];
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: Record<string, unknown>) => {
      if (op.startsWith("devices.") && op.includes("airing")) calls.push(op);
      const f = override[op];
      if (f) return f(args);
      return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
    }) as DataAdapter["call"],
  });
  return { adapter, calls };
}

function Harness({ team = null }: { team?: string | null }) {
  const { data: machines = [], setupGuide } = useMachines();
  return (
    <>
      <ul aria-label="Computers">
        {machines.map((m) => (
          <li key={m.id}>{m.name}</li>
        ))}
      </ul>
      <AddMachineDialog open onOpenChange={() => {}} guide={setupGuide} pollMs={20} team={team} />
    </>
  );
}

async function mount(adapter: DataAdapter, team: string | null = null, withCards = false) {
  const none = unavailableCore("test: no core");
  // The core's `upsell.card`, as one fixed card per reason.
  const core = withCards
    ? {
        ...none,
        tryCall: (<T,>(method: string, args: Record<string, unknown> = {}) =>
          method === "upsell.card"
            ? ({ reason: args.reason, title: "Your team is out of seats", body: "Add a seat.", price: "$30 a seat a month", cta: { label: "Add seats", url: "https://app.cua.ai/billing" }, dismiss: "Not now" } as T)
            : undefined) as typeof none.tryCall,
      }
    : none;
  render(
    <BridgeProvider adapter={adapter} core={core}>
      <Harness team={team} />
    </BridgeProvider>,
  );
  await act(() => new Promise((r) => setTimeout(r, 10)));
}

const dialog = () => document.querySelector<HTMLElement>("[data-add-machine]")!;
const phase = () => dialog()?.getAttribute("data-add-machine");

describe("Add a computer by link or QR code", () => {
  it("makes a link on opening: QR code, link, both commands and how long it lasts", async () => {
    const { adapter, calls } = host();
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    expect(calls[0]).toBe("devices.createPairingCode");
    expect(document.querySelector("[data-pairing-qr] path")?.getAttribute("d")).toMatch(/^M\d/);
    expect(screen.getByText(/^https:\/\/cua\.ai\/add\/K7QM-4XPB-/)).toBeTruthy();
    expect(screen.getByText(/sh -s -- --pair K7QM-4XPB-/)).toBeTruthy();
    // The bare code too, for Join with a code on a computer that has the app.
    expect(document.querySelector("[data-pairing-code] code")?.textContent).toMatch(/^K7QM-4XPB-[0-9A-Z]{4}$/);
    expect(screen.getByLabelText("Copy code")).toBeTruthy();
    expect(screen.getByText("Waiting for the computer…")).toBeTruthy();
    expect(document.querySelector("[data-pairing-expires]")?.textContent).toBe("Works once. Expires in 30 min");

    fireEvent.click(screen.getByText("Windows (preview)"));
    await waitFor(() => expect(screen.getByText(/install\.ps1\)\)\) -Pair K7QM-4XPB-/)).toBeTruthy());
    // The by-hand steps stay one click away.
    expect(document.querySelector("details[data-manual-steps] summary")?.textContent?.trim()).toBe("Set it up by hand instead");
  });

  it("waits for the other computer and says which one was added", async () => {
    const { adapter, calls } = host();
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("used"), { timeout: 3000 });
    expect(calls.filter((c) => c === "devices.pairingCodes").length).toBeGreaterThanOrEqual(3);
    expect(document.querySelector("[data-pairing-added]")?.textContent).toMatch(/^Added Office PC\./);
    // The machine list reads it again: the new computer is there.
    // (The page behind the dialog is inert, so not by role.)
    await waitFor(() => expect(document.querySelector('ul[aria-label="Computers"]')?.textContent).toContain("Office PC"));
    expect(screen.queryByText("Revoke link")).toBeNull();
  });

  it("adds the new machine to the team once it joined (pairing alone does not)", async () => {
    const org = "0f1e2d3c-0000-4000-8000-000000000001";
    const shared: Record<string, unknown>[] = [];
    let finish: () => void = () => {};
    const { adapter } = host({}, {
      "sharing.share": (args) => {
        shared.push(args);
        // A plan bought a moment ago: the host is still retrying.
        return new Promise((r) => (finish = () => r([])));
      },
    });
    await mount(adapter, org);
    await waitFor(() => expect(document.querySelector('[data-pairing-team="adding"]')?.textContent).toContain("Finishing setup…"), { timeout: 3000 });
    expect(shared).toHaveLength(1);
    expect(shared[0]).toMatchObject({ who: `team:${org}`, role: "editor" });
    expect(String(shared[0]?.spaceId)).toMatch(/^relay:/);
    finish();
    await waitFor(() => expect(document.querySelector('[data-pairing-team="added"]')?.textContent).toMatch(/Your team can use it now\./));
  });

  it("a team that never paid fails at once in words, with an Upgrade button for the owner", async () => {
    const org = "0f1e2d3c-0000-4000-8000-000000000001";
    const opened: unknown[] = [];
    const { adapter } = host({}, {
      "sharing.share": () => Promise.reject(new Error("Team computers need the Teams plan. https://app.cua.ai/buy/teams")),
      "session.openExternal": (args) => {
        opened.push(args.url);
        return Promise.resolve(undefined);
      },
    });
    await mount(adapter, org);
    await waitFor(() => expect(document.querySelector('[data-pairing-team="failed"]')).toBeTruthy(), { timeout: 3000 });
    const text = document.querySelector("[data-pairing-added]")?.textContent ?? "";
    expect(text).toContain("It isn't on your team yet. Team computers need the Teams plan.");
    expect(text).not.toContain("HTTP 402");
    expect(text).not.toContain("https://");
    fireEvent.click(document.querySelector("[data-pairing-upgrade]")!);
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/teams"]));
  });

  it("a member is told to ask the owner, and a raw plan error from an older host is reworded", async () => {
    const { adapter } = host({}, {
      "sharing.share": () => Promise.reject(new Error("Spaces API: HTTP 402 (feature_required: team_machines)")),
    });
    await mount(adapter, "0f1e2d3c-0000-4000-8000-000000000001");
    await waitFor(() => expect(document.querySelector('[data-pairing-team="failed"]')).toBeTruthy(), { timeout: 3000 });
    const text = document.querySelector("[data-pairing-added]")?.textContent ?? "";
    expect(text).toContain("Ask the team owner to upgrade.");
    expect(text).not.toContain("feature_required");
    expect(document.querySelector("[data-pairing-upgrade]")).toBeNull();
  });

  it("lets you choose the team, the current one by default, and adds the machine to it", async () => {
    const acme = "0f1e2d3c-0000-4000-8000-0000000000a1";
    const shared: Record<string, unknown>[] = [];
    const { adapter } = host({}, {
      "teams.list": () =>
        Promise.resolve({
          teams: [
            { id: acme, alias: "acme", name: "Acme", role: "owner", plan: "teams" },
            { id: "0f1e2d3c-0000-4000-8000-0000000000b2", alias: "side", name: "", role: "member", plan: "teams" },
          ],
          currentTeamId: acme,
        }),
      "sharing.share": (args) => {
        shared.push(args);
        return Promise.resolve([]);
      },
    });
    await mount(adapter);
    // Offered: just me, or Acme (the member-only team is not); Acme is the current team.
    await waitFor(() => expect(document.querySelector("[data-add-machine-team]")?.getAttribute("data-add-machine-team")).toBe(acme));
    await waitFor(() => expect(document.querySelector('[data-pairing-team="added"]')).toBeTruthy(), { timeout: 3000 });
    expect(shared[0]).toMatchObject({ who: `team:${acme}` });
  });

  it("adds the machine to no team when there is none to choose", async () => {
    const shared: unknown[] = [];
    const { adapter } = host({}, {
      "teams.list": () => Promise.resolve({ teams: [], currentTeamId: null }),
      "sharing.share": (args) => {
        shared.push(args);
        return Promise.resolve([]);
      },
    });
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("used"), { timeout: 3000 });
    expect(document.querySelector("[data-add-machine-team]")).toBeNull();
    expect(shared).toHaveLength(0);
  });

  it("says when the machine could not join the team", async () => {
    const { adapter } = host({}, {
      "sharing.share": () => Promise.reject(new Error("Teams are not available on this server yet. Try again later, or update Cua.")),
    });
    await mount(adapter, "acme");
    await waitFor(() => expect(document.querySelector('[data-pairing-team="failed"]')?.textContent).toMatch(/isn't on your team yet. Teams are not available/), { timeout: 3000 });
  });

  it("revokes the link, then makes a new one", async () => {
    const { adapter, calls } = host();
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("ready"));
    fireEvent.click(screen.getByText("Revoke link"));
    await waitFor(() => expect(phase()).toBe("revoked"));
    expect(calls).toContain("devices.revokePairingCode");
    expect(screen.getByText(/This link was revoked/)).toBeTruthy();
    fireEvent.click(screen.getByText("Make a new link"));
    await waitFor(() => expect(phase()).toBe("ready"));
    expect(calls.filter((c) => c === "devices.createPairingCode")).toHaveLength(2);
  });

  it("says when the link expired and offers a new one", async () => {
    // A code the relay made 31 minutes ago (30 minutes is its life).
    const { adapter } = host({ now: () => Date.now() - 31 * 60_000 });
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("expired"));
    expect(screen.getByText("This link expired.")).toBeTruthy();
    expect(screen.getByText("Make a new link")).toBeTruthy();
  });

  it("shows that it is making the link", async () => {
    const { adapter } = host({}, { "devices.createPairingCode": () => new Promise(() => {}) });
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("minting"));
    expect(screen.getByText("Making a link…")).toBeTruthy();
  });

  it("falls back to the steps by hand when the relay can't make links", async () => {
    const { adapter } = host({}, { "devices.createPairingCode": () => Promise.reject(new HostError("relay: 404 Not Found", "failed")) });
    await mount(adapter);
    await waitFor(() => expect(phase()).toBe("error"));
    expect(screen.getByText(/Your Cua relay can't make links yet/)).toBeTruthy();
    expect(document.querySelector("[data-manual-steps]")?.tagName).toBe("DIV");
    expect(screen.getByText("Install Cua Spaces on the other computer")).toBeTruthy();
  });

  it("says what failed otherwise, and tries again", async () => {
    let fail = true;
    const { adapter, calls } = host({}, {});
    const base = adapter.call.bind(adapter);
    const flaky = Object.assign(Object.create(adapter) as DataAdapter, {
      call: ((op: string, args: never) =>
        op === "devices.createPairingCode" && fail ? Promise.reject(new Error("not found: relay: no such device")) : base(op as never, args)) as DataAdapter["call"],
    });
    await mount(flaky);
    await waitFor(() => expect(phase()).toBe("error"));
    expect(screen.getByText(/This computer isn't set up for access yet\. Set it up first, then add computers\./)).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/relay: no such device|not found:/);
    fail = false;
    fireEvent.click(screen.getByText("Try again"));
    await waitFor(() => expect(phase()).toBe("ready"));
    expect(calls).toContain("devices.createPairingCode");
  });

  it("asks to sign in when signed out, and makes no link", async () => {
    const { adapter, calls } = host({ signedIn: false });
    await mount(adapter);
    expect(screen.getByText(/Sign in to add a computer with a link/)).toBeTruthy();
    expect(screen.getByText("Sign in")).toBeTruthy();
    expect(document.querySelector("[data-manual-steps]")?.tagName).toBe("DIV");
    expect(calls).toEqual([]);
  });

  it("makes no link on a device that is not enrolled", async () => {
    const notice = { kind: "needs-enrollment", status: "Needs enrollment", text: "Enroll this Mac to open your computers.", actionLabel: "Enroll This Mac…" };
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
    await waitFor(() => expect(screen.getByText(/Enroll this Mac to open your computers\. Set the computer up by hand instead\./)).toBeTruthy());
    expect(calls).toEqual([]);
  });
});

describe("expiresInText", () => {
  it("rounds minutes up, and counts seconds in the last minute", () => {
    expect(expiresInText(1800, 0)).toBe("Expires in 30 min");
    expect(expiresInText(1800, 1_000)).toBe("Expires in 30 min");
    expect(expiresInText(100, 50_000)).toBe("Expires in 50 s");
    expect(expiresInText(100, 200_000)).toBe("Expires in 0 s");
  });

  it("with the upsell cards on, an owner out of seats gets the card, and a member does not", async () => {
    setFlagOverride("upsell_cards", true);
    const org = "0f1e2d3c-0000-4000-8000-000000000001";
    const { adapter } = host({}, { "sharing.share": () => Promise.reject(new Error("Your team has no free seats. https://app.cua.ai/buy/teams")) });
    await mount(adapter, org, true);
    await waitFor(() => expect(document.querySelector('[data-upgrade-card="seats"]')).toBeTruthy(), { timeout: 3000 });
    expect(document.querySelector("[data-pairing-upgrade]")).toBeNull();
    cleanup();
    const member = host({}, { "sharing.share": () => Promise.reject(new Error("Your team has no free seats. Ask the team owner to add seats.")) });
    await mount(member.adapter, org, true);
    await waitFor(() => expect(document.querySelector('[data-pairing-team="failed"]')).toBeTruthy(), { timeout: 3000 });
    expect(document.querySelector("[data-upgrade-card]")).toBeNull();
  });
});
