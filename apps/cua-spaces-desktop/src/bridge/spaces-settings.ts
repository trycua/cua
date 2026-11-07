// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The account's default Space (`spaces.settings`, `spaces.setSettings`,
// `spaces.resolveDefault`; the web bridge's ops/spaces-settings.ts): the
// SDK's `Devices.spaces_settings`, `set_spaces_settings` and
// `resolve_default_space` from this enrolled device, the same calls as the
// SwiftUI host's. Null before the session is up or signed out. Nothing here
// creates a Space.
import { optionalString, bool, strings } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { encode } from "./value";

/** Space ids the relay keeps: `relay:<machine>` or `relay:<machine>/<name>`. */
const SPACE_ID = /^relay:[^/\s]+(\/[a-z0-9-]{1,63})?$/;

export function spacesSettingsMethods({ model }: BridgeContext): Handlers {
  /** The session's `Devices`, or null (signed out, not set up yet): the page then shows nothing. */
  const devices = () => (model.servicesIn && model.devices.signedIn ? model.devices.devices() : null);
  return {
    "spaces.settings": async () => {
      const d = devices();
      return d ? encode(await d.spacesSettings()) : null;
    },
    "spaces.setSettings": async (args) => {
      const defaultSpace = optionalString(args, "defaultSpace");
      const fallback = strings(args, "fallback");
      const fallbackOn = bool(args, "fallbackOn");
      for (const id of [defaultSpace, ...fallback]) if (id !== null && !SPACE_ID.test(id)) throw Failure.badArgs(`not a Space id: ${id}`);
      const d = devices();
      if (!d) return null;
      return encode(await d.setSpacesSettings({ defaultSpace: defaultSpace ?? undefined, fallback, fallbackOn, updatedAt: 0n }));
    },
    "spaces.resolveDefault": async () => {
      const d = devices();
      return d ? encode(await d.resolveDefaultSpace()) : null;
    },
  };
}
