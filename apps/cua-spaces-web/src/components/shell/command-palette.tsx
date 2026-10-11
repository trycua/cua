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
  PanelLeftIcon,
  PlusIcon,
  SearchIcon,
  SparklesIcon,
  SunIcon,
  type LucideIcon,
} from "lucide-react";
import { useState, type KeyboardEvent } from "react";

import { useKeyvault, useSession, useSpaces } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Kbd, Shortcut } from "@/components/ui/kbd";
import { toast, toastError } from "@/components/ui/toast";
import { filterCommands, type PaletteCommand } from "@/lib/commands";
import { osLabel, realSpaces, spaceState } from "@/lib/spaces";
import { bindingFor } from "@/lib/keybindings";
import { useTheme } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { useNewSpace } from "@/hooks/use-new-space";
import { useUiStore } from "@/stores/ui";
import { useNavItems } from "./nav";

export function CommandPalette() {
  const open = useUiStore((s) => s.paletteOpen);
  const setOpen = useUiStore((s) => s.setPaletteOpen);
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogPopup data-command-palette="">
        <DialogTitle className="sr-only">Command palette</DialogTitle>
        {open ? <PaletteBody onClose={() => setOpen(false)} /> : null}
      </DialogPopup>
    </Dialog>
  );
}

type Command = PaletteCommand & { icon?: React.ReactNode };

const icon = (Icon: LucideIcon) => <Icon className="size-4 text-muted-foreground" strokeWidth={1.75} />;

function PaletteBody({ onClose }: { onClose: () => void }) {
  const router = useRouter();
  const navigate = useNavigate();
  const { data: spaces = [], startSpace, stopSpace } = useSpaces();
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
  const commands: Command[] = [
    {
      id: "space:new",
      group: "Actions",
      label: "New Space",
      keywords: "create add",
      icon: icon(PlusIcon),
      run: () => {
        void navigate({ to: "/spaces" });
        newSpace();
      },
    },
    {
      id: "theme:toggle",
      group: "Actions",
      label: `Switch to ${other} appearance`,
      keywords: "theme toggle dark light mode",
      shortcut: bindingFor("theme.cycle"),
      icon: icon(resolved === "dark" ? SunIcon : MoonIcon),
      run: () => setPreference(other),
    },
    ...(preference !== "system"
      ? [{ id: "theme:system", group: "Actions", label: "Match system appearance", keywords: "theme auto", icon: icon(MonitorIcon), run: () => setPreference("system") }]
      : []),
    ...(!vaultLocked && unattended.length > 0
      ? [
          {
            id: "keyvault:lock",
            group: "Actions",
            label: "Lock Keyvault",
            hint: "Every login asks before use",
            keywords: "secure ask",
            icon: icon(LockIcon),
            run: () =>
              void lockItems(unattended)
                .then(() => toast("Keyvault locked", { description: "Every login now asks before each use." }))
                .catch(toastError("Couldn't lock Keyvault")),
          },
        ]
      : []),
    session?.signedIn
      ? { id: "session:signOut", group: "Actions", label: "Sign out", hint: session.identity ?? undefined, keywords: "account log out", icon: icon(LogOutIcon), run: () => void signOut().catch(toastError("Couldn't sign out")) }
      : {
          id: "session:signIn",
          group: "Actions",
          label: "Sign in",
          keywords: "account log in cloud",
          icon: icon(LogInIcon),
          run: () =>
            void signIn()
              .then(() => toast("Sign-in opened in your browser", { description: "Finish there, then come back." }))
              .catch(toastError("Couldn't start sign-in")),
        },
    { id: "sidebar", group: "Actions", label: "Show or hide sidebar", keywords: "toggle", shortcut: bindingFor("sidebar.toggle"), icon: icon(PanelLeftIcon), run: toggleSidebar },
    ...navItems.map((n) => ({
      id: `nav:${n.to}`,
      group: "Go to",
      label: n.label,
      keywords: "page",
      shortcut: n.command ? bindingFor(n.command) : undefined,
      icon: icon(n.icon),
      run: () => void navigate({ to: n.to }),
    })),
    { id: "nav:onboarding", group: "Go to", label: "Onboarding", keywords: "welcome setup", icon: icon(SparklesIcon), run: () => void navigate({ to: "/onboarding" }) },
    ...realSpaces(spaces).flatMap((s): Command[] => {
      const state = spaceState(s);
      const meta = { group: "Spaces", hint: osLabel(s), keywords: `${s.os} ${state}`, icon: <OsIcon os={s.os} className="size-3.5 text-muted-foreground" /> };
      const out: Command[] = [{ ...meta, id: `space:open:${s.id}`, label: `Open ${s.name}`, run: () => openSpace(s.id) }];
      if (state === "stopped") {
        out.push({ ...meta, id: `space:start:${s.id}`, label: `Start ${s.name}`, run: () => void startSpace(s.id).catch(toastError(`Couldn't start ${s.name}`)) });
      } else if (state === "running") {
        out.push({ ...meta, id: `space:stop:${s.id}`, label: `Stop ${s.name}`, run: () => void stopSpace(s.id).catch(toastError(`Couldn't stop ${s.name}`)) });
      }
      return out;
    }),
  ];

  const results = filterCommands(commands, query);
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
          placeholder="Search for a Space or command"
          aria-label="Search for a Space or command"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-list"
          aria-activedescendant={results[current] ? `palette-${results[current].id}` : undefined}
          className="h-12 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground"
        />
      </div>
      <div id="palette-list" role="listbox" className="flex-1 overflow-y-auto p-1.5">
        {results.length === 0 ? <p className="px-3 py-8 text-center text-[13px] text-muted-foreground">No matches</p> : null}
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
        <span className="flex items-center gap-1"><Kbd>↑</Kbd><Kbd>↓</Kbd> to move</span>
        <span className="flex items-center gap-1"><Kbd><CornerDownLeftIcon className="size-3" /></Kbd> to run</span>
        <span className="flex items-center gap-1"><Kbd>esc</Kbd> to close</span>
      </div>
    </div>
  );
}
