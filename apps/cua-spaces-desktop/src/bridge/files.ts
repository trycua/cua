// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's drop well (`spaces.chooseFiles`, `spaces.droppedFiles`,
// `spaces.sendFiles`), as the SwiftUI host's WebUIBridge+Files.swift: "Send
// file…" picks files with the native picker, files dropped on the well are
// read from the drop itself (the page sees only their names), and either
// list is sent with the app's transfer. Only paths picked or dropped here
// are ever sent.
//
// The drop: the sandboxed page cannot read a dropped file's path. The
// preload can (`webUtils.getPathForFile`): it keeps the last drop's paths
// and puts them on `spaces.droppedFiles` as `paths`, replacing whatever the
// page sent, so the page can never name a path itself (`preload.ts`).
import { basename } from "node:path";
import type { AppModel } from "../model/app-model";
import { string, strings } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { encode } from "./value";

/** The paths picked or dropped in this run: the only ones `spaces.sendFiles` sends. */
export class FileOffers {
  private readonly offered = new Set<string>();

  /** Remembers `paths` as sendable and answers them. */
  offer(paths: readonly string[]): string[] {
    const files = paths.filter((p) => typeof p === "string" && p !== "");
    for (const p of files) this.offered.add(p);
    return files;
  }

  has(path: string): boolean {
    return this.offered.has(path);
  }

  private fromNotch: string[] = [];

  /** Apps and files dropped on a tile in the notch: the page asks for them by name (`spaces.droppedFiles`) as for a drop on the well, once. */
  setNotchDrop(paths: readonly string[]): void {
    this.fromNotch = [...paths];
  }

  /** The notch's last drop, handed over once. */
  takeNotchDrop(): string[] {
    const paths = this.fromNotch;
    this.fromNotch = [];
    return paths;
  }
}

/** The drop's paths whose names the page saw dropped (`WebUIBridge.dropped(names:)`). */
export function dropped(paths: readonly string[], names: readonly string[]): string[] {
  const wanted = new Set(names);
  if (wanted.size === 0) return [];
  return paths.filter((p) => wanted.has(basename(p)));
}

/** Sends offered paths into a listed Space (`spaces.sendFiles`): the transfer's report per path. */
export async function sendOffered(model: AppModel, offers: FileOffers, spaceId: string, paths: string[]) {
  if (paths.length === 0) throw Failure.badArgs("paths: [string]");
  if (!paths.every((p) => offers.has(p))) throw new Failure("forbidden", "only files picked or dropped here are sent");
  if (!model.spaces.some((s) => s.id === spaceId)) throw Failure.notFound(`no Space ${spaceId}`);
  model.recordFeature("file_send");
  return model.backend.sendFiles(spaceId, paths);
}

const offersOf = new WeakMap<AppModel, FileOffers>();

/** The app's offered paths (the notch's drops join them). */
export function fileOffers(model: AppModel): FileOffers {
  let o = offersOf.get(model);
  if (!o) {
    o = new FileOffers();
    offersOf.set(model, o);
  }
  return o;
}

export function filesMethods({ model, ui }: BridgeContext): Handlers {
  const offers = fileOffers(model);
  return {
    "spaces.chooseFiles": async (_args, caller) => {
      if (!ui.chooseFiles) throw Failure.unsupported("This build has no file picker");
      return offers.offer((await ui.chooseFiles(caller.window)) ?? []);
    },
    "spaces.droppedFiles": (args) => {
      const names = strings(args, "names");
      // Put there by the preload from the drop itself (none: nothing was dropped on this window), else the notch's drop.
      const fromPage = Array.isArray(args.paths) ? args.paths.filter((p): p is string => typeof p === "string") : [];
      const paths = fromPage.length > 0 ? fromPage : offers.takeNotchDrop();
      return offers.offer(dropped(paths, names));
    },
    "spaces.sendFiles": async (args) => {
      const id = string(args, "spaceId");
      const paths = Array.isArray(args.paths) && args.paths.every((p) => typeof p === "string") ? (args.paths as string[]) : [];
      return encode(await sendOffered(model, offers, id, paths));
    },
  };
}
