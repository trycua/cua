// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { useEffect } from "react";

import { WEBKIT_EVENT, WEBKIT_OPEN_SETTINGS_EVENT, type WebkitEventDetail } from "@/bridge/webkit-protocol";
import { resolveCommand, type KeybindingCommand } from "@/lib/keybindings";
import { useThemeStore } from "@/lib/theme";
import { useUiStore } from "@/stores/ui";

const NAV: Partial<Record<KeybindingCommand, "/spaces" | "/machines" | "/agents" | "/keyvault" | "/settings">> = {
  "nav.spaces": "/spaces",
  "nav.machines": "/machines",
  "nav.agents": "/agents",
  "nav.keyvault": "/keyvault",
  "nav.settings": "/settings",
};

const THEME_CYCLE = { system: "light", light: "dark", dark: "system" } as const;

export function useGlobalKeybindings(): void {
  const navigate = useNavigate();
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const command = resolveCommand(event);
      if (!command) return;
      event.preventDefault();
      const ui = useUiStore.getState();
      const to = NAV[command];
      if (to) {
        ui.setPaletteOpen(false);
        void navigate({ to });
        return;
      }
      switch (command) {
        case "commandPalette.toggle":
          ui.setPaletteOpen(!ui.paletteOpen);
          break;
        case "sidebar.toggle":
          ui.toggleSidebar();
          break;
        case "theme.cycle": {
          const theme = useThemeStore.getState();
          theme.setPreference(THEME_CYCLE[theme.preference]);
          break;
        }
      }
    };
    // The SwiftUI app's Settings command (⌘, in its menu) reaches the page as an event, so it works
    // when the web view has no focus too (the native menu gets the key first then).
    const onHostEvent = (event: Event) => {
      if ((event as CustomEvent<WebkitEventDetail>).detail?.event !== WEBKIT_OPEN_SETTINGS_EVENT) return;
      useUiStore.getState().setPaletteOpen(false);
      void navigate({ to: "/settings" });
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener(WEBKIT_EVENT, onHostEvent);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener(WEBKIT_EVENT, onHostEvent);
    };
  }, [navigate]);
}
