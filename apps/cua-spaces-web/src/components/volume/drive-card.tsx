// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { storageChoose, storageEdit, useBridge, useSession, type DriveCard, type SettingsRow, type StorageAction, type StorageChoice } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";
import { DriveMountPreview } from "./previews";

export interface DriveCardActions {
  toggle(on: boolean): void;
  chooseStorage(choice: StorageChoice): void;
  storage(action: StorageAction): void;
}

/**
 * The first run's Cua Volume card (`onboarding::DriveCard`), laid out like
 * the SwiftUI app's `DriveCardView`: where the files live, the bucket's rows
 * when it is the S3 bucket (they take the miniature's place), the checkbox
 * with its note, and Open System Settings while macOS waits for approval.
 */
export function DriveCardView({ card, act, fixedMs }: { card: DriveCard; act: DriveCardActions; fixedMs?: number | "still" }) {
  const { openExternal } = useSession();
  const { core } = useBridge();
  const edit = (id: string, value: string) => {
    const a = storageEdit(core, id, value);
    if (a) act.storage(a);
  };
  const choose = (id: string, option: string) => {
    const a = storageChoose(core, id, option);
    if (a) act.storage(a);
  };
  const press = (id: string) => {
    // The SwiftUI app's `pressStorageRow`.
    if (id === "s3-test") act.storage({ type: "test" });
    else if (id === "s3-manual") act.storage({ type: "show-manual", on: true });
    else if (id === "s3-use-prompt") act.storage({ type: "show-manual", on: false });
  };

  return (
    <div className="flex flex-col gap-3" data-drive-card="">
      {card.storageRows.length === 0 ? <DriveMountPreview label={card.imageLabel} fixedMs={fixedMs} /> : null}
      {card.storageTitle && card.storageOptions.length > 0 ? (
        <div className="flex flex-col items-start gap-2">
          <span className="text-[13px]">{card.storageTitle}</span>
          <Segmented
            aria-label={card.storageTitle}
            value={card.storageOptions.find((o) => o.active)?.id ?? "local"}
            options={card.storageOptions.map((o) => ({
              value: o.id,
              label: (
                <span className="whitespace-nowrap" data-storage-option={o.id} data-active={o.active ? "true" : "false"}>
                  {o.label}
                </span>
              ),
            }))}
            onValueChange={(id) => !card.busy && act.chooseStorage(id as StorageChoice)}
          />
        </div>
      ) : null}
      {card.storedIn ? (
        <p className="truncate text-xs text-muted-foreground" title={card.storedPath ?? undefined} data-drive-stored="">
          {card.storedIn}
        </p>
      ) : null}
      {card.mountedAt ? (
        <p className="truncate text-xs text-muted-foreground" title={card.mountedPath ?? undefined} data-drive-mounted="">
          {card.mountedAt}
        </p>
      ) : null}
      {card.storageRows.length > 0 ? (
        <div className="divide-y rounded-lg border" aria-label={card.storageTitle ?? undefined}>
          {card.storageRows.map((row) => (
            <StorageRow key={row.id} row={row} onEdit={edit} onChoose={choose} onPress={press} onOpen={(url) => void openExternal(url)} />
          ))}
        </div>
      ) : null}
      {card.storageNote ? (
        <p className="text-xs text-muted-foreground" data-drive-storage-note="">
          {card.storageNote}
        </p>
      ) : null}
      <label className="flex items-center gap-2.5 text-[13px]">
        <Checkbox
          checked={card.checked}
          disabled={!card.enabled}
          onCheckedChange={(on) => act.toggle(Boolean(on))}
          data-drive-toggle=""
          data-busy={card.busy ? "true" : undefined}
        />
        <span data-drive-label="">{card.label}</span>
      </label>
      {card.note ? (
        <p className="text-xs text-muted-foreground" data-drive-note="">
          {card.note}
        </p>
      ) : null}
      {card.error ? (
        <p className="text-xs text-destructive" role="alert" data-drive-error="">
          {card.error}
        </p>
      ) : null}
      {card.settingsUrl && card.settingsLabel ? (
        <div>
          <Button variant="outline" size="sm" data-drive-settings="" onClick={() => void openExternal(card.settingsUrl!)}>
            {card.settingsLabel}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

/** One of the bucket's rows, drawn by kind like Settings' rows. */
function StorageRow({
  row,
  onEdit,
  onChoose,
  onPress,
  onOpen,
}: {
  row: SettingsRow;
  onEdit: (id: string, value: string) => void;
  onChoose: (id: string, option: string) => void;
  onPress: (id: string) => void;
  onOpen: (url: string) => void;
}) {
  const [copied, setCopied] = useState(false);
  const common = { "data-row-id": row.id, "data-row-kind": row.kind, title: row.help ?? undefined };
  switch (row.kind) {
    case "field":
    case "secret":
      return (
        <label {...common} className="flex items-center gap-4 px-3 py-2">
          <span className="w-32 shrink-0 text-[13px]" data-row-label="">
            {row.label}
          </span>
          <Input
            type={row.kind === "secret" ? "password" : "text"}
            value={row.value ?? ""}
            placeholder={row.placeholder ?? undefined}
            disabled={!row.enabled}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => onEdit(row.id, e.target.value)}
          />
        </label>
      );
    case "choice":
      return (
        <div {...common} className="flex items-center justify-between gap-4 px-3 py-2">
          <span className="text-[13px]" data-row-label="">
            {row.label}
          </span>
          <Segmented
            aria-label={row.label}
            value={row.options.find((o) => o.active)?.id ?? ""}
            options={row.options.map((o) => ({ value: o.id, label: o.label }))}
            onValueChange={(o) => onChoose(row.id, o)}
          />
        </div>
      );
    case "toggle":
      return (
        <label {...common} className="flex items-center justify-between gap-4 px-3 py-2">
          <span className="text-[13px]" data-row-label="">
            {row.label}
          </span>
          <Switch
            checked={row.options.some((o) => o.id === "on" && o.active)}
            disabled={!row.enabled}
            onCheckedChange={(on) => onChoose(row.id, on ? "on" : "off")}
          />
        </label>
      );
    case "prompt":
      return (
        <div {...common} className="flex flex-col gap-2 px-3 py-2.5">
          <div className="flex items-center justify-between gap-4">
            <span className="text-[13px]" data-row-label="">
              {row.label}
            </span>
            {row.button ? (
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  void navigator.clipboard?.writeText(row.value ?? "").then(() => setCopied(true), () => {});
                }}
              >
                {copied ? "Copied" : row.button}
              </Button>
            ) : null}
          </div>
          <p className="max-h-24 overflow-y-auto rounded-md bg-muted px-2.5 py-2 font-mono text-xs leading-relaxed text-muted-foreground select-text">{row.value}</p>
        </div>
      );
    case "link":
      return (
        <div {...common} className="px-3 py-2">
          <button
            type="button"
            disabled={!row.enabled}
            className="text-xs text-foreground underline-offset-2 hover:underline disabled:opacity-50"
            onClick={() => onPress(row.id)}
            data-row-label=""
          >
            {row.label}
          </button>
        </div>
      );
    case "note":
    case "error":
      return (
        <p {...common} className={cn("px-3 py-2 text-xs", row.kind === "error" ? "text-destructive" : "text-muted-foreground")}>
          <span data-row-label="">{row.label}</span>
          {row.linkLabel && row.linkUrl ? (
            <>
              {" "}
              <button type="button" className="text-foreground underline-offset-2 hover:underline" onClick={() => onOpen(row.linkUrl!)}>
                {row.linkLabel}
              </button>
            </>
          ) : null}
        </p>
      );
    case "text":
    default:
      return (
        <div {...common} className="flex items-center justify-between gap-4 px-3 py-2">
          <div className="min-w-0">
            <span className="text-[13px]" data-row-label="">
              {row.label}
            </span>
            {row.value ? <p className="truncate text-xs text-muted-foreground">{row.value}</p> : null}
          </div>
          {row.button ? (
            <Button variant="outline" size="sm" disabled={!row.enabled} onClick={() => onPress(row.id)}>
              {row.button}
            </Button>
          ) : null}
        </div>
      );
  }
}
