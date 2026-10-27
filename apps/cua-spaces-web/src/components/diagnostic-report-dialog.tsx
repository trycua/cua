// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useLayoutEffect, useRef, useState, type ComponentProps } from "react";

import type { CreateReport, ReportPart } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";

/** The tallest a part grows before it scrolls (matches `max-h-60`). */
const TEXT_MAX_PX = 240;

/** A textarea that grows to show every wrapped line, up to [`TEXT_MAX_PX`]
 * (then it scrolls). Rows would count line breaks, not wrapped lines. */
function FitTextarea({ value, ...props }: Omit<ComponentProps<"textarea">, "rows">) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const fit = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    if (el.scrollHeight === 0) return;
    const border = el.offsetHeight - el.clientHeight;
    el.style.height = `${Math.min(el.scrollHeight + border, TEXT_MAX_PX)}px`;
  };
  useLayoutEffect(fit, [value]);
  useEffect(() => {
    window.addEventListener("resize", fit);
    return () => window.removeEventListener("resize", fit);
  }, []);
  return <textarea ref={ref} rows={2} value={value} {...props} />;
}

/**
 * The review sheet for a failed create's diagnostic report (standard
 * crash-report pattern): the app core's redacted report, each part
 * editable and kept by default, and nothing sent until the person presses
 * Send in their mail app. Open email opens a message to Cua support there
 * with the parts they kept.
 */
export function DiagnosticReportDialog({
  open,
  onOpenChange,
  report,
  onSend,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  report: CreateReport | null;
  onSend: (subject: string, parts: ReportPart[]) => Promise<void>;
}) {
  const [parts, setParts] = useState<(ReportPart & { kept: boolean })[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (open && report) {
      setParts(report.parts.map((p) => ({ ...p, kept: true })));
      setError(null);
    }
  }, [open, report]);

  const kept = parts.filter((p) => p.kept);
  const send = async () => {
    if (!report || kept.length === 0) return;
    setBusy(true);
    setError(null);
    try {
      await onSend(
        report.subject,
        kept.map(({ id, label, text }) => ({ id, label, text })),
      );
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };
  const edit = (id: string, change: Partial<{ text: string; kept: boolean }>) =>
    setParts((all) => all.map((p) => (p.id === id ? { ...p, ...change } : p)));

  return (
    <Dialog open={open} onOpenChange={(next) => (busy ? undefined : onOpenChange(next))}>
      <DialogPopup className="top-[8vh] w-[min(600px,calc(100vw-2rem))]" data-diagnostic-report="">
        <div className="px-6 pt-5 pb-4">
          <DialogTitle className="text-[15px] font-semibold">{report?.title ?? "Send a diagnostic report"}</DialogTitle>
          <DialogDescription className="mt-1 text-[13px] text-muted-foreground">{report?.intro}</DialogDescription>
          <div className="mt-4 flex max-h-[55vh] flex-col gap-4 overflow-y-auto">
            {parts.map((p) => (
              <div key={p.id} data-report-part={p.id}>
                <label className="flex items-center gap-2 text-[13px] font-medium">
                  <Checkbox checked={p.kept} onCheckedChange={(v) => edit(p.id, { kept: Boolean(v) })} aria-label={`Include ${p.label}`} />
                  {p.label}
                </label>
                <FitTextarea
                  aria-label={p.label}
                  className="mt-1.5 max-h-60 min-h-[4.5rem] w-full resize-y overflow-y-auto rounded-lg bg-muted px-2.5 py-2 font-mono text-xs leading-relaxed disabled:opacity-50"
                  value={p.text}
                  disabled={!p.kept || busy}
                  spellCheck={false}
                  onChange={(e) => edit(p.id, { text: e.target.value })}
                />
              </div>
            ))}
          </div>
          {error ? (
            <p role="alert" className="mt-2 text-xs text-destructive">
              {error}
            </p>
          ) : null}
          <div className="mt-4 flex justify-end gap-2">
            <Button variant="secondary" disabled={busy} onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button data-report-send="" disabled={busy || !report || kept.length === 0} onClick={() => void send()}>
              {report?.sendLabel ?? "Open email"}
            </Button>
          </div>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
