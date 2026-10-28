// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";
import { Button } from "@/components/ui/button";

/** The page a waiting sign-in finishes on, selectable and with Copy, to open
 * on any device: a machine without a browser signs in this way, as `cua auth
 * login` prints the page and its code. */
export function SignInLink({ url, compact = false }: { url: string; compact?: boolean }) {
  const [copied, setCopied] = useState(false);
  return (
    <div data-signin-link="" className="mt-2 flex items-center gap-2">
      <code title={url} className={`min-w-0 flex-1 truncate rounded-md bg-muted px-2 py-1 font-mono select-text ${compact ? "text-2xs" : "text-xs"}`}>
        {url}
      </code>
      <Button
        size="sm"
        variant="outline"
        aria-label={copied ? "Copied" : "Copy the sign-in link"}
        onClick={() => void navigator.clipboard?.writeText(url).then(() => setCopied(true), () => {})}
      >
        {copied ? "Copied" : "Copy link"}
      </Button>
    </div>
  );
}
