// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A Space's context menu, for its tile and its rows: the actions its page
 * offers, by the same rules (the core's `sidebar.detail` actions and power
 * button, `canDelete`), so the menu never offers what the page would not.
 */

import { useLocation, useNavigate } from "@tanstack/react-router";
import { useEffect } from "react";

import { useBridge, useExperiments, useMachines, useSession, useShareSheet, useSpaceFiles, useSpaces, useTeleport, type CoreClient, type DropStatus, type MachineAccessNotice, type Space, type SpaceDetailView } from "@/bridge";
import { detailCopy, spaceDetail } from "@/bridge/space-detail";
import { SEPARATOR, useContextMenu, type ContextMenuEntry, type ContextMenuProps } from "@/components/context-menu";
import { toast, toastError } from "@/components/ui/toast";
import { canDelete, canPower, powerButton, spaceState } from "@/lib/spaces";

/** What a Space's menu runs: the page's own actions. */
export interface SpaceMenuActions {
  open(): void;
  start(): Promise<unknown>;
  stop(): Promise<unknown>;
  restart(): Promise<unknown>;
  teleport(): Promise<unknown>;
  sendFiles(): Promise<unknown>;
  share(): Promise<unknown>;
  remove(removeOnly: boolean): Promise<unknown>;
  dismiss(): Promise<unknown>;
}

/** What the menu is built from, as the Space's page reads it. */
export interface SpaceMenuInput {
  space: Space;
  core: CoreClient;
  detail: SpaceDetailView;
  /** The Send file… label (the core's `sidebar.detailCopy`). */
  sendFileLabel?: string;
}

/**
 * The entries of `space`'s menu, in its page's words. Stable action ids:
 * `space.open`, `space.start`, `space.stop`, `space.restart`,
 * `space.teleport`, `space.sendFiles`, `space.share`, `space.copyName`,
 * `space.copyId`, `space.delete` and `space.remove` (a failed create).
 */
export function spaceMenuEntries({ space, core, detail, sendFileLabel }: SpaceMenuInput, act: SpaceMenuActions): ContextMenuEntry[] {
  const state = spaceState(space);
  const action = (id: string) => detail.actions.find((a) => a.id === id);
  const teleport = action("teleport");
  const share = action("share");
  // The core's power button: none while it is being created or deleted, or
  // where its provider can't turn it off.
  const power = canPower(space) ? powerButton(core, space) : null;
  const suspends = space.power?.control === "suspend";
  const turnOff = power && !power.turnOn && !power.busy ? power.help : null;
  const ellipsis = (s: string) => (s.endsWith("…") ? s : `${s}…`);
  return [
    state !== "deleting" && { id: "space.open", label: "Open", run: act.open },
    SEPARATOR,
    power?.turnOn && { id: "space.start", label: power.help, disabled: power.busy, run: act.start },
    turnOff && {
      id: "space.stop",
      label: ellipsis(turnOff),
      confirm: {
        title: `${turnOff} ${space.name}?`,
        message: suspends
          ? "It's suspended in memory and picks up where it left off when you start it again."
          : "It shuts down. Anything unsaved inside it is lost.",
        confirmLabel: turnOff,
        destructive: !suspends,
      },
      run: act.stop,
    },
    turnOff && !suspends && {
      id: "space.restart",
      label: "Restart…",
      confirm: {
        title: `Restart ${space.name}?`,
        message: "It shuts down and starts again. Anything unsaved inside it is lost.",
        confirmLabel: "Restart",
        destructive: true,
      },
      run: act.restart,
    },
    SEPARATOR,
    teleport && { id: "space.teleport", label: ellipsis(teleport.label), disabled: !teleport.enabled, run: act.teleport },
    // Where the page has its drop well (the core's Teleport section).
    detail.sections.includes("Teleport") && { id: "space.sendFiles", label: sendFileLabel ?? "Send file…", run: act.sendFiles },
    share && { id: "space.share", label: ellipsis(share.label), disabled: !share.enabled, run: act.share },
    SEPARATOR,
    { id: "space.copyName", label: "Copy Name", copy: space.name },
    { id: "space.copyId", label: "Copy ID", copy: space.id },
    SEPARATOR,
    // A failed create only leaves the list, as its page's Remove does.
    state === "failed" && { id: "space.remove", label: "Remove from List", run: act.dismiss },
    state !== "failed" &&
      canDelete(space) && {
        id: "space.delete",
        label: ellipsis(detail.deleteLabel),
        destructive: true,
        confirm: {
          title: detail.confirm.title,
          message: detail.confirm.message,
          confirmLabel: detail.confirm.confirmLabel,
          cancelLabel: detail.confirm.cancelLabel,
          disabledReason: detail.confirm.confirmEnabled ? null : (detail.confirm.disabledReason ?? null),
          alternative: detail.confirm.removeLabel ? { label: detail.confirm.removeLabel, run: () => act.remove(true) } : null,
        },
        run: () => act.remove(detail.removeOnly),
      },
  ];
}

/** Toasts what a Send file… from a menu did (the tile's drop says it the same way). */
export function useSendFilesToast(space: { id: string; name: string }, status: DropStatus): void {
  useEffect(() => {
    if (!status) return;
    const group = `tile-drop:${space.id}`;
    if (status.kind === "working") toast(status.text, { group });
    else if (status.kind === "done") toast(`Sent to ${space.name}`, { description: status.text, type: "success", group });
    else toast(`Couldn't send to ${space.name}`, { description: status.text, type: "error", group });
  }, [status, space.id, space.name]);
}

/** Props for a Space's tile or row: its context menu. */
export function useSpaceContextMenu(space: Space): ContextMenuProps {
  const { core } = useBridge();
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const { startSpace, stopSpace, deleteSpace, dismissCreate } = useSpaces();
  const { data: machines = [] } = useMachines();
  const exp = useExperiments();
  const { data: session } = useSession();
  const teleport = useTeleport();
  const sharing = useShareSheet();
  const files = useSpaceFiles(space);
  useSendFilesToast(space, files.status);

  const thisMachine = machines.find((m) => m.current);
  const experiments = exp.unsupported ? undefined : (exp.data?.experiments ?? {});
  const access: MachineAccessNotice | null = thisMachine?.accessNotice ?? null;
  const fail = (word: string) => toastError(`Couldn't ${word} ${space.name}`);
  // Leaving the page of a Space that goes away.
  const leave = () => {
    if (pathname === `/spaces/${space.id}`) void navigate({ to: "/spaces", replace: true });
  };
  const act: SpaceMenuActions = {
    open: () => void navigate({ to: "/spaces/$spaceId", params: { spaceId: space.id } }),
    start: () => startSpace(space.id).catch(fail("start")),
    stop: () => stopSpace(space.id).catch(fail("stop")),
    restart: () =>
      stopSpace(space.id)
        .then(() => startSpace(space.id))
        .catch(fail("restart")),
    teleport: () => teleport.open(space).catch(fail("open Teleport for")),
    sendFiles: () => files.choose(),
    share: () => sharing.open(space, session?.signedIn ?? false).catch(fail("share")),
    remove: (removeOnly) => {
      leave();
      return deleteSpace(space.id, removeOnly).then(
        () => toast(removeOnly ? `Removed ${space.name} from the list` : `Deleted ${space.name}`, { type: "success" }),
        fail(removeOnly ? "remove" : "delete"),
      );
    },
    dismiss: () => {
      leave();
      return dismissCreate(space.id).catch(fail("remove"));
    },
  };
  return useContextMenu(() =>
    spaceMenuEntries(
      { space, core, detail: spaceDetail(core, space, null, thisMachine?.arch ?? null, experiments, access), sendFileLabel: detailCopy(core)?.sendFile },
      act,
    ),
  );
}
