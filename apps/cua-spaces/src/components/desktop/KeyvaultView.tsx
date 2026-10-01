// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useRef, useState } from "react";

import {
  approvalItems,
  approvalView,
  kvList,
  kvPage,
  kvPassphraseCheck,
  kvRecoveryKeyText,
  kvSidebar,
  kvSiteDetail,
  kvSiteToggle,
  openApproval,
  reduceApproval,
  type ApprovalState,
  type ItemRow,
  type KvCommand,
  type KvCredentialForm,
  type KvPage,
  type KvSelection,
  type SiteGroup,
} from "../../model/keyvault";
import type { KeyvaultBridge, KeyvaultOverview } from "../../native/keyvault";
import { Sym } from "./Sym";

/** How often the page re-reads the broker while it is open. */
export const KEYVAULT_POLL_MS = 5_000;
/** How often the sidebar badge re-reads it otherwise. */
export const KEYVAULT_IDLE_POLL_MS = 30_000;

/**
 * Keeps the broker's overview fresh: every [`KEYVAULT_POLL_MS`] while the
 * page is open, every [`KEYVAULT_IDLE_POLL_MS`] for the sidebar badge.
 */
export function useKeyvaultOverview(bridge: KeyvaultBridge, active: boolean) {
  const [overview, setOverview] = useState<KeyvaultOverview | null>(null);
  const alive = useRef(true);
  const refresh = useCallback(async () => {
    try {
      const next = await bridge.overview();
      if (alive.current) setOverview(next);
    } catch (error) {
      if (alive.current)
        setOverview({
          availability: "error",
          message: error instanceof Error ? error.message : String(error),
          serverVerified: false,
          items: [],
          pending: [],
          grants: [],
          rules: [],
          deliveries: [],
          audit: [],
          partialErrors: [],
        });
    }
  }, [bridge]);
  useEffect(() => {
    alive.current = true;
    if (!bridge.isNative && !active) return;
    void refresh();
    const timer = window.setInterval(() => void refresh(), active ? KEYVAULT_POLL_MS : KEYVAULT_IDLE_POLL_MS);
    return () => {
      alive.current = false;
      window.clearInterval(timer);
    };
  }, [bridge, active, refresh]);
  return { overview, refresh };
}

/** One broker request at a time, its error, and the recovery key after setup. */
export function useKeyvaultActions(bridge: KeyvaultBridge, refresh: () => Promise<void>) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null);
  const run = useCallback(
    async (key: string, action: () => Promise<unknown>) => {
      setBusy(key);
      setError(null);
      try {
        await action();
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(null);
        await refresh();
      }
    },
    [refresh],
  );
  /** Runs a core command against the broker. */
  const command = useCallback(
    (cmd: KvCommand) => {
      switch (cmd.type) {
        case "setup":
          return run("setup", async () => setRecoveryKey(await bridge.setup()));
        case "unlock":
          return run("unlock", () => bridge.unlock());
        case "set-disabled":
          return run("kill", () => bridge.setDisabled(cmd.disabled));
        case "set-unattended":
          return run(`toggle:${cmd.itemIds.join(",")}`, () => bridge.setUnattended(cmd.itemIds, cmd.unattended));
        case "revoke-grant":
          return run(`revoke:${cmd.id}`, () => bridge.revokeGrant(cmd.id));
        case "remove-rule":
          return run(`rule:${cmd.id}`, () => bridge.removeRule(cmd.id));
        case "release":
          return run(`wipe:${cmd.target}`, () => bridge.release(cmd.target));
        case "approve":
          return run(`approve:${cmd.requestId}`, () => bridge.approve(cmd.requestId, cmd.items));
        case "deny":
          return run(`deny:${cmd.requestId}`, () => bridge.deny(cmd.requestId));
      }
    },
    [bridge, run],
  );
  /** Sets up or unlocks the way the form offers. The passphrase goes to
   * the shell's broker client only; nothing here keeps it. */
  const submitCredential = useCallback(
    (form: KvCredentialForm, passphrase: string) => {
      if (form.mode === "setup")
        return run("setup", async () =>
          setRecoveryKey(
            form.method === "passphrase" ? await bridge.setupWithPassphrase(passphrase) : await bridge.setup(),
          ),
        );
      return run("unlock", () =>
        form.method === "passphrase" ? bridge.unlockWithPassphrase(passphrase) : bridge.unlock(),
      );
    },
    [bridge, run],
  );
  return { busy, error, setError, recoveryKey, setRecoveryKey, command, submitCredential };
}

export type KeyvaultActions = ReturnType<typeof useKeyvaultActions>;

/**
 * The Keyvault's sidebar section: the title with the
 * global switch (on while the Keyvault works; turning it back on asks for
 * Touch ID in the Cua daemon), then the categories and one row per site.
 */
export function KeyvaultSidebarSection({
  title,
  overview,
  now,
  selection,
  active,
  actions,
  onSelect,
}: {
  title: string;
  overview: KeyvaultOverview | null;
  now: number;
  selection: KvSelection;
  active: boolean;
  actions: KeyvaultActions;
  onSelect: (selection: KvSelection) => void;
}) {
  const page = overview ? kvPage(overview, now) : null;
  const sb = overview ? kvSidebar(overview, now) : null;
  const isSel = (s: KvSelection) =>
    active &&
    s.kind === selection.kind &&
    (s.kind === "category"
      ? selection.kind === "category" && s.category === selection.category
      : selection.kind === "site" && s.key === selection.key);
  return (
    <div className="dw-kv-section">
      <div className="dw-nav-section dw-kv-head">
        <span>{title}</span>
        {page?.killSwitchVisible && (
          <button
            type="button"
            role="switch"
            className="kv-switch"
            aria-checked={!page.disabled}
            aria-label={title}
            title={page.killSwitchHelp}
            disabled={actions.busy !== null || !page.killSwitchEnabled}
            onClick={() => void actions.command({ type: "set-disabled", disabled: !page.disabled })}
          />
        )}
      </div>
      <ul className="dw-nav-list" role="listbox" aria-label={title}>
        {(sb?.categories ?? []).map((c) => {
            const s: KvSelection = { kind: "category", category: c.category };
            return (
              <li key={c.category}>
                <button
                  type="button"
                  role="option"
                  aria-selected={isSel(s)}
                  className="dw-row"
                  onClick={() => onSelect(s)}
                >
                  <Sym name={c.symbol} size={13} />
                  <span className="dw-row-name">{c.title}</span>
                  {c.badge ? (
                    <span className="dw-primary-count" data-tone="accent" aria-label={`${c.badge} waiting`}>
                      {c.badge}
                    </span>
                  ) : null}
                </button>
              </li>
            );
          })}
        {(sb?.sites ?? []).map((site) => {
          const s: KvSelection = { kind: "site", key: site.key };
          return (
            <li key={site.key}>
              <button type="button" role="option" aria-selected={isSel(s)} className="dw-row" onClick={() => onSelect(s)}>
                <span className="dw-row-name">{site.title}</span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

export interface KeyvaultViewProps {
  overview: KeyvaultOverview | null;
  selection: KvSelection;
  actions: KeyvaultActions;
  now?: () => number;
}

/**
 * The Keyvault pane for the sidebar's selection: a category (All, Waiting,
 * Access, Recent) or one site. What shows is the app core's
 * (`keyvault::browse`); the page asks the Cua daemon's broker for
 * everything, and anything that widens access (approve, turn the Keyvault
 * back on, allow an item unattended) is confirmed by the daemon with Touch
 * ID or the login password, not by this window.
 */
export function KeyvaultView({ overview, selection, actions, now = Date.now }: KeyvaultViewProps) {
  const [query, setQuery] = useState("");
  const [approval, setApproval] = useState<ApprovalState | null>(null);
  const t = now();
  const page = overview ? kvPage(overview, t) : null;
  const list = overview && page?.ready ? kvList(overview, selection, t, query) : null;
  const site = overview && page?.ready && selection.kind === "site" ? kvSiteDetail(overview, selection.key, t) : null;
  const busy = actions.busy !== null;
  const blocked = Boolean(page?.disabled);
  const labels = page?.labels;
  const title = list?.title || "Keyvault";

  return (
    <>
      <header className="dw-toolbar" data-tauri-drag-region>
        <div className="dw-toolbar-title" data-tauri-drag-region>
          <h1>{title}</h1>
        </div>
      </header>

      {page?.disabledBanner && (
        <div className="dw-banner" data-tone="error" role="status">
          <span>{page.disabledBanner}</span>
        </div>
      )}
      {actions.recoveryKey && (
        <div className="dw-banner" role="status">
          <span>{kvRecoveryKeyText(actions.recoveryKey)}</span>
          <button type="button" className="dw-icon-btn" aria-label="Dismiss" onClick={() => actions.setRecoveryKey(null)}>
            <Sym name="xmark" size={12} />
          </button>
        </div>
      )}
      {actions.error && (
        <div className="dw-banner" data-tone="error" role="alert">
          <span>{actions.error}</span>
          <button type="button" className="dw-icon-btn" aria-label="Dismiss" onClick={() => actions.setError(null)}>
            <Sym name="xmark" size={12} />
          </button>
        </div>
      )}

      <div className="dw-content">
        {!overview || !page || !labels ? null : !page.ready ? (
          <div className="dw-content-inner kv">
            <h2 className="kv-empty" title={page.message ?? undefined}>
              {page.unavailableTitle}
            </h2>
            {page.message && <p className="kv-none">{page.message}</p>}
            {page.form && (
              <CredentialForm
                key={`${page.form.mode}:${page.form.method}`}
                form={page.form}
                busy={busy}
                onSubmit={(passphrase) => void actions.submitCredential(page.form!, passphrase)}
              />
            )}
          </div>
        ) : selection.kind === "site" ? (
          <div className="dw-content-inner kv">
            {site ? (
              <>
                <dl className="dw-list" aria-label={site.group.title}>
                  <div>
                    <dt>{labels.appLabel}</dt>
                    <dd>{site.group.app}</dd>
                  </div>
                </dl>
                {site.siteSwitch && (
                  <div className="kv-row">
                    <span className="kv-text">{labels.everyAccount}</span>
                    <Switch
                      label={`Allow every ${site.group.title} account unattended`}
                      checked={site.siteState === "on" ? true : site.siteState === "off" ? false : "mixed"}
                      disabled={busy || blocked || !site.siteSwitchEnabled}
                      title={site.siteSwitchHelp}
                      onChange={(on) => void actions.command(kvSiteToggle(site.group, on))}
                    />
                  </div>
                )}
                <section className="dw-section" aria-label={labels.accountsTitle}>
                  <h2 className="kv-h">{labels.accountsTitle}</h2>
                  <div className="kv-group" role="list" aria-label="Saved items">
                    {site.group.rows.map((row) => (
                      <AccountRow key={row.item.id} group={site.group} row={row} busy={busy} blocked={blocked} actions={actions} />
                    ))}
                  </div>
                </section>
              </>
            ) : (
              <p className="kv-none">{list?.emptyText}</p>
            )}
          </div>
        ) : (
          list && (
            <div className="dw-content-inner kv">
              {page.partialErrors.length > 0 && (
                <p className="dw-error" role="alert">
                  {page.partialErrors.join(" · ")}
                </p>
              )}
              {selection.category === "all" && page.searchVisible && (
                <input
                  className="dw-input kv-search"
                  type="search"
                  placeholder="Search"
                  aria-label="Search items"
                  value={query}
                  spellCheck={false}
                  onChange={(e) => setQuery(e.target.value)}
                />
              )}
              {list.sites.length > 0 && (
                <div role="list" aria-label="Saved items">
                  {list.sites.map((g) => (
                    <section className="dw-section" key={g.key} role="listitem" aria-label={g.title}>
                      <h2 className="kv-h">{g.title}</h2>
                      <div className="kv-group">
                        {g.rows.map((row) => (
                          <AccountRow key={row.item.id} group={g} row={row} busy={busy} blocked={blocked} actions={actions} />
                        ))}
                      </div>
                    </section>
                  ))}
                </div>
              )}
              {list.pending.length > 0 && overview && (
                <div className="kv-group" role="list" aria-label={list.title}>
                  {list.pending.map((p) => (
                    <div key={p.id} role="listitem">
                      <div className="kv-row" title={p.claims.join("\n") || undefined}>
                        <span className="kv-text">
                          {p.caller}{" "}
                          <span className="kv-trust" data-tone={p.badge.tone}>
                            ({p.badge.text})
                          </span>{" "}
                          · {p.summary} · {p.wants}
                        </span>
                        <button
                          type="button"
                          className="dw-btn"
                          disabled={busy}
                          onClick={() => {
                            setApproval(null);
                            void actions.command({ type: "deny", requestId: p.id });
                          }}
                        >
                          {labels.deny}
                        </button>
                        <button
                          type="button"
                          className="dw-btn dw-btn-primary"
                          disabled={busy || blocked}
                          onClick={() => setApproval(openApproval(p.id))}
                        >
                          {labels.review}
                        </button>
                      </div>
                      {approval?.requestId === p.id && (
                        <ApprovalSheet
                          overview={overview}
                          page={page}
                          state={approval}
                          busy={busy}
                          onChange={setApproval}
                          onCancel={() => setApproval(null)}
                          onDeny={() => {
                            setApproval(null);
                            void actions.command({ type: "deny", requestId: p.id });
                          }}
                          onApprove={(items) => {
                            setApproval(null);
                            void actions.command({ type: "approve", requestId: p.id, items });
                          }}
                        />
                      )}
                    </div>
                  ))}
                </div>
              )}
              {list.access.length > 0 && (
                <>
                  {page.revokeAll && (
                    <div className="kv-head">
                      <button
                        type="button"
                        className="dw-btn dw-btn-quiet"
                        disabled={busy}
                        onClick={() => void actions.command({ type: "revoke-grant", id: "*" })}
                      >
                        {labels.revokeAll}
                      </button>
                    </div>
                  )}
                  <div className="kv-group" role="list" aria-label={list.title}>
                    {list.access.map((a) => (
                      <div className="kv-row" role="listitem" key={a.key}>
                        <span className="kv-text">
                          {a.text}
                          {a.detail && <span className="kv-muted"> · {a.detail}</span>}
                        </span>
                        <button
                          type="button"
                          className="dw-btn"
                          disabled={busy}
                          onClick={() => void actions.command(a.command)}
                        >
                          {a.actionLabel}
                        </button>
                      </div>
                    ))}
                  </div>
                </>
              )}
              {list.recent.length > 0 && (
                <>
                  <ol className="kv-group" aria-label="Audit log">
                    {list.recent.map((r) => (
                      <li className="kv-row" key={r.decision.entry.seq}>
                        <span className="kv-verb" data-tone={r.decision.tone}>
                          {r.decision.verb}
                        </span>
                        <span className="kv-text">{r.decision.what}</span>
                        <span className="kv-muted">{r.age}</span>
                      </li>
                    ))}
                  </ol>
                  {page.logStatus && (
                    <p className="kv-muted" data-tone={page.logTampered ? "danger" : undefined} title="Hash-chained, MAC'd audit log">
                      {page.logStatus}
                    </p>
                  )}
                </>
              )}
              {list.emptyText && <p className="kv-none">{list.emptyText}</p>}
              {selection.category === "all" && page.protection.length > 0 && (
                <section className="dw-section" aria-label={labels.protectionTitle}>
                  <h2 className="kv-h">{labels.protectionTitle}</h2>
                  <dl className="dw-list">
                    {page.protection.map((f) => (
                      <div key={f.label}>
                        <dt>{f.label}</dt>
                        <dd>{f.value}</dd>
                      </div>
                    ))}
                  </dl>
                </section>
              )}
            </div>
          )
        )}
      </div>
    </>
  );
}

/** One account: its line, its state and its unattended switch. */
/**
 * Set up or unlock: one button for Touch ID, or password fields (twice for
 * setup) when the daemon cannot use the OS key store. The fields live only
 * in this component's state and are cleared when sent.
 */
function CredentialForm({
  form,
  busy,
  onSubmit,
}: {
  form: KvCredentialForm;
  busy: boolean;
  onSubmit: (passphrase: string) => void;
}) {
  const [passphrase, setPassphrase] = useState("");
  const [confirm, setConfirm] = useState("");
  const usesPassphrase = form.method === "passphrase";
  const check = usesPassphrase ? kvPassphraseCheck(form.mode, passphrase, confirm) : null;
  const canSubmit = !busy && (!usesPassphrase || Boolean(check?.canSubmit));
  const submit = () => {
    if (!canSubmit) return;
    const secret = passphrase;
    setPassphrase("");
    setConfirm("");
    onSubmit(secret);
  };
  return (
    <form
      className="kv-credential"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      {usesPassphrase && (
        <>
          <label className="dw-field">
            <span>{form.passphraseLabel}</span>
            <input
              type="password"
              className="dw-input"
              autoComplete={form.mode === "setup" ? "new-password" : "current-password"}
              value={passphrase}
              onChange={(event) => setPassphrase(event.target.value)}
              autoFocus
            />
          </label>
          {form.confirmLabel && (
            <label className="dw-field">
              <span>{form.confirmLabel}</span>
              <input
                type="password"
                className="dw-input"
                autoComplete="new-password"
                value={confirm}
                onChange={(event) => setConfirm(event.target.value)}
              />
            </label>
          )}
        </>
      )}
      <div>
        <button type="submit" className="dw-btn dw-btn-primary" disabled={!canSubmit}>
          {form.submitLabel}
        </button>
      </div>
      <p className="kv-none">{check?.hint ?? form.help}</p>
    </form>
  );
}

function AccountRow({
  group,
  row,
  busy,
  blocked,
  actions,
}: {
  group: SiteGroup;
  row: ItemRow;
  busy: boolean;
  blocked: boolean;
  actions: KeyvaultActions;
}) {
  const state = row.consent.find((c) => c.kind !== "asks");
  return (
    <div className="kv-row" role="listitem" aria-label={row.account}>
      <span className="kv-text" title={row.item.warnings[0]}>
        {row.account}
      </span>
      {state && (
        <span className="kv-state" data-kind={state.kind}>
          {state.text}
        </span>
      )}
      <Switch
        label={`Allow ${row.account} on ${group.title} unattended`}
        checked={row.item.policy.unattended}
        disabled={busy || blocked || !row.toggleEnabled}
        title={row.toggleHelp}
        onChange={(on) => void actions.command({ type: "set-unattended", itemIds: [row.item.id], unattended: on })}
      />
    </div>
  );
}

/** Approve exactly the items the user ticks; nothing is ticked at first. */
function ApprovalSheet({
  overview,
  page,
  state,
  busy,
  onChange,
  onCancel,
  onDeny,
  onApprove,
}: {
  overview: KeyvaultOverview;
  page: KvPage;
  state: ApprovalState;
  busy: boolean;
  onChange: (next: ApprovalState) => void;
  onCancel: () => void;
  onDeny: () => void;
  onApprove: (items: string[] | null) => void;
}) {
  const v = approvalView(overview, state);
  const items = approvalItems(overview, state);
  return (
    <div className="kv-approval" role="group" aria-label={v.title}>
      <p className="kv-text">
        <strong>{v.title}</strong>
      </p>
      <p className="kv-muted" title={v.claims.join("\n") || undefined}>
        {v.badge.text} · to {v.targets} · {v.wants}
      </p>
      {v.rows.map((row) => (
        <label className="kv-row kv-sub" key={row.key}>
          <input
            type="checkbox"
            checked={row.selected}
            aria-label={`${row.title} ${row.account}`}
            onChange={() => onChange(reduceApproval(overview, state, { type: "toggle", key: row.key }))}
          />
          <span className="kv-text">
            {row.title}
            <span className="kv-muted"> · {row.account}</span>
          </span>
        </label>
      ))}
      {v.blockedReason && <p className="kv-none">{v.blockedReason}</p>}
      <div className="kv-row">
        <span className="kv-text kv-muted">{page.labels.confirmNote}</span>
        <button type="button" className="dw-btn" onClick={onCancel}>
          {page.labels.cancel}
        </button>
        <button type="button" className="dw-btn" disabled={busy} onClick={onDeny}>
          {page.labels.deny}
        </button>
        <button
          type="button"
          className="dw-btn dw-btn-primary"
          disabled={busy || !v.canApprove || items === undefined}
          onClick={() => items !== undefined && onApprove(items)}
        >
          {v.approveLabel}
        </button>
      </div>
    </div>
  );
}

function Switch({
  label,
  checked,
  disabled,
  title,
  onChange,
}: {
  label: string;
  checked: boolean | "mixed";
  disabled?: boolean;
  title?: string;
  onChange: (next: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      className="kv-switch"
      aria-checked={checked === "mixed" ? "mixed" : checked}
      aria-label={label}
      title={title}
      disabled={disabled}
      onClick={() => onChange(checked !== true)}
    />
  );
}
