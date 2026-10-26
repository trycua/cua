// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The terminal command next to a GUI action, for people who would rather
 * type it: quiet, copyable, and only where the app's CLI is on this
 * computer (`SettingsSnapshot.cli`). The commands are `lib/cli-commands.ts`'s.
 */

import { CheckIcon, CopyIcon } from "lucide-react";
import { useState } from "react";

import { useSettings } from "@/bridge";
import { Button } from "@/components/ui/button";
import { shownCli } from "@/lib/cli-commands";
import { cn } from "@/lib/utils";

/** The CLI to name in hints, or null when they stay hidden. */
export function useCliName(): string | null {
  return shownCli(useSettings().data?.cli);
}

function useCopy(text: string): [boolean, () => void] {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };
  return [copied, copy];
}

function CopyButton({ text, className }: { text: string; className?: string }) {
  const [copied, copy] = useCopy(text);
  return (
    <Button
      type="button"
      size="icon-sm"
      variant="ghost"
      data-cli-copy=""
      aria-label={copied ? "Copied" : `Copy ${text}`}
      title={copied ? "Copied" : "Copy"}
      onClick={copy}
      className={cn("size-6 shrink-0 text-muted-foreground hover:text-foreground", className)}
    >
      {copied ? <CheckIcon className="size-3" /> : <CopyIcon className="size-3" />}
    </Button>
  );
}

/** The empty state's line: "Prefer the terminal?" and each command, copyable. */
export function CliHintLine({ commands, className }: { commands: string[]; className?: string }) {
  if (!commands.length) return null;
  return (
    <div data-cli-hint="line" className={cn("flex flex-wrap items-center justify-center gap-x-2 gap-y-1.5 text-xs text-muted-foreground", className)}>
      <span>Prefer the terminal?</span>
      {commands.map((command) => (
        <span key={command} className="inline-flex items-center rounded-md bg-foreground/[0.05] py-0.5 pr-0.5 pl-2">
          <code data-cli-command="" className="font-mono text-[11px] text-foreground/80 select-all">
            {command}
          </code>
          <CopyButton text={command} />
        </span>
      ))}
    </div>
  );
}

/** New Space's Summary block: "Or run this in Terminal", the commands, one copy. */
export function CliHintBlock({ commands }: { commands: string[] }) {
  const text = commands.join("\n");
  const [copied, copy] = useCopy(text);
  if (!commands.length) return null;
  return (
    <div data-cli-hint="block" className="mt-4">
      <div className="flex items-center justify-between px-1 pb-1.5">
        <span className="text-xs text-muted-foreground">Or run this in Terminal</span>
        <Button type="button" size="sm" variant="ghost" data-cli-copy="" onClick={copy} className="h-6 gap-1 px-1.5 text-xs text-muted-foreground hover:text-foreground">
          {copied ? <CheckIcon className="size-3" /> : <CopyIcon className="size-3" />}
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
      <pre data-cli-command="" className="overflow-x-auto rounded-lg bg-foreground/[0.04] px-3 py-2.5 font-mono text-[11.5px] leading-[1.6] whitespace-pre-wrap [overflow-wrap:anywhere] text-foreground/85 select-all dark:bg-black/25">
        {text}
      </pre>
    </div>
  );
}
