// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The getting-started checklist (the SwiftUI app's ChecklistModel.swift):
// the core's `appChecklistReduce`, kept in `checklist.json` beside the app
// core's settings. The one copy: the page reads and changes it through the
// bridge (`checklist.get`, `checklist.send`), and the app ticks steps off
// from what it shows (`appChecklistObserveFrom`, AppModel). On macOS, a
// first launch with no file of its own starts from the SwiftUI app's
// (`~/Library/Application Support/com.trycua.spaces.macos/checklist.json`),
// so nobody loses ticked steps moving between the two apps.
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import * as path from "node:path";
import type { AppChecklistAction, AppChecklistState, AppChecklistStep, AppTelemetrySignal } from "../native/generated/index";
import type { Native } from "../native/load";

/** The state as the file and the page keep it: steps as their words. */
export interface StoredChecklist {
  done: string[];
  dismissed: boolean;
  shown: boolean;
}

/** Each step and its word (`checklist::ChecklistStep::word`), in checklist order. */
export function checklistWords(native: Native): [AppChecklistStep, string][] {
  const S = native.AppChecklistStep;
  return [
    [S.AddMachine, "add_machine"],
    [S.CreateSpace, "create_space"],
    [S.Teleport, "teleport"],
    [S.Share, "share"],
    [S.RunAgent, "run_agent"],
  ];
}

export class ChecklistModel {
  state: AppChecklistState;
  /** Where usage events go (`checklist_<step>`, `checklist_completed`, ...). */
  telemetry: { record(signals: AppTelemetrySignal[]): void } | null = null;
  /** The state changed (the page's `checklist.changed`). */
  onChange: () => void = () => {};
  private readonly words: [AppChecklistStep, string][];

  /** `file`: where it is kept (null: in memory); `seed`: a file to start from when `file` has none. */
  constructor(
    private readonly native: Native,
    private readonly file: string | null,
    seed: string | null = null,
  ) {
    this.words = checklistWords(native);
    const read = (f: string | null) => (f ? this.read(f) : null);
    this.state = read(file) ?? read(seed) ?? { done: [], dismissed: false, shown: false };
  }

  /** The state with steps as words. */
  get stored(): StoredChecklist {
    return {
      done: this.state.done.map((s) => this.words.find(([step]) => step === s)?.[1]).filter((w): w is string => Boolean(w)),
      dismissed: this.state.dismissed,
      shown: this.state.shown,
    };
  }

  /** The core's state from steps as words (unknown words are dropped). */
  fromStored(s: Partial<StoredChecklist>): AppChecklistState {
    const done = Array.isArray(s.done) ? s.done : [];
    return {
      done: this.words.filter(([, w]) => done.includes(w)).map(([step]) => step),
      dismissed: s.dismissed === true,
      shown: s.shown === true,
    };
  }

  send(action: AppChecklistAction): void {
    const t = this.native.appChecklistReduce(this.state, action);
    const before = JSON.stringify(this.stored);
    this.state = t.state;
    // A merge changes the state without events (they were recorded where
    // the steps were ticked).
    if (JSON.stringify(this.stored) !== before) {
      this.save();
      this.onChange();
    }
    if (t.signals.length) this.telemetry?.record(t.signals);
  }

  private read(file: string): AppChecklistState | null {
    if (!existsSync(file)) return null;
    try {
      const raw = JSON.parse(readFileSync(file, "utf8")) as Partial<StoredChecklist> | null;
      return raw && typeof raw === "object" ? this.fromStored(raw) : null;
    } catch {
      return null;
    }
  }

  private save(): void {
    if (!this.file) return;
    try {
      mkdirSync(path.dirname(this.file), { recursive: true });
      const tmp = `${this.file}.tmp`;
      writeFileSync(tmp, JSON.stringify(this.stored));
      renameSync(tmp, this.file);
    } catch {
      // A full disk or a read-only folder: it still works until the app quits.
    }
  }
}
