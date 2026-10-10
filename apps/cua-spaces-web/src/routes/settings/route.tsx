// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link, Outlet } from "@tanstack/react-router";

import { Page, PageHeader } from "@/components/page";

export const Route = createFileRoute("/settings")({ component: SettingsLayout });

/** The native Settings window's tabs: General, Agents, Devices, Experiments, About. */
const TABS = [
  { to: "/settings", label: "General", exact: true },
  { to: "/settings/agents", label: "Agents", exact: false },
  { to: "/settings/devices", label: "Devices", exact: false },
  { to: "/settings/experiments", label: "Experiments", exact: false },
  { to: "/settings/about", label: "About", exact: false },
] as const;

function SettingsLayout() {
  return (
    <Page className="max-w-2xl">
      <PageHeader title="Settings" />
      <nav aria-label="Settings" className="mb-6 inline-flex h-8 items-center gap-0.5 rounded-lg bg-muted p-0.5">
        {TABS.map((t) => (
          <Link
            key={t.to}
            to={t.to}
            activeOptions={{ exact: t.exact }}
            className="inline-flex h-7 items-center rounded-md px-3 text-[13px] font-medium text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 data-[status=active]:bg-card data-[status=active]:text-foreground data-[status=active]:shadow-xs dark:data-[status=active]:bg-accent"
          >
            {t.label}
          </Link>
        ))}
      </nav>
      <Outlet />
    </Page>
  );
}
