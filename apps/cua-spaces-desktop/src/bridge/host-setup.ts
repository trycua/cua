// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This machine's setup and buttons (`host.setUp`, `host.action`,
// `host.openSettings`), over `model/host.ts` (the SwiftUI host's
// WebUIBridge+Pages "This machine"). A failure is worded for people (the
// title, the plain message, the raw error as details, and Retry or Sign In):
// the page shows it as the native form does. Setup and Sign In can wait on
// the browser sign-in for minutes; the page waits with them (no timeout).
import type { AppHostActionId, AppHostSetupRequest } from "../native/generated/index";
import type { HostSetupFailure } from "../model/host-failure";
import { object, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Handlers } from "./host";
import { hostStatus } from "./machines";

/** A failed setup or button as the page shows it: the plain message, with the title, the raw error and the button's label alongside. */
export function hostFailure(f: HostSetupFailure): Failure {
  return new Failure("failed", f.message, f);
}

const isHostFailure = (e: unknown): e is HostSetupFailure =>
  !!e && typeof e === "object" && typeof (e as HostSetupFailure).kind === "string" && typeof (e as HostSetupFailure).details === "string";

/** The page's setup request (`AppHostSetupRequest`); empty strings are absent. */
export function hostRequest(r: BridgeArgs): AppHostSetupRequest {
  const mode = r.mode;
  if (typeof mode !== "string" || mode === "") throw Failure.badArgs("request.mode");
  const text = (k: string) => (typeof r[k] === "string" && r[k] !== "" ? (r[k] as string) : undefined);
  const strings = Array.isArray(r.allow) && r.allow.every((a) => typeof a === "string") ? (r.allow as string[]) : undefined;
  const flag = (k: string) => (typeof r[k] === "boolean" ? (r[k] as boolean) : undefined);
  return {
    mode,
    relayUrl: text("relayUrl"),
    direct: text("direct"),
    name: text("name"),
    allow: strings,
    profile: text("profile"),
    shareDesktop: flag("shareDesktop"),
    provideSpaces: flag("provideSpaces"),
  };
}

/** The page's action words, as the SwiftUI host reads them. */
export const HOST_ACTIONS = {
  "sign-in": "SignIn",
  "stop-sharing": "StopSharing",
  "resume-sharing": "ResumeSharing",
  remove: "Remove",
  "share-desktop": "ShareDesktop",
  "hide-desktop": "HideDesktop",
  "provide-spaces": "ProvideSpaces",
  "stop-providing-spaces": "StopProvidingSpaces",
} as const;

/** Where `host.openSettings` may go: macOS's privacy panes (the host reports none elsewhere). */
export function settingsPaneUrl(url: string, platform: NodeJS.Platform): string {
  if (platform !== "darwin") throw Failure.unsupported("This system has no privacy settings for the host to open");
  if (!url.startsWith("x-apple.systempreferences:")) throw Failure.badArgs("Only System Settings panes open here");
  return url;
}

export function hostSetupMethods({ model, platform, system }: BridgeContext): Handlers {
  const unavailable = () => Failure.unsupported(`${platform === "darwin" ? "This Mac" : platform === "win32" ? "This PC" : "This computer"} can't be set up for access in this build`);
  return {
    "host.setUp": async (args) => {
      if (!model.host.host) throw unavailable();
      const request = hostRequest(object(args, "request"));
      try {
        await model.host.setUp(request);
      } catch (error) {
        throw isHostFailure(error) ? hostFailure(error) : error;
      }
      return hostStatus(model);
    },
    "host.action": async (args) => {
      if (!model.host.host) throw unavailable();
      const word = string(args, "action");
      const key = HOST_ACTIONS[word as keyof typeof HOST_ACTIONS];
      if (!key) throw Failure.badArgs(`${word} is not a host action`);
      await model.host.run(model.native.AppHostActionId[key] as AppHostActionId);
      if (model.host.actionFailure) throw hostFailure(model.host.actionFailure);
      return hostStatus(model);
    },
    "host.openSettings": async (args) => {
      const url = settingsPaneUrl(string(args, "url"), platform);
      if (!system) throw Failure.unsupported("Opening settings is not available in this build");
      await system.openExternal(url);
      return null;
    },
  };
}
