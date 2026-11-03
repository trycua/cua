// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The user's own clouds (the SwiftUI app's CloudModel.swift): what the
// daemon's `cloud_status` says, for the New Space wizard's "Your cloud"
// (its connected clouds and the default location). "Connect a cloud"
// (`clouds.*`) builds on this.
import type { Native } from "../native/load";
import type { AppConnectedCloud } from "../native/generated/index";
import type { SpacesBackend } from "./backend";
import { TimeoutError, withTimeout } from "./time";

/** How long `refresh` waits for `cloud_status` (s). */
export const CLOUD_REFRESH_SECONDS = 10;

export class CloudModel {
  /** `cloud_status` as it came (null: not read, or no answer). */
  status: unknown = null;

  constructor(
    private readonly native: Native,
    private readonly backend: () => SpacesBackend | null,
  ) {}

  private get statusJson(): string {
    return this.status === null ? "{}" : JSON.stringify(this.status);
  }

  /** The connected clouds, for the New Space wizard. */
  get clouds(): AppConnectedCloud[] {
    try {
      return this.native.appConnectedCloudsFromStatusJson(this.statusJson);
    } catch {
      return [];
    }
  }

  /** The default location's word (`default.on`), when the status says. */
  get defaultOn(): string | null {
    const on = (this.status as { default_on?: unknown } | null)?.default_on;
    return typeof on === "string" ? on : null;
  }

  /** Reads `cloud_status` again, bounded; a daemon that does not answer in time leaves what was read before. */
  async refresh(): Promise<void> {
    const backend = this.backend();
    if (!backend) return;
    const o = await withTimeout(CLOUD_REFRESH_SECONDS, () => backend.tool("cloud_status", {}));
    if (o.ok) this.status = o.value;
    else if (!(o.error instanceof TimeoutError)) this.status = null;
  }
}
