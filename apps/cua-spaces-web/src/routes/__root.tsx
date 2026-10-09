// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createRootRoute, Outlet, useLocation, useNavigate } from "@tanstack/react-router";
import { lazy, Suspense, useEffect, useRef } from "react";

import { useBridge, useSavedOnboarding, useSession, useStartup } from "@/bridge";

import { NewSpace } from "@/components/new-space";
import { CommandPalette } from "@/components/shell/command-palette";
import { Sidebar } from "@/components/shell/sidebar";
import { SpacesNotice } from "@/components/shell/spaces-notice";
import { SpaceSheets } from "@/components/space-sheets";
import { TopBar } from "@/components/shell/top-bar";
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
      <div className="flex h-full flex-col bg-background">
        <div className="app-drag h-(--titlebar-height) shrink-0" />
        <main className="min-h-0 flex-1">
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
        <SpacesNotice />
        <main className="min-h-0 flex-1">
          <Outlet />
        </main>
      </div>
      <CommandPalette />
      <NewSpace />
      <SpaceSheets />
    </div>
  );
}

/**
 * Opens the first run once per launch while the host says it isn't done,
 * unless the user chose "Set up later" (the sidebar then offers it). The
 * SwiftUI host runs its own.
 */
function useFirstRun() {
  const { mode } = useBridge();
  const { data: session } = useSession();
  const { skipped } = useSavedOnboarding();
  const { pathname } = useLocation();
  const navigate = useNavigate();
  const checked = useRef(false);
  useEffect(() => {
    if (checked.current || !session) return;
    checked.current = true;
    if (mode !== "webkit" && !session.onboarding.completed && !skipped && !pathname.startsWith("/onboarding")) {
      void navigate({ to: "/onboarding", replace: true });
    }
  }, [mode, session, skipped, pathname, navigate]);
}
