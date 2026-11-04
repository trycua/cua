// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { XIcon } from "lucide-react";

import { usePlanLimitTelemetry, useRelayUsage, useSession, useUpsellGate, useUpsellPrompt } from "@/bridge";
import { Button } from "@/components/ui/button";

/**
 * A Pro or Teams prompt at the moment it fits (`share`: a Space was just
 * shared with someone; `machine`: a second machine is reached over the
 * relay). One quiet line with one button and a close; it never blocks
 * anything. The words, the plan it fits and the rules (one prompt every 3
 * days, not during a live session or an agent run) are the app core's
 * (`bridge/upsell.ts`).
 */
export function UpsellPrompt({ surface, active, className }: { surface: "share" | "machine"; active: boolean; className?: string }) {
  const usage = useRelayUsage();
  const prompt = useUpsellPrompt(surface, usage);
  const gate = useUpsellGate(surface, active && prompt !== null, true, { plan: usage?.plan });
  const plans = usePlanLimitTelemetry();
  const { openExternal } = useSession();
  if (!prompt || !gate.show) return null;
  return (
    <div role="note" data-upsell={surface} className={`flex items-start gap-3 rounded-lg bg-muted px-3.5 py-2.5 text-xs ${className ?? ""}`}>
      <p className="min-w-0 flex-1 leading-relaxed text-muted-foreground">{prompt.text}</p>
      <Button
        size="sm"
        variant="secondary"
        data-upsell-cta=""
        onClick={() => {
          gate.click();
          void openExternal(plans.link(prompt.cta.url, surface === "share" ? "app_share" : "app_machine")).catch(() => {});
        }}
      >
        {prompt.cta.label}
      </Button>
      <Button size="icon-sm" variant="ghost" aria-label={prompt.dismiss} data-upsell-dismiss="" onClick={gate.dismiss}>
        <XIcon className="size-3.5" />
      </Button>
    </div>
  );
}
