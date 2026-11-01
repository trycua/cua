// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronRightIcon, CopyIcon, ExternalLinkIcon, LockIcon } from "lucide-react";
import { useState } from "react";

import { useConnectCloud, useSession, type CloudSheetAction, type CloudSheetField, type CloudSheetView } from "@/bridge";
import { CloudLogo } from "@/components/cloud-logo";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { MenuSelect } from "./menu-select";

/** The sheet's own background, not the glass over the page: AWS's dark logo (a white file) blends into it. */
const SOLID = { backgroundColor: "var(--popover)" } as const;

/**
 * "Connect a cloud": a picker of five clouds, then one short sheet for the
 * cloud picked (region, a required monthly budget, the largest machine and
 * one button), and "Or have your agent do it" with the prompt in a
 * read-only field. The app core decides every word and request
 * (`cloudSheet.*`); this draws it.
 */
/** The cloud comparison on cua.ai (live since cloud #8819). */
const COMPARE_CLOUDS_URL = "https://cua.ai/compare/clouds";

export function ConnectCloudDialog() {
  const connect = useConnectCloud();
  const { openExternal } = useSession();
  const v = connect.view;
  const send = (action: CloudSheetAction) => void connect.send(action);

  return (
    <Dialog open={connect.open && v !== null} onOpenChange={(open) => !open && connect.close()}>
      <DialogPopup className="top-[7vh] w-[min(520px,calc(100vw-2rem))]" style={SOLID} data-connect-cloud="" data-stage={v?.stage}>
        {v ? (
          <form
            onSubmit={(e) => {
              e.preventDefault();
              if (v.primaryEnabled) send({ type: "press" });
            }}
          >
            <div className="max-h-[80vh] overflow-y-auto px-6 pt-5 pb-3">
              <div className="flex items-center gap-2">
                {v.logo ? <CloudLogo cloud={v.logo} size={16} /> : null}
                <DialogTitle className="text-[15px] font-semibold" data-cloud-title="">
                  {v.title}
                </DialogTitle>
              </div>
              <p className="mt-1.5 text-[13px] leading-relaxed text-muted-foreground" data-cloud-intro="">
                {v.intro}{" "}
                {/* The one cua.ai compare page the product links to (tests/test_product_compare_links.py, CUA-2200). */}
                <button
                  type="button"
                  data-compare-clouds=""
                  onClick={() => void openExternal(COMPARE_CLOUDS_URL).catch(() => {})}
                  className="cursor-default whitespace-nowrap text-muted-foreground underline decoration-foreground/30 underline-offset-2 outline-none hover:text-foreground focus-visible:text-foreground"
                >
                  Compare clouds ↗
                </button>
              </p>

              {v.stage === "picker" ? (
                <div className="mt-3.5 divide-y border-y" role="list" aria-label="Clouds">
                  {v.rows.map((r) => (
                    <button
                      key={r.id}
                      type="button"
                      role="listitem"
                      data-cloud-row={r.id}
                      onClick={() => send({ type: "choose", id: r.id })}
                      className="flex w-full cursor-default items-center gap-3 py-2.5 text-left outline-none hover:bg-foreground/[0.03] focus-visible:bg-foreground/[0.05]"
                    >
                      <span className="grid w-8 place-items-center">
                        <CloudLogo cloud={r.logo} size={16} />
                      </span>
                      <span className="w-40 text-[13px]" data-cloud-row-title="">
                        {r.name}
                      </span>
                      <span className="flex-1 text-xs text-muted-foreground" data-cloud-row-auth="">
                        {r.note || r.auth}
                      </span>
                      <ChevronRightIcon className="size-3.5 text-muted-foreground/60" />
                    </button>
                  ))}
                </div>
              ) : (
                <>
                  {v.fields.length ? (
                    <div className="mt-3.5 divide-y border-y">
                      {v.fields.map((f) => (
                        <FieldRow key={f.id} field={f} token={connect.token} setToken={connect.setToken} send={send} />
                      ))}
                    </div>
                  ) : null}
                  {v.trust ? <TrustStep trust={v.trust} openPage={connect.openPage} /> : null}
                  {v.error ? (
                    <p role="alert" data-cloud-error="" className="mt-2.5 text-xs text-destructive">
                      {v.error}
                    </p>
                  ) : null}
                  {v.note ? (
                    <p className="mt-2.5 text-xs text-muted-foreground" data-cloud-note="">
                      {v.note}
                      {v.detailsLabel ? (
                        <>
                          {" "}
                          <button type="button" onClick={() => connect.openPage(v.detailsUrl)} className="text-brand-strong hover:underline" data-cloud-details="">
                            {v.detailsLabel}
                          </button>
                        </>
                      ) : null}
                    </p>
                  ) : null}
                </>
              )}

              {v.prompt ? <PromptField title={v.promptTitle} copyLabel={v.copyLabel} prompt={v.prompt} /> : null}
            </div>

            <div className="flex items-center gap-2 border-t px-6 py-3">
              <Button type="button" variant="outline" data-cloud-cancel="" onClick={connect.close}>
                {v.cancelLabel}
              </Button>
              {v.stage === "cloud" ? (
                <Button type="button" variant="ghost" data-cloud-back="" onClick={() => send({ type: "back" })}>
                  {v.backLabel}
                </Button>
              ) : null}
              <span className="flex-1" />
              {v.stage === "cloud" ? (
                <Button type="submit" data-cloud-primary="" disabled={!v.primaryEnabled}>
                  {v.primaryLabel}
                  {v.primaryLabel.startsWith("Open ") ? <ExternalLinkIcon /> : null}
                </Button>
              ) : null}
            </div>
          </form>
        ) : null}
      </DialogPopup>
    </Dialog>
  );
}

/** One line of the sheet: its label, and the control on the right. */
function FieldRow({ field: f, token, setToken, send }: { field: CloudSheetField; token: string; setToken: (t: string) => void; send: (a: CloudSheetAction) => void }) {
  const action = (text: string): CloudSheetAction =>
    f.id === "place" ? { type: "set-place", text } : f.id === "budget" ? { type: "set-budget", text } : f.id === "size" ? { type: "set-size", text } : { type: "set-account-id", text };
  return (
    <div className="flex items-center justify-between gap-4 py-2" data-cloud-field={f.id}>
      <label htmlFor={`cc-${f.id}`} className="text-[13px]">
        {f.label}
      </label>
      {f.kind === "select" ? (
        <MenuSelect aria-label={f.label} value={f.value} items={f.options} onValueChange={(value) => send(action(value))} dataName="cloud-select" />
      ) : f.kind === "secret" ? (
        <div className="relative w-[250px]">
          <LockIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3 -translate-y-1/2 text-muted-foreground" />
          <Input
            id={`cc-${f.id}`}
            type="password"
            value={token}
            placeholder={f.placeholder}
            spellCheck={false}
            autoComplete="off"
            className="h-7 pl-7"
            onChange={(e) => setToken(e.target.value)}
          />
        </div>
      ) : f.kind === "money" ? (
        <div className="relative w-[104px]">
          <span aria-hidden className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-[13px] text-muted-foreground">
            $
          </span>
          <Input
            id={`cc-${f.id}`}
            value={f.value}
            placeholder={f.placeholder}
            spellCheck={false}
            autoComplete="off"
            inputMode="decimal"
            className="h-7 pl-5 text-right"
            onChange={(e) => send(action(e.target.value))}
          />
        </div>
      ) : (
        <Input
          id={`cc-${f.id}`}
          value={f.value}
          placeholder={f.placeholder}
          spellCheck={false}
          autoComplete="off"
          className="h-7 w-[200px]"
          onChange={(e) => send(action(e.target.value))}
        />
      )}
    </div>
  );
}

/** After AWS opened: what to do there, and what to bring back (the account id is the field above). */
function TrustStep({ trust, openPage }: { trust: NonNullable<CloudSheetView["trust"]>; openPage: (url: string) => void }) {
  return (
    <div className="mt-3 space-y-2" data-cloud-trust="">
      <p className="text-[13px] leading-relaxed text-muted-foreground">{trust.text}</p>
      {trust.openUrl ? (
        <button type="button" onClick={() => openPage(trust.openUrl)} className="inline-flex items-center gap-1 text-[13px] text-brand-strong hover:underline" data-cloud-open="">
          {trust.openLabel}
          <ExternalLinkIcon className="size-3" />
        </button>
      ) : null}
      {trust.paste ? <PromptField title="Paste this in AWS CloudShell" copyLabel={trust.copyLabel} prompt={trust.paste} mono /> : null}
    </div>
  );
}

/** "Or have your agent do it": the prompt in a read-only field, and Copy. */
function PromptField({ title, copyLabel, prompt, mono }: { title: string; copyLabel: string; prompt: string; mono?: boolean }) {
  const [copied, setCopied] = useState(false);
  const copy = () =>
    void navigator.clipboard?.writeText(prompt).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  return (
    <div className="mt-4" data-cloud-prompt-field="">
      <div className="flex items-center justify-between">
        <label htmlFor="cc-prompt" className="text-[13px] font-medium">
          {title}
        </label>
        <Button type="button" size="sm" variant="ghost" className="-mr-2" onClick={copy} data-cloud-copy="">
          {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
          {copied ? "Copied" : copyLabel}
        </Button>
      </div>
      <textarea
        id="cc-prompt"
        readOnly
        rows={Math.max(2, Math.ceil(prompt.length / 62))}
        value={prompt}
        data-cloud-prompt=""
        className={`mt-1 block w-full resize-none rounded-lg border border-input bg-card px-3 py-2 text-[13px] leading-relaxed outline-none [field-sizing:content] focus-visible:border-ring dark:bg-input/30 ${mono ? "font-mono text-xs" : "font-sans"}`}
      />
    </div>
  );
}
