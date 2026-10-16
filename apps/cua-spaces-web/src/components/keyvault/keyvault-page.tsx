// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, CopyIcon, KeyRoundIcon, LockIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { isNativeHost, useBridge, useKeyvault, type KvCategory } from "@/bridge";
import { KEYVAULT_SETUP_COMMAND } from "@/bridge/ops/keyvault-setup";
import { EmptyState, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Segmented } from "@/components/ui/segmented";
import { toastError } from "@/components/ui/toast";
import { hostOs, keyvaultUnlockWay } from "@/lib/host-labels";

import { ApprovalDialog } from "./approval-dialog";
import { CategoryPane } from "./category-pane";
import { VaultList } from "./vault-list";

/** A failed action says why; an answer of "no" to the host's own question is not a failure. */
const reportFailure = (title: string) => {
  const report = toastError(title);
  return (e: unknown) => {
    if ((e as { code?: string } | null)?.code !== "cancelled") report(e);
  };
};

/** Items waiting on the user's confirmation to delete (a host that asks itself needs no page dialog). */
interface DeleteAsk {
  ids: string[];
  title: string;
  message: string;
}

/**
 * The Keyvault, as the SwiftUI app has it: a vault list (items grouped by the
 * app they came from, each with a lock) and the Waiting, Access and Recent
 * categories. Every list, count and label is the core's (`views.sidebar`,
 * `views.vault`, `pane`); the host decides access and no secret value is here.
 * `initialView` and `focus` come from a Space's "Signed in" badge: its Access
 * row is brought forward.
 */
export function KeyvaultPage({ initialView = "all", focus = null }: { initialView?: KvCategory; focus?: string | null }) {
  const kv = useKeyvault();
  const { data, unlock, unlockItems, lockItems, deleteItems, setUp, approve, deny, runCommand, dismissAccess, showItems, vaultAction, pane } = kv;
  const { core, mode } = useBridge();
  const views = data?.views ?? null;
  // No vault yet: the core's page offers setup; without it, the overview says so.
  const canSetup = views ? views.page.canSetup : data?.overview.availability === "no_vault";
  // The core's page decides (a vault not set up yet is not locked); without it, the overview.
  const locked = views
    ? views.page.canUnlock
    : data
      ? data.overview.availability === "locked" || data.overview.status?.unlocked === false
      : false;
  // Not running, not set up, or only partly read: the host says why.
  const availability = data?.overview.availability;
  const unavailable = !canSetup && Boolean(availability && availability !== "ready" && availability !== "locked");
  const notice = canSetup
    ? null
    : unavailable
      ? (views?.page.unavailableTitle ?? data?.overview.message ?? "Keyvault isn't available.")
      : (data?.overview.partialErrors ?? []).join(" ");
  // Under the heading, why and what to do (the broker's sentence), as the
  // SwiftUI app's unavailable page has it.
  const noticeDetail = unavailable && views?.page.unavailableTitle ? (views.page.message ?? data?.overview.message ?? null) : null;
  const [category, setCategory] = useState<KvCategory>(initialView);
  const [focused, setFocused] = useState<string | null>(focus);
  const [settingUp, setSettingUp] = useState(false);
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null);
  const [reviewing, setReviewing] = useState<string | null>(null);
  const [asking, setAsking] = useState<DeleteAsk | null>(null);
  // A Space's badge brings its Access row forward, then lets the highlight go.
  useEffect(() => {
    if (!focused) return;
    const t = setTimeout(() => setFocused(null), 2500);
    return () => clearTimeout(t);
  }, [focused]);
  // Requests in flight (the broker answers one at a time, and Touch ID can take a moment).
  const [pending, setPending] = useState(0);
  const busy = pending > 0;
  const act = (title: string, p: Promise<unknown>) => {
    setPending((n) => n + 1);
    void p.catch(reportFailure(title)).finally(() => setPending((n) => n - 1));
  };
  const unlockVault = () => act("Couldn't unlock Keyvault", unlock());
  const setUpVault = () => {
    setSettingUp(true);
    void setUp()
      .then(setRecoveryKey, reportFailure("Couldn't set up Keyvault"))
      .finally(() => setSettingUp(false));
  };
  const actions = {
    lock: (ids: string[]) => act("Couldn't change access", lockItems(ids)),
    unlock: (ids: string[]) => act("Couldn't change access", unlockItems(ids)),
  };
  /** A native host confirms in its own alert; other hosts get the core's words here. */
  const requestDelete = (ids: string[]) => {
    if (isNativeHost(mode) || !data) return act("Couldn't delete", deleteItems(ids));
    const copies = core.tryCall<number>("keyvault.liveCopySpaces", { overview: data.overview, ids, now: Date.now() }) ?? 0;
    const ask = core.tryCall<{ title: string; message: string }>("keyvault.deleteConfirm", { count: ids.length, liveCopies: copies });
    if (ask) setAsking({ ids, ...ask });
    else act("Couldn't delete", deleteItems(ids));
  };
  const categories = views?.sidebar.categories ?? [];
  const list = views && category !== "all" ? pane({ kind: "category", category }) : null;
  const waiting = data?.overview.pending ?? [];

  return (
    <div className="flex h-full flex-col">
      <div className="mx-auto w-full max-w-3xl px-8 pt-6">
        <PageHeader
          title="Keyvault"
          description="Saved logins your agents can use. Each use asks you first unless you allow it."
          actions={
            locked ? (
              <Button variant="outline" onClick={unlockVault}>
                <LockIcon /> Unlock
              </Button>
            ) : null
          }
        />
        {locked ? (
          <div className="mb-4 flex items-center gap-3 rounded-xl border bg-brand-surface/60 px-4 py-3">
            <LockIcon className="size-4 text-brand-strong" />
            <p className="flex-1 text-[13px]">Keyvault is locked. Unlock to change who can use a login.</p>
          </div>
        ) : null}
        {views?.page.disabledBanner ? (
          <p className="mb-4 rounded-xl border border-destructive/30 bg-destructive/10 px-4 py-2.5 text-[13px]" data-vault-disabled>
            {views.page.disabledBanner}
          </p>
        ) : null}
        {views?.page.resetNotice ? <p className="mb-4 rounded-xl border bg-brand-surface/60 px-4 py-2.5 text-[13px]">{views.page.resetNotice}</p> : null}
        {notice ? <p className="mb-4 px-1 text-xs text-muted-foreground" data-vault-notice>{notice}</p> : null}
        {noticeDetail && noticeDetail !== notice ? (
          <p className="-mt-2 mb-4 px-1 text-xs text-muted-foreground" data-vault-notice-detail>
            {noticeDetail}
          </p>
        ) : null}
        {recoveryKey ? <RecoveryKey value={recoveryKey} onDone={() => setRecoveryKey(null)} /> : null}
        {canSetup ? (
          <SetUpVault
            passphrase={views?.page.form?.method === "passphrase"}
            help={views?.page.form?.help ?? null}
            label={views?.page.labels.setUp ?? "Set up Keyvault"}
            busy={settingUp}
            onSetUp={setUpVault}
          />
        ) : categories.length ? (
          <div className="mb-3">
            <Segmented
              aria-label="Keyvault"
              value={category}
              onValueChange={setCategory}
              options={categories.map((c) => ({
                value: c.category,
                label: (
                  <>
                    {c.title}
                    {c.badge ? (
                      <span data-waiting-badge className="min-w-4 rounded-full bg-brand px-1 text-center text-2xs font-medium text-white tabular-nums">
                        {c.badge}
                      </span>
                    ) : c.count !== null ? (
                      <span className="text-2xs text-muted-foreground tabular-nums">{c.count}</span>
                    ) : null}
                  </>
                ),
              }))}
            />
          </div>
        ) : null}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        {canSetup ? null : !views || !data ? (
          <div className="mx-auto max-w-3xl px-8">
            <EmptyState icon={<KeyRoundIcon />} title="Keyvault">
              The list needs the app core, which isn't loaded.
            </EmptyState>
          </div>
        ) : !views.page.ready ? null : list ? (
          <CategoryPane
            list={list}
            page={views.page}
            dismissed={data.overview.dismissed ?? []}
            focus={focused}
            busy={busy}
            onReview={setReviewing}
            onDeny={(id) => act("Couldn't deny", deny(id))}
            onRun={(c) => act("Couldn't change access", runCommand(c))}
            onDismiss={(imports) => act("Couldn't dismiss", dismissAccess(imports))}
          />
        ) : (
          <VaultList
            view={views.vault}
            query={data.vaultState.query}
            blocked={locked || views.page.disabled}
            busy={busy}
            send={vaultAction}
            actions={actions}
            onShowItems={() => act("Couldn't show the items", showItems())}
            onDelete={requestDelete}
          />
        )}
      </div>
      {reviewing && data && views && waiting.some((p) => p.id === reviewing) ? (
        <ApprovalDialog
          overview={data.overview}
          labels={views.page.labels}
          requestId={reviewing}
          busy={busy}
          onClose={() => setReviewing(null)}
          onDeny={(id) => {
            setReviewing(null);
            act("Couldn't deny", deny(id));
          }}
          onApprove={(id, items) => {
            setReviewing(null);
            act("Couldn't approve", approve(id, items));
          }}
        />
      ) : null}
      <ConfirmDialog
        open={asking !== null}
        onOpenChange={(open) => !open && setAsking(null)}
        title={asking?.title ?? ""}
        description={asking?.message}
        confirmLabel="Delete"
        destructive
        onConfirm={() => asking && act("Couldn't delete", deleteItems(asking.ids))}
      />
    </div>
  );
}

/** No vault yet: what it is for, how it is protected, and the one next step. */
function SetUpVault({ passphrase, help, label, busy, onSetUp }: { passphrase: boolean; help: string | null; label: string; busy: boolean; onSetUp: () => void }) {
  return (
    <div data-vault-setup>
      <EmptyState
        icon={<KeyRoundIcon />}
        title="No Keyvault yet"
        action={
          <Button onClick={onSetUp} disabled={busy}>
            {busy ? "Setting up\u2026" : label}
          </Button>
        }
      >
        Set it up to save logins your agents can use.{" "}
        {passphrase ? "You choose a passphrase to protect it." : (help ?? `${capitalized(keyvaultUnlockWay())} or a passphrase protects it.`)}
      </EmptyState>
      <p className="mt-3 px-1 text-center text-xs text-muted-foreground">
        Or run <code className="font-mono">{KEYVAULT_SETUP_COMMAND}</code> in {hostOs() === "macos" ? "Terminal" : "a terminal"}.
      </p>
    </div>
  );
}

/** The recovery key, shown once after setup: the only other way in. */
function RecoveryKey({ value, onDone }: { value: string; onDone: () => void }) {
  const [copied, setCopied] = useState(false);
  return (
    <div data-vault-recovery className="mb-4 rounded-xl border bg-brand-surface/60 px-4 py-3">
      <p className="text-[13px] font-medium">Save your recovery key</p>
      <p className="mt-0.5 text-xs text-muted-foreground">It's shown once. You need it if {keyvaultUnlockWay()} or your passphrase stops working.</p>
      <div className="mt-2 flex items-center gap-2">
        <code className="flex-1 truncate rounded-md bg-background px-2 py-1 font-mono text-[13px]" data-recovery-key>
          {value}
        </code>
        <Button
          size="sm"
          variant="outline"
          aria-label={copied ? "Copied" : "Copy recovery key"}
          onClick={() => void navigator.clipboard?.writeText(value).then(() => setCopied(true), () => {})}
        >
          {copied ? <CheckIcon /> : <CopyIcon />} {copied ? "Copied" : "Copy"}
        </Button>
        <Button size="sm" variant="ghost" onClick={onDone}>
          Done
        </Button>
      </div>
    </div>
  );
}

const capitalized = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
