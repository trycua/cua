// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The first run's saved state (the SwiftUI app's OnboardingModel
// `completed`, `finish` and `restart`): `onboarding.json` next to the app
// settings, `{"completed": true, "mode": "host" | "client"}`, the same file
// the Swift app writes (taken over on macOS, `migrate-swift.ts`). The pages
// themselves are the web UI's (`routes/onboarding`, on the app core's
// flow); this host says whether they are due and records the end.
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import * as path from "node:path";

import { keepDamaged } from "../damaged";

export type OnboardingMode = "client" | "host";

export interface OnboardingState {
  completed: boolean;
  mode: OnboardingMode;
}

export class OnboardingStore {
  private shownAgain = false;
  private readonly listeners = new Set<() => void>();

  constructor(readonly file: string) {}

  /** What the file says (a missing or damaged file: the first run is due; a damaged one is kept as `.damaged`). */
  read(): OnboardingState {
    let text: string;
    try {
      text = readFileSync(this.file, "utf8");
    } catch {
      return { completed: false, mode: "client" };
    }
    try {
      const raw: unknown = JSON.parse(text);
      if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw new Error("not an object");
      const { completed, mode } = raw as { completed?: unknown; mode?: unknown };
      return { completed: completed === true && !this.shownAgain, mode: mode === "host" ? "host" : "client" };
    } catch {
      keepDamaged(this.file);
      return { completed: false, mode: "client" };
    }
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private changed(): void {
    for (const l of [...this.listeners]) l();
  }

  /** "Start using Cua Spaces": saved, as the Swift app saves it. */
  finish(mode: OnboardingMode): void {
    this.shownAgain = false;
    mkdirSync(path.dirname(this.file), { recursive: true });
    const tmp = `${this.file}.tmp`;
    writeFileSync(tmp, JSON.stringify({ completed: true, mode }));
    renameSync(tmp, this.file);
    this.changed();
  }

  /** Settings' Welcome "Show again": the flow is due again until it finishes (the file is left as it was). */
  restart(): void {
    this.shownAgain = true;
    this.changed();
  }
}
