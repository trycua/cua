// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { LoaderCircleIcon } from "lucide-react";

import { useRuntimeFix, useSession } from "@/bridge";
import { Button } from "@/components/ui/button";
import { toastError } from "@/components/ui/toast";

/**
 * What the host can do about "no local runtime" (Windows: install or start
 * Docker Desktop), under the reason New Space and onboarding show. Nothing
 * when the host offers no fix.
 */
export function RuntimeFixRow({ className }: { className?: string }) {
  const { fix, run, error } = useRuntimeFix();
  const { openExternal } = useSession();
  if (!fix) return null;
  return (
    <div data-runtime-fix={fix.action ?? "busy"} className={className ?? "mt-2 flex flex-col items-start gap-1.5"}>
      <Button size="sm" variant={fix.busy ? "ghost" : "outline"} disabled={fix.busy || !fix.action} onClick={() => void run()}>
        {fix.busy ? <LoaderCircleIcon className="size-3.5 animate-spin" /> : null}
        {fix.label}
      </Button>
      {fix.note ? (
        <p className="text-xs text-muted-foreground">
          {fix.note}
          {fix.termsUrl ? (
            <>
              {" "}
              <button
                type="button"
                className="text-brand-strong hover:underline"
                onClick={() => void openExternal(fix.termsUrl!).catch(toastError("Couldn't open the page"))}
              >
                Docker's terms
              </button>
            </>
          ) : null}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      ) : null}
    </div>
  );
}
