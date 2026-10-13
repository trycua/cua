// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Teams: the Spaces API read through the app export's `appTeams`
// with the account's token, kept per account, and a failure said in the row
// rather than thrown.
import { describe, expect, it, vi } from "vitest";
import type { AppTeamsInput } from "../src/native/generated/index";
import { TEAMS_FRESH_MS, TeamsModel, type TeamsDeps } from "../src/model/teams";

/** The enum's variants as plain tagged objects (the real ones come from the addon). */
const tagged = (tag: string) =>
  class {
    readonly tag = tag;
    constructor(readonly inner?: unknown) {}
  };

const READ = {
  accountAppUrl: "https://app.cua.ai",
  state: { tag: "Ready", inner: { teams: [{ id: "o1", alias: "acme", name: "", role: "owner", plan: "teams", seats: 3, members: [], invites: ["cy@example.com"] }] } },
} as unknown as AppTeamsInput;

function context(o: { signedIn?: boolean; fail?: string } = {}) {
  let clock = 1_000;
  const accessToken = vi.fn(async () => "acct-token");
  const appTeams = vi.fn(async (_token: string) => {
    if (o.fail) throw new Error(o.fail);
    return READ;
  });
  const deps = {
    native: {
      appTeams,
      appAccountAppUrl: () => "https://app.cua.ai",
      AppTeamsState: { SignedOut: tagged("SignedOut"), Loading: tagged("Loading"), Failed: tagged("Failed") },
    },
    account: () => ({ accessToken }),
    identity: () => (o.signedIn === false ? null : "ada@example.com"),
  } as unknown as TeamsDeps;
  const model = new TeamsModel(deps, () => clock);
  const changed = vi.fn();
  model.onChange = changed;
  return { model, appTeams, accessToken, changed, advance: (ms: number) => (clock += ms) };
}

const tag = (m: TeamsModel) => (m.input().state as unknown as { tag: string }).tag;

describe("Settings, Teams", () => {
  it("reads the account's teams with its token and hands them to the core's page", async () => {
    const { model, appTeams, changed } = context();
    expect(tag(model)).toBe("Loading");
    await model.reload();
    expect(appTeams).toHaveBeenCalledWith("acct-token");
    expect(model.input()).toMatchObject({ accountAppUrl: "https://app.cua.ai", state: { tag: "Ready" } });
    expect(changed).toHaveBeenCalledTimes(1);
  });

  it("does not read again within 30 s, unless forced", async () => {
    const { model, appTeams, advance } = context();
    await model.reload();
    await model.reload();
    expect(appTeams).toHaveBeenCalledTimes(1);
    advance(TEAMS_FRESH_MS + 1);
    await model.reload();
    expect(appTeams).toHaveBeenCalledTimes(2);
    await model.reload(true);
    expect(appTeams).toHaveBeenCalledTimes(3);
  });

  it("reads nothing while signed out", async () => {
    const { model, appTeams } = context({ signedIn: false });
    await model.reload();
    expect(appTeams).not.toHaveBeenCalled();
    expect(tag(model)).toBe("SignedOut");
  });

  it("says a failed read in the row and keeps the app usable", async () => {
    const { model } = context({ fail: "Spaces API: HTTP 503" });
    await model.reload();
    expect(model.input().state).toMatchObject({ tag: "Failed", inner: { message: expect.stringContaining("503") } });
  });

  it("forgets the last account's teams when the account changes", async () => {
    const { model } = context();
    await model.reload();
    model.reset();
    expect(tag(model)).toBe("Loading");
  });
});
