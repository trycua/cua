// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { fixtureOverview, kvItem, NOW } from "../test/fakeKeyvault";
import { groupItems, pendingSummary, recentDecisions, shortCaller } from "./keyvault";

describe("keyvault model", () => {
  it("groups items per site and per account, with nothing allowed unattended by default", () => {
    const groups = groupItems(fixtureOverview(), NOW);
    expect(groups.map((g) => g.title)).toEqual(["accounts.example-idp.test", "github.example.test", "Slack"]);
    const gh = groups.find((g) => g.title === "github.example.test")!;
    expect(gh.rows.map((r) => r.account)).toEqual(["ada@example.test", "bob@example.test"]);
    expect(groups.every((g) => g.unattended === "off")).toBe(true);
    expect(groups.flatMap((g) => g.rows).every((r) => !r.item.policy.unattended)).toBe(true);
    expect(groups.find((g) => g.title === "accounts.example-idp.test")!.locked).toBe(true);
  });

  it("derives each item's consent state from pending requests, grants, rules and copies", () => {
    const o = fixtureOverview({
      rules: [
        {
          id: "rule-1",
          items: ["gh-bob"],
          targets: ["*"],
          callers: [{ fp: "fp", display: 'x (team X, signed id "com.example.ci")' }],
          created_ms: NOW - 1,
          not_after_ms: NOW + 86_400_000,
          enabled: true,
          note: "",
        },
      ],
      deliveries: [
        {
          import_id: "imp-1",
          target: "dev-2",
          provider_id: "slack",
          items: ["slack"],
          caller_fp: "fp-koala",
          delivered_ms: NOW - 1,
          expires_ms: NOW + 3_600_000,
          wiped: false,
        },
      ],
    });
    const rows = new Map(groupItems(o, NOW).flatMap((g) => g.rows.map((r) => [r.item.id, r.consent])));
    expect(rows.get("gh-ada")![0]).toEqual({ kind: "pending", text: "Waiting: com.example.koalabot" });
    expect(rows.get("gh-bob")![0]).toEqual({ kind: "rule", text: "Rule: com.example.ci → any Space" });
    expect(rows.get("slack")!.map((c) => c.kind)).toEqual(["delivered", "granted"]);
    expect(rows.get("idp")).toEqual([{ kind: "asks", text: "Asks every time" }]);
  });

  it("ignores expired, revoked and used-up grants", () => {
    const o = fixtureOverview();
    o.grants = [
      { ...o.grants[0]!, id: "a", not_after_ms: NOW - 1 },
      { ...o.grants[0]!, id: "b", revoked: true },
      { ...o.grants[0]!, id: "c", uses_left: 0 },
    ];
    const slack = groupItems(o, NOW).find((g) => g.title === "Slack")!.rows[0]!;
    expect(slack.consent).toEqual([{ kind: "asks", text: "Asks every time" }]);
  });

  it("reports a site as mixed when only some accounts are allowed", () => {
    const o = fixtureOverview();
    o.items[0]!.policy.unattended = true;
    expect(groupItems(o, NOW).find((g) => g.title === "github.example.test")!.unattended).toBe("mixed");
  });

  it("filters by site, account or app", () => {
    expect(groupItems(fixtureOverview(), NOW, "bob").flatMap((g) => g.rows.map((r) => r.item.id))).toEqual(["gh-bob"]);
    expect(groupItems(fixtureOverview(), NOW, "slack").map((g) => g.title)).toEqual(["Slack"]);
  });

  it("lists recent decisions from the audit log, newest first, labelled", () => {
    const d = recentDecisions(fixtureOverview());
    expect(d.map((x) => x.entry.seq)).toEqual([5, 4, 3]);
    expect(d[0]).toMatchObject({ verb: "Refused", tone: "deny" });
    expect(d[0]!.what).toBe("github.example.test (Chrome, Default) → dev-9");
    expect(d[1]).toMatchObject({ verb: "Approved", tone: "ok", what: "Slack (whole app session) → dev-2" });
  });

  it("names callers by their signing identifier", () => {
    expect(shortCaller('team X (team X, signed id "com.example.koalabot")')).toBe("com.example.koalabot");
    expect(shortCaller("com.example.adhoc (ad hoc signature, UNVERIFIED)")).toBe("com.example.adhoc");
    expect(shortCaller("/tmp/evil (UNSIGNED, UNTRUSTED)")).toBe("/tmp/evil");
  });

  it("summarises what a pending request would move and where", () => {
    const p = fixtureOverview().pending[0]!;
    expect(pendingSummary(p)).toBe("github.example.test → dev-1");
    expect(pendingSummary({ ...p, items: [p.items[0]!, p.items[0]!] })).toBe("github.example.test (2 accounts) → dev-1");
    expect(
      pendingSummary({ ...p, items: [], needs_import: [{ kind: "site", app: "firefox", site: "example.test" }] }),
    ).toBe("example.test (not imported) → dev-1");
    expect(kvItem("x").policy.unattended).toBe(false);
  });
});
