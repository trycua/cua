// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The words for Cua Spaces' two permissions, the same wherever they show
 * (the first run's step, its next steps, the Cua Driver page, This Mac).
 * Each names the app System Settings lists ("Cua Spaces Local" for the
 * Local build, which has its own grants), so nobody turns on the wrong one.
 */

import type { DriverPermission } from "@/bridge/ops/driver";

export interface PermissionWords {
  title: string;
  sub: string;
}

export function permissionWords(id: DriverPermission["id"], appName: string): PermissionWords {
  return id === "screen"
    ? { title: "See the screen", sub: `Screen Recording, given to ${appName}` }
    : { title: "Use the mouse and keyboard", sub: `Accessibility, given to ${appName}` };
}

/** What Allow does, under the rows. */
export function allowHint(appName: string): string {
  return `Allow opens System Settings. Turn on ${appName} there, then come back.`;
}

/** Under the Cua Driver page's rows: one set of permissions, no second app. */
export function driverUsesAppPermissions(appName: string): string {
  return `Cua Driver uses the permissions you give ${appName}. It never asks for its own.`;
}

/** The first run's permissions step. */
export interface PermissionsStepView {
  title: string;
  line: string;
  /** Under the rows. */
  note: string;
  /** The button: Continue once both are on, else Later (the step never blocks the run). */
  primary: { label: "Continue" | "Later"; white: boolean };
}

export function permissionsStepView(input: { appName: string; allGranted: boolean }): PermissionsStepView {
  return {
    title: `Let ${input.appName} see and use apps`,
    line: `This lets your agents work in the apps on this Mac, in the background. Cua Driver uses these same permissions, so there's no second app to allow.`,
    note: input.allGranted ? "Both are on." : `${allowHint(input.appName)} You can also do this later on the Cua Driver page.`,
    primary: input.allGranted ? { label: "Continue", white: true } : { label: "Later", white: false },
  };
}
