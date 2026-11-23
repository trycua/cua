// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { CI_OPERATIONS } from "../../ops/ci";

/** The CI Runners operations the demo host answers. */
export type CiOp = (typeof CI_OPERATIONS)[number];
