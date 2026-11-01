// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, CopyIcon } from "lucide-react";
import { useState, type CSSProperties } from "react";

import { Button } from "@/components/ui/button";

/** Copy `text` to the clipboard; "Copied" for a moment after. */
export function CopyButton({ text, label, disabled, className }: { text: string; label: string; disabled?: boolean; className?: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1200);
      },
      () => {},
    );
  };
  return (
    <Button size="sm" variant="ghost" aria-label={label} className={className} disabled={disabled || !text} onClick={copy}>
      {copied ? <CheckIcon /> : <CopyIcon />} {copied ? "Copied" : "Copy"}
    </Button>
  );
}

/**
 * A line to give an AI agent: a read-only filled box with Copy. The words come
 * from the app core (`agent_prompts`); this only draws them.
 */
export function PromptLine({ id, label, text }: { id: string; label: string; text: string }) {
  return (
    <div data-prompt-line={id} className="mt-1.5">
      <div className="flex items-center justify-between">
        <label htmlFor={`prompt-${id}`} className="text-xs text-muted-foreground">
          {label}
        </label>
        <CopyButton text={text} label={`Copy ${label.toLowerCase()}`} />
      </div>
      <textarea
        id={`prompt-${id}`}
        readOnly
        rows={2}
        value={text}
        style={{ fieldSizing: "content" } as CSSProperties}
        className="mt-1 block w-full resize-none rounded-lg bg-muted px-2.5 py-2 text-[12.5px] leading-relaxed text-foreground outline-none"
      />
    </div>
  );
}
