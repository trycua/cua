// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The passphrase form's preload (src/passphrase.ts): the form's page can
// check what was typed, send it, or cancel, and nothing else. Self-contained:
// a sandboxed preload can only require "electron".
import { contextBridge, ipcRenderer } from "electron";

contextBridge.exposeInMainWorld("passphrase", {
  check: (passphrase: string, confirm: string) => ipcRenderer.invoke("cua-passphrase:check", passphrase, confirm),
  submit: (passphrase: string, confirm: string) => ipcRenderer.send("cua-passphrase:submit", passphrase, confirm),
  cancel: () => ipcRenderer.send("cua-passphrase:cancel"),
});
