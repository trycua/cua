// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";
import { useEffect, useRef, useState, type PointerEvent, type ReactNode } from "react";

import { Loader2Icon, TriangleAlertIcon } from "lucide-react";

import type { SignInFooter, Space } from "@/bridge";
import { spokenName } from "@/bridge/space-names";

import { signInFooter, useBridge, useFeatureRecorded, useMachines, useNotifications, useSession, useSpaces } from "@/bridge";
import { CuaKey } from "@/components/cua-key";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { bindingFor } from "@/lib/keybindings";
import { useListedSpaces } from "@/components/spaces/listed";
import { spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";
import { SIDEBAR_DEFAULT, useUiStore } from "@/stores/ui";
import { Shortcut } from "@/components/ui/kbd";
import { SignInLink } from "@/components/sign-in-link";
import { useNavItems } from "./nav";
import { SidebarUpdateButton } from "./update-indicator";
import { SidebarTip } from "@/components/tips";
import { useMachinesAccount } from "@/components/machines/account-parts";
import { AccountLine, ProBanner } from "./account-area";

export function Sidebar() {
  const width = useUiStore((s) => s.sidebarWidth);
  const { data: spaces, listStatus } = useSpaces();
  const { data: machines } = useMachines();
  const notifications = useNotifications();
  // No count before the list was read (not "0" while the daemon starts); while computers still answer, the
  // Spaces known so far.
  const { counted } = useListedSpaces(spaces);
  const spacesCount = spaces && listStatus.loaded ? counted.length : undefined;
  // Machines waiting to be added show beside the item, so they are seen from every page.
  const { pending } = useMachinesAccount();
  const counts: Partial<Record<string, number>> = { "/spaces": spacesCount, "/machines": machines?.length };
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
              className="group flex h-7 items-center gap-2.5 rounded-lg px-2 text-[13px] outline-none hover:bg-foreground/5 focus-visible:ring-2 focus-visible:ring-ring/60 data-[status=active]:bg-foreground/[0.07] data-[status=active]:font-medium data-[status=active]:text-foreground dark:data-[status=active]:bg-white/[0.08]"
            >
              {({ isActive }) => (
                <>
                  <item.icon className={cn("size-4 text-muted-foreground", isActive && "text-brand-strong")} strokeWidth={1.75} />
                  <span className="flex-1 truncate">{item.label}</span>
                  {item.to === "/notifications" && unread > 0 ? (
                    // The Cua key says they're Cua's; the count, in plain text, says how many are unread.
                    <span data-unread-badge className="flex items-center gap-1" aria-label={`${unread} unread`}>
                      <CuaKey size={16} />
                      <span className="text-2xs font-medium tabular-nums text-foreground">{unread}</span>
                    </span>
                  ) : null}
                  {item.to === "/machines" && pending.length > 0 ? (
                    <span data-pending-badge className="rounded-full bg-warning/20 px-1.5 text-2xs font-medium tabular-nums text-foreground" aria-label={`${pending.length} waiting to be added`}>
                      {pending.length}
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
      {/* A spacer, not a wrapper pushed down: what appears at the foot (a tip, the account) grows
          upward without moving anything already drawn (CUA-1951). */}
      <div className="flex-1" />
      <SidebarCreating spaces={spaces ?? []} />
      <SidebarTip />
      {/* Free only: "Run more Spaces at once with Pro", right above the account line. */}
      <ProBanner />
      {/* A ready update is a compact button at the end of the account line. */}
      <AccountFooter trailing={<SidebarUpdateButton />} />
      <ResizeHandle />
    </aside>
  );
}

/** Spaces being made, wherever the user is (a create started in the first
 * run keeps going after it): the name, the progress line and a bar. Opens
 * the Space's row. */
function SidebarCreating({ spaces }: { spaces: Space[] }) {
  const making = spaces.filter((s) => spaceState(s) === "creating");
  if (making.length === 0) return null;
  return (
    <div className="flex flex-col gap-1.5 px-2.5 pb-2" data-sidebar-creating="">
      {making.slice(0, 2).map((s) => {
        const p = s.progress;
        const fraction = p ? Math.max(0, Math.min(1, p.permille / 1000)) : 0;
        return (
          <Link
            key={s.id}
            to="/spaces"
            className="block rounded-lg border bg-background/70 px-3 py-2 text-left outline-none hover:bg-background focus-visible:ring-2 focus-visible:ring-ring/60"
            data-sidebar-creating-row={s.id}
          >
            <div className="text-2xs font-medium text-muted-foreground">Making</div>
            <div className="truncate text-xs font-medium">{spokenName(s.name)}</div>
            <div className="truncate text-2xs text-muted-foreground">{p?.transfer ?? p?.label ?? "Starting…"}</div>
            <div className="mt-1.5 h-1 overflow-hidden rounded-full bg-foreground/10" role="progressbar" aria-label={`Making ${s.name}`} aria-valuenow={Math.round(fraction * 100)} aria-valuemin={0} aria-valuemax={100}>
              <div className="h-full rounded-full bg-brand transition-[width]" style={{ width: `${Math.round(fraction * 100)}%` }} />
            </div>
          </Link>
        );
      })}
    </div>
  );
}

/** The account and its sign-in, every state in one place (the core's
 * `signIn.footer`): the account line (`AccountLine`), waiting for the browser with the code,
 * Open again and Cancel, or why it failed with Try again and a toast. And a
 * way back into a first run left halfway. `trailing` (the update button)
 * sits at the far right of its first row. */
export function AccountFooter({ trailing }: { trailing?: ReactNode }) {
  const { core } = useBridge();
  const { data: session, signIn, cancelSignIn, openExternal } = useSession();
  const [details, setDetails] = useState(false);
  const footer = session
    ? signInFooter(core, {
        identity: session.signedIn ? (session.identity ?? "") : null,
        cloudConfigured: session.account.configured,
        canSignIn: true,
        signIn: session.signIn,
        hasUrl: Boolean(session.signInUrl),
      })
    : null;
  useSignInFailureToast(footer);
  // A browser sign-in that has not come back for a while: offer a code.
  const waitingOnBrowser = footer?.state === "waiting" && !footer.code;
  const stuck = useAfter(waitingOnBrowser, STILL_WAITING_MS);
  useFeatureRecorded("sign_in_still_waiting", stuck);
  // The last attempt used a code: another code would not help.
  const triedCode = useRef(false);
  if (footer?.state === "waiting" && footer.code) triedCode.current = true;
  if (!session || !footer) return <div className="mt-auto flex h-12 items-center justify-end border-t px-3">{trailing}</div>;

  const unfinished = !session.onboarding.completed;
  // A failure is shown here with its reason (and the toast): nothing to add.
  const startSignIn = () => {
    setDetails(false);
    triedCode.current = false;
    void signIn().catch(() => {});
  };
  // For when the browser never gets back to the app (another browser, a
  // blocked connection): a code to type on any device.
  const startWithCode = () => {
    setDetails(false);
    void signIn({ code: true }).catch(() => {});
  };
  const identity = footer.identity || (session.signedIn ? "Signed in" : null);

  return (
    <div className={cn("mt-auto", footer.state === "signedIn" ? "px-2.5 pt-1 pb-2.5" : "border-t px-3 py-2.5")} data-signin={footer.state}>
      {unfinished ? (
        <Link to="/onboarding" className="mb-2 block text-xs text-muted-foreground hover:text-foreground">
          Finish setting up
        </Link>
      ) : null}
      {footer.state === "signedIn" && identity ? (
        // One line: the initial and the name (or email); its menu has Settings, Plan and Sign out.
        <div className="flex items-center gap-1">
          <div className="min-w-0 flex-1">
            <AccountLine identity={identity} />
          </div>
          {trailing}
        </div>
      ) : footer.state === "starting" || footer.state === "waiting" ? (
        // The code to confirm, the page to open again if the tab was
        // closed, and a way out (it also ends by itself, with a reason).
        <div>
          <p className="flex items-center gap-2 text-xs" role="status">
            <Loader2Icon className="size-3.5 shrink-0 animate-spin text-muted-foreground" aria-hidden />
            <span className="min-w-0 flex-1 truncate">{footer.text}</span>
            <span className="shrink-0">{trailing}</span>
          </p>
          <div className="mt-1 flex flex-wrap items-center gap-x-1 pl-5.5 text-2xs text-muted-foreground">
            {footer.code ? (
              <>
                <span className="select-text">
                  {footer.codeLabel} <span className="font-mono font-semibold text-foreground">{footer.code}</span>
                </span>
                <span aria-hidden>·</span>
              </>
            ) : null}
            {footer.reopenLabel && session.signInUrl ? (
              <>
                <button
                  type="button"
                  className="underline underline-offset-2 hover:text-foreground"
                  onClick={() => void openExternal(session.signInUrl!).catch(() => {})}
                  data-signin-reopen=""
                >
                  {footer.reopenLabel}
                </button>
                <span aria-hidden>·</span>
              </>
            ) : null}
            {footer.cancelLabel ? (
              <button type="button" className="underline underline-offset-2 hover:text-foreground" onClick={() => cancelSignIn()} data-signin-cancel="">
                {footer.cancelLabel}
              </button>
            ) : null}
          </div>
          {stuck ? (
            <div className="mt-1.5 pl-5.5 text-2xs text-muted-foreground" data-signin-stuck="">
              <p>Still waiting? If the page won&apos;t finish, get a code and sign in from any browser or phone.</p>
              <button type="button" className="mt-0.5 underline underline-offset-2 hover:text-foreground" onClick={startWithCode} data-signin-code="">
                Get a code instead
              </button>
            </div>
          ) : null}
          {/* No browser opened: the page, to copy to any device. */}
          {session.signIn.kind === "waiting" && session.signIn.opened === false && session.signInUrl ? (
            <SignInLink url={session.signInUrl} compact />
          ) : null}
        </div>
      ) : footer.state === "failed" ? (
        <div role="alert">
          <p className="flex items-start gap-2 text-xs">
            <TriangleAlertIcon className="mt-px size-3.5 shrink-0 text-destructive" aria-hidden />
            <span data-signin-reason={footer.reason ?? "unknown"}>{footer.text}</span>
          </p>
          <div className="mt-2 flex items-center gap-3 pl-5.5">
            <Button size="sm" onClick={startSignIn} data-signin-retry="">
              {footer.actionLabel}
            </Button>
            {footer.reason === "notConfirmed" && !triedCode.current ? (
              <button
                type="button"
                className="text-2xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
                onClick={startWithCode}
                data-signin-code=""
              >
                Get a code instead
              </button>
            ) : null}
            {footer.detail ? (
              <button
                type="button"
                className="text-2xs text-muted-foreground underline underline-offset-2 hover:text-foreground"
                aria-expanded={details}
                onClick={() => setDetails((d) => !d)}
                data-signin-details=""
              >
                {footer.detailsLabel}
              </button>
            ) : null}
            <span className="ml-auto">{trailing}</span>
          </div>
          {details && footer.detail ? (
            <pre className="mt-2 max-h-28 overflow-auto rounded-md bg-muted px-2 py-1.5 font-mono text-2xs break-words whitespace-pre-wrap text-muted-foreground select-text">
              {footer.detail}
            </pre>
          ) : null}
        </div>
      ) : (
        <div className="flex items-center justify-between gap-2">
          <span className="truncate text-xs text-muted-foreground">{footer.text}</span>
          <div className="flex shrink-0 items-center gap-1.5">
            {footer.actionLabel ? (
              <Button variant="outline" size="sm" onClick={startSignIn}>
                {footer.actionLabel}
              </Button>
            ) : null}
            {trailing}
          </div>
        </div>
      )}
    </div>
  );
}

/** A failed sign-in also toasts, once per failure, so it is seen wherever
 * the person is looking. */
/** How long a browser sign-in waits before the page offers a code. */
const STILL_WAITING_MS = 45_000;

/** True once `active` has stayed true for `ms`; false again when it stops. */
function useAfter(active: boolean, ms: number): boolean {
  const [late, setLate] = useState(false);
  useEffect(() => {
    if (!active) {
      setLate(false);
      return;
    }
    const t = setTimeout(() => setLate(true), ms);
    return () => clearTimeout(t);
  }, [active, ms]);
  return late;
}

function useSignInFailureToast(footer: SignInFooter | null) {
  const shown = useRef<string | null>(null);
  const key = footer?.state === "failed" ? `${footer.reason}:${footer.text}:${footer.detail ?? ""}` : null;
  useEffect(() => {
    if (!key || !footer?.toast) {
      shown.current = key;
      return;
    }
    if (shown.current === key) return;
    shown.current = key;
    toast(footer.toast.title, { description: footer.toast.body ?? undefined, type: "error", group: "sign-in" });
  }, [key, footer]);
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
