// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The account (`session.*`): who is signed in, the sign-in in progress and
// the window chrome's words (the SwiftUI host's `session`).
import type { AppModel } from "../model/app-model";
import type { BridgeContext } from "./context";
import type { Handlers } from "./host";
import { encode } from "./value";

export function sessionState(model: AppModel) {
  return {
    identity: model.identity,
    signedIn: model.identity !== null,
    cloudConfigured: model.cloudConfigured,
    signIn: encode(model.signIn),
    chrome: encode(model.chrome),
  };
}

export function sessionMethods({ model }: BridgeContext): Handlers {
  return {
    "session.get": () => sessionState(model),
    // The browser flow can take minutes: answer now, and `session.changed`
    // follows when it ends.
    "session.signIn": () => {
      void model.beginSignIn();
      return sessionState(model);
    },
    "session.signOut": async () => {
      await model.signOut();
      return sessionState(model);
    },
  };
}
