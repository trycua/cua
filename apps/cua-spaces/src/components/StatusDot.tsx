// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { SpaceStatus } from "../model/types";

export const STATUS_LABEL: Record<SpaceStatus, string> = {
  local: "Local",
  running: "Running",
  approval: "Needs approval",
  suspended: "Suspended",
  provisioning: "Starting",
  deleting: "Deleting…",
};

export function StatusDot({ status, className = "" }: { status: SpaceStatus; className?: string }) {
  return <span className={`status-dot status-${status} ${className}`.trim()} aria-hidden="true" />;
}
