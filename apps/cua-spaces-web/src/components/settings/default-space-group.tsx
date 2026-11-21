// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { GripVerticalIcon } from "lucide-react";
import { useState, type DragEvent, type KeyboardEvent } from "react";

import type { SpacesSettings } from "@/bridge";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { TreeIcon } from "@/components/spaces/tree-parts";
import { useSpacesTreeData } from "@/components/spaces/use-spaces-tree";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { toastError } from "@/components/ui/toast";
import { findNode, treeRows, type SpacesTree, type TreeNode } from "@/lib/spaces-tree";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";
import { thisComputerLabel } from "@/lib/host-labels";

/**
 * The order to try when the default is offline: the saved one (computers
 * still listed), else every other computer in the account's order. This
 * Mac is never in it: it's always the last resort.
 */
export function orderToTry(tree: SpacesTree, settings: Pick<SpacesSettings, "fallback"> | null, defaultKey: string | null): TreeNode[] {
  const def = findNode(tree, defaultKey);
  const usable = (n: TreeNode | undefined): n is TreeNode => Boolean(n?.settingsId) && !n!.current && n!.key !== def?.key;
  if (settings?.fallback.length) return settings.fallback.map((id) => findNode(tree, id)).filter(usable);
  return tree.roots.filter((r) => r.kind === "computer").filter(usable);
}

/** Moves the item at `from` to `to`. */
export function moved<T>(list: readonly T[], from: number, to: number): T[] {
  const next = [...list];
  const [item] = next.splice(from, 1);
  next.splice(Math.max(0, Math.min(to, next.length)), 0, item!);
  return next;
}

/**
 * Settings, General: where new agent tasks run.
 * The same setting as "Make this my default" in Home's picker; the relay
 * keeps it for the account, so every computer and the CLI agree.
 */
export function DefaultSpaceGroup() {
  const t = useT();
  const { tree, defaults } = useSpacesTreeData();
  const [dragging, setDragging] = useState<number | null>(null);
  if (!tree || defaults.unsupported || tree.roots.length === 0) return null;
  const settings = defaults.settings;
  const defaultKey = settings?.defaultSpace ?? tree.defaultKey;
  const choices = treeRows(tree).filter((n) => n.settingsId);
  const parentName = (n: TreeNode) => (n.parentKey ? findNode(tree, n.parentKey)?.name : null);
  const value = findNode(tree, defaultKey)?.settingsId ?? choices[0]?.settingsId ?? "";
  const fallbackOn = settings?.fallbackOn ?? true;
  const order = orderToTry(tree, settings, defaultKey);
  const here = tree.roots.find((r) => r.current);
  const fail = toastError(t("settings.default_space.save_failed"));
  const save = (patch: Partial<Pick<SpacesSettings, "defaultSpace" | "fallback" | "fallbackOn">>) =>
    void defaults
      .save({
        defaultSpace: patch.defaultSpace !== undefined ? patch.defaultSpace : (settings?.defaultSpace ?? null),
        fallback: patch.fallback ?? settings?.fallback ?? [],
        fallbackOn: patch.fallbackOn ?? fallbackOn,
      })
      .catch(fail);
  const reorder = (from: number, to: number) => {
    if (from === to) return;
    save({ fallback: moved(order, from, to).map((n) => n.settingsId!) });
  };
  const onKey = (i: number) => (e: KeyboardEvent<HTMLLIElement>) => {
    if (e.key === "ArrowUp" && i > 0) reorder(i, i - 1);
    else if (e.key === "ArrowDown" && i < order.length - 1) reorder(i, i + 1);
    else return;
    e.preventDefault();
  };
  return (
    <SettingsGroup title={t("settings.default_space.title")} footer={t("settings.default_space.footer")} data-default-space-group="">
      <SettingsRow
        label={t("settings.default_space.label")}
        description={t("settings.default_space.help")}
        control={
          <Select
            aria-label={t("settings.default_space.label")}
            className="min-w-56"
            value={value}
            options={choices.map((n) => ({
              value: n.settingsId!,
              label: parentName(n) ? t("settings.default_space.option_on", { name: n.name, parent: parentName(n) }) : n.name,
            }))}
            onValueChange={(v) => save({ defaultSpace: v, fallback: (settings?.fallback ?? []).filter((id) => id !== v) })}
          />
        }
      />
      <SettingsRow
        label={t("settings.default_space.offline")}
        description={t("settings.default_space.offline_help", { fallback: thisComputerLabel() })}
        control={<Switch aria-label={t("settings.default_space.offline_switch")} checked={fallbackOn} onCheckedChange={(v) => save({ fallbackOn: v })} />}
      />
      {fallbackOn && (order.length > 0 || here) ? (
        <SettingsRow
          label={t("settings.default_space.order")}
          description={t("settings.default_space.order_help")}
          control={
            <ol className="flex min-w-56 flex-col gap-1.5" data-order-to-try="">
              {order.map((n, i) => (
                <li
                  key={n.key}
                  draggable
                  tabIndex={0}
                  aria-label={t("settings.default_space.order_item", { position: String(i + 1), name: n.name })}
                  onKeyDown={onKey(i)}
                  onDragStart={(e: DragEvent) => {
                    setDragging(i);
                    e.dataTransfer.effectAllowed = "move";
                  }}
                  onDragOver={(e) => e.preventDefault()}
                  onDrop={(e) => {
                    e.preventDefault();
                    if (dragging !== null) reorder(dragging, i);
                    setDragging(null);
                  }}
                  onDragEnd={() => setDragging(null)}
                  data-order-item={n.settingsId!}
                  className={cn(
                    "flex h-8 cursor-grab items-center gap-2 rounded-lg bg-foreground/[0.05] px-2.5 text-[13px] outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
                    dragging === i && "opacity-50",
                  )}
                >
                  <GripVerticalIcon className="size-3.5 text-muted-foreground" aria-hidden />
                  <TreeIcon node={n} />
                  <span className="truncate">{n.name}</span>
                </li>
              ))}
              {here ? (
                <li className="flex h-8 items-center gap-2 px-2.5 text-[13px] text-muted-foreground" data-order-item="here">
                  <TreeIcon node={here} />
                  <span className="truncate">{thisComputerLabel()}</span>
                  <span className="ml-auto">{t("settings.default_space.last_resort")}</span>
                </li>
              ) : null}
            </ol>
          }
        />
      ) : null}
    </SettingsGroup>
  );
}
