// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link, Outlet } from "@tanstack/react-router";

import { Page, PageHeader } from "@/components/page";
import { SETTINGS_TABS } from "@/lib/settings-tabs";

export const Route = createFileRoute("/settings")({ component: SettingsLayout });

function SettingsLayout() {
  return (
    // 32px under the top bar, 32px between sections, 48px rows (the agreed Settings rhythm).
    <Page className="max-w-2xl pt-8">
      <PageHeader title="Settings" />
      <nav aria-label="Settings" className="mb-8 inline-flex h-12 items-center gap-0.5 rounded-xl bg-muted p-1">
        {SETTINGS_TABS.map((t) => (
          <Link
            key={t.to}
            to={t.to}
            activeOptions={{ exact: t.exact }}
            className="inline-flex h-10 items-center rounded-lg px-3.5 text-[13px] font-medium text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 data-[status=active]:bg-card data-[status=active]:text-foreground data-[status=active]:shadow-xs dark:data-[status=active]:bg-accent"
          >
            {t.label}
          </Link>
        ))}
      </nav>
      <Outlet />
    </Page>
  );
}
