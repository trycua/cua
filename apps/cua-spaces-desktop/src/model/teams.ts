// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Teams: the account's organizations from the Spaces API
// (api.cua.ai; api.staging.cua.ai under CUA_ENV=staging) through the app
// export's `appTeams`, with the account's token: each team with its members
// and pending invites. The app administers nothing; invites, roles and
// billing are the account app's (app.cua.ai, `appAccountAppUrl`). The words
// are the core's (`teams::rows`, in the Settings page).
import type { AppTeamsInput, AppTeamsState } from "../native/generated/index";
import { words } from "./errors";
import type { AccountRunning } from "./services";

/** What the model hands over: the native export, the account and who is signed in. */
export interface TeamsDeps {
  native: {
    appTeams(token: string): Promise<AppTeamsInput>;
    appAccountAppUrl(): string;
    AppTeamsState: {
      SignedOut: new () => AppTeamsState;
      Loading: new () => AppTeamsState;
      Failed: new (v: { message: string }) => AppTeamsState;
    };
  };
  account: () => AccountRunning | null;
  identity: () => string | null;
  /** The saved current team (`AppSettings.currentTeamId`). */
  currentTeamId?: () => string | undefined;
}

/** A read this recent is not repeated by opening Settings again. */
export const TEAMS_FRESH_MS = 30_000;

export class TeamsModel {
  /** Where Teams stand (`undefined`: nothing read for this account yet). */
  private state: AppTeamsState | undefined;
  private readAt = 0;
  private generation = 0;
  onChange: () => void = () => {};

  constructor(private readonly deps: TeamsDeps, private readonly now: () => number = Date.now) {}

  /** Settings' Teams input: the read, or "loading" / "signed out" before it. */
  input(): AppTeamsInput {
    const { native } = this.deps;
    const signedOut = this.deps.identity() === null;
    const state = signedOut ? new native.AppTeamsState.SignedOut() : (this.state ?? new native.AppTeamsState.Loading());
    return { accountAppUrl: native.appAccountAppUrl(), state, currentTeamId: this.deps.currentTeamId?.() };
  }

  /** The account changed (signed in, out, or another one): forget the last read. */
  reset(): void {
    this.generation += 1;
    this.state = undefined;
    this.readAt = 0;
  }

  /** Reads the teams (at most every 30 s unless `force`). A failure says so in the row. */
  async reload(force = false): Promise<void> {
    const { native } = this.deps;
    const account = this.deps.account();
    if (!account || this.deps.identity() === null) {
      this.reset();
      return;
    }
    if (!force && this.state && this.now() - this.readAt < TEAMS_FRESH_MS) return;
    const generation = this.generation;
    try {
      const token = await account.accessToken(false);
      if (!token) throw new Error("Sign in to see your teams");
      const read = await native.appTeams(token);
      if (generation !== this.generation) return;
      this.state = read.state;
    } catch (error) {
      if (generation !== this.generation) return;
      this.state = new native.AppTeamsState.Failed({ message: words(error) });
    }
    this.readAt = this.now();
    this.onChange();
  }
}
