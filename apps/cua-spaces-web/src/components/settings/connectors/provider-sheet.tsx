// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronLeftIcon, ChevronRightIcon, CopyIcon, KeyRoundIcon, Loader2Icon, LogInIcon, SmartphoneIcon, TerminalIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { importSheet, useBridge, type ProviderWay } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useSession } from "@/bridge";
import { closeDeviceSignIn, closeImport, useConnectors, useDeviceSignIn, useImporting, useProviderSheetOpen, useSheetWay } from "@/hooks/use-connectors";

const ICON = { "app-token": LogInIcon, "app-device": SmartphoneIcon, token: KeyRoundIcon, key: KeyRoundIcon, shell: TerminalIcon } as const;

/**
 * "Connect <provider>": the ways the core offers (`connectors.providerSheet`),
 * one row each, then the key field when Paste an API key is picked. Opened
 * from Providers, from an app's page, and from an app's Connect when the app
 * has no sign-in command of its own. Mounted once, beside Share.
 */
export function ProviderSheetHost() {
  const device = useDeviceSignIn();
  const open = useProviderSheetOpen();
  const importing = useImporting();
  if (importing) return <ImportSheet />;
  if (device) return <DeviceCodeSheet />;
  return open ? <ProviderSheet /> : null;
}

/**
 * Importing the sign-in Claude Code already has: first what is read, kept and
 * sent (the core's `connectors.importSheet` words), then Use my sign-in; while
 * it reads (macOS may ask to allow it); and when nothing is found, the ways
 * on in order: sign in with Claude Code, paste a token, use an API key.
 */
function ImportSheet() {
  const { core } = useBridge();
  const c = useConnectors();
  const importing = useImporting();
  const [copied, setCopied] = useState(false);
  const v = importSheet(core);
  if (!importing || !v) return null;
  const reading = importing.phase === "reading";
  return (
    <Dialog open onOpenChange={(o) => !o && !reading && closeImport()}>
      <DialogPopup data-import-sheet={importing.phase} className="top-[12vh] w-[min(520px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          {importing.phase === "not-found" ? (
            <>
              <DialogTitle className="text-[15px] font-semibold">{v.notFoundTitle}</DialogTitle>
              <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{importing.line ?? v.notFoundLede}</DialogDescription>
              <div className="mt-3">
                {v.options.map((o) => {
                  const Icon = o.id === "login" ? LogInIcon : KeyRoundIcon;
                  return (
                    <div key={o.id} data-import-option={o.id} className="-mx-2 flex w-[calc(100%+1rem)] items-center gap-3 rounded-lg px-2 py-2.5">
                      <span className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                        <Icon className="size-3.5" />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="block text-[13px]">{o.label}</span>
                        <span className="block text-xs text-muted-foreground">{o.help}</span>
                        {o.command ? (
                          <span className="mt-1.5 flex items-center justify-between gap-2 rounded-lg bg-muted px-3 py-1.5">
                            <code data-import-command="" className="font-mono text-xs select-all">
                              {o.command}
                            </code>
                            <Button
                              size="sm"
                              variant="ghost"
                              onClick={() => {
                                void navigator.clipboard?.writeText(o.command ?? "").catch(() => undefined);
                                setCopied(true);
                              }}
                            >
                              {copied ? <CheckIcon /> : <CopyIcon />}
                              {copied ? "Copied" : "Copy"}
                            </Button>
                          </span>
                        ) : null}
                      </span>
                      <Button
                        size="sm"
                        variant="outline"
                        data-import-option-action={o.id}
                        onClick={() => {
                          if (o.id === "login") void c.confirmImport();
                          else {
                            closeImport();
                            c.openProvider("anthropic", importing.app, o.id);
                          }
                        }}
                      >
                        {o.button}
                      </Button>
                    </div>
                  );
                })}
              </div>
              <div className="mt-4 flex justify-end">
                <Button variant="outline" onClick={closeImport}>
                  {v.cancelLabel}
                </Button>
              </div>
            </>
          ) : (
            <>
              <DialogTitle className="text-[15px] font-semibold" data-import-title="">
                {v.title}
              </DialogTitle>
              <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{v.lede}</DialogDescription>
              <ul data-import-points="" className="mt-3 space-y-2 text-[13px]">
                {v.points.map((p) => (
                  <li key={p}>{p}</li>
                ))}
              </ul>
              {reading ? (
                <p className="mt-4 flex items-center gap-2 text-xs text-muted-foreground" data-import-reading="">
                  <Loader2Icon className="size-3.5 animate-spin" /> {v.readingLabel}
                </p>
              ) : null}
              <div className="mt-4 flex justify-end gap-2">
                <Button variant="outline" disabled={reading} onClick={closeImport}>
                  {v.cancelLabel}
                </Button>
                <Button disabled={reading} data-import-confirm="" onClick={() => void c.confirmImport()}>
                  {v.confirmLabel}
                </Button>
              </div>
            </>
          )}
        </div>
      </DialogPopup>
    </Dialog>
  );
}

/**
 * A device-code sign-in (Codex with ChatGPT): the one-time code, Copy, and
 * the page to enter it on. The sheet closes by itself once it's approved;
 * the sign-in was made for Cua in a folder Cua owns.
 */
function DeviceCodeSheet() {
  const device = useDeviceSignIn();
  const { openExternal } = useSession();
  const [copied, setCopied] = useState(false);
  if (!device) return null;
  return (
    <Dialog open onOpenChange={(o) => !o && closeDeviceSignIn()}>
      <DialogPopup data-device-sign-in={device.app} className="top-[12vh] w-[min(440px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          <DialogTitle className="text-[15px] font-semibold">Sign in with ChatGPT</DialogTitle>
          <DialogDescription className="mt-1 text-[13px] text-muted-foreground">Enter this code on the sign-in page. It works once and expires in 15 minutes.</DialogDescription>
          <div className="mt-4 flex items-center justify-between gap-3 rounded-lg bg-muted px-4 py-3">
            <span data-device-code="" className="font-mono text-[20px] font-semibold tracking-[0.12em] select-all">
              {device.code}
            </span>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                void navigator.clipboard?.writeText(device.code).catch(() => undefined);
                setCopied(true);
              }}
            >
              {copied ? <CheckIcon /> : <CopyIcon />}
              {copied ? "Copied" : "Copy"}
            </Button>
          </div>
          <p className="mt-3 flex items-center gap-2 text-xs text-muted-foreground">
            <Loader2Icon className="size-3.5 animate-spin" /> Waiting for you to enter it…
          </p>
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="outline" onClick={() => closeDeviceSignIn()}>
              Close
            </Button>
            <Button data-device-open="" onClick={() => void openExternal(device.url).catch(() => {})}>
              Open sign-in page
            </Button>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

function ProviderSheet() {
  const c = useConnectors();
  const v = c.sheet;
  const [keyWay, setKeyWay] = useState<ProviderWay | null>(null);
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const startWay = useSheetWay();
  // Sent here from "nothing found" to paste a token or a key: open straight on that field.
  useEffect(() => {
    if (startWay && v) setKeyWay(v.ways.find((w) => w.id === startWay) ?? null);
  }, [startWay, v?.provider]); // eslint-disable-line react-hooks/exhaustive-deps
  const close = () => {
    setKeyWay(null);
    setValue("");
    setError(null);
    c.closeProvider();
  };
  if (!v) return null;
  const run = async (way: ProviderWay, typed?: string) => {
    setBusy(true);
    setError(null);
    const ok = await c.runWay(way, typed);
    setBusy(false);
    if (ok) close();
    else if (way.id === "key" || way.id === "token") setError(way.id === "token" ? "Couldn't save the token. Check it and try again." : "Couldn't save the key. Check it and try again.");
  };
  return (
    <Dialog open onOpenChange={(o) => !o && close()}>
      <DialogPopup data-provider-sheet={v.provider} className="top-[12vh] w-[min(520px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          {keyWay ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                if (value.trim()) void run(keyWay, value);
              }}
            >
              <DialogTitle className="flex items-center gap-1.5 text-[15px] font-semibold">
                <button
                  type="button"
                  aria-label="Back"
                  onClick={() => setKeyWay(null)}
                  className="-ml-1.5 inline-flex size-6 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
                >
                  <ChevronLeftIcon className="size-4" />
                </button>
                {v.title}
              </DialogTitle>
              <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{keyWay.help}</DialogDescription>
              <label className="mt-4 block">
                <span className="mb-1 block text-xs font-medium">{keyWay.id === "token" ? v.tokenLabel : v.keyLabel}</span>
                <Input
                  type="password"
                  autoFocus
                  data-provider-key=""
                  value={value}
                  placeholder={keyWay.id === "token" ? v.tokenPlaceholder : v.keyPlaceholder}
                  autoComplete="off"
                  spellCheck={false}
                  className="font-mono"
                  onChange={(e) => setValue(e.currentTarget.value)}
                />
              </label>
              <p className="mt-1.5 text-xs text-muted-foreground">{v.keyHelp}</p>
              {error ? <p className="mt-2 text-[13px] text-destructive">{error}</p> : null}
              <div className="mt-4 flex justify-end gap-2">
                <Button type="button" variant="outline" onClick={close}>
                  {v.cancelLabel}
                </Button>
                <Button type="submit" disabled={busy || !value.trim()} data-provider-save="">
                  {v.saveLabel}
                </Button>
              </div>
            </form>
          ) : (
            <>
              <DialogTitle className="text-[15px] font-semibold" data-provider-sheet-title="">
                {v.title}
              </DialogTitle>
              <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{v.lede}</DialogDescription>
              <div className="mt-3">
                {v.ways.map((w) => {
                  const Icon = ICON[w.id];
                  return (
                    <button
                      key={w.id}
                      type="button"
                      data-provider-way={w.id}
                      disabled={busy}
                      onClick={() => (w.id === "key" || w.id === "token" ? setKeyWay(w) : void run(w))}
                      className="-mx-2 flex w-[calc(100%+1rem)] cursor-default items-center gap-3 rounded-lg px-2 py-2.5 text-left outline-none hover:bg-accent focus-visible:bg-accent disabled:opacity-50"
                    >
                      <span className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
                        <Icon className="size-3.5" />
                      </span>
                      <span className="min-w-0 flex-1">
                        <span className="block text-[13px]">{w.label}</span>
                        <span className="block text-xs text-muted-foreground">{w.help}</span>
                      </span>
                      <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
                    </button>
                  );
                })}
              </div>
              {v.others.length ? (
                <div data-provider-others="" className="mt-3">
                  <p className="text-xs text-muted-foreground">{v.othersLabel}</p>
                  <div className="mt-1.5 flex flex-wrap gap-1.5">
                    {v.others.map((o) => (
                      <Button key={o.provider} size="sm" variant="outline" disabled={busy} data-provider-other={o.provider} onClick={() => c.openProvider(o.provider, c.sheetApp)}>
                        {o.label}
                      </Button>
                    ))}
                  </div>
                </div>
              ) : null}
              <div className="mt-4 flex justify-end">
                <Button variant="outline" onClick={close}>
                  {v.cancelLabel}
                </Button>
              </div>
            </>
          )}
        </div>
      </DialogPopup>
    </Dialog>
  );
}
