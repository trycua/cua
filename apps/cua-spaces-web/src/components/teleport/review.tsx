// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { GlobeIcon, LoaderCircleIcon, SearchIcon } from "lucide-react";
import type { ReactNode } from "react";

import type { ReviewView, TeleportSession, TeleportStore } from "@/bridge";
import { Checkbox } from "@/components/ui/checkbox";
import { cn } from "@/lib/utils";
import { SavedItems } from "./saved-items";

/**
 * The consent review, as the SwiftUI app's `TeleportReview`: a browser's
 * sites with counts (the minimal default is the sites that keep a sign-in,
 * never an identity provider), the other items with what each sends, the
 * installs and caveats, and the checkboxes the core gates Teleport on. What
 * the review shows and what is sent are the core's; the SDK's approval
 * check enforces them.
 */
export function TeleportReview({
  review: r,
  readingSites,
  vault,
  teleport,
}: {
  review: ReviewView;
  readingSites: boolean;
  vault: TeleportSession["vault"];
  teleport: TeleportStore;
}) {
  const installs = r.items.filter((i) => i.kind === "install");
  const fromVault = r.source === "vault";
  return (
    <div className="space-y-4 px-5 pb-4">
      {r.offersVault ? <Source review={r} teleport={teleport} /> : null}

      {fromVault ? (
        <Group title="Saved in the Keyvault">
          {vault ? <SavedItems view={vault.view} query={vault.query} teleport={teleport} /> : <Reading />}
        </Group>
      ) : null}

      {!fromVault && r.offersDomains ? (
        <Group title="Sites" aside={r.domains.length || r.domainQuery ? r.domainSummary : null}>
          <Sites review={r} reading={readingSites || r.needsDomains} teleport={teleport} />
        </Group>
      ) : null}

      {!fromVault && r.toggles.length ? (
        <Group title="Also sent">
          <ul className="divide-y">
            {r.toggles.map((t) => (
              <li key={t.key} data-review-item={t.key} data-kind={t.sensitive ? "secret" : "state"} data-sent={t.selected || undefined}>
                <label className="flex cursor-default items-start gap-3 px-4 py-2.5">
                  <Checkbox
                    className="mt-0.5"
                    aria-label={`Send ${t.label}`}
                    checked={t.selected}
                    onCheckedChange={() => teleport.send({ type: "toggle-item", key: t.key })}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block text-[13px]" data-review-label>
                      {t.label}
                    </span>
                    {t.detail ? <span className="block truncate text-xs text-muted-foreground">{t.detail}</span> : null}
                  </span>
                  <span className="shrink-0 text-xs text-muted-foreground">{t.sensitive ? "Secret" : t.bytes > 0 ? teleport.bytes(t.bytes) : null}</span>
                </label>
              </li>
            ))}
          </ul>
        </Group>
      ) : null}

      {r.offersPasswords ? (
        <Group>
          <label className="flex cursor-default items-start gap-3 px-4 py-2.5">
            <Checkbox className="mt-0.5" checked={r.includePasswords} onCheckedChange={(value) => teleport.send({ type: "toggle-passwords", value })} />
            <span>
              <span className="block text-[13px]">{r.passwordsLabel}</span>
              <span className="block text-xs text-muted-foreground">
                Off by default. Passwords are re-encrypted for the browser in the Space and need that browser to have been opened once.
              </span>
            </span>
          </label>
        </Group>
      ) : null}

      {installs.length || r.warnings.length ? (
        <Group>
          <ul className="divide-y">
            {installs.map((i) => (
              <li key={i.key} className="flex items-baseline justify-between gap-4 px-4 py-2.5" data-review-item={i.key} data-kind="install">
                <span className="text-[13px]" data-review-label>
                  {i.label}
                </span>
                <span className="text-xs text-muted-foreground">{i.detail}</span>
              </li>
            ))}
            {r.warnings.map((w) => (
              <li key={w} className="px-4 py-2.5 text-xs text-muted-foreground" data-review-warning>
                {w}
              </li>
            ))}
          </ul>
        </Group>
      ) : null}

      {r.needsAcknowledgement || (r.offersSaveToKeyvault && r.source === "live") || r.needsRelayPlaintextAcknowledgement ? (
        <div className="space-y-2.5 px-1">
          {r.needsAcknowledgement ? (
            <Gate data="ack" checked={r.acknowledged} onChange={(value) => teleport.send({ type: "acknowledge", value })}>
              Send the secrets listed above
            </Gate>
          ) : null}
          {r.offersSaveToKeyvault && r.source === "live" ? (
            <Gate data="save" checked={r.saveToKeyvault} onChange={(value) => teleport.send({ type: "save-to-keyvault", value })}>
              Save to Keyvault for reuse
            </Gate>
          ) : null}
          {r.needsRelayPlaintextAcknowledgement ? (
            <Gate
              data="ack-relay"
              checked={r.acknowledgedRelayPlaintext}
              onChange={(value) => teleport.send({ type: "acknowledge-relay-plaintext", value })}
              note="This Space predates end-to-end sealing. The relay could read these secrets in transit."
            >
              Send without end-to-end encryption
            </Gate>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

/** Live app or saved items (`flow.review`'s labels and note). */
function Source({ review: r, teleport }: { review: ReviewView; teleport: TeleportStore }) {
  const options = [
    { value: "live", label: r.liveLabel },
    { value: "vault", label: r.vaultLabel },
  ] as const;
  return (
    <Group>
      <RadioGroup value={r.source} onValueChange={(v) => void teleport.sendFrom(v as "live" | "vault")} className="divide-y" aria-label="Send from" data-review-source>
        {options.map((o) => (
          <label key={o.value} className="flex cursor-default items-center gap-3 px-4 py-2.5 text-[13px]" data-source={o.value}>
            <Radio.Root
              value={o.value}
              className="flex size-4 shrink-0 items-center justify-center rounded-full border border-input bg-card shadow-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/60 data-checked:border-brand data-checked:bg-brand dark:bg-input/30"
            >
              <Radio.Indicator className="size-1.5 rounded-full bg-white" />
            </Radio.Root>
            {o.label}
          </label>
        ))}
      </RadioGroup>
      {r.sourceNote ? (
        <p className="border-t px-4 py-2 text-xs text-muted-foreground" data-review-source-note>
          {r.sourceNote}
        </p>
      ) : null}
    </Group>
  );
}

function Reading() {
  return (
    <p className="flex items-center gap-2 px-4 py-3 text-[13px] text-muted-foreground">
      <LoaderCircleIcon className="size-3.5 animate-spin" /> Reading the saved items…
    </p>
  );
}

function Group({ title, aside, children }: { title?: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <section className="overflow-hidden rounded-lg border bg-card">
      {title ? (
        <header className="flex items-center justify-between border-b px-4 py-2 text-xs">
          <h3 className="font-medium text-muted-foreground">{title}</h3>
          {aside ? (
            <span className="text-muted-foreground tabular-nums" data-review-sites-summary>
              {aside}
            </span>
          ) : null}
        </header>
      ) : null}
      {children}
    </section>
  );
}

function Gate({ data, checked, onChange, note, children }: { data: string; checked: boolean; onChange: (v: boolean) => void; note?: string; children: ReactNode }) {
  return (
    <label className="flex cursor-default items-start gap-2.5" data-gate={data} data-checked={checked || undefined}>
      <Checkbox className="mt-0.5" checked={checked} onCheckedChange={onChange} />
      <span>
        <span className="block text-[13px]">{children}</span>
        {note ? <span className="block text-xs text-muted-foreground">{note}</span> : null}
      </span>
    </label>
  );
}

/** The browser's sites, with counts and a search. */
function Sites({ review: r, reading, teleport }: { review: ReviewView; reading: boolean; teleport: TeleportStore }) {
  if (reading) {
    return (
      <p className="flex items-center gap-2 px-4 py-3 text-[13px] text-muted-foreground">
        <LoaderCircleIcon className="size-3.5 animate-spin" /> Reading the sites…
      </p>
    );
  }
  if (!r.domains.length && !r.domainQuery) {
    return <p className="px-4 py-3 text-[13px] text-muted-foreground">No sites to choose. The cookies listed under Also sent are sent as they are.</p>;
  }
  return (
    <>
      <div className="flex items-center gap-2 border-b px-4 py-1.5">
        <SearchIcon className="size-3.5 shrink-0 text-muted-foreground" />
        <input
          aria-label="Search sites"
          placeholder="Search sites"
          value={r.domainQuery}
          onChange={(e) => teleport.send({ type: "domain-query", text: e.target.value })}
          className="h-7 min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-muted-foreground"
        />
        <button type="button" className="text-xs text-brand hover:underline" onClick={() => teleport.send({ type: "select-shown-domains", value: true })}>
          All
        </button>
        <button type="button" className="text-xs text-brand hover:underline" onClick={() => teleport.send({ type: "select-shown-domains", value: false })}>
          None
        </button>
      </div>
      <ul className="max-h-52 divide-y overflow-y-auto">
        {r.domains.map((d) => (
          <li key={d.domain} data-review-site={d.domain} data-selected={d.selected || undefined} className={cn(!d.selectable && "opacity-55")}>
            <label className="flex cursor-default items-center gap-2.5 px-4 py-2">
              <Checkbox
                aria-label={`Send ${d.domain}`}
                checked={d.selected}
                disabled={!d.selectable}
                onCheckedChange={() => teleport.send({ type: "toggle-domain", domain: d.domain })}
              />
              <GlobeIcon className="size-3.5 shrink-0 text-muted-foreground" />
              <span className="truncate text-[13px]">{d.domain}</span>
              <span className="truncate text-xs text-muted-foreground">{d.counts}</span>
              <span className="flex-1" />
              {d.identityProvider ? (
                <span className="shrink-0 text-xs text-amber-600 dark:text-amber-400" title="Its session signs in to other apps. Send it only if you mean to.">
                  Identity provider
                </span>
              ) : d.signin ? (
                <span className="shrink-0 text-xs text-muted-foreground">Signs you in</span>
              ) : null}
            </label>
            {d.unavailable > 0 ? <p className="-mt-1 pr-4 pb-2 pl-[66px] text-xs text-muted-foreground/70">{d.unavailableNote}</p> : null}
          </li>
        ))}
      </ul>
    </>
  );
}
