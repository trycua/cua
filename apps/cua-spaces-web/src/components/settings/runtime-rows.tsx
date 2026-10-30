// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef, useState } from "react";

import { useBridge, type RuntimePathCheck, type RuntimePathVerdict, type SettingsRow, type SettingsSection } from "@/bridge";
import { Button } from "@/components/ui/button";
import { GroupedRow } from "@/components/ui/grouped";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { CoreSettingsRow } from "@/components/settings/core-section";
import { cn } from "@/lib/utils";

/** How long typing pauses before the path is checked. */
const CHECK_DELAY_MS = 300;

/**
 * Settings, Advanced: one row per kind of Space (Containers, Virtual
 * machines, MicroVMs), as the app core lays them out
 * (`settings::runtimes_section`). The core decides the labels, the options,
 * "Not available" and its reason, the runtimes found on this computer and
 * whether the path in use is the detected one or a custom one; this draws
 * them, and checks a path while it is typed (the host probes it, the core
 * words the result).
 */
export function RuntimeKindRows({ section, onChoose }: { section: SettingsSection; onChoose: (row: SettingsRow, option: string) => void }) {
  const byId = new Map(section.rows.map((r) => [r.id, r]));
  const kinds = section.rows.filter((r) => r.id.startsWith("runtime:")).map((r) => r.id.slice("runtime:".length));
  return (
    <>
      {kinds.map((kind) => (
        <RuntimeKind key={kind} kind={kind} rows={byId} onChoose={onChoose} />
      ))}
    </>
  );
}

function RuntimeKind({ kind, rows, onChoose }: { kind: string; rows: Map<string, SettingsRow>; onChoose: (row: SettingsRow, option: string) => void }) {
  const main = rows.get(`runtime:${kind}`)!;
  const detected = rows.get(`runtime-detected:${kind}`);
  const path = rows.get(`runtime-path:${kind}`);
  const check = rows.get(`runtime-check:${kind}`);
  return (
    <div data-runtime-kind={kind}>
      {main.kind === "choice" ? (
        <CoreSettingsRow row={main} asSelect description={main.help ?? undefined} onChoose={onChoose} />
      ) : (
        <CoreSettingsRow row={{ ...main, help: null }} description={main.help ?? undefined} />
      )}
      {detected || path ? (
        <div data-runtime-installed={kind} className="pb-2">
          {detected ? <DetectedRow row={detected} onChoose={onChoose} /> : null}
          {path ? <PathRow kind={kind} row={path} check={check} onChoose={onChoose} /> : null}
        </div>
      ) : null}
    </div>
  );
}

/** The runtimes found for this kind (name, path, version), or why none is listed. */
function DetectedRow({ row, onChoose }: { row: SettingsRow; onChoose: (row: SettingsRow, option: string) => void }) {
  const attrs = { "data-setting-row": row.id, "data-kind": row.kind };
  if (row.kind !== "choice") {
    return (
      <div {...attrs} className="px-4 pl-8 pt-0.5 pb-1.5 text-xs text-muted-foreground">
        <span data-row-label>{row.label}</span>
      </div>
    );
  }
  // A custom path matches none of them: the drop-down then says Custom.
  const value = row.options.find((o) => o.active)?.id ?? "";
  const options = [...(value ? [] : [{ value: "", label: "Custom" }]), ...row.options.map((o) => ({ value: o.id, label: o.label }))];
  return (
    <GroupedRow {...attrs} className="justify-between gap-6 py-1.5 pl-8">
      <span className="text-[13px] text-muted-foreground" data-row-label>
        {row.label}
      </span>
      <Select aria-label={row.label} className="max-w-[26rem] min-w-64" value={value} options={options} disabled={!row.enabled} onValueChange={(v) => v && onChoose(row, v)} />
    </GroupedRow>
  );
}

/**
 * The binary that will be used, autofilled from detection: type or browse
 * to another one (checked as it is typed, saved on Enter or when the field
 * is left); "Reset to detected" goes back to the detected one.
 */
function PathRow({ kind, row, check, onChoose }: { kind: string; row: SettingsRow; check?: SettingsRow; onChoose: (row: SettingsRow, option: string) => void }) {
  const { core, data } = useBridge();
  const saved = row.value ?? "";
  const [draft, setDraft] = useState(saved);
  const [live, setLive] = useState<RuntimePathVerdict | null>(null);
  const savedRef = useRef(saved);
  useEffect(() => {
    savedRef.current = saved;
    setDraft(saved);
    setLive(null);
  }, [saved]);
  const editing = draft.trim() !== saved;
  useEffect(() => {
    if (!editing || !data) return;
    const path = draft.trim();
    if (!path) {
      setLive(null);
      return;
    }
    let stale = false;
    const t = setTimeout(() => {
      void data
        .call("runtimes.check", { kind, path })
        .then((probe: RuntimePathCheck) => {
          if (stale) return;
          setLive(core.tryCall<RuntimePathVerdict>("settings.runtimePathVerdict", { kind, check: probe }) ?? null);
        })
        .catch(() => {
          if (!stale) setLive(null);
        });
    }, CHECK_DELAY_MS);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [draft, editing, kind, data, core]);
  const commit = (value: string) => {
    if (value.trim() !== savedRef.current) onChoose(row, value.trim());
  };
  const browse = async () => {
    if (!data) return;
    const picked = await data.call("runtimes.browse", { kind }).catch(() => null);
    if (picked?.path) {
      setDraft(picked.path);
      commit(picked.path);
    }
  };
  // The state badge: what the core says of the saved path, or Custom while another is typed.
  const state = editing ? "Custom" : row.label;
  const verdict = editing ? live : check ? { ok: check.kind !== "error", line: check.label } : null;
  const id = `setting-${row.id.replace(/[^a-z0-9-]/gi, "-")}`;
  return (
    <div data-setting-row={row.id} data-kind={row.kind} data-runtime-state={state.toLowerCase()} className="px-4 pl-8 pt-1">
      <div className="flex items-center gap-2">
        <label htmlFor={id} className="w-16 shrink-0 text-[13px] text-muted-foreground" data-row-label>
          {state}
        </label>
        <Input
          id={id}
          className="h-7 min-w-0 flex-1 font-mono text-xs"
          autoComplete="off"
          spellCheck={false}
          value={draft}
          placeholder={row.placeholder ?? undefined}
          disabled={!row.enabled}
          onChange={(e) => setDraft(e.currentTarget.value)}
          onBlur={() => commit(draft)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit(draft);
            if (e.key === "Escape") setDraft(saved);
          }}
        />
        <Button size="sm" variant="outline" disabled={!row.enabled || !data} onClick={() => void browse()} data-runtime-browse>
          Browse…
        </Button>
      </div>
      <div className="flex min-h-6 items-center justify-between gap-3 pl-[4.5rem]">
        {verdict ? (
          <span data-runtime-check={verdict.ok ? "ok" : "bad"} className={cn("text-xs", verdict.ok ? "text-muted-foreground" : "text-destructive")}>
            {verdict.line}
          </span>
        ) : (
          <span />
        )}
        {row.button && !editing ? (
          <button type="button" className="shrink-0 text-xs text-brand-strong hover:underline" onClick={() => onChoose(row, "")} data-row-button>
            {row.button}
          </button>
        ) : null}
      </div>
    </div>
  );
}
