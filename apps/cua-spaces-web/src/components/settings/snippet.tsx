// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, CopyIcon } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";

/** A file name over text to paste into it, with a Copy button. */
export function Snippet({ file, text }: { file: string; text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    });
  };
  return (
    <div className="mt-2.5 overflow-hidden rounded-lg border bg-muted/40" data-snippet="">
      <div className="flex h-8 items-center justify-between border-b pr-1 pl-3">
        <span className="font-mono text-2xs text-muted-foreground">{file}</span>
        <Button size="icon-sm" variant="ghost" aria-label="Copy" onClick={copy}>
          {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
        </Button>
      </div>
      <pre className="overflow-x-auto px-3 py-2.5 font-mono text-xs leading-[1.55]">{text}</pre>
    </div>
  );
}
