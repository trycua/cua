// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The clouds on the Computers page, from the app core (`cloud_page`): the
 * Clouds group and each cloud's page, read from `cloud_status`.
 */

import type { CloudsInput, CloudsView } from "./contracts/host";
import type { CloudStatusWire } from "./contracts/new-space";
import type { CoreClient } from "./core";

/** What the page draws the clouds from: `cloud_status` and now. */
export const cloudsInput = (core: CoreClient, status: CloudStatusWire | null, now: number) =>
  core.tryCall<CloudsInput>("clouds.inputFromStatus", { status: status ?? {}, now, canApprove: status?.can_approve === true });

/** The Clouds group and each cloud's page; undefined without the core. */
export const cloudsView = (core: CoreClient, input: CloudsInput) => core.tryCall<CloudsView>("clouds.view", { input });
