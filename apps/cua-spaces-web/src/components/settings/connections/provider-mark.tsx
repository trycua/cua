// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { HarnessMark } from "@/components/agents/harness-mark";

/** The providers whose maker's mark Cua already ships with an app, keyed by the app's id. */
const MARK_OF: Record<string, string> = {
  anthropic: "claude-code",
  openai: "codex",
  google: "gemini-cli",
  github: "copilot-cli",
};

/** A provider in a small tile: its maker's mark where there is one, else a neutral tile with its initial. */
export function ProviderMark({ id, name, large, plain }: { id: string; name: string; large?: boolean; plain?: boolean }) {
  return (
    <span data-provider-mark={id} className="contents">
      <HarnessMark agent={MARK_OF[id] ?? `provider:${id}`} name={name} large={large} plain={plain} />
    </span>
  );
}
