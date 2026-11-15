// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The launch on the page's startup screen (`ops/startup.ts` in the web
// bridge): `startup.get` and `startup.act {action}` answer the
// `StartupState`, and `startup.changed` (with the state) goes out whenever
// it changes (the SwiftUI host's WebUIBridge+Startup.swift).
import { STARTUP_ACTIONS, startupState, type StartupAction } from "../model/startup";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

export function startupMethods({ model, ui }: BridgeContext): Handlers {
  const startup = model.startup;
  return {
    "startup.get": () => startupState(startup),
    "startup.act": (args) => {
      const action = args.action;
      if (typeof action !== "string" || !STARTUP_ACTIONS.includes(action as StartupAction)) {
        throw Failure.badArgs("startup.act: action is allowAccess, tryAgain or signInAgain");
      }
      // The keychain prompt shows over the app that asked: be in front.
      if (action !== "signInAgain") ui.activate();
      startup.act(action as StartupAction);
      return startupState(startup);
    },
  };
}

/** Tells the page each time the launch moves on. */
export function followStartup({ model, events }: BridgeContext): () => void {
  return model.startup.subscribe(() => events.emit("startup.changed", startupState(model.startup)));
}
