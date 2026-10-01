// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type {
  KeyvaultBridge,
  KeyvaultOverview,
  KvAuditEntry,
  KvCaller,
  KvGrant,
  KvItem,
  KvPending,
} from "../native/keyvault";

/** Fixture data only: made-up sites, accounts and callers. No values. */

export const NOW = 1_800_000_000_000;

export function kvItem(id: string, over: Partial<KvItem> = {}): KvItem {
  return {
    id,
    kind: "browser_site",
    label: `${over.site ?? id} (Chrome, Default)`,
    provider_id: "chrome",
    app_display: "Chrome",
    site: over.site,
    account: over.account,
    source: "Default",
    summary: { cookies: [], storage_origins: [], passwords: 0, files: [], keychain_services: [], bytes: 0 },
    warnings: [],
    identity_provider: false,
    policy: { allowed_targets: [], ttl_secs: 3600, unattended: false },
    created_ms: NOW - 86_400_000,
    updated_ms: NOW - 86_400_000,
    rev: 1,
    record_digest: "00",
    ...over,
  };
}

export const KOALA: KvCaller = {
  pid: 4242,
  uid: 501,
  path: "/Applications/OpenKoalaBots.app/Contents/MacOS/OpenKoalaBots",
  signing: { kind: "signed", team_id: "ABCDE12345", identifier: "com.example.koalabot", cdhash: "ab" },
  first_party: false,
  os_verified: true,
};
export const KOALA_DISPLAY = 'team ABCDE12345 (team ABCDE12345, signed id "com.example.koalabot")';

export function fixtureOverview(over: Partial<KeyvaultOverview> = {}): KeyvaultOverview {
  const items = [
    kvItem("gh-ada", { site: "github.example.test", account: "ada@example.test" }),
    kvItem("gh-bob", { site: "github.example.test", account: "bob@example.test" }),
    kvItem("idp", { site: "accounts.example-idp.test", account: "ada@example.test", identity_provider: true }),
    kvItem("slack", {
      kind: "app_session",
      label: "Slack (whole app session)",
      provider_id: "slack",
      app_display: "Slack",
      account: "Example Workspace",
    }),
  ];
  const pending: KvPending[] = [
    {
      id: "req-1",
      caller: KOALA,
      caller_fp: "fp-koala",
      caller_display: KOALA_DISPLAY,
      request: {
        selectors: [{ kind: "item", id: "gh-ada" }],
        targets: ["dev-1"],
        actions: ["teleport"],
        duration_secs: 900,
        reason: "open the pull request",
        claimed_name: "OpenKoalaBots",
      },
      items: [items[0]!],
      needs_import: [],
      created_ms: NOW - 30_000,
    },
  ];
  const grants: KvGrant[] = [
    {
      id: "grant-1",
      request_id: "req-0",
      caller_fp: "fp-koala",
      caller_display: KOALA_DISPLAY,
      items: ["slack"],
      targets: ["dev-2"],
      actions: ["teleport"],
      created_ms: NOW - 60_000,
      not_after_ms: NOW + 600_000,
      uses_left: 1,
      revoked: false,
    },
  ];
  const audit: KvAuditEntry[] = [
    { seq: 1, ts_ms: NOW - 86_400_000, kind: "vault.create", actor: "Cua", caller_fp: "cua", decision: "ok" },
    { seq: 2, ts_ms: NOW - 3_600_000, kind: "item.import", actor: "Cua", caller_fp: "cua", item: "gh-ada", decision: "ok" },
    { seq: 3, ts_ms: NOW - 120_000, kind: "consent.request", actor: KOALA_DISPLAY, caller_fp: "fp-koala", item: "slack", decision: "ok" },
    { seq: 4, ts_ms: NOW - 60_000, kind: "consent.allow", actor: "Cua", caller_fp: "cua", item: "slack", target: "dev-2", decision: "allow" },
    { seq: 5, ts_ms: NOW - 45_000, kind: "teleport.deny", actor: KOALA_DISPLAY, caller_fp: "fp-koala", item: "gh-bob", target: "dev-9", decision: "deny" },
  ];
  return {
    availability: "ready",
    serverVerified: true,
    status: {
      version: "0.1.0",
      initialized: true,
      unlocked: true,
      disabled: false,
      caller_first_party: true,
      caller_display: 'Cua: Cua (team YCK386LBJ7, signed id "com.trycua.spaces.prototype")',
      items: items.length,
      pending: pending.length,
      unlock_policy: "auto",
    },
    items,
    pending,
    grants,
    rules: [],
    deliveries: [],
    audit,
    auditVerification: { ok: true, entries: audit.length, unauthenticated: 0 },
    partialErrors: [],
    ...over,
  };
}

/**
 * An in-memory broker with the real one's rules for the page's actions: the
 * kill switch refuses approvals, identity providers never go unattended,
 * and `presence` decides the actions that widen access.
 */
export function createFakeKeyvaultBridge(initial: KeyvaultOverview = fixtureOverview()) {
  const state = { overview: structuredClone(initial), presence: true };
  const calls: string[] = [];
  /** Passphrases the page sent (fixtures only). */
  const passphrases: string[] = [];
  const requirePresence = () => {
    if (!state.presence) throw new Error("user presence was not confirmed: declined");
  };
  const bridge: KeyvaultBridge & { calls: string[]; passphrases: string[]; state: typeof state } = {
    isNative: true,
    calls,
    passphrases,
    state,
    overview: async () => structuredClone(state.overview),
    setup: async () => {
      calls.push("setup");
      requirePresence();
      state.overview = fixtureOverview({ items: [], pending: [], grants: [], audit: [] });
      return "ABCDE-FGHIJ-KLMNO-PQRST-UVWXY-Z2345-67ABC-DEFGH";
    },
    setupWithPassphrase: async (passphrase) => {
      calls.push("setupWithPassphrase");
      passphrases.push(passphrase);
      requirePresence();
      state.overview = fixtureOverview({ items: [], pending: [], grants: [], audit: [] });
      return "ABCDE-FGHIJ-KLMNO-PQRST-UVWXY-Z2345-67ABC-DEFGH";
    },
    unlock: async () => {
      calls.push("unlock");
      state.overview = { ...fixtureOverview(), status: { ...fixtureOverview().status!, unlocked: true } };
    },
    unlockWithPassphrase: async (passphrase) => {
      calls.push("unlockWithPassphrase");
      passphrases.push(passphrase);
      state.overview = { ...fixtureOverview(), status: { ...fixtureOverview().status!, unlocked: true } };
    },
    setDisabled: async (disabled) => {
      calls.push(`setDisabled:${disabled}`);
      if (!disabled) requirePresence();
      state.overview.status = { ...state.overview.status!, disabled };
      if (disabled) state.overview.pending = [];
    },
    setUnattended: async (ids, on) => {
      calls.push(`setUnattended:${ids.join(",")}:${on}`);
      if (state.overview.status?.disabled) throw new Error("the Keyvault is disabled (global kill switch)");
      if (on) requirePresence();
      const out: KvItem[] = [];
      for (const id of ids) {
        const item = state.overview.items.find((i) => i.id === id)!;
        if (on && item.identity_provider) throw new Error(`forbidden: ${item.label} is an identity provider`);
        item.policy = { ...item.policy, unattended: on };
        out.push(item);
      }
      return out;
    },
    revokeGrant: async (id) => {
      calls.push(`revokeGrant:${id}`);
      let n = 0;
      for (const g of state.overview.grants)
        if ((id === "*" || g.id === id) && !g.revoked) {
          g.revoked = true;
          n++;
        }
      return n;
    },
    removeRule: async (id) => {
      calls.push(`removeRule:${id}`);
      state.overview.rules = state.overview.rules.filter((r) => r.id !== id);
    },
    release: async (target) => {
      calls.push(`release:${target}`);
      const wiped = state.overview.deliveries.filter((d) => d.target === target && !d.wiped);
      for (const d of wiped) d.wiped = true;
      return wiped.map((d) => d.import_id);
    },
    approve: async (requestId, items) => {
      calls.push(`approve:${requestId}`);
      calls.push(`approve-items:${items === null ? "*" : items.join(",")}`);
      if (state.overview.status?.disabled) throw new Error("the Keyvault is disabled (global kill switch)");
      requirePresence();
      const p = state.overview.pending.find((x) => x.id === requestId)!;
      state.overview.pending = state.overview.pending.filter((x) => x.id !== requestId);
      const grant: KvGrant = {
        id: `grant-for-${requestId}`,
        request_id: requestId,
        caller_fp: p.caller_fp,
        caller_display: p.caller_display,
        items: items ?? p.items.map((i) => i.id),
        targets: p.request.targets,
        actions: ["teleport"],
        created_ms: NOW,
        not_after_ms: NOW + 900_000,
        uses_left: 1,
        revoked: false,
      };
      state.overview.grants.push(grant);
      return grant;
    },
    deny: async (requestId) => {
      calls.push(`deny:${requestId}`);
      state.overview.pending = state.overview.pending.filter((x) => x.id !== requestId);
    },
  };
  return bridge;
}
