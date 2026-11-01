// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The system notification for a Space the first run created
// ("<Space> is ready", `spaces.create` with `notifyWhenReady`): the SwiftUI
// host's `SpaceReadyNote`. It carries the Cua key and a click opens the
// Space's page.
import type { SystemNote } from "./system";

/** The Cua key in the web build's directory (`public/cua-key-128.png`). */
export const SPACE_READY_ICON = "cua-key-128.png";

const OS_WORDS: Record<string, string> = { macos: "macOS ", linux: "Linux ", windows: "Windows " };

/** What the notification says: no OS word for a Space whose OS is not known, "this Mac" only on a Mac. */
export function spaceReadyNote(space: { id: string; name: string; os: string }, platform: NodeJS.Platform): SystemNote {
  const where = platform === "darwin" ? "this Mac" : "this machine";
  return {
    id: `space-ready-${space.id}`,
    title: `${space.name} is ready`,
    body: `A fresh ${OS_WORDS[space.os] ?? ""}computer inside ${where}. Your agent can use it now.`,
    route: `/spaces/${encodeURIComponent(space.id)}`,
    icon: SPACE_READY_ICON,
  };
}
