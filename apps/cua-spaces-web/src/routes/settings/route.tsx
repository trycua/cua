// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link, Outlet, useRouterState } from "@tanstack/react-router";

import { Page, PageHeader } from "@/components/page";
import { SETTINGS_TABS } from "@/lib/settings-tabs";

export const Route = createFileRoute("/settings")({ component: SettingsLayout });

/** The tab whose page is showing: Account is exactly `/settings`, the rest also own their sub-paths. */
function isActiveTab(pathname: string, t: (typeof SETTINGS_TABS)[number]) {
  const path = pathname.replace(/\/+$/, "") || "/";
  return t.exact ? path === t.to : path === t.to || path.startsWith(`${t.to}/`);
}

function SettingsLayout() {
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  return (
    // 32px under the top bar, 32px between sections, 48px rows (the agreed Settings rhythm).
    <Page className="max-w-2xl pt-8">
      <PageHeader title="Settings" />
      <nav aria-label="Settings" role="tablist" className="mb-8 inline-flex h-12 items-center gap-0.5 rounded-xl bg-muted p-1">
        {SETTINGS_TABS.map((t) => (
          <Link
            key={t.to}
            to={t.to}
            role="tab"
            // Link's own active state (data-status) can't tell Account's "/settings" from its siblings under it.
            aria-selected={isActiveTab(pathname, t)}
            activeOptions={{ exact: t.exact }}
            className="inline-flex h-10 items-center rounded-lg px-3.5 text-[13px] font-medium text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 aria-selected:bg-card aria-selected:text-foreground aria-selected:shadow-xs dark:aria-selected:bg-accent"
          >
            {t.label}
          </Link>
        ))}
      </nav>
      <Outlet />
    </Page>
  );
}
