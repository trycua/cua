// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Where the old Machines and Cua Driver links land now (CUA-3551). Machines
 * and Spaces are one list of your computers: a `/machines` link (a
 * bookmark, a notification, a `cua://add` link from an older Cua Spaces)
 * opens it, with "Add a computer" open when the link asked for it, and a
 * named computer opens on its own page. `/driver` opens Cua Driver under the
 * computer it belongs to.
 */

import { THIS_COMPUTER } from "./machines";

/** What `/spaces` takes from a link: `add=1` opens "Add a computer"; `join` opens it on "Enter a code" with the code filled in; `relay` names a relay other than the production one. */
export interface ComputersSearch {
  add?: true;
  join?: string;
  relay?: string;
}

export function validateComputersSearch(s: Record<string, unknown>): ComputersSearch {
  return {
    ...(s.add === 1 || s.add === "1" || s.add === true ? { add: true as const } : {}),
    ...(typeof s.join === "string" && s.join ? { join: s.join.slice(0, 200) } : {}),
    ...(typeof s.relay === "string" && s.relay ? { relay: s.relay.slice(0, 300) } : {}),
  };
}

export type OldLinkTarget =
  | { to: "/spaces"; search: ComputersSearch }
  | { to: "/spaces/computer/$computerId"; params: { computerId: string } }
  | { to: "/spaces/computer/$computerId/driver"; params: { computerId: string } };

const named = (s: Record<string, unknown>): string | null => {
  for (const k of ["machine", "computer", "id"]) {
    const v = s[k];
    if (typeof v === "string" && v) return v.slice(0, 200);
  }
  return null;
};

/** `/machines?…`: the list (Add a computer open for `add` or `join`), or one computer's page for `machine=<id>`. */
export function machinesLinkTarget(s: Record<string, unknown>): OldLinkTarget {
  const search = validateComputersSearch(s);
  const id = named(s);
  if (id && !search.add && !search.join) return { to: "/spaces/computer/$computerId", params: { computerId: id } };
  return { to: "/spaces", search };
}

/** `/driver?machine=<id>`: Cua Driver under that computer; This Mac's without one. */
export function driverLinkTarget(s: Record<string, unknown>): OldLinkTarget {
  return { to: "/spaces/computer/$computerId/driver", params: { computerId: named(s) ?? THIS_COMPUTER } };
}
