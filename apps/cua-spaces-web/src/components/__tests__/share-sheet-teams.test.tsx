// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider, defaultTeam, eligibleTeams, useShareSheet } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import type { CoreClient } from "@/bridge/core";
import type { ShareInput, ShareSheetAction, ShareSheetState } from "@/bridge/contracts/share";
import ShareSheet from "@/components/share-sheet";

afterEach(cleanup);

const ACME = "0f1e2d3c-0000-4000-8000-0000000000a1";
const TEAMS = {
  teams: [
    { id: "0f1e2d3c-0000-4000-8000-0000000000c3", alias: "beta", name: "Beta", role: "admin", plan: "teams" },
    { id: ACME, alias: "acme", name: "Acme", role: "owner", plan: "teams" },
    // A member cannot add machines; a free team has no team machines.
    { id: "0f1e2d3c-0000-4000-8000-0000000000b2", alias: "side", name: "", role: "member", plan: "teams" },
    { id: "0f1e2d3c-0000-4000-8000-0000000000d4", alias: "free", name: "Free", role: "owner", plan: "free" },
  ],
  currentTeamId: ACME,
};

/** A small stand-in for the core's share state machine (`share.*`). */
function shareCore(): CoreClient {
  const reduce = (input: ShareInput, s: ShareSheetState, a: ShareSheetAction): ShareSheetState => {
    switch (a.type) {
      case "set-who":
        return { ...s, who: a.who };
      case "submit":
        return { ...s, busy: true, request: { kind: "share", space: input.spaceId, who: s.who, role: s.role } };
      case "done":
        return { ...s, busy: false, who: "", request: null };
      case "failed":
        return { ...s, busy: false, error: a.error, request: null };
      default:
        return s;
    }
  };
  const call = <T,>(method: string, args: Record<string, unknown> = {}): T => {
    const input = args.input as ShareInput;
    const state = args.state as ShareSheetState;
    switch (method) {
      case "share.initial":
        return { who: "", role: "viewer", busy: false, error: null, request: null } as T;
      case "share.reduce":
        return reduce(input, state, args.action as ShareSheetAction) as T;
      case "share.view":
        return {
          title: `Share ${input.spaceName}`,
          rows: input.shares.map((r) => ({ who: r.who, role: r.role, connected: false })),
          emptyText: "Not shared",
          who: state.who,
          whoPlaceholder: "Email",
          hint: null,
          role: state.role,
          roles: [{ id: "viewer", label: "Can view" }, { id: "editor", label: "Can use" }],
          shareLabel: "Share",
          canShare: state.who !== "",
          removeLabel: "Remove",
          doneLabel: "Done",
          disabledReason: null,
          busy: state.busy,
          error: state.error,
          request: state.request,
        } as T;
      default:
        throw new Error(`no ${method}`);
    }
  };
  return { status: "ready", methods: ["share.initial", "share.reduce", "share.view"], call, tryCall: () => undefined };
}

function host() {
  const base = createDemoAdapter({ latencyMs: 0, stepMs: 0, signedIn: true, onboarded: true });
  const shared: Record<string, unknown>[] = [];
  const adapter = Object.assign(Object.create(base) as DataAdapter, {
    call: ((op: string, args: Record<string, unknown>) => {
      if (op === "teams.list") return Promise.resolve(TEAMS);
      if (op === "sharing.list") return Promise.resolve([]);
      if (op === "sharing.share") {
        shared.push(args);
        return Promise.resolve([{ who: args.who, role: "editor" }]);
      }
      return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
    }) as DataAdapter["call"],
  });
  return { adapter, shared };
}

function Opener() {
  const { open } = useShareSheet();
  return (
    <button type="button" onClick={() => void open({ id: "relay:abc", name: "Build box", sdk: undefined }, true)}>
      open
    </button>
  );
}

describe("Teams in Share and Add a machine", () => {
  it("offers only teams that can take machines, the current one by default", () => {
    expect(eligibleTeams(TEAMS).map((t) => t.alias)).toEqual(["beta", "acme"]);
    expect(defaultTeam(TEAMS)).toBe(ACME);
    expect(defaultTeam({ ...TEAMS, currentTeamId: "0f1e2d3c-0000-4000-8000-0000000000b2" })).toBeNull();
  });

  it("shares a Space with a team without typing team:<id>, and names the team's row", async () => {
    const { adapter, shared } = host();
    render(
      <BridgeProvider adapter={adapter} core={shareCore()}>
        <Opener />
        <ShareSheet />
      </BridgeProvider>,
    );
    await act(() => new Promise((r) => setTimeout(r, 10)));
    fireEvent.click(screen.getByText("open"));
    await waitFor(() => expect(document.querySelector("[data-share-teams]")).toBeTruthy());
    const offered = [...document.querySelectorAll("[data-share-team]")].map((b) => b.textContent);
    // The current team first; no member-only or free team.
    expect(offered).toEqual(["Share with Acme", "Share with Beta"]);
    fireEvent.click(screen.getByText("Share with Acme"));
    await waitFor(() => expect(shared).toHaveLength(1));
    expect(shared[0]).toMatchObject({ spaceId: "relay:abc", who: `team:${ACME}` });
    await waitFor(() => expect(document.querySelector(`[data-share-row="team:${ACME}"]`)?.textContent).toContain("Acme (team)"));
    // Shared with Acme: only Beta is offered now.
    expect([...document.querySelectorAll("[data-share-team]")].map((b) => b.textContent)).toEqual(["Share with Beta"]);
  });
});
