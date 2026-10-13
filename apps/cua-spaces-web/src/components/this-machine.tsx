// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, ChevronDownIcon, ChevronRightIcon, CopyIcon, LoaderCircleIcon, TriangleAlertIcon } from "lucide-react";
import { useState, type ReactNode } from "react";

import {
  useSession,
  useThisMachine,
  type HostAccessRow,
  type HostAction,
  type HostFailure,
  type HostFormView,
  type HostPanelView,
  type HostSetupFormView,
} from "@/bridge";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { Switch } from "@/components/ui/switch";
import { toast } from "@/components/ui/toast";
import { Tooltip } from "@/components/ui/tooltip";
import { cn, relativeTime } from "@/lib/utils";

/** The log open in full ("Show All…"). */
type HostLog = "access" | "activity";

/**
 * This machine: how it is shared, who is connected, the permissions left
 * to grant and its buttons; or the host setup form. Everything shown is the
 * core's `host.panel` / `host.formView`, laid out as the SwiftUI app's
 * `ThisMachineView`. `progress` is what a running setup or Sign In waits
 * for (the host's `HostModel.progress`).
 */
export function ThisMachine({ panel, progress }: { panel: HostPanelView; progress?: string }) {
  const { data: session } = useSession();
  const host = useThisMachine(session?.identity ?? null);
  const [confirming, setConfirming] = useState<HostAction | null>(null);
  const [fullLog, setFullLog] = useState<HostLog | null>(null);
  const now = Date.now();

  if (host.formView) {
    return <HostSetupForm view={host.formView} host={host} failure={host.setupError} progress={progress} onRetry={() => void host.retrySetUp()} />;
  }

  const choices = panel.setupChoices ?? [];
  const press = (a: HostAction) => (a.confirm ? setConfirming(a) : void host.run(a.id));
  const openSettings = (url: string) =>
    host.openSettings(url).catch((e: unknown) => toast("Couldn't open System Settings", { description: e instanceof Error ? e.message : String(e), type: "error" }));
  const notice = panel.notice && !panel.intro ? panel.notice : null;
  const noticeAction = notice ? panel.noticeAction : null;

  return (
    <div data-host-panel={panel.configured ? "configured" : "not-configured"}>
      <Group title={panel.title}>
        <div className="px-4 py-3">
          {notice ? (
            // Relay sharing paused (signed out, or another account): one
            // line in place of the summary, and its button.
            <div className="flex items-center justify-between gap-3">
              <p data-host-notice className="text-[13px]">
                {notice}
              </p>
              {noticeAction ? (
                <ActionButton action={noticeAction} primary disabled={host.busy} onPress={() => void host.run(noticeAction.id)} data-host-notice-action={noticeAction.id} />
              ) : null}
            </div>
          ) : (
            <p data-host-summary className="text-[13px]">
              {panel.summary}
            </p>
          )}
          {panel.intro ? <p className="mt-1 text-xs text-muted-foreground">{panel.intro}</p> : null}
          {progress ? <Progress text={progress} /> : null}
        </div>
        {choices.map((c) => (
          <Row key={c.id} label={c.label} data-host-choice={c.id}>
            <Button size="sm" variant="outline" disabled={host.busy} onClick={() => host.openForm(c.id)}>
              {c.buttonLabel}
            </Button>
          </Row>
        ))}
      </Group>

      {panel.facts.length ? (
        <Group>
          {panel.facts.map((f) => (
            <Row key={f.label} label={f.label} data-host-fact={f.label}>
              <span className="truncate select-text">{f.value}</span>
            </Row>
          ))}
        </Group>
      ) : null}

      {panel.toggles?.length ? (
        <Group>
          {panel.toggles.map((t) => (
            <Row key={t.id} label={t.label} hint={t.help} data-host-toggle={t.id}>
              <Switch checked={t.on} disabled={host.busy || !t.enabled} aria-label={t.label} onCheckedChange={() => void host.run(t.action)} />
            </Row>
          ))}
          {panel.limits ? <p className="px-4 py-2.5 text-xs text-muted-foreground">{panel.limits}</p> : null}
        </Group>
      ) : null}

      {panel.providedTitle ? (
        <Group title={panel.providedTitle}>
          {panel.providedEmpty ? <p className="px-4 py-2.5 text-[13px] text-muted-foreground">{panel.providedEmpty}</p> : null}
          <TimedRows rows={panel.provided ?? []} now={now} />
        </Group>
      ) : null}

      {panel.accessWarning ? <Warning>{panel.accessWarning}</Warning> : null}
      {panel.clientsTitle ? (
        <Group title={panel.clientsTitle}>
          {panel.clientsEmpty ? (
            <p data-host-clients-empty className="px-4 py-2.5 text-[13px] text-muted-foreground">
              {panel.clientsEmpty}
            </p>
          ) : null}
          {panel.clients.map((c) => (
            <p key={c} data-host-client className="truncate px-4 py-2.5 text-[13px]">
              {c}
            </p>
          ))}
        </Group>
      ) : null}

      {panel.recentTitle && panel.recent?.length ? (
        // The newest few (repeats collapsed); the rest in a sheet, so the
        // page never grows with the log.
        <Group title={panel.recentTitle} more={panel.recentMore ? <ShowAll label={panel.recentMore} data-host-more="access" onClick={() => setFullLog("access")} /> : null}>
          <TimedRows rows={panel.recent} now={now} />
        </Group>
      ) : null}
      {panel.activityWarning ? <Warning>{panel.activityWarning}</Warning> : null}
      {panel.activityTitle && panel.activity?.length ? (
        <Group title={panel.activityTitle} more={panel.activityMore ? <ShowAll label={panel.activityMore} data-host-more="activity" onClick={() => setFullLog("activity")} /> : null}>
          <TimedRows rows={panel.activity} now={now} keep="head" />
        </Group>
      ) : null}

      {panel.permissionsTitle ? (
        <Group title={panel.permissionsTitle}>
          {panel.permissions.map((p) => (
            <Row key={p.id} label={p.title} hint={p.help} data-host-permission={p.id}>
              {p.settingsUrl ? (
                <Button size="sm" variant="outline" onClick={() => void openSettings(p.settingsUrl!)}>
                  {panel.openSettingsLabel}
                </Button>
              ) : null}
            </Row>
          ))}
        </Group>
      ) : null}

      {host.error ? (
        // A button that failed: what happened in plain words, Retry, and
        // the raw error under Details.
        <div className="mb-6 rounded-xl border bg-card px-4 py-3 shadow-xs">
          <FailureView failure={host.error} retrying={host.busy} onRetry={() => void host.retry()} />
        </div>
      ) : null}

      {choices.length === 0 && panel.actions.length ? (
        // Pinned under the page: Stop sharing and Remove stay in view
        // however long the logs get.
        <div data-host-actions className="sticky bottom-0 z-10 -mx-1 mb-2 flex gap-2 border-t bg-background/90 px-1 py-3 backdrop-blur">
          {panel.actions.map((a, i) => (
            <ActionButton key={a.id} action={a} primary={i === 0} disabled={host.busy} onPress={() => press(a)} data-host-action={a.id} />
          ))}
        </div>
      ) : null}

      <HostLogDialog
        open={fullLog !== null}
        onOpenChange={(o) => !o && setFullLog(null)}
        title={(fullLog === "activity" ? panel.activityTitle : panel.recentTitle) ?? ""}
        rows={(fullLog === "activity" ? panel.activityAll : panel.recentAll) ?? []}
        backgroundRows={fullLog === "access" ? (panel.recentWithBackground ?? null) : null}
        keep={fullLog === "activity" ? "head" : "tail"}
        now={now}
      />

      <ConfirmDialog
        open={confirming !== null}
        onOpenChange={(o) => !o && setConfirming(null)}
        title={confirming?.confirm?.title ?? ""}
        description={confirming?.confirm?.message ?? ""}
        confirmLabel={confirming?.confirm?.confirmLabel ?? ""}
        cancelLabel={confirming?.confirm?.cancelLabel}
        destructive
        onConfirm={() => confirming && void host.run(confirming.id)}
      />
    </div>
  );
}

/* ---- The setup form ------------------------------------------------------------- */

type FormHost = Pick<ReturnType<typeof useThisMachine>, "send" | "submit" | "closeForm">;

/** A host action button: the core's label, disabled when it says so, with its help as the tooltip. */
function ActionButton({
  action: a,
  primary,
  disabled,
  onPress,
  ...data
}: { action: HostAction; primary: boolean; disabled: boolean; onPress: () => void } & Record<`data-${string}`, string>) {
  return (
    <span title={a.help ?? undefined} className="inline-flex">
      <Button
        {...data}
        size="sm"
        variant={primary ? (a.destructive ? "destructive" : "default") : "outline"}
        className={cn(!primary && a.destructive && "text-destructive")}
        disabled={disabled || a.enabled === false}
        onClick={onPress}
      >
        {a.label}
      </Button>
    </span>
  );
}

const TEXT_ACTION: Record<string, (text: string) => Parameters<FormHost["send"]>[0]> = {
  allow: (allow) => ({ type: "set-allow", allow }),
  listen: (listen) => ({ type: "set-listen", listen }),
  relay: (url) => ({ type: "set-relay-url", url }),
};

/** The host setup form: the core's fields in order, Advanced ones under
 * Advanced. A failed setup shows in plain words with Retry (the same
 * request again) and the raw error under Details; `progress` is what a
 * running setup waits for (the sign-in in the browser). */
export function HostSetupForm({
  view,
  host,
  failure,
  progress,
  onRetry,
}: {
  view: HostSetupFormView;
  host: FormHost;
  failure?: HostFailure | null;
  progress?: string;
  onRetry?: () => void;
}) {
  const basic = view.fields.filter((f) => !f.advanced);
  const advanced = view.fields.filter((f) => f.advanced);
  return (
    <form
      data-host-form
      onSubmit={(e) => {
        e.preventDefault();
        if (view.canSubmit) void host.submit();
      }}
    >
      <Group title={view.title}>
        <p data-host-lede className="px-4 pt-3 pb-1 text-xs text-muted-foreground">
          {view.lede}
        </p>
        {basic.map((f) => (
          <Field key={f.id} field={f} host={host} />
        ))}
        <button
          type="button"
          data-host-advanced={view.advancedOpen ? "open" : "closed"}
          onClick={() => host.send({ type: "toggle-advanced" })}
          className="flex w-full cursor-default items-center gap-1 px-4 py-2.5 text-left text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground"
        >
          {view.advancedOpen ? <ChevronDownIcon className="size-3.5" /> : <ChevronRightIcon className="size-3.5" />}
          {view.advancedLabel}
        </button>
        {advanced.map((f) => (
          <Field key={f.id} field={f} host={host} />
        ))}
        {failure && onRetry ? (
          <div data-host-form-error className="px-4 py-2.5">
            <FailureView failure={failure} retrying={view.busy} onRetry={onRetry} />
          </div>
        ) : view.error ? (
          <div data-host-form-error className="flex items-start gap-2 px-4 py-2.5 text-[13px] text-destructive">
            <TriangleAlertIcon className="mt-0.5 size-3.5 shrink-0" />
            <span className="min-w-0 break-words whitespace-pre-line">{view.error}</span>
          </div>
        ) : null}
        {progress && view.busy ? (
          <div className="px-4 py-2.5">
            <Progress text={progress} />
          </div>
        ) : null}
      </Group>
      <div className="mb-6 flex justify-end gap-2">
        <Button type="button" size="sm" variant="outline" onClick={() => host.closeForm()}>
          {view.backLabel}
        </Button>
        <Button type="submit" size="sm" data-host-submit disabled={!view.canSubmit}>
          {view.submitLabel}
        </Button>
      </div>
    </form>
  );
}

function Field({ field: f, host }: { field: HostFormView["fields"][number]; host: FormHost }) {
  if (f.choices?.length) {
    return (
      <Row label={f.label} data-host-field={f.id}>
        <Segmented
          aria-label={f.label}
          value={f.value}
          options={f.choices.map((c) => ({ value: c.id, label: c.label }))}
          onValueChange={(profile) => host.send({ type: "set-profile", profile })}
        />
      </Row>
    );
  }
  if (f.toggle) {
    return (
      <Row label={f.label} data-host-field={f.id}>
        <Switch checked={f.on} aria-label={f.label} onCheckedChange={(on) => host.send({ type: "set-direct", on })} />
      </Row>
    );
  }
  const id = `host-field-${f.id}`;
  return (
    <div data-host-field={f.id} className="flex min-h-11 items-center justify-between gap-6 px-4 py-2">
      <label htmlFor={id} className="shrink-0 text-[13px]">
        {f.label}
      </label>
      <Input
        id={id}
        value={f.value}
        placeholder={f.placeholder ?? undefined}
        aria-invalid={f.invalid || undefined}
        onChange={(e) => host.send(TEXT_ACTION[f.id]?.(e.target.value) ?? { type: "set-name", name: e.target.value })}
        className={cn("max-w-72", f.invalid && "border-destructive text-destructive focus-visible:border-destructive focus-visible:ring-destructive/20")}
      />
    </div>
  );
}

/* ---- Pieces ------------------------------------------------------------------------ */

function Group({ title, more, children }: { title?: string; more?: ReactNode; children: ReactNode }) {
  return (
    <section className="mb-6">
      {title ? <h3 className="mb-2 px-1 text-xs font-semibold text-muted-foreground">{title}</h3> : null}
      <div className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">{children}</div>
      {more ? <div className="mt-1.5 flex justify-end px-1">{more}</div> : null}
    </section>
  );
}

/** "Show All…" under a log. */
function ShowAll({ label, onClick, ...data }: { label: string; onClick: () => void } & Record<`data-${string}`, string>) {
  return (
    <button type="button" {...data} onClick={onClick} className="cursor-default text-xs text-brand-strong outline-none hover:underline focus-visible:underline">
      {label}
    </button>
  );
}

/** A log in full: every row (repeats collapsed), newest first, and for the
 * access log a checkbox that adds the background probes. */
export function HostLogDialog({
  open,
  onOpenChange,
  title,
  rows,
  backgroundRows,
  now,
  keep = "tail",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  rows: HostAccessRow[];
  /** Every row with background activity too (null: no checkbox). */
  backgroundRows: HostAccessRow[] | null;
  now: number;
  /** Which part of a long row stays whole (see `TimedRows`). */
  keep?: LogKeep;
}) {
  const [withBackground, setWithBackground] = useState(false);
  const shown = withBackground && backgroundRows ? backgroundRows : rows;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogPopup className="top-[10vh] flex max-h-[76vh] w-[min(720px,calc(100vw-2rem))] flex-col" data-host-log>
        <DialogTitle className="border-b px-5 py-3 text-[15px] font-semibold">{title}</DialogTitle>
        <div data-host-log-rows className="min-h-0 flex-1 divide-y overflow-y-auto">
          <TimedRows rows={shown} now={now} keep={keep} />
        </div>
        <div className="flex items-center gap-3 border-t bg-muted/60 px-5 py-3">
          {backgroundRows ? (
            <label className="flex items-center gap-2 text-[13px]">
              <Checkbox data-host-log-background="" checked={withBackground} onCheckedChange={(on) => setWithBackground(on === true)} />
              Include background activity
            </label>
          ) : null}
          <span className="flex-1" />
          <Button data-host-log-done="" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}

/** What a running setup or Sign In waits for. */
function Progress({ text }: { text: string }) {
  return (
    <p data-host-progress className="mt-2 flex items-start gap-1.5 text-xs text-muted-foreground">
      <LoaderCircleIcon className="mt-px size-3.5 shrink-0 animate-spin" />
      <span>{text}</span>
    </p>
  );
}

/** A failed setup or button, as the SwiftUI app's `HostSetupFailureView`:
 * a warning and a short title, Retry, one line saying what to do, and the
 * raw error under a collapsed Details (selectable, with Copy). */
export function FailureView({ failure, retrying, onRetry }: { failure: HostFailure; retrying: boolean; onRetry: () => void }) {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const details = failure.details && failure.details !== failure.message ? failure.details : null;
  return (
    <div data-host-failure className="text-[13px]">
      <div className="flex items-center gap-2">
        <TriangleAlertIcon className="size-3.5 shrink-0 text-amber-500" />
        <span data-host-failure-title className={cn("min-w-0 flex-1 break-words", failure.title ? "font-medium" : "text-destructive")}>
          {failure.title ?? failure.message}
        </span>
        <Button size="sm" variant="outline" data-host-retry="" disabled={retrying} onClick={onRetry}>
          {retrying ? "Retrying…" : (failure.actionLabel ?? "Retry")}
        </Button>
      </div>
      {failure.title ? (
        <p data-host-failure-message className="mt-1 text-xs text-muted-foreground">
          {failure.message}
        </p>
      ) : null}
      {details ? (
        <div className="mt-1">
          <button
            type="button"
            data-host-details={open ? "open" : "closed"}
            onClick={() => setOpen(!open)}
            className="flex cursor-default items-center gap-1 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground"
          >
            {open ? <ChevronDownIcon className="size-3.5" /> : <ChevronRightIcon className="size-3.5" />}
            Details
          </button>
          {open ? (
            <div className="mt-1 flex items-start gap-2">
              <pre data-host-details-text className="max-h-20 min-w-0 flex-1 overflow-y-auto font-mono text-[11px] break-words whitespace-pre-wrap text-muted-foreground select-text">
                {details}
              </pre>
              <Button
                size="sm"
                variant="ghost"
                data-host-copy-details=""
                onClick={() =>
                  void navigator.clipboard?.writeText(details).then(() => {
                    setCopied(true);
                    setTimeout(() => setCopied(false), 1500);
                  })
                }
              >
                {copied ? <CheckIcon className="size-3.5" /> : <CopyIcon className="size-3.5" />}
                Copy
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function Row({ label, hint, children, ...data }: { label: string; hint?: string; children?: ReactNode } & Record<`data-${string}`, string>) {
  return (
    <div {...data} className="flex min-h-10 items-center justify-between gap-6 px-4 py-2.5">
      <div className="min-w-0">
        <div className="truncate text-[13px]">{label}</div>
        {hint ? <div className="mt-0.5 text-xs text-muted-foreground">{hint}</div> : null}
      </div>
      <div className="flex min-w-0 shrink-0 items-center text-right text-[13px] text-muted-foreground">{children}</div>
    </div>
  );
}

/** Where a log line's text is cut, so what happened stays in view: the
 * part after the last " · " (the action, "Screen and input refused ×108"),
 * else a trailing repeat count. */
export function logLineParts(text: string): [head: string, tail: string] {
  const dot = text.lastIndexOf(" · ");
  if (dot >= 0) return [text.slice(0, dot + 3), text.slice(dot + 3)];
  const repeat = / ×\d+$/.exec(text);
  return repeat ? [text.slice(0, repeat.index), repeat[0]] : [text, ""];
}

/** Which part of a log row stays whole when it is cut: the access log's
 * tail ("who · what they did ×N": who gives way), the Spaces activity's
 * head ("Created space-1 · who": who gives way). */
export type LogKeep = "head" | "tail";

/** Rows with a relative time after them. A long line is cut in the middle,
 * as the SwiftUI app's (`truncationMode(.middle)`): who it was gives way,
 * what happened stays; the whole line is the tooltip. */
function TimedRows({ rows, now, keep = "tail" }: { rows: { text: string; atMs: number }[]; now: number; keep?: LogKeep }) {
  // The kept part never truncates, whatever the font: it takes its own
  // width, and wraps (never cut) only when it alone is wider than the row.
  const whole = "max-w-full shrink-0 whitespace-pre-wrap [overflow-wrap:anywhere]";
  const gives = "min-w-0 shrink truncate whitespace-pre";
  return rows.map((r, i) => {
    const [head, tail] = logLineParts(r.text);
    return (
      <div key={i} data-host-log-row className="flex min-h-10 items-center justify-between gap-6 px-4 py-2.5">
        <div className="flex min-w-0 flex-1 text-[13px]" title={r.text}>
          {/* Only who gives way: the other part keeps its whole width. */}
          <span data-host-log-head className={tail && keep === "tail" ? gives : whole}>
            {head}
          </span>
          {tail ? (
            <span data-host-log-tail className={keep === "tail" ? whole : gives}>
              {tail}
            </span>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center text-right text-[13px] text-muted-foreground">
          <Tooltip content={new Date(r.atMs).toLocaleString()}>
            <span>{relativeTime(r.atMs, now)}</span>
          </Tooltip>
        </div>
      </div>
    );
  });
}

function Warning({ children }: { children: ReactNode }) {
  return <p className="mb-6 px-1 text-[13px] text-destructive">{children}</p>;
}
