// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Keyvault's passphrase form (src/passphrase.ts) and the prompts around
// it (src/prompts.ts), without Electron: the page is built here and loads
// nothing, a passphrase leaves the form only through the core's check, and
// the dialogs map their buttons to the answers the bridge reads.
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { PASSPHRASE_CANCEL, PASSPHRASE_CHECK, PASSPHRASE_SUBMIT, PassphraseSession, checkView, escapeHtml, formTitle, passphrasePage, type PassphraseCheck } from "../src/passphrase";
import { keyvaultPrompts, unlockDetail, type MessageBoxes } from "../src/prompts";
import type { KvCredentialForm, KvFormMode, KvMethod, KvPassphraseCheck } from "../src/native/generated/index";

const setup = { mode: "setup" as KvFormMode, method: "passphrase" as KvMethod, help: "12 or more characters.", passphraseLabel: "Passphrase", confirmLabel: "Confirm passphrase", submitLabel: "Set up Keyvault" };
const unlock = { mode: "unlock" as KvFormMode, method: "passphrase" as KvMethod, help: "Enter the Keyvault passphrase.", passphraseLabel: "Passphrase", confirmLabel: undefined, submitLabel: "Unlock" };

describe("the passphrase form's page", () => {
  it("asks twice at setup and once to unlock", () => {
    const a = passphrasePage(setup as KvCredentialForm, "n");
    expect(a).toContain('id="p"');
    expect(a).toContain('id="c"');
    expect(a).toContain("Confirm passphrase");
    expect(a).toContain("Set up Keyvault");
    const b = passphrasePage(unlock as KvCredentialForm, "n");
    expect(b).toContain('id="p"');
    expect(b).not.toContain('id="c"');
    expect(b).toContain("Unlock");
  });

  it("loads nothing and runs only its own script, under the nonce", () => {
    const html = passphrasePage(unlock as KvCredentialForm, "abc123");
    expect(html).toContain("default-src 'none'");
    expect(html).toContain("script-src 'nonce-abc123'");
    expect(html).not.toMatch(/https?:|<img|<iframe|<link|src=/i);
    expect(html.match(/<script/g)).toHaveLength(1);
    expect(html).toContain('<script nonce="abc123">');
  });

  it("escapes what the core's words and a hostile label could carry", () => {
    const html = passphrasePage({ ...unlock, help: `<img src=x onerror="alert(1)">`, submitLabel: "A&B" } as KvCredentialForm, "n");
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img src=x onerror=&quot;alert(1)&quot;&gt;");
    expect(html).toContain("A&amp;B");
    expect(escapeHtml(`'"<>&`)).toBe("&#39;&quot;&lt;&gt;&amp;");
  });

  it("starts with the button off and has no field a password manager would fill as a username", () => {
    const html = passphrasePage(setup as KvCredentialForm, "n");
    expect(html).toMatch(/id="ok"[^>]*disabled/);
    expect(html).toContain('autocomplete="new-password"');
    expect(html).not.toContain('type="text"');
  });

  it("titles the window by what it does", () => {
    expect(formTitle("setup" as KvFormMode)).toBe("Set up Keyvault");
    expect(formTitle("unlock" as KvFormMode)).toBe("Unlock Keyvault");
  });
});

describe("the form's preload", () => {
  // The preload is a bundle of its own and cannot import the channel names.
  const preload = readFileSync(path.join(import.meta.dirname, "../src/passphrase-preload.ts"), "utf8");

  it("speaks the channels the main process answers, and exposes nothing else", () => {
    for (const channel of [PASSPHRASE_CHECK, PASSPHRASE_SUBMIT, PASSPHRASE_CANCEL]) expect(preload).toContain(`"${channel}"`);
    expect([...preload.matchAll(/"(cua[\w-]*:[\w-]+)"/g)].map((m) => m[1]).sort()).toEqual([PASSPHRASE_CANCEL, PASSPHRASE_CHECK, PASSPHRASE_SUBMIT].sort());
    expect(preload.match(/exposeInMainWorld\(/g)).toHaveLength(1);
    // Only what the page calls: check, submit, cancel.
    for (const name of ["check", "submit", "cancel"]) expect(passphrasePage(unlock as KvCredentialForm, "n")).toContain(`window.passphrase.${name}(`);
  });
});

describe("a passphrase session", () => {
  // The core's rule in miniature: twelve characters, and the two fields equal.
  const check = (p: string, c: string): PassphraseCheck => ({ canSubmit: p.length >= 12 && p === c, strength: null, hint: p.length < 12 ? "Too short" : null });

  it("answers the page's checks from the core, and tolerates anything else", () => {
    const s = new PassphraseSession(check);
    expect(s.checkFields("short", "short")).toEqual({ canSubmit: false, strength: null, hint: "Too short" });
    expect(s.checkFields("long enough pass", "long enough pass").canSubmit).toBe(true);
    expect(s.checkFields(42, undefined).canSubmit).toBe(false);
  });

  it("settles once with the passphrase, only when the core allows it", async () => {
    const s = new PassphraseSession(check);
    expect(s.submit("short", "short")).toBe(false);
    expect(s.submit("long enough pass", "different one!!")).toBe(false);
    expect(s.submit({}, "x")).toBe(false);
    expect(s.settled).toBe(false);
    expect(s.submit("long enough pass", "long enough pass")).toBe(true);
    expect(await s.answer).toEqual({ passphrase: "long enough pass", confirm: "long enough pass" });
    // Settled: later input changes nothing.
    expect(s.submit("another long pass!", "another long pass!")).toBe(false);
    s.cancel();
    expect(await s.answer).toEqual({ passphrase: "long enough pass", confirm: "long enough pass" });
  });

  it("settles with null when cancelled or closed", async () => {
    const s = new PassphraseSession(check);
    s.cancel();
    expect(await s.answer).toBeNull();
    expect(s.settled).toBe(true);
    expect(s.submit("long enough pass", "long enough pass")).toBe(false);
  });

  it("reads the core's check as the page does", () => {
    const c: KvPassphraseCheck = { canSubmit: true, strength: "strong" as KvPassphraseCheck["strength"], hint: undefined };
    expect(checkView(c)).toEqual({ canSubmit: true, strength: "strong", hint: null });
  });
});

describe("the Keyvault's native prompts", () => {
  /** Dialogs that answer with `response` and record what they showed. */
  function boxes(response: number) {
    const shown: Parameters<MessageBoxes["showMessageBox"]>[] = [];
    const impl: MessageBoxes = {
      showMessageBox: async (...args) => {
        shown.push(args);
        return { response };
      },
    };
    return { impl, shown };
  }
  const native = { kvPassphraseCheck: () => ({ canSubmit: true, strength: undefined, hint: undefined }) };
  const prompt = { title: "Allow unattended access?", message: "Cua will sign in without asking.", subject: "3 items", deny: "Deny", allow: "Allow", neverAsk: "Never ask again" };

  it("maps the unlock prompt's buttons: Allow, Deny, Never ask again, and closing is Deny", async () => {
    for (const [response, answer] of [[0, "allow"], [1, "deny"], [2, "neverAskAgain"], [-1, "deny"]] as const) {
      const b = boxes(response);
      expect(await keyvaultPrompts(b.impl, native).askUnlock("win", prompt)).toBe(answer);
      expect(b.shown[0]![0]).toBe("win");
      expect(b.shown[0]![1]).toMatchObject({ message: "Allow unattended access?", buttons: ["Allow", "Deny", "Never ask again"], defaultId: 0, cancelId: 1 });
    }
  });

  it("words the unlock prompt with its subject above what Allow lets through", () => {
    expect(unlockDetail(prompt)).toBe("3 items\n\nCua will sign in without asking.");
    expect(unlockDetail({ ...prompt, subject: "" })).toBe("Cua will sign in without asking.");
  });

  it("confirms a delete as a warning with the destructive button first", async () => {
    const confirm = { title: "Delete 2 items?", message: "Copies delivered to a Space will be wiped as well.", confirm: "Delete", cancel: "Cancel" };
    const yes = boxes(0);
    expect(await keyvaultPrompts(yes.impl, native).askDelete("win", confirm)).toBe(true);
    expect(yes.shown[0]![1]).toMatchObject({ type: "warning", message: "Delete 2 items?", buttons: ["Delete", "Cancel"] });
    expect(await keyvaultPrompts(boxes(1).impl, native).askDelete("win", confirm)).toBe(false);
    expect(await keyvaultPrompts(boxes(-1).impl, native).askDelete("win", confirm)).toBe(false);
  });
});
