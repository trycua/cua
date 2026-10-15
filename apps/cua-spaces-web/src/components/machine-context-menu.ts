// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A machine's context menu on Machines: Open, New Space on it, Copy Name,
 * and Remove where its detail offers Remove (the core's Delete action for
 * the machine's own Space, by the same rules as the detail's button).
 */

import { useNewSpaceWizard, useSpaces, type Machine, type Space, type SpaceDetailView } from "@/bridge";
import { SEPARATOR, useContextMenu, type ContextMenuEntry, type ContextMenuProps } from "@/components/context-menu";
import { toast } from "@/components/ui/toast";
import { useNewSpace } from "@/hooks/use-new-space";
import { reachable } from "@/lib/machines";

/** The core's feature for a machine that provides Spaces (`sidebar::HOST_SPACES_FEATURE`). */
export const HOST_SPACES_FEATURE = "host_spaces";

/** The machine's own Space and its detail (null for this machine). */
export type MachineOwnSpace = { space: Space; detail: SpaceDetailView } | null;

export interface MachineMenuActions {
  open(): void;
  /** New Space with "Run on" set to `on` (`local`, `host:<id>`). */
  newSpace(on: string): void;
  remove(removeOnly: boolean): Promise<unknown>;
}

/**
 * Where New Space runs on `m`, or null when it can't: this machine, or one
 * of your machines that provides Spaces and is reachable (the core's
 * "New Space on <name>…" for one that keeps its desktop private).
 */
export function newSpaceOn(m: Machine, own: MachineOwnSpace): string | null {
  if (m.current) return "local";
  if (m.device || !reachable(m)) return null;
  if (own?.detail.newSpace) return own.detail.newSpace.on;
  return own?.space.sdk?.features.includes(HOST_SPACES_FEATURE) ? `host:${m.id}` : null;
}

/**
 * The entries of `m`'s menu. Stable action ids: `machine.open`,
 * `machine.newSpace`, `machine.copyName` and `machine.remove`.
 */
export function machineMenuEntries(m: Machine, own: MachineOwnSpace, act: MachineMenuActions, { wizard = true }: { wizard?: boolean } = {}): ContextMenuEntry[] {
  // Another machine needs the wizard ("Run on"); without it a New Space runs here.
  const where = newSpaceOn(m, own);
  const on = where === "local" || wizard ? where : null;
  const remove = own?.detail.actions.find((a) => a.id === "delete") ?? null;
  const confirm = own?.detail.confirm;
  return [
    { id: "machine.open", label: "Open", run: act.open },
    on && { id: "machine.newSpace", label: "New Space on This Machine…", run: () => act.newSpace(on) },
    SEPARATOR,
    { id: "machine.copyName", label: "Copy Name", copy: m.name },
    SEPARATOR,
    remove &&
      confirm && {
        id: "machine.remove",
        label: remove.label.endsWith("…") ? remove.label : `${remove.label}…`,
        disabled: !remove.enabled,
        destructive: true,
        confirm: {
          title: confirm.title,
          message: confirm.message,
          confirmLabel: confirm.confirmLabel,
          cancelLabel: confirm.cancelLabel,
          disabledReason: confirm.confirmEnabled ? null : (confirm.disabledReason ?? null),
          alternative: confirm.removeLabel ? { label: confirm.removeLabel, run: () => act.remove(true) } : null,
        },
        run: () => act.remove(Boolean(own?.detail.removeOnly)),
      },
  ];
}

/** Props for a machine's row: its context menu. `onOpen` shows its detail. */
export function useMachineContextMenu(m: Machine, own: MachineOwnSpace, { onOpen, onRemoved }: { onOpen: () => void; onRemoved?: () => void }): ContextMenuProps {
  const wizard = useNewSpaceWizard();
  const newSpace = useNewSpace();
  const { deleteSpace } = useSpaces();
  const act: MachineMenuActions = {
    open: onOpen,
    newSpace: (on) => (wizard.offered ? wizard.show(on) : newSpace()),
    remove: (removeOnly) => {
      if (!own) return Promise.resolve();
      onRemoved?.();
      return deleteSpace(own.space.id, removeOnly).then(
        () => toast(`Removed ${m.name}`, { type: "success" }),
        (e: unknown) => toast(`Couldn't remove ${m.name}`, { description: e instanceof Error ? e.message : String(e), type: "error" }),
      );
    },
  };
  return useContextMenu(() => machineMenuEntries(m, own, act, { wizard: wizard.offered }));
}
