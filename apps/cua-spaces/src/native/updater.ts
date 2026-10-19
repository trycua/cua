// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * In-app auto-update. On portal launch we quietly ask the release feed whether a
 * newer signed build exists; if so we download + install it in the background
 * and relaunch. Everything is dynamically imported so the browser/test builds
 * (no Tauri, no plugins) simply no-op.
 *
 * The feed + signature are configured in `tauri.conf.json` under
 * `plugins.updater` (endpoint + pubkey); CI signs each release with the matching
 * private key (TAURI_SIGNING_PRIVATE_KEY).
 */
import { telemetryBridge, type TelemetryBridge } from "./telemetry";

export async function checkForUpdateSilently(telemetry: TelemetryBridge = telemetryBridge()): Promise<void> {
  // Whether people update: a check, what it found, the install (fixed
  // words; this updater has one feed, the stable channel).
  const record = (action: "checked" | "found" | "not_found" | "installed" | "failed") =>
    telemetry.recordSignals([{ type: "app-update", action, channel: "stable", trigger: "background" }]);
  let checked = false;
  try {
    const { check } = await import("@tauri-apps/plugin-updater");
    const update = await check();
    checked = true;
    record("checked");
    if (!update) {
      record("not_found");
      return;
    }
    record("found");
    // A newer version is available: pull it down and stage the install.
    await update.downloadAndInstall();
    record("installed");
    // Relaunch into the freshly installed version.
    const { relaunch } = await import("@tauri-apps/plugin-process");
    await relaunch();
  } catch {
    // No updater (browser/dev), offline, or an unconfigured feed: never block
    // startup on it. Only a failure after a check is an update failure.
    if (checked) record("failed");
  }
}
