// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useState } from "react";

import { isUnsupported, useBridge } from "@/bridge";
import type { DiskCommand, DiskKind, DiskResult, DiskView } from "@/bridge/contracts/settings";
import { SettingsGroup, SettingsRow } from "@/components/settings-group";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Checkbox } from "@/components/ui/checkbox";
import { toastError } from "@/components/ui/toast";
import { useT } from "@/i18n";
import { diskBytes, movableKinds, tildePath, userHomeOf } from "@/lib/disk-storage";

/** How often sizes are read again while Settings is open. */
const REFRESH_MS = 30_000;

type Pending =
  | { kind: "move"; folder: DiskKind; path: string; needed: number }
  | { kind: "remove"; path: string; name: string; bytes: number }
  | { kind: "uninstall" };

/**
 * Settings, Advanced, "Storage": what Cua keeps on this computer and where. The large folders can
 * move (to an external drive, say), computers' images are listed with their sizes and can be
 * removed, and on the Mac app Cua can uninstall itself. Nothing is deleted without a question
 * that names the size; the host does the work (`cua storage`).
 */
export function DiskGroup() {
  const t = useT();
  const { data: adapter } = useBridge();
  const [view, setView] = useState<DiskView | null>(null);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<Pending | null>(null);
  const [keychain, setKeychain] = useState(false);

  const read = useCallback(async () => {
    if (!adapter) return;
    try {
      setView(await adapter.call("disk.get", {}));
    } catch (e) {
      // A host without storage settings has no group; any other failure keeps what is shown.
      if (isUnsupported(e)) setView(null);
    }
  }, [adapter]);

  useEffect(() => {
    void read();
    const timer = setInterval(() => void read(), REFRESH_MS);
    return () => clearInterval(timer);
  }, [read]);

  if (!adapter || !view) return null;

  const home = userHomeOf(view.home);
  const shown = (p: string) => tildePath(p, home);
  const kindName = (k: string) => t(`settings.disk.kind.${k}` as never);

  const act = async (command: DiskCommand, failed: string): Promise<DiskResult | null> => {
    setBusy(true);
    try {
      const r = await adapter.call("disk.do", command);
      if (!r.ok) throw new Error(r.error ?? failed);
      return r;
    } catch (e) {
      toastError(failed)(e);
      return null;
    } finally {
      setBusy(false);
    }
  };

  const change = async (folder: DiskKind) => {
    const picked = await act({ op: "locate", kind: folder.kind }, t("settings.disk.locate_failed"));
    if (!picked?.path) return;
    const checked = await act({ op: "check", kind: folder.kind, path: picked.path }, t("settings.disk.change_failed"));
    if (!checked) return;
    setPending({ kind: "move", folder, path: picked.path, needed: checked.neededBytes ?? folder.bytes });
  };

  const confirm = async () => {
    const p = pending;
    setPending(null);
    if (!p) return;
    if (p.kind === "move") {
      await act({ op: "set", kind: p.folder.kind, path: p.path, move: true }, t("settings.disk.change_failed"));
    } else if (p.kind === "remove") {
      await act({ op: "remove", path: p.path }, t("settings.disk.remove_failed"));
    } else {
      await act({ op: "uninstall", keychain }, t("settings.disk.uninstall_failed"));
      return;
    }
    await read();
  };

  const reset = async (folder: DiskKind) => {
    if (await act({ op: "reset", kind: folder.kind }, t("settings.disk.reset_failed"))) await read();
  };

  const ask = (p: Pending) => {
    setKeychain(false);
    setPending(p);
  };

  const running = view.daemonRunning ? t("settings.disk.running") : null;
  const free = view.freeBytes === null ? t("settings.disk.free_unknown") : t("settings.disk.free", { free: diskBytes(view.freeBytes) });

  const dialog = (() => {
    if (!pending) return null;
    if (pending.kind === "move") {
      return {
        title: t("settings.disk.move_title", { kind: kindName(pending.folder.kind).toLowerCase() }),
        description: t("settings.disk.move_body", { size: diskBytes(pending.needed), path: shown(pending.path) }),
        confirmLabel: t("settings.disk.move_confirm"),
        destructive: false,
        disabledReason: running,
      };
    }
    if (pending.kind === "remove") {
      return {
        title: t("settings.disk.remove_title", { name: pending.name }),
        description: t("settings.disk.remove_body", { size: diskBytes(pending.bytes) }),
        confirmLabel: t("settings.disk.remove_confirm"),
        destructive: true,
        disabledReason: null,
      };
    }
    return {
      title: t("settings.disk.uninstall_title"),
      description: (
        <>
          <span className="block">{t("settings.disk.uninstall_body")}</span>
          <label className="mt-3 flex items-center gap-2 text-[13px] text-foreground">
            <Checkbox data-setting-row="disk-uninstall-keychain" checked={keychain} onCheckedChange={(v) => setKeychain(v === true)} />
            {t("settings.disk.uninstall_keychain")}
          </label>
        </>
      ),
      confirmLabel: t("settings.disk.uninstall_confirm"),
      destructive: true,
      disabledReason: null,
    };
  })();

  return (
    <SettingsGroup title={t("settings.disk.title")} data-settings-group="disk">
      <SettingsRow label={t("settings.disk.summary", { used: diskBytes(view.homeBytes) })} description={free} control={null} />
      {movableKinds(view.kinds).map((folder) => {
        const fixed = folder.source === "environment";
        return (
          <SettingsRow
            key={folder.kind}
            label={kindName(folder.kind)}
            description={
              <>
                {t(`settings.disk.kind.${folder.kind}_help` as never, {
                  days: view.trajectoriesMaxDays,
                  size: diskBytes(view.trajectoriesMaxBytes),
                })}
                <span className="block" data-disk-path={folder.kind}>
                  {shown(folder.path)}
                </span>
                {fixed ? <span className="block">{t("settings.disk.in_environment", { variable: `CUA_${folder.kind.toUpperCase()}_DIR` })}</span> : null}
              </>
            }
            control={
              <span className="flex items-center gap-3">
                <span className="text-[13px] text-muted-foreground">{diskBytes(folder.bytes)}</span>
                {folder.source === "settings" ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    data-setting-row={`disk-reset-${folder.kind}`}
                    disabled={busy || view.daemonRunning}
                    onClick={() => void reset(folder)}
                  >
                    {t("settings.disk.use_default")}
                  </Button>
                ) : null}
                <Button
                  size="sm"
                  variant="outline"
                  data-setting-row={`disk-change-${folder.kind}`}
                  disabled={busy || fixed}
                  onClick={() => void change(folder)}
                >
                  {t("settings.disk.change")}
                </Button>
              </span>
            }
          />
        );
      })}
      {view.oldLocations.map((old) => (
        <SettingsRow
          key={old.path}
          label={t("settings.disk.old_label", { kind: shown(old.path) })}
          description={diskBytes(old.bytes)}
          control={
            <Button
              size="sm"
              variant="outline"
              data-setting-row="disk-remove-old"
              disabled={busy}
              onClick={() => ask({ kind: "remove", path: old.path, name: shown(old.path), bytes: old.bytes })}
            >
              {t("settings.disk.remove")}
            </Button>
          }
        />
      ))}
      {view.items.map((item) => (
        <SettingsRow
          key={item.path}
          label={t("settings.disk.item_label", { family: item.family === "lume" ? "macOS" : "Linux", name: item.name })}
          description={t("settings.disk.item_help")}
          control={
            <span className="flex items-center gap-3">
              <span className="text-[13px] text-muted-foreground">{diskBytes(item.bytes)}</span>
              <Button
                size="sm"
                variant="outline"
                data-setting-row="disk-remove-item"
                disabled={busy || view.daemonRunning}
                title={running ?? undefined}
                onClick={() => ask({ kind: "remove", path: item.path, name: `${item.family} ${item.name}`, bytes: item.bytes })}
              >
                {t("settings.disk.remove")}
              </Button>
            </span>
          }
        />
      ))}
      {view.canUninstall ? (
        <SettingsRow
          label={t("settings.disk.uninstall")}
          description={t("settings.disk.uninstall_help")}
          control={
            <Button size="sm" variant="outline" data-setting-row="disk-uninstall" disabled={busy} onClick={() => ask({ kind: "uninstall" })}>
              {t("settings.disk.uninstall_button")}
            </Button>
          }
        />
      ) : null}
      {dialog ? (
        <ConfirmDialog
          open
          onOpenChange={(open) => {
            if (!open) setPending(null);
          }}
          title={dialog.title}
          description={dialog.description}
          confirmLabel={dialog.confirmLabel}
          destructive={dialog.destructive}
          disabledReason={dialog.disabledReason}
          cancelFirst
          onConfirm={() => void confirm()}
        />
      ) : null}
    </SettingsGroup>
  );
}
