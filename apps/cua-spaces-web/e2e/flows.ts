// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Every app core parity flow (libs/cua/crates/cua-spaces-app-core/parity),
 * and whether the web UI can replay it yet. A flow runs when the screen it
 * drives exists here; the rest are skipped with the screen they wait for.
 * Flows for native-only surfaces (the notch, the menu bar item) are
 * `native`: no web screen will draw them, and the Swift parity runner
 * (`apps/cua-spaces-macos/Tests/CuaSpacesMacTests/ParityTests.swift`)
 * covers them. `parity.spec.ts` fails when the core adds a flow this list
 * does not name.
 */

/** The page a flow opens on: `spaces`, `keyvault`, the Settings tabs (Account is `settings`),
 * `notifications`, `machines` (This machine), `agents`, `volume`,
 * `onboarding`, a Space's detail (`space-detail`, opened from Spaces), or a
 * sheet over Spaces (`new-space`, `teleport`, `share`). */
export type ParityScreen =
  | "spaces"
  | "keyvault"
  | "settings"
  | "settings/general"
  | "settings/about"
  | "settings/advanced"
  | "settings/connected-apps"
  | "settings/devices"
  | "notifications"
  | "machines"
  | "agents"
  | "space-detail"
  | "new-space"
  | "teleport"
  | "share"
  | "volume"
  | "onboarding";

export type FlowPlan =
  | { status: "run"; screen: ParityScreen }
  | { status: "skip"; reason: string }
  | { status: "native"; reason: string };

const native = (reason: string): FlowPlan => ({ status: "native", reason });

export const FLOW_PLAN: Record<string, FlowPlan> = {
  "create-space": { status: "run", screen: "new-space" },
  "create-resources": { status: "run", screen: "new-space" },
  "create-gpu": { status: "run", screen: "new-space" },
  "create-cancel": { status: "run", screen: "spaces" },
  "teleport-review": { status: "run", screen: "teleport" },
  "teleport-sign-ins": { status: "run", screen: "teleport" },
  "keyvault-approve-deny": { status: "run", screen: "keyvault" },
  "keyvault-unlock": { status: "run", screen: "keyvault" },
  "main-window": { status: "run", screen: "machines" },
  notch: native("The notch panel is an AppKit panel over the menu bar; the web UI has no notch"),
  "notch-drag-trigger": native("Dragging a window to the notch is AppKit window tracking; the web UI has no notch"),
  provisioning: { status: "run", screen: "spaces" },
  "stream-section": { status: "run", screen: "space-detail" },
  "space-facts": { status: "run", screen: "space-detail" },
  "picker-grid": { status: "run", screen: "teleport" },
  "delete-space": { status: "run", screen: "spaces" },
  "space-power": { status: "run", screen: "spaces" },
  "create-progress": { status: "run", screen: "spaces" },
  "create-stages": { status: "run", screen: "spaces" },
  "create-retry": { status: "run", screen: "spaces" },
  devices: { status: "run", screen: "settings/devices" },
  "driver-card": { status: "run", screen: "settings/connected-apps" },
  "share-sheet": { status: "run", screen: "share" },
  "agents-page": { status: "run", screen: "agents" },
  "drive-page": { status: "run", screen: "volume" },
  "menu-count": native("The menu bar item and the notch count Spaces natively; the web UI has neither"),
  "drive-onboarding": {
    status: "skip",
    reason: "The 0.10.0 first run has no Cua Volume page (connect the agent, then this computer); Cua Volume is set up from its own page (drive-page, drive-storage)",
  },
  "drive-storage": { status: "run", screen: "volume" },
  notifications: { status: "run", screen: "notifications" },
  about: { status: "run", screen: "settings/about" },
  "agent-keys": { status: "run", screen: "settings/connected-apps" },
  "your-cloud": { status: "run", screen: "new-space" },
  "telemetry-funnel": { status: "run", screen: "spaces" },
  "launch-at-login": { status: "run", screen: "settings/general" },
  experiments: { status: "run", screen: "settings/advanced" },
  "placement-picker": { status: "run", screen: "new-space" },
  machines: { status: "run", screen: "machines" },
  "first-run": {
    status: "skip",
    reason: "The page draws firstRun.view as is; src/__tests__/first-run-parity.test.ts replays this flow on the wasm core and src/__tests__/first-run.test.tsx drives the screen",
  },
  checklist: {
    status: "skip",
    reason: "The host app keeps the checklist and reads what it shows (checklist.observeFrom); the page only draws checklist.view of the host's copy (bridge/__tests__/checklist.test.ts)",
  },
};
