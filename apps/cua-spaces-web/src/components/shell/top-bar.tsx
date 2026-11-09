// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useMatches } from "@tanstack/react-router";
import { MoonIcon, PanelLeftIcon, SearchIcon, SunIcon } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Shortcut } from "@/components/ui/kbd";
import { Tooltip } from "@/components/ui/tooltip";
import { useT } from "@/i18n";
import { bindingFor } from "@/lib/keybindings";
import { useTheme } from "@/lib/theme";
import { useUiStore } from "@/stores/ui";
import { EnvironmentTag } from "./environment-tag";
import { useNavItems } from "./nav";
import { TopBarUpdateIndicator } from "./update-indicator";

export function TopBar() {
  const t = useT();
  const sidebarOpen = useUiStore((s) => s.sidebarOpen);
  const toggleSidebar = useUiStore((s) => s.toggleSidebar);
  const setPaletteOpen = useUiStore((s) => s.setPaletteOpen);
  const { resolved, setPreference } = useTheme();
  const matches = useMatches();
  const path = matches.at(-1)?.pathname ?? "";
  const navItems = useNavItems();
  const title = navItems.find((n) => path.startsWith(n.to))?.label ?? "";

  return (
    <header
      className="app-drag flex h-(--titlebar-height) shrink-0 items-center gap-2 border-b border-transparent px-3"
      style={{
        paddingLeft: sidebarOpen ? undefined : "calc(var(--titlebar-left-inset) + 0.75rem)",
        paddingRight: "calc(var(--titlebar-right-inset) + 0.75rem)",
      }}
    >
      <Tooltip
        content={
          <span className="flex items-center gap-2">
            {t("shell.top_bar.toggle_sidebar")} <Shortcut spec={bindingFor("sidebar.toggle") ?? ""} />
          </span>
        }
      >
        <Button variant="ghost" size="icon-sm" aria-label={t("shell.top_bar.toggle_sidebar")} onClick={toggleSidebar}>
          <PanelLeftIcon className="text-muted-foreground" strokeWidth={1.75} />
        </Button>
      </Tooltip>
      <span className="truncate text-[13px] font-semibold">{title}</span>
      <EnvironmentTag />
      <div className="flex-1" />
      <button
        type="button"
        onClick={() => setPaletteOpen(true)}
        className="flex h-7 w-60 items-center gap-2 rounded-lg border bg-card/60 px-2 text-xs text-muted-foreground shadow-xs outline-none hover:bg-card focus-visible:ring-2 focus-visible:ring-ring/60 dark:bg-white/[0.04]"
      >
        <SearchIcon className="size-3.5" />
        <span className="flex-1 truncate text-left">{t("shell.top_bar.search")}</span>
        <Shortcut spec={bindingFor("commandPalette.toggle") ?? ""} />
      </button>
      {sidebarOpen ? null : <TopBarUpdateIndicator />}
      <Tooltip content={resolved === "dark" ? t("shell.top_bar.switch_to_light") : t("shell.top_bar.switch_to_dark")}>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={resolved === "dark" ? t("shell.top_bar.switch_to_light") : t("shell.top_bar.switch_to_dark")}
          onClick={() => setPreference(resolved === "dark" ? "light" : "dark")}
        >
          {resolved === "dark" ? (
            <SunIcon className="text-muted-foreground" strokeWidth={1.75} />
          ) : (
            <MoonIcon className="text-muted-foreground" strokeWidth={1.75} />
          )}
        </Button>
      </Tooltip>
    </header>
  );
}
