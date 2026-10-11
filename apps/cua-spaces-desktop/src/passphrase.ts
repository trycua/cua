// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Keyvault's passphrase form: a small window of this app, loaded from a
// page built here (never the web UI), talking to the main process only
// through its own preload (`passphrase-preload.ts`). A passphrase is typed
// here and goes from this window to the main process to the broker over the
// verified Keyvault socket; it never crosses the web UI's bridge (the
// SwiftUI app types it in a native secure field). The form is the core's:
// its labels, and `kvPassphraseCheck` for the hint and whether it can be sent.
import { randomBytes } from "node:crypto";
import * as path from "node:path";
import type { KvCredentialForm, KvFormMode, KvPassphraseCheck } from "./native/generated/index";
import type { CredentialAnswer } from "./model/keyvault";

/** The preload's channels (shell-internal; not the web bridge's). */
export const PASSPHRASE_CHECK = "cua-passphrase:check";
export const PASSPHRASE_SUBMIT = "cua-passphrase:submit";
export const PASSPHRASE_CANCEL = "cua-passphrase:cancel";

/** What the page needs to say about the fields as they are typed. */
export interface PassphraseCheck {
  canSubmit: boolean;
  strength: string | null;
  hint: string | null;
}

export const escapeHtml = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

/** The core's check as the page reads it. */
export function checkView(c: KvPassphraseCheck): PassphraseCheck {
  return { canSubmit: c.canSubmit, strength: c.strength ?? null, hint: c.hint ?? null };
}

/** The window's title: what the form does. */
export function formTitle(mode: KvFormMode): string {
  // Flat enums carry the Swift case names as their values.
  return (mode as string) === "setup" ? "Set up Keyvault" : "Unlock Keyvault";
}

/**
 * The form's page. Its script runs under a nonce and nothing else loads:
 * no network, no frames, no other script.
 */
export function passphrasePage(form: KvCredentialForm, nonce: string): string {
  const confirm = form.confirmLabel !== undefined;
  const field = (id: string, label: string, auto: string) =>
    `<label for="${id}">${escapeHtml(label)}</label><input id="${id}" type="password" autocomplete="${auto}" spellcheck="false" autocapitalize="off">`;
  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'">
<title>${escapeHtml(formTitle(form.mode))}</title>
<style nonce="${nonce}">
:root { color-scheme: light dark; --bg: #f7f8fa; --fg: #181b1f; --muted: #667; --line: #cfd3d8; --accent: #2f6df6; }
@media (prefers-color-scheme: dark) { :root { --bg: #16181c; --fg: #e9ebef; --muted: #9aa0a8; --line: #3a3e45; --accent: #5b8cff; } }
html, body { margin: 0; background: var(--bg); color: var(--fg); font: 13px/1.4 system-ui, sans-serif; }
form { display: flex; flex-direction: column; gap: 6px; padding: 18px 20px 16px; }
h1 { margin: 0 0 2px; font-size: 15px; font-weight: 600; }
p { margin: 0 0 8px; color: var(--muted); }
label { margin-top: 6px; }
input { padding: 6px 8px; border: 1px solid var(--line); border-radius: 6px; background: transparent; color: inherit; font: inherit; }
input:focus { outline: 2px solid var(--accent); outline-offset: -1px; }
#hint { min-height: 1.4em; color: var(--muted); }
.row { display: flex; justify-content: flex-end; gap: 8px; margin-top: 8px; }
button { padding: 5px 14px; border: 1px solid var(--line); border-radius: 6px; background: transparent; color: inherit; font: inherit; }
button.primary { background: var(--accent); border-color: var(--accent); color: #fff; }
button:disabled { opacity: 0.45; }
</style></head>
<body><form id="form">
<h1>${escapeHtml(formTitle(form.mode))}</h1>
<p>${escapeHtml(form.help)}</p>
${field("p", form.passphraseLabel ?? "Passphrase", confirm ? "new-password" : "current-password")}
${confirm ? field("c", form.confirmLabel ?? "Confirm passphrase", "new-password") : ""}
<div id="hint" role="status"></div>
<div class="row"><button type="button" id="cancel">Cancel</button><button type="submit" id="ok" class="primary" disabled>${escapeHtml(form.submitLabel)}</button></div>
</form>
<script nonce="${nonce}">
const p = document.getElementById("p"), c = document.getElementById("c"), ok = document.getElementById("ok"), hint = document.getElementById("hint");
const second = () => (c ? c.value : "");
async function update() {
  const r = await window.passphrase.check(p.value, second());
  ok.disabled = !r.canSubmit;
  hint.textContent = r.hint || "";
}
p.addEventListener("input", update);
if (c) c.addEventListener("input", update);
document.getElementById("form").addEventListener("submit", (e) => {
  e.preventDefault();
  if (!ok.disabled) window.passphrase.submit(p.value, second());
});
document.getElementById("cancel").addEventListener("click", () => window.passphrase.cancel());
document.addEventListener("keydown", (e) => { if (e.key === "Escape") window.passphrase.cancel(); });
p.focus();
</script></body></html>`;
}

/**
 * One form's life: answers the page's checks, and settles once with the
 * passphrase (submitted), or null (cancelled, or the window closed).
 */
export class PassphraseSession {
  readonly answer: Promise<CredentialAnswer>;
  private settle!: (a: CredentialAnswer) => void;
  private done = false;

  constructor(private readonly check: (passphrase: string, confirm: string) => PassphraseCheck) {
    this.answer = new Promise((resolve) => (this.settle = resolve));
  }

  get settled(): boolean {
    return this.done;
  }

  checkFields(passphrase: unknown, confirm: unknown): PassphraseCheck {
    return this.check(typeof passphrase === "string" ? passphrase : "", typeof confirm === "string" ? confirm : "");
  }

  /** Sent only when the core's check allows it; anything else keeps the form open. */
  submit(passphrase: unknown, confirm: unknown): boolean {
    if (this.done || typeof passphrase !== "string") return false;
    const second = typeof confirm === "string" ? confirm : "";
    if (!this.check(passphrase, second).canSubmit) return false;
    this.finish({ passphrase, confirm: second });
    return true;
  }

  cancel(): void {
    this.finish(null);
  }

  private finish(a: CredentialAnswer): void {
    if (this.done) return;
    this.done = true;
    this.settle(a);
  }
}

let open: Promise<unknown> = Promise.resolve();

/**
 * Asks in a window of its own (over `parent` when it is showing): the
 * passphrase and, at setup, its confirmation; null when the user cancels.
 * One form at a time: a second ask waits for the first.
 */
export function askPassphraseWindow(
  parent: unknown,
  form: KvCredentialForm,
  check: (passphrase: string, confirm: string) => PassphraseCheck,
): Promise<CredentialAnswer> {
  const turn = open.then(() => showForm(parent, form, check));
  open = turn.catch(() => {});
  return turn;
}

async function showForm(
  parent: unknown,
  form: KvCredentialForm,
  check: (passphrase: string, confirm: string) => PassphraseCheck,
): Promise<CredentialAnswer> {
  const { BrowserWindow, ipcMain, nativeTheme } = await import("electron");
  const owner = parent instanceof BrowserWindow && !parent.isDestroyed() && parent.isVisible() ? parent : undefined;
  const session = new PassphraseSession(check);
  const win = new BrowserWindow({
    width: 400,
    height: form.confirmLabel === undefined ? 220 : 290,
    useContentSize: true,
    title: formTitle(form.mode),
    parent: owner,
    modal: owner !== undefined,
    show: false,
    resizable: false,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    backgroundColor: nativeTheme.shouldUseDarkColors ? "#16181c" : "#f7f8fa",
    webPreferences: {
      preload: path.join(__dirname, "passphrase-preload.cjs"),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      devTools: false,
      spellcheck: false,
    },
  });
  win.setMenuBarVisibility(false);
  const mine = (e: { sender: unknown }) => e.sender === win.webContents;
  const onCheck = (e: Electron.IpcMainInvokeEvent, p: unknown, c: unknown) => (mine(e) ? session.checkFields(p, c) : { canSubmit: false, strength: null, hint: null });
  const onSubmit = (e: Electron.IpcMainEvent, p: unknown, c: unknown) => {
    if (mine(e)) session.submit(p, c);
  };
  const onCancel = (e: Electron.IpcMainEvent) => {
    if (mine(e)) session.cancel();
  };
  ipcMain.handle(PASSPHRASE_CHECK, onCheck);
  ipcMain.on(PASSPHRASE_SUBMIT, onSubmit);
  ipcMain.on(PASSPHRASE_CANCEL, onCancel);
  win.webContents.on("will-navigate", (e) => e.preventDefault());
  win.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  win.once("ready-to-show", () => win.show());
  win.on("closed", () => session.cancel());
  void session.answer.then(() => {
    ipcMain.removeHandler(PASSPHRASE_CHECK);
    ipcMain.removeListener(PASSPHRASE_SUBMIT, onSubmit);
    ipcMain.removeListener(PASSPHRASE_CANCEL, onCancel);
    if (!win.isDestroyed()) win.close();
  });
  const nonce = randomBytes(16).toString("base64");
  void win.loadURL(`data:text/html;charset=utf-8,${encodeURIComponent(passphrasePage(form, nonce))}`);
  return session.answer;
}
