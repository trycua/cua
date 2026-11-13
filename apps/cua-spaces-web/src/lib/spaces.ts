// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { CoreClient, Machine, Space } from "@/bridge";
import { OS_NAME } from "@/components/os-icon";
import { t } from "@/i18n";
import { thisComputerLabel } from "@/lib/host-labels";
import { messageText } from "@/lib/messages";
import type { Msg } from "../bridge/contracts/teleport";

/** The states the Spaces screens draw. The bridge's `SpaceStatus` maps onto these. */
export type SpaceState = "running" | "stopped" | "creating" | "deleting" | "failed";

/** Each word is looked up when read, so a language switch shows at once. */
export const STATE_LABEL: Record<SpaceState, string> = {
  get failed() {
    return t("spaces.status.failed");
  },
  get running() {
    return t("spaces.status.running");
  },
  get stopped() {
    return t("spaces.status.stopped");
  },
  get creating() {
    return t("computers.status.creating");
  },
  get deleting() {
    return t("computers.status.deleting");
  },
};

export function spaceState(space: Pick<Space, "status" | "power"> & { progress?: Space["progress"] }): SpaceState {
  switch (space.status) {
    case "provisioning":
      return "creating";
    case "deleting":
      return "deleting";
    case "suspended":
      // A create that failed: its row stays, with why, until removed.
      return space.progress?.error ? "failed" : "stopped";
    default:
      return space.power?.off ? "stopped" : "running";
  }
}

/**
 * A machine's own desktop, not a Space: this Mac (`this-mac`, status
 * `local`) or a relay machine sharing its desktop (`relay:<machine>` with no
 * host above it). These belong on Machines; the Spaces grid, its count and
 * the command palette leave them out.
 */
export function isMachineDesktop(space: Pick<Space, "id" | "status" | "provider" | "host">): boolean {
  if (space.status === "local" || space.id === "this-mac") return true;
  if (space.provider !== "relay" || space.host || !space.id.startsWith("relay:")) return false;
  const own = space.id.slice("relay:".length);
  return own.length > 0 && !own.includes("/") && !own.startsWith("space-");
}

/** The Spaces a user created: every row but the machines' own desktops. */
export function realSpaces<T extends Pick<Space, "id" | "status" | "provider" | "host">>(spaces: readonly T[]): T[] {
  return spaces.filter((s) => !isMachineDesktop(s));
}

/** This Mac's own desktop (`this-mac`): a row of the Spaces list that reads "Not sharing" while its switch is off. */
export const isThisMacDesktop = (space: Pick<Space, "id">): boolean => space.id === "this-mac";

/**
 * This Mac's desktop while it is not shared: the host lists it `local` (the list rows read it as running)
 * only while the desktop is shared, and suspended otherwise.
 */
export const isUnsharedDesktop = (space: Pick<Space, "id" | "status">): boolean =>
  space.id === "this-mac" && space.status !== "local" && space.status !== "running";

/**
 * The rows of the Spaces list: your Spaces and the machines' desktops that are shared. This Mac's desktop
 * shows even while it is not shared ("Not sharing"), but only once something else is listed, so a person
 * with no Space still sees the empty state. `hidden` (Hide on a row) leaves a desktop out; another
 * machine's desktop shows only while it shares it (`shares`).
 */
export function listedSpaces<T extends Pick<Space, "id" | "status" | "provider" | "host">>(
  all: readonly T[],
  hidden: ReadonlySet<string> = new Set(),
  shares: (space: T) => boolean = () => true,
): T[] {
  const real = realSpaces(all);
  const shared = all.filter((s) => isMachineDesktop(s) && !isUnsharedDesktop(s) && !hidden.has(s.id) && (isThisMacDesktop(s) || shares(s)));
  const base = [...real, ...shared];
  const own = all.find((s) => isUnsharedDesktop(s) && !hidden.has(s.id));
  return own && base.length > 0 ? [...base, own] : base;
}

/** The listed rows that count as Spaces ("N Spaces", "N running", the sidebar): never this Mac's unshared desktop. */
export const countedSpaces = <T extends Pick<Space, "id" | "status">>(listed: readonly T[]): T[] => listed.filter((s) => !isUnsharedDesktop(s));

/** Running Spaces first, then the most recently used (Home, the Spaces page, the switcher and ⌘K). */
export function orderSpaces<T extends Pick<Space, "id" | "status" | "power" | "lastUsedAt" | "provider" | "host"> & { progress?: Space["progress"] }>(
  spaces: readonly T[],
): T[] {
  // A machine's desktop follows the Spaces in its state, and This Mac's unshared one is last.
  const rank = (s: T) => (isUnsharedDesktop(s) ? 9 : ORDER[spaceState(s)] + (isMachineDesktop(s) ? 0.5 : 0));
  // A desktop has no "last used": the host stamps it with the time it read the list (This Mac's
  // with the time it composed it, another machine's with the time the registry answered), and
  // those move apart on every poll. Sorted by it, desktops swapped tiles on each refresh; they keep
  // a fixed order instead, This Mac first, then by id. Every tie ends on the id, so the order never
  // depends on the order the host listed the rows in.
  const used = (s: T) => (isMachineDesktop(s) ? 0 : s.lastUsedAt);
  const mac = (s: T) => (isThisMacDesktop(s) ? 0 : 1);
  return spaces.toSorted((a, b) => rank(a) - rank(b) || used(b) - used(a) || mac(a) - mac(b) || (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
}

export type SpaceFilter = "all" | "running" | "stopped" | "creating";

/** Running first, then the Space being created, then the rest (each by most recent use). */
const ORDER: Record<SpaceState, number> = { running: 0, creating: 1, failed: 2, stopped: 3, deleting: 4 };

/** Filter by state, then: creating first, running next, then stopped; most recent first within each. */
export function visibleSpaces(spaces: readonly Space[], filter: SpaceFilter, keep: (space: Space) => boolean = () => true): Space[] {
  return spaces
    .filter((s) => (filter === "all" || spaceState(s) === filter) && keep(s))
    .toSorted((a, b) => ORDER[spaceState(a)] - ORDER[spaceState(b)] || b.lastUsedAt - a.lastUsedAt);
}

/** "macOS Tahoe 26.0", or the OS family when the Space did not report one;
 * empty when its OS is not known (its System fact says Unknown). */
export function osLabel(space: Pick<Space, "os" | "osPrettyName">): string {
  return space.osPrettyName?.trim() || (space.os === "unknown" ? "" : OS_NAME[space.os]);
}

/** "macOS on Studio", or only the machine when the OS is not known. */
export function spacePlace(space: Pick<Space, "os" | "osPrettyName">, machine: string): string {
  const os = osLabel(space);
  return os ? t("computers.status.os_on_machine", { os, machine }) : machine;
}

/** The machine a Space runs on, by the machines' `spaceIds`. */
export function machineName(machines: readonly Machine[], space: Pick<Space, "id" | "provider" | "hostName">): string {
  const machine = machines.find((m) => m.spaceIds.includes(space.id));
  if (machine) return machine.name;
  if (space.hostName) return space.hostName;
  if (space.provider === "direct" || space.provider === undefined) return t("computers.status.added_by_address");
  return t("computers.status.unknown_computer");
}

/** What a Space's ⌘K entries also match: its OS, state, detail and the machine it runs on. */
export function paletteKeywords(space: Pick<Space, "os" | "detail" | "hostName">, state: SpaceState, machine: string): string {
  return [...new Set([space.os, state, space.detail, space.hostName, machine].filter((w): w is string => Boolean(w)))].join(" ");
}

/** Why the Space's last power action failed (the core's `sidebar::power_error`), for its tile. */
export function powerError(space: Pick<Space, "status" | "power">): string | null {
  return canPower(space) ? (space.power?.error ?? null) || null : null;
}

/** This Mac's macOS Spaces that run or are being created (Apple's limit). */
export function busyMacosSpaces(spaces: readonly Pick<Space, "id" | "os" | "provider" | "status" | "power" | "progress">[]): number {
  return spaces.filter((s) => s.os === "macos" && s.provider === "local" && ["running", "creating"].includes(spaceState(s))).length;
}

/**
 * Why another macOS Space can't start on this Mac now, in the core's words
 * (`wizard.macosLimit`): two of its macOS Spaces already run (or are being
 * created), or the host says two macOS VMs run here (`vmsRunning`: Apple's
 * license counts every one, Spaces or not), and macOS refuses a third only
 * after the 22 GB download. Null when there is room, or without the core
 * (then there is no wizard and no one-click offer either).
 */
export function macosLimitText(
  spaces: readonly Pick<Space, "id" | "os" | "provider" | "status" | "power" | "progress">[],
  vmsRunning: number | null | undefined,
  core: CoreClient,
): string | null {
  if (core.status !== "ready") return null;
  return core.tryCall<string | null>("wizard.macosLimit", { spacesBusy: busyMacosSpaces(spaces), vmsRunning: vmsRunning ?? null }) ?? null;
}

/** Why a create failed, in the core's words; null for any other Space. */
export function createError(space: Pick<Space, "status" | "progress">): string | null {
  return space.status === "suspended" ? (space.progress?.error ?? null) : null;
}

/** 0..1 while a Space is being created. */
export function createProgress(space: Pick<Space, "progress">): number {
  return (space.progress?.permille ?? 0) / 1000;
}

/** It turns off and on here (a Space without `power`, like a cloud one, can't). */
export function canPower(space: Pick<Space, "status" | "power">): boolean {
  return Boolean(space.power) && space.status !== "local" && space.status !== "provisioning" && space.status !== "deleting";
}

/**
 * The power button's word, as the SwiftUI app's (the core's
 * `sidebar.powerButton`): "Turn off" / "Turn on", or "Suspend" / "Resume"
 * where the Space suspends. Without the core, the same words.
 */
export function powerLabel(core: { tryCall<T>(op: string, args?: Record<string, unknown>): T | undefined }, space: Space, on: boolean): string {
  const button = core.tryCall<{ help: Msg; turnOn: boolean; busy: boolean }>("sidebar.powerButton", { space });
  if (button && !button.busy && button.turnOn === on) return messageText(button.help);
  const suspends = space.power?.control === "suspend";
  return on ? (suspends ? t("spaces.power.resume") : t("spaces.power.turn_on")) : suspends ? t("spaces.power.suspend") : t("spaces.power.turn_off");
}

/** The core's power words (`sidebar::power_verb`): the press, then while it runs. */
const powerWords = (kind: "suspend" | "stop", on: boolean): [string, string] =>
  kind === "suspend"
    ? on
      ? [t("spaces.power.resume"), t("spaces.status.resuming")]
      : [t("spaces.power.suspend"), t("spaces.status.suspending")]
    : on
      ? [t("spaces.power.turn_on"), t("spaces.status.turning_on")]
      : [t("spaces.power.turn_off"), t("spaces.status.turning_off")];

/** A Space row's power button (the app core's `sidebar::PowerButton`). */
export interface PowerButton {
  /** "Turn off", "Resume", or "Suspending…" while it runs. */
  help: string;
  /** A press turns it on (else off). */
  turnOn: boolean;
  /** An action runs: a spinner instead, and no press. */
  busy: boolean;
}

/**
 * The power button a Space's row shows (the core's `sidebar.powerButton`,
 * as the SwiftUI sidebar asks it): none for this machine, a Space being
 * created or deleted, or one whose provider cannot turn it off. Without the
 * core, the same rule and words.
 */
export function powerButton(core: { tryCall<T>(op: string, args?: Record<string, unknown>): T | undefined }, space: Space): PowerButton | null {
  const fromCore = core.tryCall<(Omit<PowerButton, "help"> & { help: Msg }) | null>("sidebar.powerButton", { space });
  if (fromCore !== undefined) return fromCore ? { help: messageText(fromCore.help), turnOn: fromCore.turnOn, busy: fromCore.busy } : null;
  const power = space.power;
  if (!power || space.id === "this-mac" || space.progress || space.status === "deleting") return null;
  const busy = power.turningOn !== undefined && power.turningOn !== null;
  const turnOn = busy ? Boolean(power.turningOn) : power.off;
  const [idle, running] = powerWords(power.control === "suspend" ? "suspend" : "stop", turnOn);
  return { help: busy ? running : idle, turnOn, busy };
}

/** A power action is running now: "Starting…" or "Stopping…". */
export function powerPending(space: Pick<Space, "power">): string | null {
  const on = space.power?.turningOn;
  if (on === true) return t("computers.status.pending_start");
  if (on === false) return space.power?.control === "suspend" ? t("spaces.status.suspending") : t("computers.status.pending_stop");
  return null;
}

/** This app can delete it: not this Mac itself, and not a cloud Space only deletable elsewhere. */
export function canDelete(space: Pick<Space, "id" | "status" | "cloudDelete">): boolean {
  // A failed create is removed (`dismissCreate`), not deleted.
  return (
    !space.id.startsWith("pending:") && space.status !== "local" && space.id !== "this-mac" && space.status !== "deleting" && space.cloudDelete !== "elsewhere"
  );
}

const PROVIDER = (provider: "relay" | "direct"): string =>
  provider === "relay" ? t("computers.status.provider_relay") : t("computers.status.provider_direct");

/** Where it runs, in words. */
export function locationLabel(space: Pick<Space, "provider" | "cloudPlace">): string {
  if (space.cloudPlace) return space.cloudPlace;
  const provider = space.provider ?? "direct";
  return provider === "local" ? thisComputerLabel() : PROVIDER(provider);
}

export const KIND_LABEL: Record<NonNullable<Space["kind"]>, string> = {
  get vm() {
    return t("computers.status.kind_vm");
  },
  get container() {
    return t("computers.status.kind_container");
  },
  get microvm() {
    return t("computers.status.kind_microvm");
  },
};
