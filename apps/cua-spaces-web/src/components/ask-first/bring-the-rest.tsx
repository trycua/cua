// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Now bring the rest in" on Home (CUA-3150, idea C): the extra setup (a teammate, another computer, letting agents
 * use apps on this Mac, Cua from Terminal) is asked for only once the first job is done, never during setup. Setup
 * leaves it `waiting`; the first finished job shows it; "Not now" puts it away for good.
 */

import { CheckIcon, ChevronRightIcon, MonitorIcon, ShieldIcon, TerminalIcon, UserPlusIcon } from "lucide-react";
import { useContext, useEffect, useRef, useState, type ReactNode } from "react";

import { cliMissing, firstRunCliView, useAgents, useCliInstall, useSession, useSettings, type AgentRunSummary } from "@/bridge";
import { BridgeContext } from "@/bridge/BridgeProvider";
import { readAskedAt, useAppPermissions } from "@/bridge/app-permissions";
import { useFirstRunSaved, writeFirstRun } from "@/bridge/first-run";
import { hostOs } from "@/bridge/host-os";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { PermissionStatus, type PermissionsView } from "@/components/app-permissions";
import { Button } from "@/components/ui/button";
import { GroupedSection } from "@/components/ui/grouped";
import { toastError } from "@/components/ui/toast";
import { useT } from "@/i18n";
import { allowHint, homeAppsRowLine, homeAppsRowShown, permissionWords } from "@/lib/app-permissions";
import { taskState, useAskTasks, type AskTask } from "@/lib/ask-first";
import { bringRestView, firstJobDone, placeName, type BringRestView } from "@/lib/first-run-new";

type RowId = BringRestView["rows"][number]["id"];
const ICON = { invite: UserPlusIcon, "add-computer": MonitorIcon, apps: ShieldIcon, cli: TerminalIcon } as const;

/**
 * The card itself, drawn from its view (tests draw every state). `apps` and `cli` are the rows' opened panels. `done`:
 * rows whose setup is finished (both permissions on), which show a check and "On" in place of nothing.
 */
export function BringTheRestCard({
  view,
  onRow,
  onDismiss,
  open,
  apps,
  cli,
  done,
}: {
  view: BringRestView;
  onRow: (id: RowId) => void;
  onDismiss: () => void;
  open?: RowId | null;
  apps?: ReactNode;
  cli?: ReactNode;
  done?: Partial<Record<RowId, boolean>>;
}) {
  const t = useT();
  return (
    <section aria-labelledby="bring-rest-heading" data-bring-rest="">
      <div className="mb-3 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 id="bring-rest-heading" className="text-[17px] font-semibold">
            {view.title}
          </h2>
          <p className="mt-1 text-[13px] text-muted-foreground">{view.line}</p>
        </div>
        <Button variant="ghost" size="sm" className="shrink-0" onClick={onDismiss} data-bring-rest-dismiss="">
          {view.dismiss}
        </Button>
      </div>
      <GroupedSection>
        {view.rows.map((r) => {
          const Icon = ICON[r.id];
          const expands = r.id === "apps" || r.id === "cli";
          return (
            <div key={r.id}>
              <button
                type="button"
                className="flex w-full items-center gap-3 px-4 py-3 text-left outline-none hover:bg-accent/50 focus-visible:bg-accent/50"
                onClick={() => onRow(r.id)}
                aria-expanded={expands ? open === r.id : undefined}
                data-bring-rest-row={r.id}
              >
                <Icon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
                <span className="min-w-0 flex-1">
                  <span className="block text-[13px] font-medium">{r.title}</span>
                  <span className="block text-xs text-muted-foreground">{r.sub}</span>
                </span>
                {done?.[r.id] ? (
                  <span className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground" data-bring-rest-done={r.id}>
                    <CheckIcon aria-hidden className="size-3.5" /> {t("home.bring_rest.on")}
                  </span>
                ) : null}
                <ChevronRightIcon
                  aria-hidden
                  className={expands && open === r.id ? "size-4 shrink-0 rotate-90 text-muted-foreground" : "size-4 shrink-0 text-muted-foreground"}
                />
              </button>
              {open === r.id ? (r.id === "apps" ? apps : r.id === "cli" ? cli : null) : null}
            </div>
          );
        })}
      </GroupedSection>
    </section>
  );
}

/** The apps row, opened: this Mac's two permissions for Cua Spaces, each with its live status and Allow. */
export function AppsPanel({ perms }: { perms: PermissionsView }) {
  const t = useT();
  const data = perms.data;
  if (!data?.permissions.length) return null;
  const waiting = Boolean(perms.waiting);
  return (
    <div className="px-4 pb-3 pl-11" data-bring-rest-apps="">
      {data.permissions.map((p) => {
        const words = permissionWords(p.id, data.appName);
        return (
          <div key={p.id} className="flex min-h-10 items-center gap-3" data-app-permission={p.id} data-granted={p.granted ? "" : undefined}>
            <span className="min-w-0 flex-1">
              <span className="block text-[13px]">{words.title}</span>
              <span className="block text-xs text-muted-foreground">{words.sub}</span>
            </span>
            <PermissionStatus permission={p} waiting={waiting} className="text-xs text-muted-foreground" />
            {!p.granted && p.settingsUrl ? (
              <Button variant="outline" size="sm" onClick={() => void perms.allow(p.id).catch(toastError(t("home.bring_rest.open_settings_failed")))}>
                {t("home.bring_rest.allow")}
              </Button>
            ) : null}
          </div>
        );
      })}
      <p className="mt-1 text-xs text-muted-foreground">
        {allowHint(
          data.appName,
          data.permissions.every((p) => p.granted),
        )}
      </p>
    </div>
  );
}

/** The Terminal row, opened: where the command goes and Install, or the line to try once it's there. */
export function CliPanel({
  view,
  onInstall,
}: {
  view: { status: string; installed: boolean; busy: boolean; tryCommand: string | null; error: string | null; primary: { id: string; label: string } | null };
  onInstall: () => void;
}) {
  const t = useT();
  const install = view.primary?.id === "install" ? view.primary : null;
  return (
    <div className="px-4 pb-3 pl-11 text-xs text-muted-foreground" data-bring-rest-cli={view.installed ? "installed" : "missing"}>
      <p>{view.status}</p>
      {view.error ? <p className="mt-1 text-destructive">{view.error}</p> : null}
      {install ? (
        <Button variant="outline" size="sm" className="mt-2" disabled={view.busy} onClick={onInstall} data-bring-rest-cli-install="">
          {view.busy ? t("home.bring_rest.installing") : install.label}
        </Button>
      ) : view.tryCommand ? (
        <code className="mt-2 block font-mono text-foreground select-text" data-bring-rest-cli-try="">
          {view.tryCommand}
        </code>
      ) : null}
    </div>
  );
}

/** Home's state for one task, from its run when the run is listed. */
function stateOf(task: AskTask, runs: readonly AgentRunSummary[] | undefined): string | null {
  const run = task.runId ? runs?.find((r) => r.runId === task.runId && r.spaceId === task.spaceId) : undefined;
  return taskState(task, run ? { status: run.status ?? null, needsYou: Boolean(run.needsYou), lastEventMs: null } : null);
}

/** Watches for the first finished job after setup, then shows the card from then on (until "Not now"). */
function WaitForFirstJob({ sinceMs }: { sinceMs: number }) {
  const tasks = useAskTasks();
  const { data: agents } = useAgents();
  const done = firstJobDone(
    {
      tasks: tasks.map((task) => ({ state: stateOf(task, agents?.runs), ending: task.ending })),
      runs: (agents?.runs ?? []).map((r) => ({ startedMs: r.startedMs, status: r.status ?? null })),
    },
    sinceMs,
  );
  useEffect(() => {
    if (done) writeFirstRun({ restOffer: "shown" });
  }, [done]);
  return null;
}

function Shown() {
  const ctx = useContext(BridgeContext);
  const core = ctx?.core ?? null;
  const session = useSession();
  const settings = useSettings();
  const os = hostOs();
  // Back from "Quit & Reopen" (or any relaunch) shortly after Allow: the apps row opens again, so its rows show the
  // switches being checked and then On, not a closed row with no news.
  const [open, setOpen] = useState<RowId | null>(() => (os === "macos" && readAskedAt() != null ? "apps" : null));
  const [addOpen, setAddOpen] = useState(false);
  const perms = useAppPermissions(os === "macos");
  // Any launch with a permission missing: the apps row opens by itself too, unless the person has opened or closed a
  // row already.
  const touched = useRef(false);
  const missing = perms.available && !perms.allGranted;
  useEffect(() => {
    if (missing && !touched.current) setOpen("apps");
  }, [missing]);
  const cli = useCliInstall(true);
  const cliName = settings.data?.cli?.name ?? null;
  const cliView = core && (cliMissing(cli.state) || cli.state.installed) ? firstRunCliView(core, { cli: cli.state, cliName }) : null;
  const view = bringRestView({ os, apps: os === "macos" && (perms.loading || perms.available), cli: Boolean(cliView) });
  const accountUrl = settings.data?.accountUrl ?? "https://app.cua.ai";
  return (
    <>
      <BringTheRestCard
        view={view}
        open={open}
        onRow={(id) => {
          if (id === "invite") void session.openExternal(`${accountUrl}/team`).catch(() => {});
          else if (id === "add-computer") setAddOpen(true);
          else {
            touched.current = true;
            setOpen((o) => (o === id ? null : id));
          }
        }}
        onDismiss={() => writeFirstRun({ restOffer: "dismissed" })}
        apps={<AppsPanel perms={perms} />}
        done={{ apps: perms.allGranted }}
        cli={cliView ? <CliPanel view={cliView} onInstall={() => void cli.install()} /> : null}
      />
      <AddComputerSheet open={addOpen} onOpenChange={setAddOpen} />
    </>
  );
}

/**
 * Home's apps row on its own (CUA-3551), drawn from `useAppPermissions`: the row's title, how many switches are still
 * off, and the two permissions with their live status and Allow. Tests draw every state.
 */
export function AppsOnThisMacCard({ perms, os }: { perms: PermissionsView & { allGranted: boolean }; os: string }) {
  const t = useT();
  const permissions = perms.data?.permissions ?? [];
  return (
    <section aria-labelledby="home-apps-heading" data-home-apps="">
      <GroupedSection>
        <div>
          <div className="flex items-center gap-3 px-4 py-3">
            <ShieldIcon aria-hidden className="size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1">
              <span id="home-apps-heading" className="block text-[13px] font-medium">
                {t("home.bring_rest.apps_title", { place: placeName(os) })}
              </span>
              <span className="block text-xs text-muted-foreground">{homeAppsRowLine(permissions)}</span>
            </span>
            {perms.allGranted ? (
              <span className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground" data-home-apps-done="">
                <CheckIcon aria-hidden className="size-3.5" /> {t("home.bring_rest.on")}
              </span>
            ) : null}
          </div>
          <AppsPanel perms={perms} />
        </div>
      </GroupedSection>
    </section>
  );
}

/**
 * The apps row when "Now bring the rest in" isn't showing (before the first job, after "Not now", or setup finished
 * elsewhere): on a Mac, at every launch with a permission missing, each switch live. Once shown it stays until Home is
 * left, so the switches turn to On in place.
 */
export function AppsOnThisMac() {
  const os = hostOs();
  const perms = useAppPermissions(os === "macos");
  const [shownBefore, setShownBefore] = useState(false);
  const show = homeAppsRowShown({ os, available: perms.available, allGranted: perms.allGranted, shownBefore });
  useEffect(() => {
    if (show) setShownBefore(true);
  }, [show]);
  return show ? <AppsOnThisMacCard perms={perms} os={os} /> : null;
}

/**
 * Home's "Now bring the rest in": nothing until setup ended here and the first job is done; gone after "Not now".
 * Whenever it isn't showing, a Mac with a permission missing still gets the apps row on its own.
 */
export function BringTheRest() {
  const saved = useFirstRunSaved();
  if (saved.restOffer === "shown") return <Shown />;
  return (
    <>
      {saved.restOffer === "waiting" ? <WaitForFirstJob sinceMs={saved.restSinceMs ?? 0} /> : null}
      <AppsOnThisMac />
    </>
  );
}
