// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { useRef, type PointerEvent } from "react";

import { LogOutIcon } from "lucide-react";

import { signInCodeText, useBridge, useMachines, useNotifications, useSession, useSpaces } from "@/bridge";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { Tooltip } from "@/components/ui/tooltip";
import { bindingFor } from "@/lib/keybindings";
import { realSpaces } from "@/lib/spaces";
import { cn } from "@/lib/utils";
import { SIDEBAR_DEFAULT, useUiStore } from "@/stores/ui";
import { Shortcut } from "@/components/ui/kbd";
import { useNavItems } from "./nav";

export function Sidebar() {
  const width = useUiStore((s) => s.sidebarWidth);
  const { data: spaces } = useSpaces();
  const { data: machines } = useMachines();
  const notifications = useNotifications();
  const counts: Partial<Record<string, number>> = { "/spaces": spaces && realSpaces(spaces).length, "/machines": machines?.length };
  const navItems = useNavItems();
  const unread = notifications.data?.unread ?? 0;

  return (
    <aside
      style={{ width }}
      className="glass relative flex h-full shrink-0 flex-col border-r bg-sidebar text-sidebar-foreground"
      aria-label="Sidebar"
    >
      {/* Room for the top bar and traffic lights; draggable like the title bar. */}
      <div className="app-drag h-(--titlebar-height) shrink-0" />
      <nav className="flex flex-col gap-px px-2.5">
        {navItems.map((item) => {
          // A host without the feed has no Notifications page.
          if (item.to === "/notifications" && notifications.unsupported) return null;
          const shortcut = item.command ? bindingFor(item.command) : undefined;
          return (
            <Link
              key={item.to}
              to={item.to}
              className="group flex h-7 items-center gap-2.5 rounded-md px-2 text-[13px] outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/60 data-[status=active]:bg-foreground/[0.07] data-[status=active]:font-medium data-[status=active]:text-foreground dark:data-[status=active]:bg-white/[0.08]"
            >
              {({ isActive }) => (
                <>
                  <item.icon className={cn("size-4 text-muted-foreground", isActive && "text-brand-strong")} strokeWidth={1.75} />
                  <span className="flex-1 truncate">{item.label}</span>
                  {item.to === "/notifications" && unread > 0 ? (
                    <span data-unread-badge className="min-w-4 rounded-full bg-brand px-1 text-center text-2xs font-medium tabular-nums text-white" aria-label={`${unread} unread`}>
                      {unread}
                    </span>
                  ) : null}
                  {counts[item.to] !== undefined ? (
                    <span className="text-2xs tabular-nums text-muted-foreground group-hover:hidden">{counts[item.to]}</span>
                  ) : null}
                  {shortcut ? <Shortcut spec={shortcut} className="hidden opacity-70 group-hover:inline-flex" /> : null}
                </>
              )}
            </Link>
          );
        })}
      </nav>
      <AccountFooter />
      <ResizeHandle />
    </aside>
  );
}

/** The account, or "Sign in"; and a way back into a first run left halfway. */
export function AccountFooter() {
  const { mode, core } = useBridge();
  const { data: session, signIn, signOut, cancelSignIn, openExternal } = useSession();
  if (!session) return <div className="mt-auto h-12 border-t" />;

  const identity = session.signedIn ? (session.identity ?? "Signed in") : null;
  const phase = session.signIn.kind;
  const waiting = phase === "starting" || phase === "waiting";
  // The SwiftUI host runs its own first run.
  const unfinished = mode !== "webkit" && !session.onboarding.completed;
  const run = (f: () => Promise<unknown>, what: string) =>
    f().catch((e: unknown) => toast(what, { description: e instanceof Error ? e.message : String(e) }));

  return (
    <div className="mt-auto border-t px-3 py-2.5">
      {unfinished ? (
        <Link to="/onboarding" className="mb-2 block text-xs text-muted-foreground hover:text-foreground">
          Finish setting up
        </Link>
      ) : null}
      {identity ? (
        <div className="flex items-center gap-2.5">
          <div className="flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-2xs font-semibold text-muted-foreground uppercase">
            {identity.slice(0, 1)}
          </div>
          <div className="min-w-0 flex-1 leading-tight">
            <div className="truncate text-xs font-medium">{identity}</div>
            <div className="truncate text-2xs text-muted-foreground">Cua account</div>
          </div>
          {session.fleet.authMode === "user" ? (
            <Tooltip content="Sign out" side="top">
              <Button variant="ghost" size="icon-sm" aria-label="Sign out" onClick={() => void run(signOut, "Couldn't sign out")}>
                <LogOutIcon className="size-3.5 text-muted-foreground" />
              </Button>
            </Tooltip>
          ) : null}
        </div>
      ) : waiting ? (
        // As in the first run: the code to confirm, the page to open again
        // if the tab was closed, and a way out (it also times out).
        <div data-signin="waiting">
          <p className="text-xs text-muted-foreground" role="status">
            {phase === "waiting" && session.signIn.kind === "waiting"
              ? signInCodeText(core, session.signIn.userCode)
              : "Waiting for the browser…"}
          </p>
          <div className="mt-1.5 flex items-center gap-3 text-2xs">
            {session.signInUrl ? (
              <button
                type="button"
                className="text-foreground underline-offset-2 hover:underline"
                onClick={() => void openExternal(session.signInUrl!)}
                data-signin-reopen=""
              >
                Open the browser again
              </button>
            ) : null}
            <button
              type="button"
              className="text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
              onClick={() => cancelSignIn()}
              data-signin-cancel=""
            >
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <div className="flex items-center justify-between gap-2">
          <Button variant="outline" size="sm" onClick={() => void run(signIn, "Couldn't sign in")}>
            {phase === "failed" ? "Try again" : "Sign in"}
          </Button>
          {phase === "failed" ? <span className="truncate text-2xs text-destructive">{session.signIn.message}</span> : null}
        </div>
      )}
    </div>
  );
}

function ResizeHandle() {
  const setWidth = useUiStore((s) => s.setSidebarWidth);
  const start = useRef<{ x: number; width: number } | null>(null);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
    start.current = { x: e.clientX, width: useUiStore.getState().sidebarWidth };
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (start.current) setWidth(start.current.width + e.clientX - start.current.x);
  };
  const onPointerUp = () => {
    start.current = null;
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize sidebar"
      tabIndex={0}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onDoubleClick={() => setWidth(SIDEBAR_DEFAULT)}
      onKeyDown={(e) => {
        const w = useUiStore.getState().sidebarWidth;
        if (e.key === "ArrowLeft") setWidth(w - 16);
        if (e.key === "ArrowRight") setWidth(w + 16);
      }}
      className="app-no-drag absolute top-0 -right-1 z-10 h-full w-2 cursor-col-resize outline-none after:absolute after:inset-y-0 after:left-1/2 after:w-px after:bg-transparent after:transition-colors hover:after:bg-brand/60 focus-visible:after:bg-brand"
    />
  );
}
