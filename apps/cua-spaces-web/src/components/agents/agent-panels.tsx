// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ReactNode } from "react";

import type { AgentDetailHook, AgentDetailView, AgentLineView, AgentTab, RoutineScheduleKind } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

const WEEKDAYS = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"] as const;

/**
 * A persistent agent's Memory, Routines and Access to this computer, as the
 * SwiftUI app's Agents page draws them (`AgentsViews.swift`). Every word in
 * the lists and buttons is the app core's (`AgentDetailView`).
 */
export function AgentPanel({ tab, detail }: { tab: AgentTab; detail: AgentDetailHook }) {
  const view = detail.view;
  if (!view) return <div className="flex-1" />;
  return (
    <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4" data-agent-panel={tab}>
      {tab === "memory" ? <Memory view={view} detail={detail} /> : tab === "routines" ? <Routines view={view} detail={detail} /> : <Access view={view} detail={detail} />}
      {detail.error ? (
        <p className="mt-3 px-1 text-[13px] text-destructive" role="alert" data-agent-detail-error="">
          {detail.error}
        </p>
      ) : null}
    </div>
  );
}

function Memory({ view, detail }: { view: AgentDetailView; detail: AgentDetailHook }) {
  const { act, busy } = detail;
  const f = view.file;
  if (f) {
    return (
      <>
        <Section title={f.path} kind="file">
          <pre className="max-h-72 overflow-auto px-4 py-3 font-mono text-xs whitespace-pre-wrap select-text" data-agent-file-text="">
            {f.text}
          </pre>
        </Section>
        <Section kind="versions">
          {f.versions.map((l) => (
            <Line key={l.id} line={l} busy={busy} onAction={() => act({ type: "restore", version: l.id })} />
          ))}
        </Section>
        <Button variant="outline" size="sm" onClick={() => act({ type: "close-file" })} data-agent-file-close="">
          {f.closeLabel}
        </Button>
      </>
    );
  }
  return (
    <Section kind="memory">
      {view.memory.length === 0 ? <Empty>{view.memoryEmpty}</Empty> : null}
      {view.memory.map((l) => (
        <Line key={l.id} line={l} busy={busy} onOpen={() => act({ type: "open-file", path: l.id })} />
      ))}
    </Section>
  );
}

function Routines({ view, detail }: { view: AgentDetailView; detail: AgentDetailHook }) {
  const { act, busy } = detail;
  const form = view.form;
  const schedules = view.schedules.map((s) => ({ value: s.id as RoutineScheduleKind, label: s.text }));
  return (
    <>
      <Section kind="routines">
        {view.routines.length === 0 ? <Empty>{view.routinesEmpty}</Empty> : null}
        {view.routines.map((l) => (
          <Line
            key={l.id}
            line={l}
            busy={busy}
            onToggle={() => act({ type: "toggle-routine", id: l.id })}
            onAction={() => act({ type: "remove-routine", id: l.id })}
          />
        ))}
      </Section>
      <Section kind="new-routine">
        <Field label="Title">
          <Input value={form.title} onValueChange={(title) => act({ type: "set-title", title })} data-routine-title="" />
        </Field>
        <Field label="Prompt">
          <Input value={form.prompt} onValueChange={(prompt) => act({ type: "set-prompt", prompt })} data-routine-prompt="" />
        </Field>
        <Field label="Schedule">
          <Select aria-label="Schedule" value={form.schedule} options={schedules} onValueChange={(schedule) => act({ type: "set-schedule", schedule })} />
        </Field>
        {form.schedule === "every" ? (
          <Field label="Minutes">
            <Input
              type="number"
              min={1}
              max={1440}
              className="w-24"
              value={String(form.minutes)}
              onValueChange={(v) => act({ type: "set-minutes", minutes: Math.min(1440, Math.max(1, Math.floor(Number(v) || 1))) })}
              data-routine-minutes=""
            />
          </Field>
        ) : (
          <Field label="Time">
            <Input type="time" className="w-32" value={form.time} onValueChange={(time) => act({ type: "set-time", time })} data-routine-time="" />
          </Field>
        )}
        {form.schedule === "weekly" ? (
          <Field label="Day">
            <Select aria-label="Day" value={form.weekday} options={WEEKDAYS.map((d) => ({ value: d, label: d }))} onValueChange={(weekday) => act({ type: "set-weekday", weekday })} />
          </Field>
        ) : null}
        <div className="flex justify-end px-4 py-2.5">
          <Button size="sm" disabled={!view.canAddRoutine} onClick={() => act({ type: "add-routine" })} data-routine-add="">
            {view.addRoutineLabel}
          </Button>
        </div>
      </Section>
    </>
  );
}

function Access({ view, detail }: { view: AgentDetailView; detail: AgentDetailHook }) {
  const { act, busy, thisMachine } = detail;
  return (
    <>
      <Section kind="access">
        {view.access.length === 0 ? <Empty>{view.accessEmpty}</Empty> : null}
        {view.access.map((l) => (
          <Line key={l.id} line={l} busy={busy} onAction={() => act({ type: "revoke-computer", machine: l.id })} />
        ))}
        {view.allowThisMachineLabel && thisMachine ? (
          <div className="flex justify-end px-4 py-2.5">
            <Button size="sm" disabled={busy} onClick={() => act({ type: "allow-computer", machine: thisMachine })} data-agent-allow="">
              {view.allowThisMachineLabel}
            </Button>
          </div>
        ) : null}
      </Section>
      {view.audit.length > 0 ? (
        <Section title="Audit" kind="audit">
          {view.audit.map((l) => (
            <Line key={l.id} line={l} busy={busy} />
          ))}
        </Section>
      ) : null}
    </>
  );
}

function Section({ title, kind, children }: { title?: string; kind: string; children: ReactNode }) {
  return (
    <section className="mb-4" data-agent-section={kind}>
      {title ? <h3 className="mb-2 truncate px-1 text-xs font-semibold text-muted-foreground">{title}</h3> : null}
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">{children}</div>
    </section>
  );
}

function Empty({ children }: { children: ReactNode }) {
  return <p className="px-4 py-3 text-[13px] text-muted-foreground">{children}</p>;
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <label className="flex min-h-11 items-center gap-4 px-4 py-2">
      <span className="w-20 shrink-0 text-[13px]">{label}</span>
      <span className="flex min-w-0 flex-1 justify-end">{children}</span>
    </label>
  );
}

/** A `LineView`: its text (a link when it opens), a muted trailing part, a switch and a button. */
function Line({ line, busy, onOpen, onToggle, onAction }: { line: AgentLineView; busy: boolean; onOpen?: () => void; onToggle?: () => void; onAction?: () => void }) {
  return (
    <div className="flex min-h-11 items-center gap-4 px-4 py-2" data-agent-line={line.id}>
      {onOpen ? (
        <button
          type="button"
          disabled={busy}
          onClick={onOpen}
          className="min-w-0 flex-1 cursor-default truncate text-left text-[13px] text-brand outline-none hover:underline focus-visible:underline disabled:opacity-60"
          title={line.text}
          data-line-text=""
        >
          {line.text}
        </button>
      ) : (
        <span className="min-w-0 flex-1 truncate text-[13px]" title={line.text} data-line-text="">
          {line.text}
        </span>
      )}
      {line.trailing ? (
        <span className="max-w-[45%] shrink-0 truncate text-xs text-muted-foreground" title={line.trailing} data-line-trailing="">
          {line.trailing}
        </span>
      ) : null}
      {line.on !== null && line.on !== undefined && onToggle ? (
        <Switch aria-label={line.text} checked={line.on} disabled={busy} onCheckedChange={onToggle} data-line-toggle="" />
      ) : null}
      {line.actionLabel && onAction ? (
        <Button variant="outline" size="sm" disabled={busy} onClick={onAction} data-line-action="">
          {line.actionLabel}
        </Button>
      ) : null}
    </div>
  );
}
