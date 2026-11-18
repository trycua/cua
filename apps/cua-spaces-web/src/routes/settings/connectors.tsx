// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, redirect } from "@tanstack/react-router";

/** Settings, Connections is now Connections: an old address (a bookmark, an older link) lands there, on the same app's page when it named one. */
export const Route = createFileRoute("/settings/connectors")({
  validateSearch: (s: Record<string, unknown>): { app?: string } => ({ app: typeof s.app === "string" ? s.app : undefined }),
  beforeLoad: ({ search, location }) => {
    const page = /^\/settings\/connectors\/([^/]+)\/?$/.exec(location.pathname)?.[1];
    if (page) throw redirect({ to: "/settings/connections/$app", params: { app: decodeURIComponent(page) }, replace: true });
    throw redirect({ to: "/settings/connections", search: { app: search.app }, replace: true });
  },
});
