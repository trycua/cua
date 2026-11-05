// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The create the new setup sends for the first Space: the wizard's own
 * Linux offer for this machine (`wizard.firstSpaceOffers`), under the home's
 * fixed name. Kept free of React and the `@/` alias so the desktop app's
 * tests run it against the host's real `spaces.create`.
 */

import type { SpaceCreateConfig } from "../../bridge/contracts/host";
import type { CreateSpaceArgs } from "../../bridge/contracts/new-space";

/** What this reads of a first-Space offer (`bridge/new-space.ts`'s `FirstSpaceOffer`). */
export interface HomeOffer {
  id: string;
  os: string;
  image: string;
  blocked: string | null;
  args: CreateSpaceArgs;
}

/** The page's create request (`bridge/store.ts`'s `CreateSpaceRequest`). */
export type HomeCreateRequest = SpaceCreateConfig & { os: "linux" };

/** The labels of the home that warms in the background (`registry::HIDDEN_LABEL` and `RELAY_LABEL`). */
export const HOME_WARMING_LABELS = { "cua.hidden": "home", "cua.relay": "off" } as const;

/** The words preparing stops with when this machine has no Linux Space to offer. */
export const NO_LINUX_OFFER = "This computer can't make a Linux Space right now. Open New Space to see why.";

/**
 * The home's create request from the offers, or an error in plain words.
 * Never a request without an image: the host refuses that one with its
 * internal argument check, which is what a person used to see.
 */
export function homeCreateRequest(offers: readonly HomeOffer[], name: string): HomeCreateRequest {
  const linux = offers.find((o) => o.id === "linux") ?? offers.find((o) => o.os === "linux");
  if (!linux) throw new Error(NO_LINUX_OFFER);
  if (linux.blocked) throw new Error(linux.blocked);
  const image = linux.args.image || linux.image;
  if (!image) throw new Error(NO_LINUX_OFFER);
  // The home warms in the background: labelled hidden and off the relay, so it stays out of the top bar and
  // off the phone's list until a person writes into its thread (the daemon then makes it visible).
  return { ...linux.args, image, os: "linux", name, labels: { ...HOME_WARMING_LABELS } };
}
