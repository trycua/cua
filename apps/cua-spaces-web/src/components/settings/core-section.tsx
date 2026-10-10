// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, CopyIcon } from "lucide-react";
import { useState } from "react";

import { useSession, type SettingsRow, type SettingsSection } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { cn } from "@/lib/utils";

/** What a row asks for, by row id: its button, a choice (`on`/`off` for a switch) or a field's text. */
export interface CoreRowHandlers {
  onPress?: (row: SettingsRow) => void;
  onChoose?: (row: SettingsRow, option: string) => void;
  onEdit?: (row: SettingsRow, value: string) => void;
}

/**
 * A Settings section as the app core lays it out (`settings::SettingsSection`),
 * drawn like the other Settings groups: one line per row, the way the SwiftUI
 * app's `SettingsView` draws them. With `foldNotes`, a note whose id is
 * `<row>-note` goes under that row's label (the Experiments tab).
 */
export function CoreSettingsSection({
  section,
  onHeaderButton,
  foldNotes,
  title,
  ...handlers
}: CoreRowHandlers & { section: SettingsSection; onHeaderButton?: () => void; foldNotes?: boolean; title?: string }) {
  const folded = new Set<string>();
  const notes = new Map<string, string>();
  if (foldNotes) {
    for (const r of section.rows) {
      if (r.kind === "note" && r.id.endsWith("-note") && section.rows.some((o) => `${o.id}-note` === r.id)) {
        notes.set(r.id.slice(0, -"-note".length), r.label);
        folded.add(r.id);
      }
    }
  }
  return (
    <section className="mb-7" data-settings-section={section.id}>
      <div className="mb-2 flex min-h-6 items-center justify-between gap-2 px-1">
        <h2 className="text-xs font-semibold text-muted-foreground">{title ?? section.title}</h2>
        {section.button ? (
          <Button size="sm" variant="outline" disabled={!section.buttonEnabled} title={section.buttonHelp ?? undefined} onClick={onHeaderButton} data-section-button>
            {section.button}
          </Button>
        ) : null}
      </div>
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
        {section.rows
          .filter((r) => !folded.has(r.id))
          .map((r) => (
            <CoreSettingsRow key={r.id} row={r} description={notes.get(r.id)} {...handlers} />
          ))}
      </div>
    </section>
  );
}

const optionOf = (row: SettingsRow) => row.options.find((o) => o.active)?.id ?? "";

/** One core row. Every row carries `data-setting-row` (its id) for tests and the parity harness. */
export function CoreSettingsRow({ row, description, onPress, onChoose, onEdit }: CoreRowHandlers & { row: SettingsRow; description?: string }) {
  const { openExternal } = useSession();
  const attrs = { "data-setting-row": row.id, "data-kind": row.kind, "data-enabled": String(row.enabled) };
  const id = `setting-${row.id.replace(/[^a-z0-9-]/gi, "-")}`;

  switch (row.kind) {
    case "toggle": {
      const on = row.options.find((o) => o.id === "on")?.active ?? false;
      return (
        <Line {...attrs} title={row.help ?? undefined}>
          <Label htmlFor={id} label={row.label} description={description} />
          <Switch id={id} checked={on} disabled={!row.enabled} onCheckedChange={(next) => onChoose?.(row, next ? "on" : "off")} />
        </Line>
      );
    }
    case "choice": {
      const options = row.options.map((o) => ({ value: o.id, label: o.label }));
      return (
        <Line {...attrs} title={row.help ?? undefined}>
          <Label label={row.label} description={description} />
          {options.length > 3 ? (
            <Select aria-label={row.label} value={optionOf(row)} options={options} disabled={!row.enabled} onValueChange={(v) => onChoose?.(row, v)} />
          ) : (
            <Segmented aria-label={row.label} value={optionOf(row)} options={options} disabled={!row.enabled} onValueChange={(v) => onChoose?.(row, v)} />
          )}
        </Line>
      );
    }
    case "field":
    case "secret":
      return (
        <Line {...attrs}>
          <Label htmlFor={id} label={row.label} />
          <Input
            id={id}
            className="w-64"
            type={row.kind === "secret" ? "password" : "text"}
            autoComplete="off"
            spellCheck={false}
            value={row.value ?? ""}
            placeholder={row.placeholder ?? undefined}
            disabled={!row.enabled}
            onChange={(e) => onEdit?.(row, e.currentTarget.value)}
          />
        </Line>
      );
    case "prompt":
      return (
        <div {...attrs} className="px-4 py-3">
          <div className="text-[13px]" data-row-label>
            {row.label}
          </div>
          <PromptBox text={row.value ?? ""} copyLabel={row.button ?? "Copy"} />
        </div>
      );
    case "link":
      return (
        <div {...attrs} className="px-4 py-2">
          <button
            type="button"
            disabled={!row.enabled}
            onClick={() => onPress?.(row)}
            className="text-[13px] text-brand-strong outline-none hover:underline focus-visible:underline disabled:opacity-50"
          >
            {row.label}
          </button>
        </div>
      );
    case "note":
      return (
        <div {...attrs} className="flex items-baseline gap-1.5 px-4 py-2.5 text-xs text-muted-foreground">
          <span data-row-label>{row.label}</span>
          {row.linkLabel && row.linkUrl ? (
            <button type="button" className="shrink-0 text-brand-strong hover:underline" onClick={() => void openExternal(row.linkUrl!)}>
              {row.linkLabel}
            </button>
          ) : null}
        </div>
      );
    case "error":
      return (
        <div {...attrs} className="px-4 py-2.5 text-xs text-destructive" title={row.label}>
          <span data-row-label>{row.label}</span>
        </div>
      );
    case "text":
    default:
      return (
        <Line {...attrs} title={row.help ?? undefined} className={cn(!row.enabled && !row.button && "opacity-50")}>
          <Label label={row.label} />
          <div className="flex min-w-0 items-center gap-2">
            {row.value ? (
              <span data-row-value className="truncate text-[13px] text-muted-foreground">
                {row.value}
              </span>
            ) : null}
            {row.button ? (
              <Button size="sm" variant="outline" disabled={!row.enabled} onClick={() => onPress?.(row)} data-row-button>
                {row.button}
              </Button>
            ) : null}
          </div>
        </Line>
      );
  }
}

function Line({ children, className, ...rest }: React.HTMLAttributes<HTMLDivElement> & Record<`data-${string}`, string>) {
  return (
    <div className={cn("flex min-h-12 items-center justify-between gap-6 px-4 py-2.5", className)} {...rest}>
      {children}
    </div>
  );
}

function Label({ label, description, htmlFor }: { label: string; description?: string; htmlFor?: string }) {
  return (
    <div className="min-w-0">
      <label htmlFor={htmlFor} className="block truncate text-[13px]" data-row-label>
        {label}
      </label>
      {description ? (
        <p className="mt-0.5 text-xs text-muted-foreground" data-row-description>
          {description}
        </p>
      ) : null}
    </div>
  );
}

function PromptBox({ text, copyLabel }: { text: string; copyLabel: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="mt-2 flex items-start gap-2 rounded-lg border bg-muted/50 p-2.5">
      <p data-row-value className="min-w-0 flex-1 font-mono text-xs leading-relaxed select-text">
        {text}
      </p>
      <Button
        size="sm"
        variant="outline"
        onClick={() => {
          void navigator.clipboard?.writeText(text).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          });
        }}
      >
        {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
        {copied ? "Copied" : copyLabel}
      </Button>
    </div>
  );
}
