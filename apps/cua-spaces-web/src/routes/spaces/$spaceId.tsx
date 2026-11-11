// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { ArrowDownCircleIcon, MonitorIcon, PictureInPicture2Icon, PlayIcon, SquareIcon, Trash2Icon, UserPlusIcon } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import {
  hostResources,
  parseDropped,
  runCreateFix,
  spaceResources,
  useBridge,
  useResources,
  useDevices,
  useMachines,
  useSession,
  useShareSheet,
  useSpaceDetail,
  useSpaceFiles,
  useSpaces,
  useSpaceThread,
  useTeleport,
  type CreateReport,
  type Machine,
  type ResourcesSnapshot,
  type Space,
  type StreamRow,
  type ThreadView,
} from "@/bridge";
import { DiagnosticReportDialog } from "@/components/diagnostic-report-dialog";
import { OsIcon } from "@/components/os-icon";
import { BackLink } from "@/components/back-link";
import { EmptyState, Page } from "@/components/page";
import { useChildren } from "@/components/children/use-children";
import { parentIdOf } from "@/lib/space-tree";
import { ChildSpacesSection, childrenEffect, childrenOfSpace, descendantCount } from "@/components/space-detail/child-spaces-section";
import { AgentsSection } from "@/components/space-detail/agents-section";
import { DropWell } from "@/components/space-detail/drop-well";
import { FactList } from "@/components/space-detail/facts";
import { SpaceResourcesPanel } from "@/components/resources/space-resources-panel";
import { ownerLabel } from "@/lib/space-access";
import { systemName } from "@/lib/resources";
import { StreamSection } from "@/components/space-detail/stream-section";
import { StreamSurface } from "@/components/space-detail/stream-surface";
import { SpaceHeader } from "@/components/space-detail/space-header";
import { SpaceCall } from "@/components/space-thread/space-call";
import { ThisMacCall } from "@/components/space-detail/this-mac-desktop";
import { useComputerActions } from "@/components/computers/computer-actions";
import { StateLabel } from "@/components/state-dot";
import { PageTip } from "@/components/tips";
import { creatingThread } from "@/components/space-thread/queued";
import { PreparePanel } from "@/components/prepare/prepare-panel";
import { usePrepare } from "@/lib/prepare/store";
import { usePrepareAction } from "@/components/prepare/use-prepare-action";
import { useAskTasks } from "@/lib/ask-first";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { t as tr, useT } from "@/i18n";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { thisComputerLabel, thisComputerText } from "@/lib/host-labels";
import { THIS_COMPUTER } from "@/lib/machines";
import { useSpaceVolumeError } from "@/lib/space-volume";
import { volumeUnavailableNote } from "@/lib/volume-notes";
import {
  canDelete,
  canPower,
  createError,
  isUnsharedDesktop,
  createProgress,
  machineName,
  powerLabel,
  powerPending,
  spacePlace,
  spaceState,
} from "@/lib/spaces";
import { useSpaceLayout } from "@/components/space-thread/layout-mode";
import { NetworkSheet } from "@/components/network/network-sheet";
import {
  BlockedNotice,
  ExitDownNotice,
  NetworkNotice,
  NetworkRow,
  ParentLine,
  RunsOnMachinesRow,
  SecurityRows,
  WithClipboardPrompt,
} from "@/components/network/space-security";
import { VolumeGrantDialog } from "@/components/network/volume-grant-dialog";

export const Route = createFileRoute("/spaces/$spaceId")({
  component: SpaceDetailPage,
  // Apps and files dropped on this Space's tile in the notch, by name (the host holds their paths).
  validateSearch: (s: Record<string, unknown>): { dropped?: string[] } => {
    const dropped = parseDropped(s.dropped);
    return dropped ? { dropped } : {};
  },
});

type Confirm = "stop" | "delete" | null;

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

function SpaceDetailPage() {
  const t = useT();
  const { spaceId } = Route.useParams();
  const { dropped } = Route.useSearch();
  const navigate = useNavigate();
  const {
    data: spaces,
    isLoading,
    error,
    startSpace,
    stopSpace,
    deleteSpace,
    openSpace,
    dismissCreate,
    retryCreate,
    canRetryCreate,
    createdId,
    createReport,
    sendCreateReport,
  } = useSpaces();
  // The failed create's diagnostic report, while its review sheet is open.
  const [report, setReport] = useState<CreateReport | null>(null);
  const [reportOpen, setReportOpen] = useState(false);
  const { data: machines = [] } = useMachines();
  const resources = useResources();
  const [confirm, setConfirm] = useState<Confirm>(null);
  // The window the stage shows instead of the desktop (the Stream rail's pick), for this Space only.
  const [shown, setShown] = useState<{ spaceId: string; row: string } | null>(null);
  const teleport = useTeleport();
  const sharing = useShareSheet();
  const { data: session, openExternal } = useSession();
  const { core } = useBridge();
  const space = spaces?.find((s) => s.id === spaceId);
  // A child Space belongs to its parent's tree: Back goes to the parent's page, and the header says whose child it is.
  const tree = useChildren();
  const parentId = space ? parentIdOf(space, tree) : undefined;
  const parentSpace = parentId ? spaces?.find((s) => s.id === parentId) : undefined;
  const parentRef = parentSpace ? { id: parentSpace.id, name: parentSpace.name } : null;
  // What stopping or deleting this Space does to the Spaces made under it, said in the confirm.
  const myKids = space ? childrenOfSpace(spaces ?? [], space.id, tree) : [];
  const myTotal = space ? descendantCount(spaces ?? [], space.id, tree) : 0;
  // Apps and files dropped on this Space's notch tile: the same as a drop on the well (an app opens Teleport at it), once.
  const files = useSpaceFiles(space ?? { id: spaceId, name: spaceId });
  const droppedOnce = useRef<string[] | null>(null);
  useEffect(() => {
    if (!dropped || !space || droppedOnce.current === dropped) return;
    droppedOnce.current = dropped;
    void files.drop(dropped);
    // Off the address, so a reload does not teleport again.
    void navigate({ to: "/spaces/$spaceId", params: { spaceId }, search: {}, replace: true });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dropped, space?.id]);
  const thisMachine = machines.find((m) => m.current);
  const hostArch = thisMachine?.arch ?? null;
  // As this device sees it: a machine on the relay greyed out while this
  // device is not enrolled.
  const access = thisMachine?.accessNotice ?? null;
  const devices = useDevices();
  const { detail, stream, windowsUnsupported, windowsLoaded, pip, query, setQuery, agents, copy } = useSpaceDetail(space, hostArch, { access });
  // A computer's own desktop page also manages the computer: its page, and Remove.
  const computer = useComputerActions(space?.id ?? spaceId);
  // Call view or thread view, remembered per Space.
  const [layout, setLayout] = useSpaceLayout(space?.id ?? "");
  const volumeError = useSpaceVolumeError(space, space ? spaceState(space) === "running" : false);
  // The Space's thread (CUA-2155). A daemon without it answers unsupported,
  // and the page keeps its classic layout; a create still in progress has none yet.
  const thread = useSpaceThread(space && !space.id.startsWith("pending:") ? space.id : null);
  // Ask first's tasks: a Space being made for one draws as the Space page in its creating state.
  const tasks = useAskTasks();
  // The first Space being prepared while the person was in onboarding: its page shows what is happening.
  const prep = usePrepare();
  const onPrepareAction = usePrepareAction();
  // Open on a creating row: once the create finished and the registry lists
  // the Space, its row (and this id) goes; follow it to the Space's own id.
  // Only a page that showed the row follows: the address of a row that is
  // already gone (Back after Remove from list) says so, and never jumps into
  // another Space.
  const [showed, setShowed] = useState<string | null>(null);
  // Who shared it, when it was shared with you: if the row goes while this page is open,
  // the owner most likely stopped sharing it, and the page says so.
  const [sharedBy, setSharedBy] = useState<{ spaceId: string; owner: string | null } | null>(null);
  useEffect(() => {
    if (!space) return;
    setShowed(spaceId);
    const a = space.access;
    setSharedBy(a && a.kind !== "mine" ? { spaceId, owner: ownerLabel(a) } : null);
  }, [space, spaceId]);
  const created = !space && showed === spaceId ? createdId(spaceId) : undefined;
  useEffect(() => {
    if (created) void navigate({ to: "/spaces/$spaceId", params: { spaceId: created }, replace: true });
  }, [created, navigate]);
  if (created) return null;

  const removedAccess = !space && !error && sharedBy?.spaceId === spaceId ? sharedBy : null;
  if (error || (!isLoading && !space)) {
    return (
      <Page>
        <BackLink />
        <EmptyState
          icon={<MonitorIcon />}
          title={error ? t("computers.page.space_load_failed") : removedAccess ? t("computers.page.space_no_access") : t("computers.page.space_missing")}
          action={
            <Button variant="outline" render={<Link to="/spaces" />}>
              {t("computers.page.back_to_spaces")}
            </Button>
          }
        >
          {error
            ? error.message
            : removedAccess
              ? t("computers.page.space_unshared", { owner: removedAccess.owner ?? t("computers.page.the_owner") })
              : t("computers.page.space_missing_body")}
        </EmptyState>
      </Page>
    );
  }
  if (!space || !detail) return null;

  const state = spaceState(space);
  const machine = machineName(machines, space);
  const pending = powerPending(space);
  // "Turn on" / "Turn off" (or Resume / Suspend), the SwiftUI app's words.
  const turnOn = powerLabel(core, space, true);
  const turnOff = powerLabel(core, space, false);

  const run = (failed: string, action: () => Promise<void>) =>
    action().catch((e: unknown) => toast(t(failed, { name: space.name }), { description: message(e), type: "error" }));

  const start = () => run("computers.tile.start_failed", () => startSpace(space.id));
  // The core's detail actions: present only where they apply (Share follows
  // Settings, Experiments), enabled only when the Space can stream and this
  // device may connect.
  const action = (id: string) => detail.actions.find((a) => a.id === id);
  const teleportAction = action("teleport");
  // A teammate who may only watch is not offered Share or control (the host refuses both anyway).
  const viewOnly = space.access?.role === "viewer";
  const sharedOwner = space.access && space.access.kind === "withMe" ? { name: ownerLabel(space.access) ?? "", email: space.access.ownerEmail } : null;
  const mayShare = space.access?.canShare !== false && !viewOnly;
  const shareAction = mayShare ? action("share") : undefined;
  // Signed in, not enrolled: the access line's action opens this computer's page with the enroll sheet.
  const enroll = () => {
    devices.startEnroll();
    void navigate({ to: "/spaces/computer/$computerId", params: { computerId: THIS_COMPUTER } });
  };
  const teleportApp = () => run("computers.page.teleport_failed", () => teleport.open(space));
  const share = () => run("computers.page.share_failed", () => sharing.open(space, session?.signedIn ?? false));
  const stop = () => run("computers.tile.stop_failed", () => stopSpace(space.id));
  // Leaving a Space that is going away: the grid takes this page's place in
  // the history, so Back never returns to a page for a Space that is gone.
  const leave = () => navigate({ to: "/spaces", replace: true });
  // Delete, or "Remove from List" (a Space in your cloud: forget it here,
  // keep it running there; `AppModel.delete(_:removeOnly:)`).
  const remove = (removeOnly = false) => {
    void leave();
    void run(removeOnly ? "computers.page.remove_failed" : "computers.page.delete_failed", async () => {
      await deleteSpace(space.id, removeOnly);
      toast(removeOnly ? t("computers.page.removed_from_list", { name: space.name }) : t("computers.page.deleted", { name: space.name }), { type: "success" });
    });
  };
  // A failed create only leaves the list (here and on the host). The grid
  // opens first, then the row goes: this page never shows "isn't here" in
  // between, and nothing else decides where the person lands.
  const dismiss = () => {
    void leave().then(() => run("computers.page.remove_failed", () => dismissCreate(space.id)));
  };
  // Try again: the same create as a new row; this page follows it.
  const retry = canRetryCreate(space.id)
    ? () => {
        const next = retryCreate(space.id);
        if (next) void navigate({ to: "/spaces/$spaceId", params: { spaceId: next }, replace: true });
      }
    : undefined;
  // Send report: the review sheet with the core's redacted report; nothing
  // is sent until the person presses Send there.
  const openReport = () =>
    void createReport(space.id).then((r) => {
      if (!r) return;
      setReport(r);
      setReportOpen(true);
    });
  // It failed the same way twice: the core's fix for that cause (copy a
  // command, open the help page, or the details) instead of Try again.
  const createFix = space.progress?.fix;
  const fix = createFix
    ? () =>
        void runCreateFix(createFix, {
          copy: async (text) => {
            await navigator.clipboard?.writeText(text);
            toast(t("computers.page.copied", { text }), { description: t("computers.page.paste_in_terminal"), type: "success" });
          },
          openExternal: (url) => openExternal(url),
          details: openReport,
        }).catch((e: unknown) => toast(t("computers.page.fix_failed"), { description: message(e), type: "error" }))
    : undefined;
  // Picture in picture where the host has panels; its own window where it doesn't.
  const onPip = (row: StreamRow) =>
    run("computers.page.open_failed", async () => {
      if ((await pip(row.id)) === "open-window") {
        await openSpace(space.id);
        toast(t("computers.page.opening", { name: space.name }), { description: t("computers.page.no_pip") });
      }
    });

  const desktopRow = stream?.rows.find((r) => r.kind === "desktop");
  // The Stream rail's pick: one of the Space's windows on the stage, or the desktop (none). A window that
  // closes puts the desktop back.
  const shownRow = shown && shown.spaceId === space?.id ? stream?.rows.find((r) => r.kind === "window" && r.id === shown.row) : undefined;
  // What a post says the person is looking at: the shown window, in the core's fields (`stream.viewContext`).
  const shownView = shownRow ? threadViewOf(core.tryCall<StreamViewContext | null>("stream.viewContext", { row: shownRow }) ?? null) : null;
  const display = desktopRow?.resolution?.split("×").map(Number);
  // The desktop plays in its picture-in-picture panel: said here, with Bring
  // it back, instead of a second stream (`SpaceScreenView`'s placeholder).
  const poppedOut = desktopRow?.actions.some((a) => a.id === "pip" && a.active) ?? false;
  const poppedOutState = poppedOut ? (
    <div data-popped-out="" className="flex flex-col items-center gap-2">
      <PictureInPicture2Icon className="size-7 text-muted-foreground" />
      <p className="text-xs text-muted-foreground">{t("computers.page.floating")}</p>
      <Button variant="outline" size="sm" onClick={() => void onPip(desktopRow!)}>
        {t("computers.page.bring_back")}
      </Button>
    </div>
  ) : undefined;
  const aspect = display?.length === 2 && display[0]! > 0 && display[1]! > 0 ? display[0]! / display[1]! : undefined;
  const showStream = detail.sections.includes("Stream") && stream;
  // The core's sections after Stream: Agents (where the host lists runs)
  // and Teleport (the drop well), as the SwiftUI detail draws them.
  const showAgents = detail.sections.includes("Agents") && !agents.unsupported;
  const showTeleport = detail.sections.includes("Teleport");

  const actions = (
    <div className="flex shrink-0 items-center gap-2">
      {teleportAction ? (
        <Button variant="outline" disabled={!teleportAction.enabled} title={teleportAction.help} onClick={teleportApp}>
          <ArrowDownCircleIcon /> {teleportAction.label}
        </Button>
      ) : null}
      {shareAction ? (
        <Button variant="outline" data-share-space="" disabled={!shareAction.enabled} title={shareAction.help} onClick={share}>
          <UserPlusIcon /> {shareAction.label}
        </Button>
      ) : null}
      {canPower(space) ? (
        state === "stopped" ? (
          <Button variant="outline" disabled={Boolean(pending)} onClick={start}>
            <PlayIcon /> {turnOn}
          </Button>
        ) : (
          <Button variant="outline" disabled={Boolean(pending)} onClick={() => setConfirm("stop")}>
            <SquareIcon className="size-3.5" /> {turnOff}
          </Button>
        )
      ) : null}
      {state === "failed" ? (
        <Button variant="outline" data-dismiss-create="" onClick={dismiss}>
          <Trash2Icon className="text-destructive" /> {t("computers.account.remove")}
        </Button>
      ) : null}
      {canDelete(space) ? (
        <Button
          variant="outline"
          aria-label={t("computers.tile.power_label", { action: detail.deleteLabel, name: space.name })}
          onClick={() => setConfirm("delete")}
        >
          <Trash2Icon className="text-destructive" /> {detail.deleteLabel}
        </Button>
      ) : null}
    </div>
  );
  const place = (
    <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted-foreground">
      <span>{spacePlace(space, machine)}</span>
      {pending ? (
        <span className="text-xs">{pending}</span>
      ) : space.id === "this-mac" ? (
        <StateLabel
          state={isUnsharedDesktop(space) ? "stopped" : "running"}
          word={isUnsharedDesktop(space) ? t("computers.row.not_sharing") : t("computers.tile.sharing")}
        />
      ) : (
        <StateLabel state={state} space={space} />
      )}
      {parentRef ? null : <ParentLine spaceId={space.id} />}
    </p>
  );
  const osMark = (
    <div className="mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg border bg-card shadow-xs">
      <OsIcon os={space.os} className="size-4.5 text-muted-foreground" />
    </div>
  );
  const preparing = space.id.startsWith("pending:") && prep.state?.wanted && prep.state.pendingId === space.id ? prep.view : null;
  const preparePanel = preparing ? <PreparePanel view={preparing} onRetry={onPrepareAction} /> : null;
  const surface = (className: string, windowId?: string | null, windowName?: string | null, interactive = !viewOnly) => (
    <StreamSurface
      interactive={interactive}
      viewOnly={viewOnly}
      spaceId={space.id}
      windowId={windowId}
      windowName={windowName}
      os={space.os}
      aspectRatio={aspect}
      canStream={detail.canStream}
      previewText={detail.previewText}
      access={detail.access}
      onAccessAction={enroll}
      className={className}
    >
      {preparePanel ?? surfaceState(space, pending, turnOn, start, dismiss, retry, openReport, fix) ?? poppedOutState}
    </StreamSurface>
  );
  // The classic page's sections under the desktop. With the thread they open from Details, More.
  const sections = (
    <>
      <ExitDownNotice spaceId={space.id} />
      <SpaceResources space={space} spaces={spaces ?? []} snapshot={resources} machines={machines} />

      <div className="mt-8 grid items-start gap-x-6 first:mt-0 md:grid-cols-2">
        <div>
          <FactList title={t("computers.page.details_title")} facts={detail.facts} extra={<RunsOnMachinesRow spaceId={space.id} />} />
          <SecurityRows spaceId={space.id} className="mt-6" />
          {volumeError ? (
            <p className="mt-3 px-1 text-[13px] text-muted-foreground" data-volume-note="" title={volumeError}>
              {volumeUnavailableNote(volumeError)}
            </p>
          ) : null}
          {showTeleport ? (
            <div className="mt-7">
              <DropWell
                title={t("computers.page.teleport_title")}
                space={space}
                copy={{
                  caption: copy?.dropCaption ?? t("computers.page.drop_caption"),
                  sendFile: copy?.sendFile ?? t("computers.page.send_file"),
                  teleportApp: copy?.teleportApp ?? t("computers.page.teleport_app"),
                }}
                onTeleport={teleportApp}
              />
            </div>
          ) : null}
        </div>
        <div>
          {showStream ? (
            <StreamSection
              title={t("computers.page.stream_title")}
              section={stream}
              query={query}
              onQuery={setQuery}
              onPip={(row) => void onPip(row)}
              hideStatus={windowsUnsupported}
            />
          ) : null}
          {showAgents ? (
            <AgentsSection
              title={t("computers.page.agents_title")}
              runs={agents.runs}
              failed={agents.failed}
              copy={{
                loading: copy?.agentsLoading ?? t("computers.page.agents_loading"),
                empty: copy?.agentsEmpty ?? t("computers.page.agents_empty"),
                failed: copy?.agentsFailed ?? t("computers.page.agents_failed"),
              }}
            />
          ) : null}
        </div>
      </div>
    </>
  );
  const dialogs = (
    <>
      <ConfirmDialog
        open={confirm === "stop"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={t("computers.page.power_confirm_title", { action: turnOff, name: space.name })}
        description={
          (space.power?.control === "suspend" ? t("computers.page.suspend_body") : t("computers.page.shutdown_body")) + childrenEffect("stop", myKids, myTotal)
        }
        confirmLabel={turnOff}
        destructive={space.power?.control !== "suspend"}
        onConfirm={stop}
      />
      <ConfirmDialog
        open={confirm === "delete"}
        onOpenChange={(o) => !o && setConfirm(null)}
        title={detail.confirm.title}
        description={detail.confirm.message + childrenEffect("delete", myKids, myTotal)}
        confirmLabel={detail.confirm.confirmLabel}
        cancelLabel={detail.confirm.cancelLabel}
        disabledReason={detail.confirm.confirmEnabled ? null : (detail.confirm.disabledReason ?? null)}
        destructive
        onConfirm={() => remove()}
        alternative={detail.confirm.removeLabel ? { label: detail.confirm.removeLabel, onClick: () => remove(true) } : null}
      />
      <NetworkSheet spaceName={(id) => spaces?.find((s) => s.id === id)?.name ?? id} />
      <VolumeGrantDialog spaceId={space.id} spaceName={space.name} />
      <DiagnosticReportDialog
        open={reportOpen}
        onOpenChange={setReportOpen}
        report={report}
        onSend={(subject, parts) => sendCreateReport({ subject, parts })}
      />
    </>
  );
  const desktopPip = desktopRow?.actions.find((a) => a.id === "pip");
  const powerError = detail.powerError ? <p className="mb-4 text-[13px] text-destructive">{detail.powerError}</p> : null;
  // One tip at a time (the core's `tips.view`, place `space`).
  // A Space made for an Ask-first job is busy with its agent: no tip (it would also appear once ready and move the page).
  const askSpace = tasks.some((task) => task.pendingId === space.id || (task.inSpace && task.spaceId === space.id));
  const tip = askSpace ? null : <PageTip place="space" space={space} onShare={shareAction?.enabled ? share : undefined} />;
  // In the call layout the tip sits directly on the stage and the rail: no gap under it, so the rail meets it flush.
  const callTip = askSpace ? null : <PageTip place="space" space={space} onShare={shareAction?.enabled ? share : undefined} className="mb-0" />;

  // The call layout: the desktop on top, the thread below, the people on the
  // right. A machine's own page (no desktop to call into) and a daemon
  // without the thread keep the classic page.
  // A create still in progress has no thread yet: its progress shows on the classic page.
  // A Space being made for an Ask-first job: the same page in its creating state (progress on
  // the stage, the job queued in the thread); a failed create keeps the classic page's fixes.
  const creating =
    space.id.startsWith("pending:") &&
    (state !== "failed" || (preparing != null && preparing.phase !== "failed")) &&
    (tasks.some((task) => task.pendingId === space.id) || preparing != null);
  const callThread = creating ? creatingThread(space.id) : thread;
  const later = t("computers.page.later");
  // This Mac's own desktop row: a Space's page as any other, with the share prompt where the stream would be.
  if (space.id === "this-mac") {
    return (
      <>
        <ThisMacCall
          spaceName={space.name}
          shared={!isUnsharedDesktop(space)}
          live={thread}
          identity={session?.signedIn ? (session.account.identity ?? null) : null}
          facts={detail.facts}
          more={sections}
          header={
            <SpaceHeader
              back={<BackLink inline parent={parentRef} />}
              os={space.os}
              title={detail.title}
              place={place}
              childOf={parentRef}
              computerSettings={computer?.settings}
              removeComputer={computer?.remove}
            />
          }
        />
        {dialogs}
        {computer?.dialogs}
      </>
    );
  }
  if (!detail.desktopNote && callThread.status !== "unsupported" && (creating || !space.id.startsWith("pending:"))) {
    return (
      <>
        <SpaceCall
          spaceId={space.id}
          spaceName={space.name}
          identity={session?.signedIn ? (session.account.identity ?? null) : null}
          header={
            <SpaceHeader
              back={<BackLink inline parent={parentRef} />}
              childOf={parentRef}
              os={space.os}
              title={detail.title}
              place={place}
              power={
                creating
                  ? { state: "pending", label: t("computers.page.being_made"), onClick: () => {} }
                  : canPower(space)
                    ? pending
                      ? { state: "pending", label: pending, onClick: () => {} }
                      : state === "stopped"
                        ? { state: "off", label: turnOn, onClick: start }
                        : { state: "on", label: turnOff, onClick: () => setConfirm("stop") }
                    : undefined
              }
              pip={
                !creating && desktopPip
                  ? {
                      id: desktopRow!.id,
                      label: desktopPip.help ?? t("computers.page.pop_out"),
                      help: desktopPip.help ?? undefined,
                      enabled: true,
                      active: desktopPip.active,
                      onClick: () => void onPip(desktopRow!),
                    }
                  : undefined
              }
              teleport={
                creating
                  ? { label: t("computers.page.teleport_app_short"), help: later, enabled: false, onClick: () => {} }
                  : teleportAction
                    ? { label: teleportAction.label, help: teleportAction.help ?? undefined, enabled: teleportAction.enabled, onClick: teleportApp }
                    : undefined
              }
              share={
                creating
                  ? { label: t("computers.page.share"), help: later, enabled: false, onClick: () => {} }
                  : shareAction
                    ? { label: shareAction.label, help: shareAction.help ?? undefined, enabled: shareAction.enabled, onClick: share }
                    : undefined
              }
              remove={
                !creating && !computer && canDelete(space) ? { label: detail.deleteLabel, enabled: true, onClick: () => setConfirm("delete") } : undefined
              }
              computerSettings={computer?.settings}
              removeComputer={computer?.remove}
              dismiss={state === "failed" ? { label: t("computers.account.remove"), enabled: true, onClick: dismiss } : undefined}
            />
          }
          notice={
            <>
              {callTip}
              <NetworkNotice spaceId={space.id} />
              <BlockedNotice spaceId={space.id} />
              {detail.powerError ? <p className="px-5 pb-2 text-[13px] text-destructive">{detail.powerError}</p> : null}
            </>
          }
          stage={
            <WithClipboardPrompt spaceId={space.id} spaceName={space.name}>
              {surface("h-full", shownRow?.id ?? null, shownView?.app ?? null)}
            </WithClipboardPrompt>
          }
          layout={layout}
          onLayout={setLayout}
          smallStage={surface("h-full", shownRow?.id ?? null, shownView?.app ?? null, false)}
          aspect={aspect}
          stageLabel={shownRow?.label ?? t("computers.tile.desktop")}
          thread={callThread}
          creating={creating}
          facts={detail.facts}
          detailsExtra={<NetworkRow spaceId={space.id} />}
          childSpaces={<ChildSpacesSection spaceId={space.id} spaceName={space.name} />}
          more={sections}
          windows={creating ? [] : (stream?.rows.filter((r) => r.kind === "window") ?? [])}
          streamReady={!creating && Boolean(stream) && windowsLoaded}
          desktop={creating ? null : (desktopRow ?? null)}
          selected={shownRow?.id ?? null}
          onSelect={(row) => setShown(row.kind === "window" ? { spaceId: space.id, row: row.id } : null)}
          view={shownView}
          onStream={(row) => void onPip(row)}
          onShare={shareAction?.enabled ? share : undefined}
          owner={sharedOwner}
        />
        {dialogs}
        {computer?.dialogs}
      </>
    );
  }

  return (
    <Page>
      <BackLink />
      <header className="mb-5 flex items-start justify-between gap-4">
        <div className="flex min-w-0 items-start gap-3">
          {osMark}
          <div className="min-w-0">
            <h1 data-detail-title className="truncate text-[22px] leading-tight font-semibold tracking-[-0.01em]">
              {detail.title}
            </h1>
            {place}
          </div>
        </div>
        {actions}
      </header>

      {tip}

      {powerError}

      {detail.desktopNote ? (
        // One of your computers that keeps its desktop private: why, in place
        // of the desktop, then the Spaces it provides (no New Space button
        // beside the note).
        <MachineSpaces
          note={detail.desktopNote}
          listSpaces={Boolean(detail.newSpace)}
          spaces={hostedSpaces(spaces ?? [], space.id)}
          // This computer's own screen: its switches live on its page.
          action={
            detail.isHost ? (
              <Button
                variant="outline"
                size="sm"
                data-host-desktop-action=""
                render={<Link to="/spaces/computer/$computerId" params={{ computerId: THIS_COMPUTER }} />}
              >
                {t("computers.page.open_named", { name: thisComputerLabel() })}
              </Button>
            ) : null
          }
        />
      ) : (
        surface("h-72")
      )}

      {sections}
      {dialogs}
    </Page>
  );
}

/** The Spaces one of your machines provides (the core's `hosted_rows`:
 * those whose host is the machine). */
function hostedSpaces(spaces: readonly Space[], machineSpaceId: string): Space[] {
  if (!machineSpaceId.startsWith("relay:")) return [];
  const machine = machineSpaceId.slice("relay:".length);
  return spaces.filter((s) => s.host === machine);
}

/** A machine that does not share its desktop, or this machine's own
 * screen (never streamed here): the core's line and its one action, then
 * the Spaces it provides when it provides any. */
function MachineSpaces({ note, listSpaces, spaces, action }: { note: string; listSpaces: boolean; spaces: Space[]; action?: ReactNode }) {
  const t = useT();
  return (
    <section data-machine-desktop-note="">
      <div className="flex items-center justify-between gap-4 rounded-xl border px-4 py-3">
        <p className="text-[13px] text-muted-foreground">{note}</p>
        {action}
      </div>
      {listSpaces ? (
        <div data-machine-spaces="" className="mt-6">
          <h2 className="mb-2 px-1 text-[13px] font-semibold">{t("computers.page.spaces_title")}</h2>
          {spaces.length === 0 ? (
            <p className="px-1 text-[13px] text-muted-foreground">{t("computers.page.no_spaces")}</p>
          ) : (
            <ul className="divide-y rounded-xl border">
              {spaces.map((s) => (
                <li key={s.id}>
                  <Link
                    to="/spaces/$spaceId"
                    params={{ spaceId: s.id }}
                    className="flex h-10 items-center gap-2.5 px-4 text-[13px] outline-none hover:bg-muted/60 focus-visible:bg-muted/60"
                  >
                    <OsIcon os={s.os} className="size-3.5 shrink-0 text-muted-foreground" />
                    <span className="min-w-0 flex-1 truncate">{s.name}</span>
                    <StateLabel state={spaceState(s)} space={s} />
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </section>
  );
}

/** What replaces the stream while there is no desktop to show: a create's
 * progress, a delete, a stopped Space. Undefined: the stream surface. */
function surfaceState(
  space: Space,
  pending: string | null,
  startLabel: string,
  onStart: () => void,
  onDismiss: () => void,
  onRetry?: () => void,
  onReport?: () => void,
  onFix?: () => void,
): ReactNode {
  const state = spaceState(space);
  if (state === "failed") {
    // The same failure twice: its fix replaces a plain Try again.
    const fix = space.progress?.fix;
    return (
      <>
        <p className="text-[13px] font-medium">{tr("computers.page.create_failed")}</p>
        <p data-create-error="" className="mt-2 max-w-md text-center text-xs text-destructive">
          {createError(space) || tr("computers.page.no_reason")}
        </p>
        {fix ? (
          <p data-create-fix="" className="mt-2 max-w-md text-center text-xs text-muted-foreground">
            {fix.text}
          </p>
        ) : null}
        <div className="mt-4 flex flex-wrap justify-center gap-2">
          {fix && onFix ? (
            <Button data-create-fix-button="" onClick={onFix}>
              {fix.label}
            </Button>
          ) : onRetry ? (
            <Button onClick={onRetry}>{tr("computers.page.try_again")}</Button>
          ) : null}
          <Button variant="outline" onClick={onDismiss}>
            {tr("computers.page.remove_from_list")}
          </Button>
          {onReport && fix?.action !== "details" ? (
            <Button variant="outline" data-send-report="" onClick={onReport}>
              {tr("computers.page.send_report")}
            </Button>
          ) : null}
        </div>
      </>
    );
  }
  if (state === "creating") {
    return (
      <>
        <p className="text-[13px] font-medium">{space.progress?.label ?? tr("computers.page.creating")}</p>
        <div className="mt-3 h-1 w-56 overflow-hidden rounded-full bg-foreground/10">
          <div className="h-full rounded-full bg-brand transition-[width]" style={{ width: `${Math.round(createProgress(space) * 100)}%` }} />
        </div>
        {space.progress?.transfer ? <p className="mt-2 text-xs text-muted-foreground tabular-nums">{space.progress.transfer}</p> : null}
        {space.progress?.error ? <p className="mt-2 text-xs text-destructive">{space.progress.error}</p> : null}
      </>
    );
  }
  if (state === "deleting") return <p className="text-[13px] text-muted-foreground">{tr("spaces.status.deleting")}</p>;
  if (state === "stopped") {
    return (
      <>
        <OsIcon os={space.os} className="mb-4 size-8 text-muted-foreground/30" />
        <p className="text-[13px] text-muted-foreground">{canPower(space) ? tr("computers.page.start_to_open") : tr("computers.page.stopped_no_start")}</p>
        {canPower(space) ? (
          <Button className="mt-4" variant="outline" disabled={Boolean(pending)} onClick={onStart}>
            <PlayIcon /> {pending ?? startLabel}
          </Button>
        ) : null}
      </>
    );
  }
  return undefined;
}

/** Processor, memory and disk for this Space, when its computer sends them. */
function SpaceResources({ space, spaces, snapshot, machines }: { space: Space; spaces: Space[]; snapshot: ResourcesSnapshot | null; machines: Machine[] }) {
  const mine = spaceResources(snapshot, space.id);
  const state = spaceState(space);
  if (!mine || (state !== "running" && state !== "stopped")) return null;
  const local = space.provider === "local";
  // A Space on another computer comes with only its size there: while it
  // runs, that alone would read as stopped.
  if (!local && mine.powerState === "running" && !mine.usage) return null;
  const machine = local ? machines.find((m) => m.current) : machines.find((m) => m.id === space.host || m.spaceIds.includes(space.id));
  const host = hostResources(snapshot, machine);
  const computer = local ? thisComputerText() : (machine?.name ?? space.hostName ?? tr("computers.page.its_computer"));
  // The Spaces on the same computer, with the numbers it sent, for how it fits.
  const sameComputer = (s: Space) => (local ? s.provider === "local" : s.host === space.host);
  const onComputer = spaces.flatMap((s) => {
    const r = sameComputer(s) ? spaceResources(snapshot, s.id) : null;
    return r ? [r] : [];
  });
  return <SpaceResourcesPanel space={mine} name={space.name} host={host} computer={computer} spaces={onComputer} baseName={systemName(space.osPrettyName)} />;
}

/** `stream.viewContext`'s answer (camelCase). */
interface StreamViewContext {
  kind: string;
  app: string | null;
  title: string | null;
  windowId: string | null;
}

/** The core's view of the shown window, as `thread.post` takes it. */
function threadViewOf(v: StreamViewContext | null): ThreadView | null {
  if (!v || v.kind !== "window") return null;
  return { kind: "window", app: v.app ?? null, title: v.title ?? null, windowId: v.windowId ?? null };
}
