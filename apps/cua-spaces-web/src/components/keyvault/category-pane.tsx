// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef } from "react";

import type { KvAccessCommand, KvListView, KvPage } from "@/bridge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * Waiting, Access or Recent (the SwiftUI `CategoryList`): the core's pane
 * for the category. Waiting rows are requests an agent made (Deny, or Review
 * to pick what to approve); Access rows are live grants, rules and copies in
 * Spaces, each with its own Revoke, Remove or Wipe and, for copies, Dismiss
 * (hides it from the notch only); Recent is the decision log.
 */
export function CategoryPane({
  list,
  page,
  dismissed,
  focus,
  busy,
  onReview,
  onDeny,
  onRun,
  onDismiss,
}: {
  list: KvListView;
  page: KvPage;
  /** Copies (import ids) already hidden from the notch. */
  dismissed: readonly string[];
  /** The Access row to bring forward (a Space's "Signed in" badge). */
  focus: string | null;
  busy: boolean;
  onReview: (requestId: string) => void;
  onDeny: (requestId: string) => void;
  onRun: (command: KvAccessCommand) => void;
  onDismiss: (imports: string[]) => void;
}) {
  const focused = useRef<HTMLLIElement | null>(null);
  useEffect(() => {
    if (focus) focused.current?.scrollIntoView?.({ block: "center" });
  }, [focus, list.access.length]);
  return (
    <div className="mx-auto w-full max-w-3xl space-y-5 px-8 pb-16">
      {list.pending.length ? (
        <Section>
          {list.pending.map((p) => (
            <Row key={p.id} data-waiting={p.id} title={p.claims.join("\n")}>
              <span className="min-w-0 flex-1 truncate">
                {p.caller} ({p.badge.text}) · {p.summary} · {p.wants}
              </span>
              <Button size="sm" variant="outline" disabled={busy} onClick={() => onDeny(p.id)}>
                {page.labels.deny}
              </Button>
              <Button size="sm" disabled={page.disabled || busy} onClick={() => onReview(p.id)}>
                {page.labels.review}
              </Button>
            </Row>
          ))}
        </Section>
      ) : null}
      {list.access.length ? (
        <Section>
          {page.revokeAll ? (
            <li className="flex justify-end px-4 py-2">
              <Button size="sm" variant="outline" disabled={busy} onClick={() => onRun({ type: "revoke-grant", id: "*" })}>
                {page.labels.revokeAll}
              </Button>
            </li>
          ) : null}
          {list.access.map((a) => {
            const hidden = a.imports.length > 0 && a.imports.every((i) => dismissed.includes(i));
            return (
              <Row key={a.key} data-access={a.key} focused={focus === a.key} ref={focus === a.key ? focused : undefined} title={a.detail}>
                <span className="min-w-0 flex-1 truncate">{a.detail ? `${a.text} · ${a.detail}` : a.text}</span>
                {/* Hides the notch's indicator only; Wipe removes the access. */}
                {a.imports.length > 0 && !hidden ? (
                  <Button size="sm" variant="ghost" title="Hide from the notch. Access stays until you wipe it." onClick={() => onDismiss(a.imports)}>
                    Dismiss
                  </Button>
                ) : null}
                <Button size="sm" variant="outline" disabled={busy} onClick={() => onRun(a.command as KvAccessCommand)}>
                  {a.actionLabel}
                </Button>
              </Row>
            );
          })}
        </Section>
      ) : null}
      {list.recent.length ? (
        <>
          <Section>
            {list.recent.map((r) => (
              <Row key={r.decision.entry.seq}>
                <span className="min-w-0 flex-1 truncate">
                  {r.decision.verb} {r.decision.what}
                </span>
                <span className="shrink-0 text-xs text-muted-foreground">{r.age}</span>
              </Row>
            ))}
          </Section>
          {page.logStatus ? <p className="px-1 text-xs text-muted-foreground">{page.logStatus}</p> : null}
        </>
      ) : null}
      {list.emptyText ? <p className="px-1 text-[13px] text-muted-foreground" data-category-empty>{list.emptyText}</p> : null}
    </div>
  );
}

function Section({ children }: { children: React.ReactNode }) {
  return <ul className="divide-y overflow-hidden rounded-lg border bg-card">{children}</ul>;
}

function Row({ children, focused, ...rest }: React.ComponentProps<"li"> & { focused?: boolean }) {
  return (
    <li {...rest} data-focused={focused || undefined} className={cn("flex items-center gap-2 px-4 py-2.5 text-[13px] transition-colors", focused && "bg-brand-surface")}>
      {children}
    </li>
  );
}
