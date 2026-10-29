// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronLeftIcon, ChevronRightIcon, CopyIcon, KeyRoundIcon, Loader2Icon, LogInIcon, ServerIcon, SmartphoneIcon, TerminalIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { importSheet, useBridge, type ProviderWay, type WayField } from "@/bridge";
import { ProviderMark } from "@/components/settings/connections/provider-mark";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { useSession } from "@/bridge";
import { closeDeviceSignIn, closeImport, useConnectors, useDeviceSignIn, useImporting, useProviderSheetOpen, useSheetWay } from "@/hooks/use-connectors";

const ICON = { "app-token": LogInIcon, "app-device": SmartphoneIcon, token: KeyRoundIcon, key: KeyRoundIcon, endpoint: ServerIcon, shell: TerminalIcon } as const;

/** Ways that ask for something typed (a key, a token, a URL) open a form; the others run at once. */
const hasForm = (w: ProviderWay) => w.id === "key" || w.id === "token" || w.id === "endpoint" || (w.id === "shell" && w.fields.length > 0);

/**
 * "Connect <provider>": the ways the core offers (`connectors.providerSheet`),
 * one row each and only those that apply, then the form for the way picked (a
 * key, a plan, a model, a server's URL). Opened from the Connections list,
 * from an app's page, from a Space's Connect card, and from an app's Connect;
 * "Other providers" opens the searchable provider choice. Mounted once,
 * beside Share.
 */
export function ProviderSheetHost() {
  const device = useDeviceSignIn();
  const open = useProviderSheetOpen();
  const importing = useImporting();
  if (importing) return <ImportSheet />;
  if (device) return <DeviceCodeSheet />;
  return open ? <SheetOrPicker /> : null;
}

/** The provider's sheet, or the provider choice when no provider is picked yet. */
function SheetOrPicker() {
  const c = useConnectors();
  if (c.picker) return <ProviderPicker />;
  return <ProviderSheet />;
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

/** The searchable provider choice: the first tier, then every provider by search or "More providers". */
function ProviderPicker() {
  const c = useConnectors();
  const v = c.picker;
  if (!v) return null;
  return (
    <Dialog open onOpenChange={(o) => !o && c.closeProvider()}>
      <DialogPopup data-provider-picker="" className="top-[12vh] w-[min(520px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          <DialogTitle className="text-[15px] font-semibold">{v.title}</DialogTitle>
          {v.lede ? <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{v.lede}</DialogDescription> : null}
          <Input
            type="search"
            autoFocus
            data-provider-picker-search=""
            aria-label={v.searchLabel}
            placeholder={v.searchLabel}
            autoComplete="off"
            spellCheck={false}
            className="mt-3 h-8"
            onChange={(e) => c.searchPicker(e.currentTarget.value)}
          />
          <div className="mt-2 max-h-[min(50vh,360px)] overflow-y-auto" data-provider-picker-list="">
            {v.items.map((i) => (
              <button
                key={i.provider}
                type="button"
                data-provider-choice={i.provider}
                onClick={() => c.openProvider(i.provider, c.sheetApp)}
                className="-mx-2 flex w-[calc(100%+1rem)] cursor-default items-center gap-3 rounded-lg px-2 py-2 text-left outline-none hover:bg-accent focus-visible:bg-accent"
              >
                <ProviderMark id={i.provider} name={i.label} />
                <span className="min-w-0 flex-1 truncate text-[13px]">{i.label}</span>
                <span className={i.state === "connected" ? "flex items-center gap-1 text-xs" : "text-xs text-muted-foreground"}>
                  {i.state === "connected" ? <CheckIcon className="size-3 text-success" /> : null}
                  {i.status}
                </span>
                <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
              </button>
            ))}
            {v.empty ? (
              <p className="px-1 py-3 text-[13px] text-muted-foreground" data-provider-picker-empty="">
                {v.empty}
              </p>
            ) : null}
            {v.moreLabel ? (
              <button
                type="button"
                data-provider-picker-more=""
                onClick={() => c.searchPicker("", true)}
                className="-mx-2 flex w-[calc(100%+1rem)] cursor-default items-center gap-3 rounded-lg px-2 py-2 text-left text-[13px] text-muted-foreground outline-none hover:bg-accent focus-visible:bg-accent"
              >
                <span className="min-w-0 flex-1">
                  {v.moreLabel} ({v.moreCount})
                </span>
                <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
              </button>
            ) : null}
          </div>
          <div className="mt-4 flex justify-end">
            <Button variant="outline" onClick={() => c.closeProvider()}>
              {v.cancelLabel}
            </Button>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

/** What the person typed for a way: the key (never kept past Save) and each field. */
interface Filled {
  value: string;
  fields: Record<string, string>;
}

const startFilled = (way: ProviderWay): Filled => ({ value: "", fields: Object.fromEntries(way.fields.map((f) => [f.id, f.value])) });

/** A way's form is ready when its key (if one is required) and every required field are filled. */
function ready(way: ProviderWay, filled: Filled): boolean {
  if (way.secret === "required" && !filled.value.trim()) return false;
  return way.fields.every((f) => !f.required || (filled.fields[f.id] ?? "").trim() !== "");
}

/** The plan or region goes as `region`; everything else as a field. */
function choicesOf(way: ProviderWay, filled: Filled) {
  const region = way.fields.some((f) => f.id === "region") ? filled.fields.region : undefined;
  const fields = Object.fromEntries(Object.entries(filled.fields).filter(([k, v]) => k !== "region" && v.trim() !== ""));
  return { ...(region ? { region } : {}), ...(Object.keys(fields).length ? { fields } : {}) };
}

function FieldInput({ field, value, onChange }: { field: WayField; value: string; onChange: (v: string) => void }) {
  return (
    <label className="mt-3 block" data-provider-field={field.id}>
      <span className="mb-1 block text-xs font-medium">{field.label}</span>
      {field.kind === "choice" ? (
        <Select value={value} options={field.options.map((o) => ({ value: o.id, label: o.label }))} onValueChange={onChange} aria-label={field.label} className="w-full" />
      ) : (
        <Input
          value={value}
          placeholder={field.placeholder}
          autoComplete="off"
          spellCheck={false}
          inputMode={field.kind === "url" ? "url" : undefined}
          className={field.kind === "url" ? "font-mono" : undefined}
          onChange={(e) => onChange(e.currentTarget.value)}
        />
      )}
    </label>
  );
}

function ProviderSheet() {
  const c = useConnectors();
  const v = c.sheet;
  const [way, setWay] = useState<ProviderWay | null>(null);
  const [filled, setFilled] = useState<Filled>({ value: "", fields: {} });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const startWay = useSheetWay();
  const open = (w: ProviderWay) => {
    setWay(w);
    setFilled(startFilled(w));
    setError(null);
  };
  // Sent here from "nothing found" to paste a token or a key: open straight on that field.
  useEffect(() => {
    const w = startWay && v ? v.ways.find((x) => x.id === startWay) : null;
    if (w) open(w);
  }, [startWay, v?.provider]); // eslint-disable-line react-hooks/exhaustive-deps
  const close = () => {
    setWay(null);
    setFilled({ value: "", fields: {} });
    setError(null);
    c.closeProvider();
  };
  if (!v) return null;
  const run = async (w: ProviderWay, typed?: string) => {
    setBusy(true);
    setError(null);
    const ok = await c.runWay(w, typed, choicesOf(w, filled));
    setBusy(false);
    if (ok) close();
    else if (w.id === "key" || w.id === "token" || w.id === "endpoint") {
      setError(w.id === "token" ? "Couldn't save the token. Check it and try again." : w.id === "endpoint" ? "Couldn't save the endpoint. Check it and try again." : "Couldn't save the key. Check it and try again.");
    }
  };
  const switcher = (
    <Button type="button" variant="ghost" className="mr-auto text-muted-foreground" data-provider-switch="" disabled={busy} onClick={() => c.openProvider(null, c.sheetApp)}>
      {v.switchLabel}
    </Button>
  );
  return (
    <Dialog open onOpenChange={(o) => !o && close()}>
      <DialogPopup data-provider-sheet={v.provider} className="top-[12vh] w-[min(520px,calc(100vw-2rem))]">
        <div className="px-6 pt-5 pb-5">
          {way ? (
            <form
              onSubmit={(e) => {
                e.preventDefault();
                if (ready(way, filled) && !busy) void run(way, filled.value);
              }}
            >
              <DialogTitle className="flex items-center gap-1.5 text-[15px] font-semibold">
                <button
                  type="button"
                  aria-label="Back"
                  onClick={() => setWay(null)}
                  className="-ml-1.5 inline-flex size-6 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
                >
                  <ChevronLeftIcon className="size-4" />
                </button>
                {v.title}
              </DialogTitle>
              <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{way.help}</DialogDescription>
              {way.fields.map((f) => (
                <FieldInput key={f.id} field={f} value={filled.fields[f.id] ?? ""} onChange={(x) => setFilled((p) => ({ ...p, fields: { ...p.fields, [f.id]: x } }))} />
              ))}
              {way.secret !== "none" ? (
                <label className="mt-3 block">
                  <span className="mb-1 block text-xs font-medium">
                    {way.id === "token" ? v.tokenLabel : v.keyLabel}
                    {way.secret === "optional" ? " (optional)" : ""}
                  </span>
                  <Input
                    type="password"
                    autoFocus={way.fields.length === 0}
                    data-provider-key=""
                    value={filled.value}
                    placeholder={way.id === "token" ? v.tokenPlaceholder : v.keyPlaceholder}
                    autoComplete="off"
                    spellCheck={false}
                    className="font-mono"
                    onChange={(e) => {
                      const typed = e.currentTarget.value;
                      setFilled((p) => ({ ...p, value: typed }));
                    }}
                  />
                </label>
              ) : null}
              {way.secret !== "none" ? <p className="mt-1.5 text-xs text-muted-foreground">{v.keyHelp}</p> : null}
              {v.note && way.id === "endpoint" ? (
                <p className="mt-2 text-xs text-muted-foreground" data-provider-note="">
                  {v.note}
                </p>
              ) : null}
              {error ? <p className="mt-2 text-[13px] text-destructive">{error}</p> : null}
              <div className="mt-4 flex justify-end gap-2">
                {switcher}
                <Button type="button" variant="outline" onClick={close}>
                  {v.cancelLabel}
                </Button>
                <Button type="submit" disabled={busy || !ready(way, filled)} data-provider-save="">
                  {v.saveLabel}
                </Button>
              </div>
            </form>
          ) : (
            <>
              <header data-provider-sheet-header="">
                <DialogTitle className="flex items-center gap-2 text-[15px] font-semibold" data-provider-sheet-title="">
                  <ProviderMark id={v.provider} name={v.title.replace(/^Connect /, "")} plain />
                  {v.title}
                </DialogTitle>
                {v.lede ? (
                  <DialogDescription className="mt-1.5 text-xs text-muted-foreground" data-provider-works-with="">
                    {v.lede}
                  </DialogDescription>
                ) : null}
              </header>
              <div className="mt-3 border-t pt-2">
                {v.ways.map((w) => {
                  const Icon = ICON[w.id];
                  return (
                    <button
                      key={w.id}
                      type="button"
                      data-provider-way={w.id}
                      disabled={busy}
                      onClick={() => (hasForm(w) ? open(w) : void run(w))}
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
              {v.onlyKeyNote ? (
                <p className="mt-2 text-xs text-muted-foreground" data-provider-only-key="">
                  {v.onlyKeyNote}
                </p>
              ) : null}
              <div className="mt-4 flex justify-end gap-2">
                {switcher}
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
