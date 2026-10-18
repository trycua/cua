// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Teleport an app" and "Share" end to end on the demo host, through the
 * real app core: the picker from the grid to a finished run, and the Share
 * sheet from an invite to a removal.
 */

import { describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import { ShareStore } from "../share";
import type { DataAdapter } from "../adapter";
import type { CoreClient } from "../core";
import { MAX_IMAGES, MAX_READS, TeleportStore, pickerConsent } from "../teleport";
import { testCore, until, wasmBuilt } from "./testCore";

const space = { id: "local:design-review", name: "design-review" };

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("teleport icons and previews", () => {
  // No core call here: only what the store keeps of the host's images.
  const store = () => new TeleportStore(createDemoAdapter({ latencyMs: 0, stepMs: 2 }), {} as CoreClient);

  it("keeps a bounded number, dropping the oldest and reading them again if shown again", async () => {
    const t = store();
    const reads = new Map<string, number>();
    const read = (key: string) => () => {
      reads.set(key, (reads.get(key) ?? 0) + 1);
      return Promise.resolve(`data:image/png;base64,${key}`);
    };
    for (let i = 0; i < MAX_IMAGES + 20; i++) t.loadImage(`k${i}`, read(`k${i}`));
    await settle();
    expect(t.getImages().size).toBe(MAX_IMAGES);
    expect(t.getImages().has("k0")).toBe(false);
    expect(t.getImages().has(`k${MAX_IMAGES + 19}`)).toBe(true);
    // Shown again after eviction: read again. Still held: not read twice.
    t.loadImage("k0", read("k0"));
    t.loadImage(`k${MAX_IMAGES + 19}`, read(`k${MAX_IMAGES + 19}`));
    await settle();
    expect(reads.get("k0")).toBe(2);
    expect(reads.get(`k${MAX_IMAGES + 19}`)).toBe(1);
    expect(t.getImages().size).toBe(MAX_IMAGES);
  });

  it("asks the host for a few at a time, the rest waiting their turn", async () => {
    const t = store();
    let inFlight = 0;
    let peak = 0;
    const finish: (() => void)[] = [];
    for (let i = 0; i < 12; i++) {
      t.loadImage(`k${i}`, () => {
        peak = Math.max(peak, ++inFlight);
        return new Promise<string>((r) =>
          finish.push(() => {
            inFlight--;
            r(`data:image/png;base64,${i}`);
          }),
        );
      });
    }
    expect(inFlight).toBe(MAX_READS);
    while (finish.length) {
      finish.shift()!();
      await settle();
    }
    expect(peak).toBe(MAX_READS);
    expect(t.getImages().size).toBe(12);
  });

  it("forgets them when the picker closes, and drops a read that finishes after", async () => {
    const t = store();
    t.loadImage("a", () => Promise.resolve("data:image/png;base64,a"));
    await settle();
    expect(t.getImages().size).toBe(1);
    t.close();
    expect(t.getImages().size).toBe(0);

    let finish!: (url: string) => void;
    t.loadImage("late", () => new Promise<string>((r) => (finish = r)));
    t.close();
    finish("data:image/png;base64,late");
    await settle();
    expect(t.getImages().size).toBe(0);
    // Asked for again by the next picker.
    let again = 0;
    t.loadImage("late", () => ((again += 1), Promise.resolve("data:image/png;base64,late")));
    await settle();
    expect(again).toBe(1);
    expect(t.getImages().get("late")).toBe("data:image/png;base64,late");
  });
});

describe.skipIf(!wasmBuilt)("teleport picker", () => {
  it("picks Chrome, reviews its sites minimal by default, and runs", async () => {
    const t = new TeleportStore(createDemoAdapter({ latencyMs: 0, stepMs: 2 }), await testCore());
    t.open(space);
    const pick = await until(() => {
      const s = t.getSession()!;
      expect(s.state.step).toBe("pick");
      expect(s.grid.sections.length).toBeGreaterThan(0);
      return s;
    });
    expect(pick.tabs.map((x) => x.label)).toEqual(["Apps", "Open windows", "From design-review"]);
    expect(pick.grid.sections.map((x) => x.title)).toEqual(["Recent", "Apps", "Not available"]);
    expect(pick.primary).toEqual({ label: "Continue", enabled: true });

    t.setQuery("chrome");
    const chrome = t.getSession()!.grid.sections.flatMap((x) => x.tiles);
    expect(chrome.map((x) => x.id)).toEqual(["com.google.Chrome"]);
    await t.activate(chrome[0]);
    expect(t.getSession()!.state.step).toBe("options");

    t.send({ type: "move", move: "app_with_state" });
    t.send({ type: "sensitive", group: "sign_ins", value: true });
    expect(t.getSession()!.frame.planSensitive).toEqual(["sign_ins"]);
    await t.plan();
    const review = await until(() => {
      const r = t.getSession()!.frame.review!;
      expect(r.needsDomains).toBe(false);
      return r;
    });
    // Sites that keep a sign-in, never the identity provider or an unreadable one.
    expect(review.domains.filter((d) => d.selected).map((d) => d.domain)).toEqual(["github.com", "linear.app", "vercel.com"]);
    expect(review.domainSummary).toBe("3 of 7 sites");
    expect(review.canConfirm).toBe(false);

    t.send({ type: "acknowledge", value: true });
    expect(t.getSession()!.frame.review!.canConfirm).toBe(true);
    // The run says which step it is on (the core's `flow.status`), with bytes while they move.
    const statuses: string[] = [];
    t.subscribe(() => {
      const status = t.getSession()?.frame.status;
      if (status) statuses.push(status);
    });
    await t.confirm();
    const done = await until(() => {
      const s = t.getSession()!;
      expect(s.state.step).toBe("done");
      return s;
    });
    expect(done.state.report?.appId).toBe("com.google.Chrome");
    expect(done.frame.progress).toBe(1);
    expect(statuses.some((s) => /^Uploading \d/.test(s))).toBe(true);
    expect(statuses.every((s) => s.trim().length > 0)).toBe(true);
    expect(done.frame.status).toBeNull();
  });

  /** Chrome's sign-ins to the review (planned, sites read). */
  async function reviewChrome(t: TeleportStore) {
    t.open(space);
    await until(() => expect(t.getSession()!.state.step).toBe("pick"));
    t.setQuery("chrome");
    await t.activate(t.getSession()!.grid.sections.flatMap((x) => x.tiles)[0]);
    t.send({ type: "move", move: "app_with_state" });
    t.send({ type: "sensitive", group: "sign_ins", value: true });
    await t.plan();
    return until(() => {
      const r = t.getSession()!.frame.review!;
      expect(r.needsDomains).toBe(false);
      return r;
    });
  }

  it("starts the review from the sites sent last time to this Space", async () => {
    const base = createDemoAdapter({ latencyMs: 0, stepMs: 2 });
    const asked: unknown[] = [];
    const adapter = Object.assign(Object.create(base) as DataAdapter, {
      call: ((op: string, args: unknown) => {
        if (op !== "teleport.remembered") return (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
        asked.push(args);
        return Promise.resolve(["linear.app", "docs.rs"]);
      }) as DataAdapter["call"],
    });
    const t = new TeleportStore(adapter, await testCore());
    const review = await reviewChrome(t);
    expect(asked).toEqual([{ providerId: "chrome", spaceId: space.id }]);
    expect(review.domains.filter((d) => d.selected).map((d) => d.domain).sort()).toEqual(["docs.rs", "linear.app"]);
  });

  it("sends from the Keyvault's saved items, the ones chosen, instead of the live app", async () => {
    const t = new TeleportStore(createDemoAdapter({ latencyMs: 0, stepMs: 2 }), await testCore());
    const live = await reviewChrome(t);
    // The Keyvault holds items for Chrome: the review offers them.
    expect(live.offersVault).toBe(true);
    expect(live.source).toBe("live");
    expect(t.getSession()!.vault).toBeNull();
    t.send({ type: "acknowledge", value: true });

    await t.sendFrom("vault");
    const s = await until(() => {
      const x = t.getSession()!;
      expect(x.frame.review!.source).toBe("vault");
      expect(x.vault).not.toBeNull();
      return x;
    });
    // Everything the app has saved is chosen to start with; passwords are never listed.
    expect(s.vault!.view.selection.count).toBeGreaterThan(0);
    expect(s.vault!.view.apps.map((a) => a.providerId)).toEqual(["chrome"]);
    expect(s.vault!.view.apps[0]!.sites.flatMap((x) => x.rows).every((r) => r.kind !== "password")).toBe(true);
    const chosen = pickerConsent(t.core, s.state);
    expect(chosen.fromVault).toHaveLength(s.vault!.view.selection.count);
    expect(chosen.cookieDomains ?? null).toBeNull();

    // Choosing none leaves nothing to confirm; choosing all sends them again.
    t.sendVault({ type: "clear" });
    expect(t.getSession()!.vault!.view.selection.count).toBe(0);
    expect(t.getSession()!.frame.review!.canConfirm).toBe(false);
    t.sendVault({ type: "query", text: "github" });
    expect(t.getSession()!.vault!.query).toBe("github");
    t.sendVault({ type: "select-all" });
    expect(t.getSession()!.frame.review!.canConfirm).toBe(true);
    expect(pickerConsent(t.core, t.getSession()!.state).fromVault!.length).toBe(t.getSession()!.vault!.view.selection.count);

    // Back to the live app: sites again, nothing from the Keyvault.
    await t.sendFrom("live");
    expect(t.getSession()!.vault).toBeNull();
    expect(pickerConsent(t.core, t.getSession()!.state).fromVault ?? null).toBeNull();
  });

  it("streams a Space's window from the From tab and closes", async () => {
    const t = new TeleportStore(createDemoAdapter({ latencyMs: 0, stepMs: 2 }), await testCore());
    t.open(space);
    t.setTab("space");
    const tiles = await until(() => {
      const x = t.getSession()!.grid.sections.flatMap((s) => s.tiles);
      expect(x.length).toBe(2);
      return x;
    });
    t.select(tiles[0]!.id);
    expect(t.getSession()!.primary).toEqual({ label: "Stream to This Mac", enabled: true });
    await t.activate();
    expect(t.getSession()).toBeNull();
  });
});

describe.skipIf(!wasmBuilt)("share sheet", () => {
  it("shares, changes nothing while busy, and removes", async () => {
    const s = new ShareStore(createDemoAdapter({ latencyMs: 0, stepMs: 2 }), await testCore());
    s.open({ ...space, sdk: { features: ["relay_attach"], reachable: true, spacesdVersion: "0.6.0" } }, true);
    await until(() => expect(s.getSession()!.view.rows.map((r) => r.who)).toEqual(["grace@cua.ai"]));

    await s.send({ type: "set-who", who: "bob@" });
    expect(s.getSession()!.view.canShare).toBe(false);
    await s.send({ type: "set-who", who: "bob@example.com" });
    await s.send({ type: "submit" });
    expect(s.getSession()!.view.rows.map((r) => `${r.who} ${r.role}`)).toEqual(["grace@cua.ai editor", "bob@example.com viewer"]);
    expect(s.getSession()!.view.busy).toBe(false);

    await s.send({ type: "remove", who: "grace@cua.ai" });
    expect(s.getSession()!.view.rows.map((r) => r.who)).toEqual(["bob@example.com"]);
  });

  it("says why a signed-out or old Space can't be shared", async () => {
    const s = new ShareStore(createDemoAdapter({ latencyMs: 0 }), await testCore());
    s.open({ ...space, sdk: { features: [], reachable: true, spacesdVersion: "0.6.0" } }, false);
    expect(s.getSession()!.view.disabledReason).toBeTruthy();
    expect(s.getSession()!.view.canShare).toBe(false);
  });
});
