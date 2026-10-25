// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useRelayUsage, useSession, useUpsellCard, type RelayUsage, type UpsellCardReason } from "@/bridge";
import { Button } from "@/components/ui/button";

/**
 * What a limit offers, at the moment it stops someone: what happened, the one
 * line of what Pro or Teams gives, the price, Upgrade (opens the checkout on
 * the account app) and Not now. Behind the `upsell_cards` flag; nothing is
 * drawn while it is off, there is nothing to offer, or after Not now. The
 * words and the plan each reason leads to are the app core's
 * (`upsell.card`). It never blocks anything: the relay and the control plane
 * enforce the limit.
 *
 * `onClose` runs on Not now, for a caller that keeps its own dismissal.
 * `usage` overrides the relay's answer (a plan known from elsewhere).
 */
export function UpgradeCard({
  reason,
  usage,
  onClose,
  className,
}: {
  reason: UpsellCardReason;
  usage?: Partial<RelayUsage> | null;
  onClose?: () => void;
  className?: string;
}) {
  const relay = useRelayUsage();
  const { openExternal } = useSession();
  const { card, click, dismiss } = useUpsellCard(reason, usage ?? relay);
  if (!card) return null;
  return (
    <section role="note" data-upgrade-card={card.reason} className={`rounded-lg border bg-card px-4 py-3.5 ${className ?? ""}`}>
      <h3 className="text-[13px] font-semibold" data-upgrade-title>
        {card.title}
      </h3>
      <p className="mt-1 text-xs leading-relaxed text-muted-foreground" data-upgrade-body>
        {card.body}
      </p>
      <div className="mt-3 flex items-center gap-2">
        <Button
          size="sm"
          data-upgrade-cta=""
          onClick={() => {
            click();
            void openExternal(card.cta.url).catch(() => {});
          }}
        >
          {card.cta.label}
        </Button>
        <span className="text-xs text-muted-foreground" data-upgrade-price>
          {card.price}
        </span>
        <Button
          size="sm"
          variant="ghost"
          className="ml-auto"
          data-upgrade-dismiss=""
          onClick={() => {
            dismiss();
            onClose?.();
          }}
        >
          {card.dismiss}
        </Button>
      </div>
    </section>
  );
}
