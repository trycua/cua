// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as Haptics from "expo-haptics";
import { useState } from "react";
import { Pressable, View } from "react-native";

import { core } from "../../core";
import { approvalResult, diffLabels, toolGroup, type ApprovalsView, type OutboxRow } from "../../core/phone";
import type { ThreadEntry } from "../../data/types";
import { useTheme } from "../../theme/theme";
import { Icon } from "../../ui/Icon";
import { Text } from "../../ui/Text";
import { clock } from "../../ui/time";
import type { Answered as AnsweredInfo, ToolCall } from "./timeline";

/* ---- The push card: the app core's `threadCards.exportCard` view, laid out ----------------- */

export interface ExportCardView {
  repo: string;
  headline: string;
  branch: string;
  base: string | null;
  branchLine: string;
  totals: { files: string; additions: string; deletions: string };
  account: string | null;
  /** There is something behind "Show changes". */
  expandable: boolean;
  showLabel: string;
  hideLabel: string;
  files: { name: string; dir: string; status: string; additions: string; deletions: string; note: string; bar: ("add" | "del" | "none")[] }[];
  moreFiles: string | null;
  commitsHeading: string;
  commits: { sha: string; author: string; subject: string }[];
  moreCommits: string | null;
  pr: { title: string; state: string; body: string[] } | null;
  allow: string;
  deny: string;
  resultApproved: string;
  resultDeclined: string;
}

/**
 * The card's view for an approval's card data: every word and number is the app core's (`threadCards.exportCard`).
 * Without the core (Android until it is linked) or for data that is not a card, null: the approval then shows as
 * plain text, never as a guess.
 */
export function exportCardView(card: Record<string, unknown> | null): ExportCardView | null {
  if (!card) return null;
  return core.tryCall<ExportCardView>("threadCards.exportCard", { input: card });
}

function Squares({ bar }: { bar: ExportCardView["files"][number]["bar"] }) {
  const { c } = useTheme();
  return (
    <View style={{ flexDirection: "row", gap: 2 }} accessible={false}>
      {bar.map((s, i) => (
        <View key={i} style={{ width: 8, height: 8, borderRadius: 1.5, backgroundColor: s === "add" ? c.diffAdd : s === "del" ? c.diffDel : c.border }} />
      ))}
    </View>
  );
}

type Scope = "once" | "space";

/**
 * Decline, Allow once and, when the daemon says a Space grant makes sense, Allow for this Space: the buttons, their
 * words and the order are the app core's (`approvals.view`), built from the approval's structured data. With the core
 * not linked there are two plain buttons. While an answer waits in the outbox the card says so and cannot be answered twice.
 */
function AnswerBar({
  approvals,
  fallback,
  onAnswer,
  busy,
  waiting,
  onRetry,
  onDiscard,
}: {
  approvals: ApprovalsView | null;
  fallback: { deny: string; allow: string };
  onAnswer(approve: boolean, scope?: Scope): void;
  busy: boolean;
  waiting: OutboxRow | null;
  onRetry?(): void;
  onDiscard?(): void;
}) {
  const { c } = useTheme();
  const buttons = approvals?.buttons ?? [
    { id: "decline" as const, label: fallback.deny, approve: false, scope: null, role: "quiet" as const, hint: null },
    { id: "allow-once" as const, label: fallback.allow, approve: true, scope: "once" as const, role: "primary" as const, hint: null },
  ];
  const main = buttons.filter((x) => x.id !== "allow-space");
  const space = buttons.find((x) => x.id === "allow-space");
  const off = busy || waiting !== null;
  const press = (approve: boolean, scope?: Scope) => {
    Haptics.notificationAsync(approve ? Haptics.NotificationFeedbackType.Success : Haptics.NotificationFeedbackType.Warning).catch(() => {});
    onAnswer(approve, scope);
  };
  return (
    <View>
      <View style={{ flexDirection: "row", gap: 10, marginTop: 12 }}>
        {main.map((x) => (
          <Pressable
            key={x.id}
            disabled={off}
            onPress={() => press(x.approve, x.scope ?? undefined)}
            accessibilityRole="button"
            accessibilityLabel={x.label}
            accessibilityState={{ disabled: off }}
            testID={`answer-${x.id}`}
            style={({ pressed }) => ({
              flex: x.role === "primary" ? 1.4 : 1,
              height: 44,
              borderRadius: 12,
              alignItems: "center",
              justifyContent: "center",
              backgroundColor: x.role === "primary" ? c.primary : pressed ? c.accent : c.muted,
              opacity: off ? 0.5 : x.role === "primary" && pressed ? 0.85 : 1,
            })}
          >
            <Text weight="600" style={x.role === "primary" ? { color: c.primaryForeground } : undefined}>
              {x.label}
            </Text>
          </Pressable>
        ))}
      </View>
      {space ? (
        <View style={{ marginTop: 6 }}>
          <Pressable
            disabled={off}
            onPress={() => press(true, "space")}
            accessibilityRole="button"
            accessibilityLabel={space.label}
            accessibilityHint={space.hint ?? undefined}
            accessibilityState={{ disabled: off }}
            testID="answer-allow-space"
            style={{ minHeight: 40, borderRadius: 12, alignItems: "center", justifyContent: "center", opacity: off ? 0.5 : 1 }}
          >
            <Text weight="600" color="brand">
              {space.label}
            </Text>
          </Pressable>
          {approvals?.scopeNote ? (
            <Text variant="caption" color="muted" style={{ textAlign: "center" }}>
              {approvals.scopeNote}
            </Text>
          ) : null}
        </View>
      ) : null}
      {waiting ? (
        <View style={{ flexDirection: "row", justifyContent: "center", gap: 14, marginTop: 8 }}>
          <Text variant="caption" color={waiting.state === "failed" ? "destructive" : "muted"} accessibilityLiveRegion="polite">
            {waiting.label}
          </Text>
          {waiting.canRetry && onRetry ? (
            <Pressable onPress={onRetry} accessibilityRole="button" hitSlop={8}>
              <Text variant="caption" weight="600" color="brand">
                {waiting.retryLabel}
              </Text>
            </Pressable>
          ) : null}
          {waiting.canDiscard && onDiscard ? (
            <Pressable onPress={onDiscard} accessibilityRole="button" hitSlop={8}>
              <Text variant="caption" color="muted">
                {waiting.discardLabel}
              </Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
    </View>
  );
}

/** "1 of 2" over a card, when several approvals wait. */
function Counter({ approvals }: { approvals: ApprovalsView | null }) {
  if (!approvals?.counter) return null;
  return (
    <Text variant="footnote" color="muted" accessibilityLabel={`Approval ${approvals.counter}`}>
      {approvals.counter}
    </Text>
  );
}

const cardShell = (c: ReturnType<typeof useTheme>["c"]) =>
  ({ marginHorizontal: 12, marginVertical: 6, padding: 14, borderRadius: 14, backgroundColor: c.card, borderWidth: 0.5, borderColor: c.border }) as const;

/** The collapsed line an answered approval becomes. */
function Answered({ text, approve }: { text: string; approve: boolean }) {
  const { c } = useTheme();
  return (
    <View style={{ marginHorizontal: 16, marginVertical: 6, flexDirection: "row", alignItems: "center", gap: 8 }} accessible accessibilityLabel={text}>
      <Icon name={approve ? "check" : "close"} size={14} color={approve ? c.success : c.mutedForeground} />
      <Text variant="footnote" color="muted" style={{ flex: 1 }}>
        {text}
      </Text>
    </View>
  );
}

export function ExportCard({
  view,
  answered,
  onAnswer,
  fromName,
  approvals = null,
  waiting = null,
  onRetry,
  onDiscard,
  onOpenDiff,
}: {
  view: ExportCardView;
  answered: AnsweredInfo | null;
  onAnswer(approve: boolean, scope?: Scope): Promise<void> | void;
  fromName: string;
  approvals?: ApprovalsView | null;
  waiting?: OutboxRow | null;
  onRetry?(): void;
  onDiscard?(): void;
  onOpenDiff?(): void;
}) {
  const { c, mono } = useTheme();
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(false);
  if (answered)
    return (
      <Answered
        approve={answered.approve}
        text={`${answered.approve ? view.resultApproved : view.resultDeclined}${answered.by === fromName ? "" : `, by ${answered.by}`}, ${clock(answered.tsMs)}`}
      />
    );
  return (
    <View style={cardShell(c)} accessibilityLabel={`Needs you. ${view.headline}`}>
      <View style={{ flexDirection: "row", alignItems: "center" }}>
        <Text variant="footnote" weight="600" color="warning" style={{ flex: 1 }}>
          Needs you
        </Text>
        <Counter approvals={approvals} />
      </View>
      <Text variant="headline" style={{ marginTop: 2 }}>
        {view.headline}
      </Text>
      <View style={{ marginTop: 10, flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 8 }}>
        <View style={{ backgroundColor: c.muted, borderRadius: 6, paddingHorizontal: 8, paddingVertical: 3 }}>
          <Text variant="mono" numberOfLines={1}>
            {view.branchLine}
          </Text>
        </View>
      </View>
      <View
        style={{ flexDirection: "row", gap: 10, marginTop: 10, alignItems: "baseline" }}
        accessible
        accessibilityLabel={`${view.totals.files}, ${view.totals.additions} added, ${view.totals.deletions} removed`}
      >
        <Text variant="subhead" weight="600">
          {view.totals.files}
        </Text>
        {view.totals.additions ? (
          <Text variant="subhead" weight="600" style={{ color: c.diffAdd, fontFamily: mono }}>
            {view.totals.additions}
          </Text>
        ) : null}
        {view.totals.deletions ? (
          <Text variant="subhead" weight="600" style={{ color: c.diffDel, fontFamily: mono }}>
            {view.totals.deletions}
          </Text>
        ) : null}
      </View>
      {view.account ? (
        <Text variant="footnote" color="muted" style={{ marginTop: 6 }}>
          {view.account}
        </Text>
      ) : null}
      {view.expandable ? (
        <Pressable
          onPress={() => setOpen((o) => !o)}
          accessibilityRole="button"
          accessibilityState={{ expanded: open }}
          accessibilityLabel={open ? view.hideLabel : view.showLabel}
          testID="card-expander"
          style={{ flexDirection: "row", alignItems: "center", gap: 6, minHeight: 32, marginTop: 4 }}
        >
          <Icon name={open ? "down" : "forward"} size={11} color={c.mutedForeground} />
          <Text variant="footnote" color="muted">
            {open ? view.hideLabel : view.showLabel}
          </Text>
        </Pressable>
      ) : null}
      {open ? (
        <View style={{ gap: 12, marginTop: 4 }}>
          {view.files.length ? (
            <View>
              <View style={{ borderRadius: 10, borderWidth: 0.5, borderColor: c.border, overflow: "hidden" }}>
                {view.files.map((f, i) => (
                  <View
                    key={f.dir + f.name}
                    style={{
                      paddingHorizontal: 10,
                      paddingVertical: 7,
                      flexDirection: "row",
                      alignItems: "center",
                      gap: 8,
                      borderTopWidth: i ? 0.5 : 0,
                      borderTopColor: c.border,
                      backgroundColor: f.status === "added" ? c.diffAddSurface : f.status === "deleted" ? c.diffDelSurface : "transparent",
                    }}
                    accessible
                    accessibilityLabel={`${f.dir}${f.name}, ${f.status}, ${f.additions} ${f.deletions}`}
                  >
                    <View style={{ flex: 1 }}>
                      <Text variant="footnote" numberOfLines={1}>
                        <Text variant="footnote" color="muted">
                          {f.dir}
                        </Text>
                        <Text variant="footnote" weight="600">
                          {f.name}
                        </Text>
                      </Text>
                      {f.note ? (
                        <Text variant="caption" color="muted">
                          {f.note}
                        </Text>
                      ) : null}
                    </View>
                    {f.additions ? (
                      <Text variant="caption" weight="600" style={{ color: c.diffAdd, fontFamily: mono }}>
                        {f.additions}
                      </Text>
                    ) : null}
                    {f.deletions ? (
                      <Text variant="caption" weight="600" style={{ color: c.diffDel, fontFamily: mono }}>
                        {f.deletions}
                      </Text>
                    ) : null}
                    <Squares bar={f.bar} />
                  </View>
                ))}
              </View>
              {view.moreFiles ? (
                <Text variant="caption" color="muted" style={{ marginTop: 4 }}>
                  {view.moreFiles}
                </Text>
              ) : null}
            </View>
          ) : null}
          {view.commits.length ? (
            <View style={{ gap: 4 }}>
              <Text variant="footnote" weight="600">
                {view.commitsHeading}
              </Text>
              {view.commits.map((m) => (
                <View key={m.sha} style={{ flexDirection: "row", gap: 8 }}>
                  <Text variant="caption" style={{ fontFamily: mono }} color="muted">
                    {m.sha}
                  </Text>
                  <Text variant="footnote" numberOfLines={1} style={{ flex: 1 }}>
                    {m.subject}
                  </Text>
                </View>
              ))}
              {view.moreCommits ? (
                <Text variant="caption" color="muted">
                  {view.moreCommits}
                </Text>
              ) : null}
            </View>
          ) : null}
          {view.pr ? (
            <View style={{ backgroundColor: c.background, borderRadius: 10, padding: 10, gap: 2 }}>
              <View style={{ flexDirection: "row", alignItems: "center", gap: 8 }}>
                <Text variant="subhead" weight="600" numberOfLines={1} style={{ flex: 1 }}>
                  {view.pr.title}
                </Text>
                <View style={{ backgroundColor: c.muted, borderRadius: 6, paddingHorizontal: 6 }}>
                  <Text variant="caption" weight="600" style={{ color: view.pr.state === "Open" ? c.diffAdd : c.mutedForeground }}>
                    {view.pr.state}
                  </Text>
                </View>
              </View>
              {view.pr.body.map((l, i) => (
                <Text key={i} variant="footnote" color="muted" numberOfLines={1}>
                  {l}
                </Text>
              ))}
            </View>
          ) : null}
          {onOpenDiff ? (
            <Pressable
              onPress={onOpenDiff}
              accessibilityRole="button"
              accessibilityLabel={diffLabels()?.open ?? "Open the full diff"}
              testID="card-open-diff"
              style={{ flexDirection: "row", alignItems: "center", gap: 6, minHeight: 32 }}
            >
              <Icon name="diff" size={13} color={c.brandStrong} />
              <Text variant="footnote" weight="600" color="brand">
                {diffLabels()?.open ?? "Open the full diff"}
              </Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
      <AnswerBar
        approvals={approvals}
        fallback={{ deny: view.deny, allow: view.allow }}
        busy={busy}
        waiting={waiting}
        onRetry={onRetry}
        onDiscard={onDiscard}
        onAnswer={async (a, scope) => {
          setBusy(true);
          try {
            await onAnswer(a, scope);
          } finally {
            setBusy(false);
          }
        }}
      />
    </View>
  );
}

/** An approval without a card: the agent's words, the exact detail, then the core's buttons. */
export function PlainApproval({
  entry,
  answered,
  onAnswer,
  approvals = null,
  waiting = null,
  onRetry,
  onDiscard,
}: {
  entry: ThreadEntry;
  answered: AnsweredInfo | null;
  onAnswer(approve: boolean, scope?: Scope): Promise<void> | void;
  approvals?: ApprovalsView | null;
  waiting?: OutboxRow | null;
  onRetry?(): void;
  onDiscard?(): void;
}) {
  const { c } = useTheme();
  const [busy, setBusy] = useState(false);
  const title = entry.text || entry.approval?.action || "Approval";
  if (answered)
    return (
      <Answered
        approve={answered.approve}
        text={`${approvalResult(answered.approve, answered.scope, title) ?? `${answered.approve ? "Allowed" : "Declined"}: ${title}`}, by ${answered.by}, ${clock(answered.tsMs)}`}
      />
    );
  return (
    <View style={cardShell(c)} accessibilityLabel={`Needs you. ${title}`}>
      <View style={{ flexDirection: "row", alignItems: "center" }}>
        <Text variant="footnote" weight="600" color="warning" style={{ flex: 1 }}>
          Needs you
        </Text>
        <Counter approvals={approvals} />
      </View>
      <Text variant="headline" style={{ marginTop: 2 }}>
        {title}
      </Text>
      {entry.approval?.details ? (
        <View style={{ marginTop: 8, backgroundColor: c.muted, borderRadius: 8, padding: 8 }}>
          <Text variant="mono">{entry.approval.details}</Text>
        </View>
      ) : null}
      <Text variant="footnote" color="muted" style={{ marginTop: 8 }}>
        Asked {clock(entry.tsMs)}. Owners and admins can answer.
      </Text>
      <AnswerBar
        approvals={approvals}
        fallback={{ deny: "Decline", allow: "Allow" }}
        busy={busy}
        waiting={waiting}
        onRetry={onRetry}
        onDiscard={onDiscard}
        onAnswer={async (a, scope) => {
          setBusy(true);
          try {
            await onAnswer(a, scope);
          } finally {
            setBusy(false);
          }
        }}
      />
    </View>
  );
}

/**
 * Tool calls: one muted phrase for the group, one short muted line per call when opened, the detail one tap further.
 * The phrase and every row's state word are the app core's (`toolRows.group`), from the rows the daemon wrote.
 */
export function ToolGroup({ calls }: { calls: ToolCall[] }) {
  const { c, mono } = useTheme();
  const [open, setOpen] = useState(false);
  const [detail, setDetail] = useState<number | null>(null);
  const g = toolGroup(calls.map((k) => ({ cat: k.cat, line: k.line, detail: k.detail, state: k.state })));
  // Without the core the rows still show, with no group phrase.
  const rows = g?.rows ?? calls.map((k) => ({ line: k.line, detail: k.detail, state: k.state, stateLabel: null }));
  return (
    <View style={{ marginHorizontal: 16, marginVertical: 4 }}>
      <Pressable
        onPress={() => {
          Haptics.selectionAsync().catch(() => {});
          setOpen((o) => !o);
        }}
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}
        accessibilityLabel={open ? `${g?.phrase ?? "Tool calls"}. ${g?.hideLabel ?? "Hide details"}` : (g?.closedLabel ?? "Tool calls. Show details")}
        testID="tool-group"
        style={{ flexDirection: "row", alignItems: "center", gap: 6, minHeight: 28 }}
      >
        <Icon name="tool" size={13} color={c.mutedForeground} />
        <Text variant="footnote" color="muted" style={{ flexShrink: 1 }}>
          {g?.phrase ?? ""}
        </Text>
        {g?.running ? (
          <Text variant="caption" color="muted" accessibilityLabel="Running">
            {"\u2026"}
          </Text>
        ) : null}
        <Icon name={open ? "down" : "forward"} size={11} color={c.mutedForeground} />
      </Pressable>
      {open ? (
        <View style={{ marginLeft: 6, paddingLeft: 12, borderLeftWidth: 1, borderLeftColor: c.border }}>
          {rows.map((k, i) => (
            <View key={i}>
              <Pressable
                onPress={() => setDetail(detail === i ? null : i)}
                accessibilityRole="button"
                accessibilityLabel={`${k.line}${k.stateLabel ? `, ${k.stateLabel}` : ""}`}
                style={{ minHeight: 26, flexDirection: "row", alignItems: "center", gap: 8 }}
              >
                <Text variant="footnote" color="muted" numberOfLines={1} style={{ flexShrink: 1 }}>
                  {k.line}
                </Text>
                {k.stateLabel ? (
                  <Text variant="caption" color={k.state === "failed" ? "destructive" : "muted"}>
                    {k.stateLabel}
                  </Text>
                ) : null}
              </Pressable>
              {detail === i && k.detail ? (
                <View style={{ backgroundColor: c.muted, borderRadius: 8, padding: 8, marginBottom: 4 }}>
                  <Text variant="mono" style={{ fontFamily: mono }}>
                    {k.detail}
                  </Text>
                </View>
              ) : null}
            </View>
          ))}
        </View>
      ) : null}
    </View>
  );
}
