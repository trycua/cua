// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate, useRouter } from "@tanstack/react-router";
import {
  CornerDownLeftIcon,
  LockIcon,
  LogInIcon,
  LogOutIcon,
  MonitorIcon,
  MoonIcon,
  MousePointer2Icon,
  PanelLeftIcon,
  PlusIcon,
  SearchIcon,
  SettingsIcon,
  SparklesIcon,
  SunIcon,
  type LucideIcon,
} from "lucide-react";
import { useMemo, useState, type KeyboardEvent } from "react";

import { useBridge, useDriver, useKeyvault, useMachines, useSession, useSpaces } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { useT } from "@/i18n";
import { useHiddenSpaceIds } from "@/lib/prepare/store";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Kbd, Shortcut } from "@/components/ui/kbd";
import { toast, toastError } from "@/components/ui/toast";
import { filterCommands, type PaletteCommand } from "@/lib/commands";
import { machineName, orderSpaces, osLabel, paletteKeywords, realSpaces, spaceState } from "@/lib/spaces";
import { bindingFor } from "@/lib/keybindings";
import { THIS_COMPUTER } from "@/lib/machines";
import { SETTINGS_TABS } from "@/lib/settings-tabs";
import { useTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useNewSpace } from "@/hooks/use-new-space";
import { useUiStore } from "@/stores/ui";
import { useNavItems } from "./nav";

export function CommandPalette() {
  const t = useT();
  const open = useUiStore((s) => s.paletteOpen);
  const setOpen = useUiStore((s) => s.setPaletteOpen);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogPopup data-command-palette="">
        <DialogTitle className="sr-only">{t("shell.palette.title")}</DialogTitle>
        {open ? <PaletteBody onClose={() => setOpen(false)} /> : null}
      </DialogPopup>
    </Dialog>
  );
}

type Command = PaletteCommand & { icon?: React.ReactNode };

const icon = (Icon: LucideIcon) => <Icon className="size-4 text-muted-foreground" strokeWidth={1.75} />;

function PaletteBody({ onClose }: { onClose: () => void }) {
  const t = useT();
  const router = useRouter();
  const navigate = useNavigate();
  const { core } = useBridge();
  const { data: allSpaces = [], startSpace, stopSpace } = useSpaces();
  // The home warming for Home's box is not a command until a person wrote into it.
  const warming = useHiddenSpaceIds(core);
  const spaces = useMemo(() => allSpaces.filter((s) => !warming.has(s.id)), [allSpaces, warming]);
  const { data: machines = [] } = useMachines();
  const newSpace = useNewSpace();
  const { data: keyvault, lockItems } = useKeyvault();
  const { data: session, signIn, signOut } = useSession();
  const { preference, resolved, setPreference } = useTheme();
  const toggleSidebar = useUiStore((s) => s.toggleSidebar);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);

  const openSpace = (id: string) => void router.navigate({ href: `/spaces/${encodeURIComponent(id)}` });
  const vaultLocked = keyvault ? keyvault.overview.availability === "locked" || keyvault.overview.status?.unlocked === false : true;
  const unattended = (keyvault?.overview.items ?? []).filter((i) => i.policy.unattended).map((i) => i.id);
  const other = resolved === "dark" ? "light" : "dark";

  const navItems = useNavItems();
  const driverAvailable = useDriver().available;
  const actions = t("shell.palette.group_actions");
  const goTo = t("shell.palette.group_go_to");
  const commands: Command[] = [
    {
      id: "space:new",
      group: actions,
      label: t("shell.palette.new_space"),
      keywords: t("shell.palette.new_space_keywords"),
      icon: icon(PlusIcon),
      run: () => {
        void navigate({ to: "/spaces" });
        newSpace();
      },
    },
    {
      id: "theme:toggle",
      group: actions,
      label: t(other === "light" ? "shell.palette.switch_to_light" : "shell.palette.switch_to_dark"),
      keywords: t("shell.palette.theme_keywords"),
      shortcut: bindingFor("theme.cycle"),
      icon: icon(resolved === "dark" ? SunIcon : MoonIcon),
      run: () => setPreference(other),
    },
    ...(preference !== "system"
      ? [
          {
            id: "theme:system",
            group: actions,
            label: t("shell.palette.match_system"),
            keywords: t("shell.palette.match_system_keywords"),
            icon: icon(MonitorIcon),
            run: () => setPreference("system"),
          },
        ]
      : []),
    ...(!vaultLocked && unattended.length > 0
      ? [
          {
            id: "keyvault:lock",
            group: actions,
            label: t("shell.palette.lock_keyvault"),
            hint: t("shell.palette.lock_keyvault_hint"),
            keywords: t("shell.palette.lock_keyvault_keywords"),
            icon: icon(LockIcon),
            run: () =>
              void lockItems(unattended)
                .then(() => toast(t("shell.palette.keyvault_locked"), { description: t("shell.palette.keyvault_locked_description") }))
                .catch(toastError(t("shell.palette.lock_keyvault_failed"))),
          },
        ]
      : []),
    session?.signedIn
      ? {
          id: "session:signOut",
          group: actions,
          label: t("shell.palette.sign_out"),
          hint: session.identity ?? undefined,
          keywords: t("shell.palette.sign_out_keywords"),
          icon: icon(LogOutIcon),
          run: () => void signOut().catch(toastError(t("shell.palette.sign_out_failed"))),
        }
      : {
          id: "session:signIn",
          group: actions,
          label: t("shell.palette.sign_in"),
          keywords: t("shell.palette.sign_in_keywords"),
          icon: icon(LogInIcon),
          run: () =>
            void signIn()
              .then(() => toast(t("shell.palette.sign_in_opened"), { description: t("shell.palette.sign_in_opened_description") }))
              .catch(toastError(t("shell.palette.sign_in_failed"))),
        },
    {
      id: "sidebar",
      group: actions,
      label: t("shell.palette.toggle_sidebar"),
      keywords: t("shell.palette.toggle_sidebar_keywords"),
      shortcut: bindingFor("sidebar.toggle"),
      icon: icon(PanelLeftIcon),
      run: toggleSidebar,
    },
    ...navItems.map((n) => ({
      id: `nav:${n.to}`,
      group: goTo,
      label: n.label,
      // Computers is where Machines and the Spaces list were (CUA-3551): either old word finds it.
      keywords: n.to === "/spaces" ? t("shell.palette.computers_keywords") : t("shell.palette.page_keywords"),
      shortcut: n.command ? bindingFor(n.command) : undefined,
      icon: icon(n.icon),
      run: () => void navigate({ to: n.to }),
    })),
    // Each Settings tab, so Cmd+K finds "Connectors" or "Advanced" by name.
    ...SETTINGS_TABS.filter((tab) => tab.to !== "/settings").map((tab) => ({
      id: `settings:${tab.to}`,
      group: goTo,
      label: t("shell.palette.settings_tab", { tab: tab.label }),
      keywords: tab.to === "/settings/connections" ? t("shell.palette.settings_connections_keywords") : t("shell.palette.settings_keywords"),
      icon: icon(SettingsIcon),
      run: () => void navigate({ to: tab.to }),
    })),
    // Cua Driver is one of This Mac's settings, where this Cua Spaces carries it.
    ...(driverAvailable
      ? [
          {
            id: "nav:driver",
            group: goTo,
            label: t("common.cua_driver"),
            keywords: t("shell.palette.driver_keywords"),
            icon: icon(MousePointer2Icon),
            run: () => void navigate({ to: "/spaces/computer/$computerId/driver", params: { computerId: THIS_COMPUTER } }),
          },
        ]
      : []),
    {
      id: "nav:onboarding",
      group: goTo,
      label: t("shell.palette.onboarding"),
      keywords: t("shell.palette.onboarding_keywords"),
      icon: icon(SparklesIcon),
      run: () => void navigate({ to: "/onboarding" }),
    },
    ...orderSpaces(realSpaces(spaces)).flatMap((s): Command[] => {
      const state = spaceState(s);
      const meta = {
        group: "Spaces",
        hint: osLabel(s),
        keywords: paletteKeywords(s, state, machineName(machines, s)),
        icon: <OsIcon os={s.os} className="size-3.5 text-muted-foreground" />,
      };
      const out: Command[] = [{ ...meta, id: `space:open:${s.id}`, label: t("shell.palette.open_space", { name: s.name }), run: () => openSpace(s.id) }];
      if (state === "stopped") {
        out.push({
          ...meta,
          id: `space:start:${s.id}`,
          label: t("shell.palette.start_space", { name: s.name }),
          run: () => void startSpace(s.id).catch(toastError(t("shell.palette.start_space_failed", { name: s.name }))),
        });
      } else if (state === "running") {
        out.push({
          ...meta,
          id: `space:stop:${s.id}`,
          label: t("shell.palette.stop_space", { name: s.name }),
          run: () => void stopSpace(s.id).catch(toastError(t("shell.palette.stop_space_failed", { name: s.name }))),
        });
      }
      return out;
    }),
  ];

  const results = filterCommands(commands, query, core);
  const current = Math.min(active, Math.max(0, results.length - 1));

  const runAt = (i: number) => {
    const cmd = results[i];
    if (!cmd) return;
    onClose();
    cmd.run();
  };

  const move = (next: number) => {
    setActive(next);
    const cmd = results[next];
    if (cmd) document.getElementById(`palette-${cmd.id}`)?.scrollIntoView({ block: "nearest" });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    const n = Math.max(1, results.length);
    if (e.key === "ArrowDown") {
      e.preventDefault();
      move((current + 1) % n);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      move((current - 1 + n) % n);
    } else if (e.key === "Enter") {
      e.preventDefault();
      runAt(current);
    }
  };

  let lastGroup = "";
  return (
    <div className="flex max-h-[min(440px,70vh)] flex-col">
      <div className="flex items-center gap-2.5 border-b px-4">
        <SearchIcon className="size-4 text-muted-foreground" />
        <input
          autoFocus
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
          placeholder={t("shell.palette.search")}
          aria-label={t("shell.palette.search")}
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={results[current] ? `palette-${results[current].id}` : undefined}
          className="h-12 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground"
        />
      </div>
      <div id="palette-list" role="listbox" className="flex-1 overflow-y-auto p-1.5">
        {results.length === 0 ? <p className="px-3 py-8 text-center text-[13px] text-muted-foreground">{t("shell.palette.no_matches")}</p> : null}
        {results.map((cmd, i) => {
          const header = cmd.group !== lastGroup ? cmd.group : null;
          lastGroup = cmd.group;
          return (
            <div key={cmd.id}>
              {header ? <div className="px-2.5 pt-2 pb-1 text-2xs font-medium text-muted-foreground">{header}</div> : null}
              <div
                id={`palette-${cmd.id}`}
                role="option"
                aria-selected={i === current}
                onPointerMove={() => setActive(i)}
                onClick={() => runAt(i)}
                className={cn("flex h-8 items-center gap-2.5 rounded-md px-2.5 text-[13px]", i === current && "bg-foreground/[0.07] dark:bg-white/[0.08]")}
              >
                <span className="flex size-4 items-center justify-center">{cmd.icon}</span>
                <span className="flex-1 truncate">{cmd.label}</span>
                {cmd.hint ? <span className="text-xs text-muted-foreground">{cmd.hint}</span> : null}
                {cmd.shortcut ? <Shortcut spec={cmd.shortcut} /> : null}
              </div>
            </div>
          );
        })}
      </div>
      <div className="flex items-center gap-3 border-t bg-foreground/[0.02] px-4 py-2 text-2xs text-muted-foreground">
        <span className="flex items-center gap-1">
          <Kbd>↑</Kbd>
          <Kbd>↓</Kbd> {t("shell.palette.hint_move")}
        </span>
        <span className="flex items-center gap-1">
          <Kbd>
            <CornerDownLeftIcon className="size-3" />
          </Kbd>{" "}
          {t("shell.palette.hint_run")}
        </span>
        <span className="flex items-center gap-1">
          <Kbd>{t("shell.palette.key_esc")}</Kbd> {t("shell.palette.hint_close")}
        </span>
      </div>
    </div>
  );
}
