// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the app model feeds the macOS notch beyond its Spaces (the SwiftUI
// app's CuaSpacesMacApp.start): each tile's thumbnail from the shared store,
// the activity indicator, and where its clicks and drops go. No Electron
// here (main.ts passes the windows and the notification in).
//
// - A tile's click selects that Space and shows it in the main window.
// - Files dropped on a tile are sent into that Space with the app's transfer
//   (the drop well's path, `spaces.sendFiles`), the indicator on while they
//   go, and the Space shown; a notification says what landed or why not.
//   An app bundle among them opens the Teleport review for that app in that
//   Space, as a drop on the Space's well does (`?dropped=` on its page).
import { basename } from "node:path";
import { fileOffers, sendOffered } from "./bridge/files";
import type { AppModel } from "./model/app-model";
import { words } from "./model/errors";
import type { Notch } from "./notch";

export interface NotchFeedUi {
  /** Shows a Space in the main window (its page); `dropped`: the names of the apps and files just dropped on its tile, which the page treats as a drop on the well. */
  showSpace(spaceId: string, dropped?: string[]): void;
  /** A system notification. */
  notify(title: string, body: string): void;
}

/** An app bundle (dropped apps go to Teleport, not into the Space as files). */
export const isAppBundle = (path: string) => /\.app\/?$/i.test(path);

/** A tile's screenshot, no older than the policy's open interval (the notch is open). */
export function notchThumbnail(model: AppModel): (spaceId: string) => Promise<Uint8Array | null> {
  return async (spaceId) => (await model.thumbnails.refresh(spaceId, model.thumbnails.policy.openIntervalMs / 1000))?.image ?? null;
}

/** A tile's click: select the Space and show it. */
export function openSpaceFromNotch(model: AppModel, ui: NotchFeedUi, spaceId: string, dropped?: string[]): void {
  if (model.spaces.some((s) => s.id === spaceId)) model.select(spaceId);
  ui.showSpace(spaceId, dropped);
}

/** Files dropped on a tile: sent into the Space with the indicator on, then said. */
export async function dropOnSpace(model: AppModel, ui: NotchFeedUi, spaceId: string, paths: string[]): Promise<void> {
  if (paths.some(isAppBundle)) {
    // An app: its Teleport review, the files beside it going with it (the Swift tile's drop). The page asks for these paths by name, as for a drop on the well.
    fileOffers(model).setNotchDrop(paths);
    openSpaceFromNotch(model, ui, spaceId, paths.map((p) => basename(p)));
    return;
  }
  openSpaceFromNotch(model, ui, spaceId);
  const files = paths.filter((p) => !isAppBundle(p));
  if (files.length === 0) return;
  const name = model.spaces.find((s) => s.id === spaceId)?.name ?? spaceId;
  // A native drag onto the notch: the person dropped exactly these.
  const offers = fileOffers(model);
  offers.offer(files);
  try {
    const sent = await model.activity.during(() => sendOffered(model, offers, spaceId, files));
    ui.notify(`Sent to ${name}`, model.native.appDropSentText(sent));
  } catch (error) {
    const what = files.length === 1 ? basename(files[0]!) : `${files.length} files`;
    ui.notify(`Couldn't send ${what} to ${name}`, words(error));
  }
}

/**
 * Feeds the notch the Spaces, the "Spaces tab in the notch" setting, the live
 * Keyvault sign-ins (less the copies dismissed from the notch: the SwiftUI
 * app's `syncNotchKeyvault`) and the activity, now and on every change.
 * Returns the stop.
 */
export function feedNotch(notch: Pick<Notch, "setSpaces" | "setShown" | "setActivity" | "setKeyvault">, model: AppModel): () => void {
  const keyvault = () => notch.setKeyvault(model.keyvault.notchLabel, model.keyvault.signedIn(model.spaces, true));
  const spaces = () => {
    notch.setSpaces(model.spaces);
    notch.setShown(!model.settings.menuBar);
    // The tiles' key follows the Spaces too.
    keyvault();
  };
  const activity = () => notch.setActivity(model.activity.hotspot, model.activity.transfer ? {} : null);
  spaces();
  activity();
  const stops = [
    model.subscribe((change) => {
      if (change === "spaces" || change === "settings") spaces();
      else if (change === "keyvault") keyvault();
    }),
    model.activity.subscribe(activity),
  ];
  return () => stops.forEach((s) => s());
}
