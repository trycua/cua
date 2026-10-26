// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Add a computer" by link or QR code: makes a one-time pairing code
 * (`devices.createPairingCode`), watches it (`devices.pairingCodes`) until
 * the other computer uses it, and revokes it on request
 * (`devices.revokePairingCode`). The relay holds the code's state; this
 * only asks.
 *
 * With `team` (an organization id) the machine that used the code is then
 * made a team machine (`sharing.share` with `team:<id>`): pairing alone
 * never puts a machine in a team. Right after a Teams checkout the host
 * waits a few seconds for the plan ("Finishing setup…"); a team that never
 * paid fails at once, with words and a way to upgrade (`planFailure`).
 */

import { useEffect, useRef, useState } from "react";

import { HostError, isUnsupported } from "./adapter";
import type { PairingCode } from "./contracts/devices";
import { plainPairingError } from "./friendly-errors";
import { useBridge } from "./hooks";

/** How often an open "Add a computer" asks whether its code was used (ms). */
export const PAIRING_POLL_MS = 3000;

export type PairingPhase = "idle" | "minting" | "ready" | "used" | "revoked" | "expired" | "error";

export interface PairingState {
  phase: PairingPhase;
  /** The code as made (with its link and commands), then as last read. */
  code: PairingCode | null;
  /** Why no code could be made; `unsupported`: the host or relay can't make them. */
  error: { message: string; unsupported: boolean } | null;
  /**
   * With a `team`, after the code was used: `adding` (show "Finishing
   * setup…"), `added`, or `failed` with the reason. Null otherwise.
   */
  team?: { phase: "adding" | "added" | "failed"; error: string | null; upgradeUrl?: string | null; kind?: PlanFailureKind } | null;
}

/** Where a team owner buys Teams, and where anyone buys Pro. */
export const BUY_TEAMS_URL = "https://app.cua.ai/buy/teams";
export const BUY_PRO_URL = "https://app.cua.ai/buy/pro";

const BUY_URL = /\s*https:\/\/app\.cua\.ai\/buy\/(?:teams|pro)\b\S*/;

/**
 * A failed team step in words. The host already words plan errors: an owner
 * gets the checkout link last in the message (it becomes an Upgrade button),
 * anyone else is told to ask the owner. A raw plan error from an older host
 * (`HTTP 402 (feature_required: team_machines)`) is reworded here, without a
 * button because the role is not known.
 */
/** Which cap a team step hit, for usage data (`cua_plan_limit_shown`'s `kind`). */
export type PlanFailureKind = "machines" | "seats" | "other";

/** The cap in a plan error's text: team computers, seats, or another. */
export function planFailureKind(raw: string): PlanFailureKind {
  if (/machine_limit|team_machines|computers?\b/i.test(raw)) return "machines";
  if (/add_seats|\bseats?\b/i.test(raw)) return "seats";
  return "other";
}

export function planFailure(raw: string): { text: string; upgradeUrl: string | null } {
  const link = BUY_URL.exec(raw);
  if (link) return { text: raw.replace(BUY_URL, "").trim(), upgradeUrl: link[0].trim() };
  const feature = /feature_required(?::\s*(\w+))?/.exec(raw)?.[1];
  if (/feature_required/.test(raw)) {
    const what =
      feature === "team_machines"
        ? "Team computers need the Teams plan."
        : feature === "team_roles"
          ? "Admin roles need the Teams plan."
          : feature === "device_approval"
            ? "Approving devices needs the Teams plan."
            : "This needs the Teams plan.";
    return { text: `${what} Ask the team owner to upgrade.`, upgradeUrl: null };
  }
  if (/add_seats/.test(raw)) return { text: "Your team has no free seats. Ask the team owner to add seats.", upgradeUrl: null };
  if (/machine_limit/.test(raw)) return { text: "Your team has as many computers as it has seats. Ask the team owner to add seats.", upgradeUrl: null };
  return { text: raw, upgradeUrl: null };
}

/** What "Add a computer" shows while a paired machine joins its team. */
export const FINISHING_SETUP = "Finishing setup…";

export interface PairingHook extends PairingState {
  /** Unix ms, ticking each second while a code is active (for the countdown). */
  now: number;
  /** Makes a new code (after one was revoked, or a failure; an expired one is replaced by itself). */
  renew(): void;
  /** Revokes the active code. */
  revoke(): Promise<void>;
}

const IDLE: PairingState = { phase: "idle", code: null, error: null };

/** Whether `e` says this host or its relay can't make pairing codes (an older relay answers 404). */
export function pairingUnsupported(e: unknown): boolean {
  if (isUnsupported(e)) return true;
  const text = e instanceof Error ? e.message : String(e);
  return (e instanceof HostError && e.code === "not_found") || /\b404\b/.test(text);
}

/** "Expires in 29 min" (rounded up), "Expires in 40 s" in the last minute. */
export function expiresInText(expiresAtSecs: number, nowMs: number): string {
  const left = Math.max(0, expiresAtSecs * 1000 - nowMs);
  if (left < 60_000) return `Expires in ${Math.ceil(left / 1000)} s`;
  return `Expires in ${Math.ceil(left / 60_000)} min`;
}

/**
 * The pairing code of an open "Add a computer". `enabled`: signed in, and this
 * device may make one (enrolled). A code that is still active stays across
 * closing and opening again; a used, revoked or expired one is replaced.
 */
export function usePairingCode({
  open,
  enabled,
  team = null,
  pollMs = PAIRING_POLL_MS,
  onUsed,
}: {
  open: boolean;
  enabled: boolean;
  team?: string | null;
  pollMs?: number;
  /** The code was used: the machine list can be read again. */
  onUsed?: (code: PairingCode) => void;
}): PairingHook {
  const { data: adapter } = useBridge();
  const [state, setState] = useState<PairingState>(IDLE);
  const [now, setNow] = useState(() => Date.now());
  const seq = useRef(0);
  const usedRef = useRef(onUsed);
  useEffect(() => {
    usedRef.current = onUsed;
  });

  const mint = () => {
    if (!adapter) return;
    const mine = ++seq.current;
    shown.current = false;
    setState({ phase: "minting", code: null, error: null });
    adapter.call("devices.createPairingCode", { team }).then(
      (code) => {
        if (mine !== seq.current) return;
        setNow(Date.now());
        // A code already past its time (a clock that is off) is expired, not ready.
        const lapsed = code.state === "active" && code.expiresAt * 1000 <= Date.now();
        setState({ phase: lapsed ? "expired" : code.state === "active" ? "ready" : code.state, code, error: null });
      },
      (e: unknown) => {
        if (mine !== seq.current) return;
        setState({ phase: "error", code: null, error: { message: plainPairingError(e), unsupported: pairingUnsupported(e) } });
      },
    );
  };

  // Opening makes a code, unless the last one is still good.
  useEffect(() => {
    if (!open || !enabled) return;
    if (state.phase !== "minting" && state.phase !== "ready") mint();
    // Only on opening (and on becoming able to): not on each change of phase.
  }, [open, enabled, adapter]);

  // While the sheet is open an expired code is replaced at once, so the
  // link, command, code and prompt it shows always work. A code the person
  // revoked is not: they asked for it to stop.
  // Only a code that was shown working: one that arrives already expired (a
  // clock that is off) is not renewed again and again.
  const shown = useRef(false);
  if (state.phase === "ready") shown.current = true;
  const expired = open && enabled && state.phase === "expired";
  useEffect(() => {
    if (expired && shown.current) mint();
  }, [expired]);

  // Another team: a code for that team (the old one just expires).
  const firstTeam = useRef(team);
  useEffect(() => {
    if (firstTeam.current === team) return;
    firstTeam.current = team;
    if (open && enabled) mint();
  }, [team]);

  // The countdown, and expiry without waiting for the relay.
  const expiresAt = state.phase === "ready" ? state.code?.expiresAt : undefined;
  useEffect(() => {
    if (!open || expiresAt === undefined) return;
    const tick = () => {
      const t = Date.now();
      setNow(t);
      if (t >= expiresAt * 1000) setState((s) => (s.phase === "ready" ? { ...s, phase: "expired" } : s));
    };
    tick();
    const timer = setInterval(tick, 1000);
    return () => clearInterval(timer);
  }, [open, expiresAt]);

  // Waiting for the other computer: the relay says when the code was used.
  const watching = open && state.phase === "ready" ? state.code?.id : undefined;
  useEffect(() => {
    if (!adapter || !watching) return;
    let stopped = false;
    let busy = false;
    const poll = async () => {
      if (busy) return;
      busy = true;
      try {
        const listed = (await adapter.call("devices.pairingCodes", {})).find((c) => c.id === watching);
        if (stopped || !listed || listed.state === "active") return;
        const phase = listed.state;
        setState((s) => {
          if (s.phase !== "ready" || s.code?.id !== watching) return s;
          // Keep the link and commands the list leaves out.
          const code: PairingCode = { ...s.code, state: listed.state, usedAt: listed.usedAt, machine: listed.machine };
          return { ...s, phase, code };
        });
        if (listed.state === "used") {
          usedRef.current?.(listed);
          if (team && listed.machine) void addToTeam(listed.machine);
        }
      } catch {
        // A missed read: the next one tries again.
      } finally {
        busy = false;
      }
    };
    const timer = setInterval(() => void poll(), pollMs);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  }, [adapter, watching, pollMs, team]);

  // The machine that used the code becomes a machine of `team`.
  const addToTeam = async (machine: string) => {
    if (!adapter || !team) return;
    setState((s) => ({ ...s, team: { phase: "adding", error: null } }));
    try {
      await adapter.call("sharing.share", { spaceId: `relay:${machine}`, who: `team:${team}`, role: "editor" });
      setState((s) => ({ ...s, team: { phase: "added", error: null } }));
    } catch (e: unknown) {
      const raw = e instanceof Error ? e.message : String(e);
      const { text, upgradeUrl } = planFailure(raw);
      setState((s) => ({ ...s, team: { phase: "failed", error: text, upgradeUrl, kind: planFailureKind(raw) } }));
    }
  };

  const revoke = async () => {
    const code = state.code;
    if (!adapter || !code || state.phase !== "ready") return;
    await adapter.call("devices.revokePairingCode", { id: code.id });
    setState((s) => (s.code?.id === code.id ? { ...s, phase: "revoked", code: { ...code, state: "revoked" } } : s));
  };

  return { ...state, now, renew: mint, revoke };
}
