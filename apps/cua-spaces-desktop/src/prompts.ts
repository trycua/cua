// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Keyvault's prompts, native to this app (the SwiftUI host's
// `WebUIWindowController.ask` sheets): the unlock prompt, the delete
// confirmation and the passphrase form. Each shows over the window that
// asked, when it is showing.
import type { KvUnlockPrompt } from "./native/generated/index";
import type { Native } from "./native/load";
import type { BridgeUi } from "./bridge/context";
import { askPassphraseWindow, checkView } from "./passphrase";

/** The part of Electron's `dialog` these use. */
export interface MessageBoxes {
  showMessageBox(
    parent: unknown,
    options: { type: "question" | "warning"; message: string; detail: string; buttons: string[]; defaultId: number; cancelId: number; noLink: true },
  ): Promise<{ response: number }>;
}

export type KeyvaultPrompts = Required<Pick<BridgeUi, "askUnlock" | "askDelete" | "askPassphrase">>;

/** The unlock prompt's text: the item (or "3 items") and what Allow lets through, under the title. */
export function unlockDetail(prompt: KvUnlockPrompt): string {
  return prompt.subject === "" ? prompt.message : `${prompt.subject}\n\n${prompt.message}`;
}

export function keyvaultPrompts(boxes: MessageBoxes, native: Pick<Native, "kvPassphraseCheck">): KeyvaultPrompts {
  return {
    async askUnlock(window, prompt) {
      const { response } = await boxes.showMessageBox(window, {
        type: "question",
        message: prompt.title,
        detail: unlockDetail(prompt),
        buttons: [prompt.allow, prompt.deny, prompt.neverAsk],
        defaultId: 0,
        cancelId: 1,
        noLink: true,
      });
      return response === 0 ? "allow" : response === 2 ? "neverAskAgain" : "deny";
    },
    async askDelete(window, confirm) {
      const { response } = await boxes.showMessageBox(window, {
        type: "warning",
        message: confirm.title,
        detail: confirm.message,
        buttons: [confirm.confirm, confirm.cancel],
        defaultId: 0,
        cancelId: 1,
        noLink: true,
      });
      return response === 0;
    },
    askPassphrase: (window, form) => askPassphraseWindow(window, form, (p, c) => checkView(native.kvPassphraseCheck(form.mode, p, c))),
  };
}

/** The prompts on Electron's dialogs. */
export async function electronKeyvaultPrompts(native: Pick<Native, "kvPassphraseCheck">): Promise<KeyvaultPrompts> {
  const { BrowserWindow, dialog } = await import("electron");
  return keyvaultPrompts(
    {
      showMessageBox: (parent, options) =>
        parent instanceof BrowserWindow && !parent.isDestroyed() && parent.isVisible() ? dialog.showMessageBox(parent, options) : dialog.showMessageBox(options),
    },
    native,
  );
}
