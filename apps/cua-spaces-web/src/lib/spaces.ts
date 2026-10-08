// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { CoreClient, Machine, Space } from "@/bridge";
import { OS_NAME } from "@/components/os-icon";
import { thisComputerLabel } from "@/lib/host-labels";

/** The states the Spaces screens draw. The bridge's `SpaceStatus` maps onto these. */
export type SpaceState = "running" | "stopped" | "creating" | "deleting" | "failed";

export const STATE_LABEL: Record<SpaceState, string> = {
  failed: "Failed",
  running: "Running",
  stopped: "Stopped",
  creating: "Creating",
  deleting: "Deleting",
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

export type SpaceFilter = "all" | "running" | "stopped" | "creating";

const ORDER: Record<SpaceState, number> = { failed: 0, creating: 0, running: 1, stopped: 2, deleting: 3 };

/** Filter by state, then: creating first, running next, then stopped; most recent first within each. */
export function visibleSpaces(spaces: readonly Space[], filter: SpaceFilter): Space[] {
  return spaces
    .filter((s) => filter === "all" || spaceState(s) === filter)
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
  return os ? `${os} on ${machine}` : machine;
}

/** The machine a Space runs on, by the machines' `spaceIds`. */
export function machineName(machines: readonly Machine[], space: Pick<Space, "id" | "provider" | "hostName">): string {
  const machine = machines.find((m) => m.spaceIds.includes(space.id));
  if (machine) return machine.name;
  if (space.hostName) return space.hostName;
  if (space.provider === "direct") return "a machine added by address";
  return space.provider === "cloud" || space.provider === undefined ? "Cua Cloud" : "Unknown machine";
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
  const button = core.tryCall<{ help: string; turnOn: boolean; busy: boolean }>("sidebar.powerButton", { space });
  if (button && !button.busy && button.turnOn === on) return button.help;
  const suspends = space.power?.control === "suspend";
  return on ? (suspends ? "Resume" : "Turn on") : suspends ? "Suspend" : "Turn off";
}

/** A power action is running now: "Starting…" or "Stopping…". */
export function powerPending(space: Pick<Space, "power">): string | null {
  const on = space.power?.turningOn;
  if (on === true) return "Starting…";
  if (on === false) return space.power?.control === "suspend" ? "Suspending…" : "Stopping…";
  return null;
}

/** This app can delete it: not this Mac itself, and not a cloud Space only deletable elsewhere. */
export function canDelete(space: Pick<Space, "id" | "status" | "cloudDelete">): boolean {
  // A failed create is removed (`dismissCreate`), not deleted.
  return !space.id.startsWith("pending:") && space.status !== "local" && space.id !== "this-mac" && space.status !== "deleting" && space.cloudDelete !== "elsewhere";
}

const PROVIDER: Record<Exclude<NonNullable<Space["provider"]>, "local">, string> = {
  relay: "Your machine, through the Cua relay",
  direct: "A machine added by address",
  cloud: "Cua Cloud",
};

/** Where it runs, in words. */
export function locationLabel(space: Pick<Space, "provider" | "cloudPlace">): string {
  if (space.cloudPlace) return space.cloudPlace;
  const provider = space.provider ?? "cloud";
  return provider === "local" ? thisComputerLabel() : PROVIDER[provider];
}

export const KIND_LABEL: Record<NonNullable<Space["kind"]>, string> = { vm: "Virtual machine", container: "Container" };
