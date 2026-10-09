// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A run's events as a conversation: a TypeScript mirror of
 * `cua-agents/src/transcript.rs` (`Transcript`), the one rule every client
 * uses. That crate is not part of the app core's wasm, so the web UI
 * carries this copy; keep the two in step.
 *
 * - the agent's `message` chunks join into one item per stretch of prose;
 * - the prompt (`turn_started`, or a `user_message` echo when the turn has
 *   no prompt yet) is one `user` item;
 * - consecutive `activity` events of a turn fold into one `activity` item
 *   (`5 steps`, `3 steps, 1 error`); a tool update rewrites its call's step;
 * - `hidden` events are dropped.
 *
 * It folds incrementally and skips events it has already absorbed. Items
 * are immutable: an absorb replaces only the items it touched, so a list
 * that memoizes rows by item re-renders only the growing one.
 */

import type { AgentEvent } from "./contracts/agents";

export interface TranscriptStep {
  /** Stable within the item (the tool id, or the event's seq). */
  id: string;
  text: string;
  /** An `error` event, or a tool whose status is `failed` (for display). */
  failed: boolean;
}

export interface TranscriptItem {
  /** Stable for the life of the item: the kind and the first event's seq. */
  id: string;
  kind: "user" | "message" | "activity";
  turn: number;
  /** The message or prompt; for `activity`, the group's summary (`5 steps`). */
  text: string;
  /** `activity` only, oldest first. */
  steps: readonly TranscriptStep[];
  /** When the first event of the item was written (Unix ms). */
  startedMs: number;
}

const SUMMARY_MAX = 160;

/** `text` on one line, cut to `max` characters with an ellipsis. */
export function oneLine(text: string, max = SUMMARY_MAX): string {
  const flat = text.split(/\s+/).filter(Boolean).join(" ");
  if ([...flat].length <= max) return flat;
  return `${[...flat].slice(0, max - 1).join("").trimEnd()}…`;
}

function stepsLabel(steps: number, errors: number): string {
  const s = steps === 1 ? "1 step" : `${steps} steps`;
  return errors === 0 ? s : errors === 1 ? `${s}, 1 error` : `${s}, ${errors} errors`;
}

const firstLine = (t?: string) => t?.split("\n").map((l) => l.trim()).find(Boolean);

/** The event's one line when the writer gave none (`AgentEvent::summary`). */
export function eventSummary(e: AgentEvent): string | undefined {
  if (e.category !== "activity") return undefined;
  const tool = () => (e.tool_title?.trim() ? e.tool_title : (e.tool_kind ?? "tool"));
  let line: string | undefined;
  switch (e.kind) {
    case "thought": {
      const f = firstLine(e.text);
      line = f ? `Thinking: ${f}` : "Thinking";
      break;
    }
    case "tool_call":
      line = `Tool ${tool()}`;
      break;
    case "tool_update": {
      if (!e.tool_status) return undefined;
      const f = firstLine(e.text);
      line = `Tool ${tool()} ${e.tool_status}${f ? `: ${f}` : ""}`;
      break;
    }
    case "plan":
      line = "Updated the plan";
      break;
    case "permission":
      line = e.text ? `Allowed ${tool()} (${e.text})` : `Permission for ${tool()}`;
      break;
    case "turn_ended":
      line = e.stop_reason ? `Turn ${e.turn} ended (${e.stop_reason})` : `Turn ${e.turn} ended`;
      break;
    case "notice":
      line = `Notice: ${e.text ?? ""}`;
      break;
    case "error":
      line = `Error: ${e.text ?? ""}`;
      break;
    case "exited": {
      const f = firstLine(e.text);
      line = f ? `Exited: ${f}` : "Exited";
      break;
    }
    case "cancel_requested":
      line = "Interrupt requested";
      break;
    case "install":
      line = `Install ${(e.text ?? "").trim()}`;
      break;
    default:
      return undefined;
  }
  return oneLine(line);
}

export class Transcript {
  private list: TranscriptItem[] = [];
  private errors = new Map<number, number>();
  /** Tool id -> where its step is, and the call's title. */
  private tools = new Map<string, { item: number; step: number; title?: string }>();
  private lastSeq = 0;

  get items(): readonly TranscriptItem[] {
    return this.list;
  }

  /** The highest seq absorbed. */
  get seq(): number {
    return this.lastSeq;
  }

  /** Absorbs a page; true when anything changed. Returns a new `items` array when it did. */
  absorbAll(events: readonly AgentEvent[]): boolean {
    const before = this.list;
    this.list = [...this.list];
    let changed = false;
    for (const e of events) changed = this.absorb(e) || changed;
    if (!changed) this.list = before;
    return changed;
  }

  private replace(i: number, patch: Partial<TranscriptItem>): void {
    this.list[i] = { ...this.list[i]!, ...patch };
  }

  private absorb(e: AgentEvent): boolean {
    if (e.seq !== 0) {
      if (e.seq <= this.lastSeq) return false;
      this.lastSeq = e.seq;
    }
    const last = this.list.length - 1;
    const tail = this.list[last];
    switch (e.category) {
      case "message": {
        const text = e.text ?? "";
        if (!text) return false;
        if (tail && tail.kind === "message" && tail.turn === e.turn) this.replace(last, { text: tail.text + text });
        else this.list.push({ id: `message:${e.seq}`, kind: "message", turn: e.turn, text, steps: [], startedMs: e.ts_ms });
        return true;
      }
      case "user": {
        const text = e.text ?? "";
        if (e.kind !== "turn_started") {
          let hasPrompt = false;
          for (let i = last; i >= 0 && this.list[i]!.turn === e.turn; i--) if (this.list[i]!.kind === "user") hasPrompt = true;
          if (hasPrompt || !text) return false;
        }
        this.list.push({ id: `user:${e.seq}`, kind: "user", turn: e.turn, text, steps: [], startedMs: e.ts_ms });
        return true;
      }
      case "activity": {
        const known = e.tool_id ? this.tools.get(e.tool_id) : undefined;
        const ev = known?.title && !e.tool_title?.trim() ? { ...e, tool_title: known.title, summary: undefined } : e;
        const line = ev.summary ?? eventSummary(ev);
        if (!line) return false;
        const failed = ev.kind === "error" || ev.tool_status === "failed";
        if (ev.kind === "tool_update" && known && this.list[known.item]) {
          const item = this.list[known.item]!;
          const steps = [...item.steps];
          steps[known.step] = { ...steps[known.step]!, text: line, failed };
          this.replace(known.item, { steps });
          return true;
        }
        const i = tail && tail.kind === "activity" && tail.turn === ev.turn ? last : this.list.length;
        if (i === this.list.length) {
          this.list.push({ id: `activity:${ev.seq}`, kind: "activity", turn: ev.turn, text: "", steps: [], startedMs: ev.ts_ms });
        }
        const item = this.list[i]!;
        // Only `error` events count toward the label, as in transcript.rs.
        const errors = (this.errors.get(i) ?? 0) + (ev.kind === "error" ? 1 : 0);
        this.errors.set(i, errors);
        const steps = [...item.steps, { id: ev.tool_id ?? `seq:${ev.seq}`, text: line, failed }];
        this.replace(i, { steps, text: stepsLabel(steps.length, errors) });
        if (ev.tool_id && (ev.kind === "tool_call" || ev.kind === "tool_update")) {
          this.tools.set(ev.tool_id, { item: i, step: steps.length - 1, title: ev.tool_title?.trim() ? ev.tool_title : undefined });
        }
        return true;
      }
      default:
        return false;
    }
  }
}
