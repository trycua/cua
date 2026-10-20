// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createRootRoute, Outlet, useLocation, useNavigate } from "@tanstack/react-router";
import { lazy, Suspense, useEffect, useRef } from "react";

import { useBridge, useSavedOnboarding, useSession, useStartup } from "@/bridge";

import { ChecklistTracker } from "@/components/getting-started";
import { NewSpace } from "@/components/new-space";
import { CommandPalette } from "@/components/shell/command-palette";
import { DevicesBanner } from "@/components/shell/devices-banner";
import { Sidebar } from "@/components/shell/sidebar";
import { PlanLimitToastAction } from "@/components/shell/plan-limit-toast-action";
import { RelayAllowanceNotice } from "@/components/shell/relay-allowance-notice";
import { SpacesNotice } from "@/components/shell/spaces-notice";
import { SpaceSheets } from "@/components/space-sheets";
import { UsageLinkNotice } from "@/components/usage-link-notice";
import { TopBar } from "@/components/shell/top-bar";
import { ThisMacNudges } from "@/components/this-mac-nudges";
import { useGlobalKeybindings } from "@/hooks/use-global-keybindings";
import { useNotificationToasts } from "@/hooks/use-notification-toasts";
import { useThemeHostSync, useThemeSync } from "@/lib/theme";
import { useUiStore } from "@/stores/ui";

export const Route = createRootRoute({ component: RootLayout });

// Shown only while the native app is still starting, so it loads then.
const StartupScreen = lazy(() => import("@/components/startup/StartupScreen").then((m) => ({ default: m.StartupScreen })));

function RootLayout() {
  useThemeSync();
  useThemeHostSync();
  const { mode } = useBridge();
  // Lets hosts and tests see which bridge the page is on.
  useEffect(() => {
    document.documentElement.dataset.bridge = mode;
  }, [mode]);
  useGlobalKeybindings();
  useFirstRun();
  useNotificationToasts();
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  const { pathname } = useLocation();
  // The app shows as usual until the host says it is still starting.
  const startup = useStartup();

  if (startup.data.phase !== "ready") {
    return (
      <Suspense fallback={<div className="h-full bg-background" />}>
        <StartupScreen state={startup.data} onAct={startup.act} />
      </Suspense>
    );
  }

  if (pathname.startsWith("/onboarding")) {
    return (
      <div className="relative h-full bg-background">
        {/* The first run draws under the title bar; this keeps it draggable. */}
        <div className="app-drag absolute inset-x-0 top-0 z-20 h-(--titlebar-height)" />
        <main className="h-full">
          <Outlet />
        </main>
      </div>
    );
  }

  return (
    <div className="flex h-full bg-chrome">
      {sidebarOpen ? <Sidebar /> : null}
      <div className="flex min-w-0 flex-1 flex-col bg-background">
        <TopBar />
        <DevicesBanner />
        <SpacesNotice />
        <RelayAllowanceNotice />
        <main className="min-h-0 flex-1">
          <Outlet />
        </main>
      </div>
      <PlanLimitToastAction />
      <CommandPalette />
      <ChecklistTracker />
      <NewSpace />
      <SpaceSheets />
      <ThisMacNudges />
      <UsageLinkNotice />
    </div>
  );
}

/**
 * Opens the first run once per launch while the host says it isn't done,
 * unless the user chose "Set up later" (the sidebar then offers it). Every
 * host runs this one (the SwiftUI app opens its window here on a first run).
 */
function useFirstRun() {
  const { data: session } = useSession();
  const { skipped } = useSavedOnboarding();
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const checked = useRef(false);
  useEffect(() => {
    if (checked.current || !session) return;
    checked.current = true;
    if (!session.onboarding.completed && !skipped && !pathname.startsWith("/onboarding")) {
      void navigate({ to: "/onboarding", replace: true });
    }
  }, [session, skipped, pathname, navigate]);
}
