// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The account's Teams for "Add a machine" and "Share" (`teams.list`: the
 * organizations of `GET /v1/me` and the current team of Settings, Teams).
 * Only teams on the Teams plan (team machines) where the account is an
 * owner or admin can take a machine, so only those are offered.
 */

import { useEffect, useState } from "react";

import type { TeamChoice, TeamsList } from "./contracts/devices";
import { useBridge } from "./hooks";

/** The teams a machine or Space can be added to: Teams plan, owner or admin. */
export function eligibleTeams(list: TeamsList | null | undefined): TeamChoice[] {
  return (list?.teams ?? []).filter((t) => t.plan === "teams" && (t.role === "owner" || t.role === "admin"));
}

/** The current team when it can take machines, else none. */
export function defaultTeam(list: TeamsList | null | undefined): string | null {
  const id = list?.currentTeamId ?? null;
  return id && eligibleTeams(list).some((t) => t.id === id) ? id : null;
}

/** A team's name: its display name, else its alias. */
export function teamName(t: Pick<TeamChoice, "name" | "alias" | "id">): string {
  return t.name || t.alias || t.id;
}

export interface TeamChoices {
  /** The teams offered (current team first). */
  teams: TeamChoice[];
  /** The current team, when it is offered. */
  current: string | null;
  /** Every team of the account (to name `team:<id>` rows). */
  all: TeamChoice[];
}

const NONE: TeamChoices = { teams: [], current: null, all: [] };

/** The teams to offer, read once while `enabled` (none on a host without Teams). */
export function useTeamChoices(enabled: boolean): TeamChoices {
  const { data: adapter } = useBridge();
  const [choices, setChoices] = useState<TeamChoices>(NONE);
  useEffect(() => {
    if (!adapter || !enabled) return;
    let stopped = false;
    adapter.call("teams.list", {}).then(
      (list) => {
        if (stopped) return;
        const current = defaultTeam(list);
        const teams = eligibleTeams(list).sort((a, b) => Number(b.id === current) - Number(a.id === current));
        setChoices({ teams, current, all: list.teams });
      },
      () => {
        if (!stopped) setChoices(NONE);
      },
    );
    return () => {
      stopped = true;
    };
  }, [adapter, enabled]);
  return choices;
}
