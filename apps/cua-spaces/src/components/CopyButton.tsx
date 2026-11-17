// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef, useState } from "react";

import type { FactCopy } from "../model/window";
import { Sym } from "./desktop/Sym";

/** Puts text on the clipboard (the webview's; injectable for tests). */
export type WriteText = (text: string) => Promise<void>;

const clipboardWrite: WriteText = (text) => navigator.clipboard.writeText(text);

/**
 * A fact's copy button (the core's `FactCopy`): `doc.on.doc`, then a
 * checkmark with "Copied" for `confirmMs` once the text is on the clipboard.
 */
export function CopyButton({ copy, write = clipboardWrite }: { copy: FactCopy; write?: WriteText }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const label = copied ? copy.doneHelp : copy.help;
  return (
    <button
      type="button"
      className="dw-icon-btn dw-copy"
      aria-label={label}
      title={label}
      data-copied={copied ? "true" : undefined}
      onClick={() => {
        void write(copy.text).then(
          () => {
            setCopied(true);
            window.clearTimeout(timer.current);
            timer.current = window.setTimeout(() => setCopied(false), copy.confirmMs);
          },
          (error: unknown) => console.warn("[Cua Spaces] copy failed", error),
        );
      }}
    >
      <Sym name={copied ? copy.doneSymbol : copy.symbol} size={13} />
    </button>
  );
}
