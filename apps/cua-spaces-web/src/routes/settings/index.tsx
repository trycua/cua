// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { AccountTab } from "@/components/settings/account-tab";

export const Route = createFileRoute("/settings/")({ component: AccountTab });
