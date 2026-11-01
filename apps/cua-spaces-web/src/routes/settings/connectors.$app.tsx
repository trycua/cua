// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

/** An old app address under Connectors: the parent route (`connectors.tsx`) sends it to the same app under Connections. */
export const Route = createFileRoute("/settings/connectors/$app")({});
