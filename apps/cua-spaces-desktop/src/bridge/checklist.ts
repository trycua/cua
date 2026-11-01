// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The getting-started checklist's one copy (`checklist.get`,
// `checklist.send`; the SwiftUI host's WebUIBridge), kept by the app
// (`model/checklist.ts`). The page sends a step done, shown, dismiss,
// reopen, or merge (what it kept itself before, merged once); the app
// ticks steps off from what it shows itself, so `observe` is refused.
import type { AppChecklistAction } from "../native/generated/index";
import type { AppModel } from "../model/app-model";
import type { StoredChecklist } from "../model/checklist";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

/** A page's checklist action as the core's. */
export function checklistAction(model: AppModel, raw: unknown): AppChecklistAction {
  const a = raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Record<string, unknown>) : null;
  if (!a || typeof a.type !== "string") throw Failure.badArgs("action: {type}");
  const A = model.native.AppChecklistAction;
  const c = model.checklist;
  switch (a.type) {
    case "done": {
      const step = c.fromStored({ done: [String(a.step)] }).done[0];
      if (step === undefined) throw Failure.badArgs("step: one of the checklist's steps");
      return new A.Done({ step });
    }
    case "skip": {
      const step = c.fromStored({ done: [String(a.step)] }).done[0];
      if (step === undefined) throw Failure.badArgs("step: one of the checklist's steps");
      return new A.Skip({ step });
    }
    case "shown":
      return new A.Shown();
    case "dismiss":
      return new A.Dismiss();
    case "reopen":
      return new A.Reopen();
    case "merge": {
      const s = a.state;
      if (!s || typeof s !== "object" || Array.isArray(s)) throw Failure.badArgs("state: {done, dismissed, shown, skipped}");
      return new A.Merge({ state: c.fromStored(s as Partial<StoredChecklist>) });
    }
    default:
      throw Failure.badArgs("action: done, skip, shown, dismiss, reopen or merge");
  }
}

export function checklistMethods({ model }: BridgeContext): Handlers {
  return {
    "checklist.get": () => ({ state: model.checklist.stored }),
    "checklist.send": (args) => {
      model.checklist.send(checklistAction(model, args.action));
      return { state: model.checklist.stored };
    },
  };
}
