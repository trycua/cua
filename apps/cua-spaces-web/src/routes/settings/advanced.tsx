// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { AdvancedTab } from "@/components/settings/advanced-tab";

export const Route = createFileRoute("/settings/advanced")({ component: AdvancedTab });
