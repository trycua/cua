// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, CopyIcon, TriangleAlertIcon } from "lucide-react";
import { useEffect, useState } from "react";

import type { SpaceFact } from "@/bridge";
import { SettingsGroup } from "@/components/settings-group";
import { Tooltip } from "@/components/ui/tooltip";

/** A Space's facts as the core lists them (`sidebar.detail`): Status,
 * Image, Identifier, System, Kind, Architecture, Memory, Storage. */
export function FactList({ title, facts }: { title: string; facts: SpaceFact[] }) {
  return (
    <SettingsGroup title={title}>
      {facts.map((f) => (
        <FactRow key={f.label} fact={f} />
      ))}
    </SettingsGroup>
  );
}

function FactRow({ fact }: { fact: SpaceFact }) {
  // The tooltip holds the full value, and an image's digest on its own line.
  const help = fact.help && fact.help !== fact.value ? fact.help : null;
  const value = (
    <span data-fact-value className="min-w-0 truncate text-right text-[13px] select-text">
      {fact.value}
    </span>
  );
  return (
    <div data-fact={fact.label} className="flex min-h-10 items-center justify-between gap-6 px-4 py-2">
      <span className="shrink-0 text-[13px] text-muted-foreground">{fact.label}</span>
      <span className="flex min-w-0 items-center gap-1.5" data-help={help ?? undefined}>
        {help ? (
          <Tooltip content={<span className="block max-w-sm break-all whitespace-pre-line">{help}</span>}>{value}</Tooltip>
        ) : (
          value
        )}
        {fact.warning ? (
          <Tooltip content={fact.warning.help}>
            <span data-fact-warning={fact.warning.help} aria-label={fact.warning.help} role="img" className="inline-flex shrink-0 text-warning">
              <TriangleAlertIcon className="size-3.5" />
            </span>
          </Tooltip>
        ) : null}
        {fact.copy ? <CopyButton copy={fact.copy} /> : null}
      </span>
    </div>
  );
}

function CopyButton({ copy }: { copy: NonNullable<SpaceFact["copy"]> }) {
  const [done, setDone] = useState(false);
  useEffect(() => {
    if (!done) return;
    const t = setTimeout(() => setDone(false), copy.confirmMs);
    return () => clearTimeout(t);
  }, [done, copy.confirmMs]);
  const label = done ? copy.doneHelp : copy.help;
  return (
    <Tooltip content={label}>
      <button
        type="button"
        data-copy-text={copy.text}
        aria-label={label}
        onClick={() => {
          void navigator.clipboard?.writeText(copy.text).then(() => setDone(true), () => {});
        }}
        className="inline-flex size-6 shrink-0 cursor-default items-center justify-center rounded-md text-muted-foreground outline-none transition-colors hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
      >
        {done ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
      </button>
    </Tooltip>
  );
}
