// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Teleport an app (`teleport.*`): the SDK's teleport handle (catalog,
// windows, icons, plans, runs with `teleport.progress`), as the SwiftUI
// host's `TeleportModel` and picker sources call it. The host keeps the
// SDK's entries and plans between the page's steps (`TeleportCache`).
import type { SpaceLike, TeleportLike } from "../native/generated/index";
import { dataURL, groupsOf, iconRequest, liveSources, moveOf, rememberChoice, runReport, spaceName, TeleportCache, teleportConsent } from "../model/teleport";
import { object, string } from "./args";
import { spaceStreams } from "./space-detail";
import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Handlers } from "./host";
import { encode } from "./value";

export function teleportMethods(ctx: BridgeContext): Handlers {
  const { model, events, ui } = ctx;
  const cache = ctx.teleportCache ?? new TeleportCache();
  const native = model.native;
  // A run (and the sites list) can make the daemon ask for presence in the
  // system's own dialog, which shows over the foreground window.
  const foreground = () => {
    if (ctx.platform !== "darwin") ui.activate();
  };

  const spaceId = (args: BridgeArgs) => string(args, "spaceId");

  /** The SDK's teleport handle and the Space's: it needs a reachable Space. */
  const context = async (args: BridgeArgs): Promise<{ teleport: TeleportLike; space: SpaceLike }> => {
    const found = await model.backend.teleportContext(spaceId(args));
    if (!found) throw Failure.unsupported("Teleport needs a reachable Space");
    return found;
  };

  const handle = (): TeleportLike => {
    const teleport = model.backend.teleportHandle();
    if (!teleport) throw Failure.unsupported("Teleport is not available");
    return teleport;
  };

  return {
    "teleport.catalog": async (args) => {
      const { teleport, space } = await context(args);
      const entries = await teleport.catalog(await teleport.spaceHint(space));
      cache.setCatalog(entries);
      return entries.map((e) => encode(native.appCatalogEntry(e)));
    },
    "teleport.entryForPath": async (args) => {
      const teleport = handle();
      const entry = teleport.catalogEntryForPath(string(args, "path"), undefined);
      cache.addEntry(entry);
      return encode(native.appCatalogEntry(entry));
    },
    "teleport.windows": async () => encode(await liveSources(model.backend.teleportHandle(), null).openWindows()),
    "teleport.remoteWindows": async (args) => {
      const { teleport, space } = await context(args);
      return encode(await liveSources(teleport, space).remoteWindows());
    },
    "teleport.icon": async (args) => {
      const icon = object(args, "icon");
      switch (icon.kind) {
        case "host":
          return dataURL(await liveSources(model.backend.teleportHandle(), null).hostIcon(string(icon, "path")));
        case "guest": {
          const [bytes] = await model.backend.appIcons(spaceId(args), [iconRequest(icon)]);
          return dataURL(bytes);
        }
        default:
          return null;
      }
    },
    "teleport.thumbnail": async (args) => {
      const t = object(args, "thumbnail");
      switch (t.kind) {
        case "host-window": {
          const id = t.windowId;
          if (typeof id !== "number" || !Number.isInteger(id) || id < 0) throw Failure.badArgs("windowId");
          return dataURL(await liveSources(model.backend.teleportHandle(), null).hostThumbnail(id));
        }
        case "guest-window": {
          const { teleport, space } = await context(args);
          const epoch = typeof t.epoch === "number" && t.epoch >= 0 ? BigInt(Math.floor(t.epoch)) : 0n;
          return dataURL(await liveSources(teleport, space).guestThumbnail(string(t, "windowId"), epoch));
        }
        default:
          return null;
      }
    },
    "teleport.plan": async (args) => {
      const { teleport, space } = await context(args);
      const entry = object(args, "entry");
      const sdk = cache.entry(string(entry, "id"));
      if (!sdk) throw Failure.notFound("Read the catalog again");
      const files = Array.isArray(args.files) && args.files.every((f) => typeof f === "string") ? (args.files as string[]) : [];
      const plan = await teleport.plan(sdk, space, {
        moves: moveOf(native, string(args, "move")),
        files,
        stateItems: undefined,
        sensitiveGroups: groupsOf(native, args.sensitiveGroups),
        scope: undefined,
        launch: true,
      });
      const view = native.appTeleportPlan(plan);
      cache.keepPlan(spaceId(args), { json: view.json, plan, providerId: view.app.providerId });
      return encode(view);
    },
    "teleport.run": async (args) => {
      const { teleport, space } = await context(args);
      const planArg = object(args, "plan");
      const planKey = spaceId(args);
      const kept = cache.plan(planKey);
      if (!kept || kept.json !== string(planArg, "json")) throw Failure.notFound("Plan the teleport again");
      try {
        const runId = string(args, "runId");
        const consentArgs = object(args, "consent");
        const consent = teleportConsent(consentArgs);
        // Next time the review starts from the sites sent now, and the notch
        // shows the transfer while it runs, as the native sheet does.
        rememberChoice(model, kept.providerId, planKey, consentArgs);
        foreground();
        const report = await model.activity.during(() =>
          teleport.run(kept.plan, space, consent, {
            onEvent: (event) => events.emit("teleport.progress", { runId, event: encode(native.appTeleportRunEvent(event)) }),
          }),
        );
        return runReport(report);
      } finally {
        // Finished, failed or cancelled: the plan is spent.
        cache.dropPlan(planKey, kept.json);
      }
    },
    "teleport.sites": async (args) => {
      // Counts per site, never values; the daemon asks for presence before it
      // shows names, as for the native review.
      const client = model.keyvault.client;
      if (!client) throw Failure.unsupported("The Keyvault is not available");
      foreground();
      return encode(await client.inventory(string(args, "providerId"), undefined));
    },
    "teleport.remembered": (args) => {
      const key = native.appReviewRememberKey(string(args, "providerId"), spaceName(model, spaceId(args)));
      return native.appReviewRemembered(model.settings.teleportChoices, key) ?? null;
    },
    "teleport.streamWindow": async (args) => {
      // The Space's window, onto this machine as a picture-in-picture panel (the Stream section's pop-out).
      const { rows, pips } = spaceStreams(ctx).get(spaceId(args));
      const id = string(args, "windowId");
      if (!rows.window(id)) await rows.refresh();
      const window = rows.window(id);
      if (!window) throw Failure.notFound(`no window ${id}`);
      if (!pips) throw Failure.unsupported("Picture in picture is not available in this build");
      pips.popOut({ kind: "window", window });
      return null;
    },
  };
}
