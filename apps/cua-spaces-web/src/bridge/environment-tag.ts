// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The title bar's small tag that says which Cua deployment this build talks
 * to ("Local · Staging", "Local · Prod"). The host resolves the channel and
 * the environment (`settings.get`'s `target`, from cua-home through
 * `app_target`), and the app core decides whether a tag shows and what it
 * says (`environment.tag`), so the macOS, Windows and Linux apps agree. The
 * released app on production shows none.
 */

import { useContext, useMemo } from "react";
import { BridgeContext } from "./BridgeProvider";
import type { CoreClient } from "./core";
import { useSettings } from "./hooks";
import type { AppTarget, UpdateChannel } from "./protocol";

/** `environment_tag::EnvironmentTag`. */
export interface EnvironmentTag {
  /** The words on the tag: `Local · Staging`. */
  label: string;
  /** The deployment, for the tint. */
  env: "prod" | "staging";
  /** The tooltip and accessible name: `This app uses Cua Staging (auth.staging.cua.ai)`. */
  description: string;
}

/** The core's tag for `target` (null: none shows, or no core to say so). */
export function environmentTag(core: CoreClient, target: AppTarget | null | undefined, updateChannel: UpdateChannel | null = null): EnvironmentTag | null {
  if (!target) return null;
  return core.tryCall<EnvironmentTag | null>("environment.tag", updateChannel ? { target, updateChannel } : { target }) ?? null;
}

/** This app's tag, or null for the released app on production. */
export function useEnvironmentTag(): EnvironmentTag | null {
  const core = useContext(BridgeContext)?.core;
  const settings = useSettings().data;
  const target = settings?.target ?? null;
  const updateChannel = settings?.values.updateChannel ?? null;
  return useMemo(() => (core ? environmentTag(core, target, updateChannel) : null), [core, target, updateChannel]);
}
