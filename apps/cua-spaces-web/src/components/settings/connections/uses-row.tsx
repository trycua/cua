// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronsUpDownIcon, PlusIcon } from "lucide-react";

import type { AppUses, UseOption } from "@/bridge";
import { ProviderMark } from "@/components/settings/connections/provider-mark";
import { Button } from "@/components/ui/button";
import { GroupedRow } from "@/components/ui/grouped";
import { ChoiceMenu } from "@/components/ui/menu";
import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

const same = (a: UseOption, b: UseOption) => a.env === b.env && a.slot === b.slot;

/** What a credential reads as under its provider's name: "API key ending 4f2a", then the model when one was set. */
function Line({ o }: { o: UseOption }) {
  const t = useT();
  return <>{o.model ? t("settings.connections.uses.line_model", { line: o.line, model: o.model }) : o.line}</>;
}

/**
 * An app's "Uses" row (Hermes, OpenCode, Goose, Pi, OpenClaw): the provider and key it runs with, a menu
 * to point it at another connected one, and Connect another provider at the bottom of that menu.
 * Choosing signs nothing out and deletes nothing; it takes effect on the app's next run.
 */
export function UsesRow({ agent, uses, onChoose, onConnect }: { agent: string; uses: AppUses; onChoose: (o: UseOption) => void; onConnect: () => void }) {
  const t = useT();
  const cur = uses.current;
  return (
    <GroupedRow className="min-h-14" data-connector-uses={uses.state} data-connector-uses-app={agent}>
      {cur ? <ProviderMark id={cur.provider} name={cur.providerName} /> : null}
      <div className="min-w-0 flex-1">
        {cur ? (
          <>
            <div className="truncate" data-uses-provider="">
              {cur.providerName}
            </div>
            <p className="mt-px truncate text-xs text-muted-foreground" data-uses-line="">
              <Line o={cur} />
            </p>
          </>
        ) : (
          <>
            <div className="truncate">{uses.title}</div>
            <p className={cn("mt-px text-xs", uses.state === "gone" ? "text-destructive" : "text-muted-foreground")} data-uses-status="">
              {uses.status}
            </p>
          </>
        )}
      </div>
      {uses.options.length > 0 ? (
        <ChoiceMenu
          popupAttrs={{ "data-uses-popup": "" }}
          trigger={
            <Button size="sm" variant="outline" disabled={!uses.enabled} data-uses-menu="" aria-label={t("settings.connections.uses.choose_aria", { agent })}>
              {cur ? t("settings.connections.uses.change") : t("settings.connections.uses.choose")}
              <ChevronsUpDownIcon className="size-3.5 text-muted-foreground" />
            </Button>
          }
          items={uses.options.map((o) => ({
            key: `${o.env}:${o.slot ?? ""}`,
            selected: Boolean(cur && same(o, cur)),
            onSelect: () => onChoose(o),
            attrs: { "data-uses-option": o.provider },
            content: (
              <>
                <ProviderMark id={o.provider} name={o.providerName} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate">{o.providerName}</span>
                  <span className="block truncate text-xs text-muted-foreground">
                    <Line o={o} />
                  </span>
                </span>
              </>
            ),
          }))}
          action={{
            onSelect: onConnect,
            attrs: { "data-uses-connect": "" },
            content: (
              <>
                <span className="flex size-8 shrink-0 items-center justify-center text-muted-foreground">
                  <PlusIcon className="size-4" />
                </span>
                {uses.connectLabel}
              </>
            ),
          }}
        />
      ) : (
        <Button size="sm" variant="outline" disabled={!uses.enabled} data-uses-connect="" onClick={onConnect}>
          {t("settings.connections.connect")}
        </Button>
      )}
    </GroupedRow>
  );
}
