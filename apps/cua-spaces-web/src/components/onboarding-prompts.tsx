// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef, useState } from "react";

/**
 * Done's example prompts (the core's three static lines, as the SwiftUI app
 * shows them under both columns): "Try asking your agent:" and a few things
 * to say once a coding agent has the cua skills and tools. Each copies to the
 * clipboard on click and says "Copied". They show on every system, whether or
 * not an agent was set up here: the lines work with any agent that has them.
 */
export function ExamplePrompts({ prompts }: { prompts: string[] }) {
  const [copied, setCopied] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  if (prompts.length === 0) return null;
  const copy = (text: string) => {
    void navigator.clipboard?.writeText(text).then(
      () => {
        setCopied(text);
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setCopied(null), 1500);
      },
      () => {},
    );
  };
  return (
    <section className="mt-8" aria-labelledby="onboarding-prompts" data-onboarding-prompts="">
      <h2 id="onboarding-prompts" className="mb-2 text-[13px] font-semibold">
        Try asking your agent:
      </h2>
      <ul className="flex flex-col gap-1">
        {prompts.map((p) => (
          <li key={p}>
            <button
              type="button"
              onClick={() => copy(p)}
              title="Copies this prompt"
              className="flex w-full items-center justify-between gap-4 rounded-lg px-3 py-1.5 text-left text-[13px] text-muted-foreground outline-none hover:bg-muted hover:text-foreground focus-visible:ring-3 focus-visible:ring-brand/25"
            >
              <span className="truncate">{`“${p}”`}</span>
              <span className="shrink-0 text-xs" aria-live="polite">
                {copied === p ? "Copied" : ""}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
