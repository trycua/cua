// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ELECTRON, electronPath } from "./host";

/** Electron runs: download the Electron binary once, here, not in each worker. */
export default function globalSetup() {
  if (ELECTRON) electronPath();
}
