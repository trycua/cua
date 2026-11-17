// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute } from "@tanstack/react-router";

import { PoolHome } from "@/components/home/pool-home";

/** Home: your computers as one supercomputer, and Run something. */
export const Route = createFileRoute("/home/")({
  component: PoolHome,
});
