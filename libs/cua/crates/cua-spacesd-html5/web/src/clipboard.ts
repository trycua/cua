// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Two-way clipboard sync over `ComputerService.GetClipboard` /
 * `SetClipboard` (text and PNG images).
 *
 * Host to guest:
 * - Ctrl/Cmd+V in the viewer: the browser's `paste` event carries the host
 *   clipboard without any permission prompt; it is set on the guest before
 *   the keys are sent, so the guest pastes the host's content.
 * - When the viewer regains focus and the page may read the clipboard
 *   (`clipboard-read` granted), the host clipboard is pushed too, so a
 *   right-click Paste in the guest also sees it.
 *
 * Guest to host: the guest clipboard generation is polled (every second,
 * and right after Ctrl/Cmd+C or X); new content is written with the async
 * Clipboard API. Where the engine refuses a write without a user gesture,
 * the viewer offers a one-click "Copy" instead.
 */

import type { Api } from "./api";

/** What is on a clipboard, in the flavors the viewer syncs. */
export interface ClipContent {
  text?: string;
  png?: Uint8Array;
}

export function sameContent(a: ClipContent | null, b: ClipContent | null): boolean {
  if (!a || !b) return a === b;
  if ((a.text ?? "") !== (b.text ?? "")) return false;
  const pa = a.png;
  const pb = b.png;
  if (!pa || !pb) return !pa && !pb;
  if (pa.byteLength !== pb.byteLength) return false;
  for (let i = 0; i < pa.byteLength; i++) if (pa[i] !== pb[i]) return false;
  return true;
}

export function isEmpty(content: ClipContent): boolean {
  return !content.text && !content.png;
}

const POLL_MS = 1000;
const MAX_TEXT_BYTES = 1024 * 1024;
const MAX_IMAGE_BYTES = 16 * 1024 * 1024;

export interface ClipboardSyncOptions {
  api: Api;
  /** Clipboard write refused without a gesture: offer a button. */
  onNeedsGesture?: (write: () => Promise<void>) => void;
  onSynced?: (direction: "to-guest" | "to-host", content: ClipContent) => void;
  onError?: (message: string) => void;
}

export class ClipboardSync {
  private generation: bigint | null = null;
  private lastGuest: ClipContent | null = null;
  private lastPushed: ClipContent | null = null;
  private timer: number | null = null;
  private stopped = true;
  private readonly off: Array<() => void> = [];
  private busy = false;

  constructor(private readonly options: ClipboardSyncOptions) {}

  start(): void {
    if (!this.stopped) return;
    this.stopped = false;
    const onFocus = () => void this.pushHostIfReadable();
    window.addEventListener("focus", onFocus);
    this.off.push(() => window.removeEventListener("focus", onFocus));
    this.schedule(0);
  }

  stop(): void {
    this.stopped = true;
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = null;
    for (const off of this.off.splice(0)) off();
  }

  /** Poll soon (after a copy shortcut in the guest). */
  soon(): void {
    for (const delay of [150, 400, 900]) window.setTimeout(() => void this.poll(), delay);
  }

  /** `paste` in the viewer: set the guest clipboard from the event. */
  async fromPasteEvent(event: ClipboardEvent): Promise<void> {
    const data = event.clipboardData;
    if (!data) return;
    const content: ClipContent = {};
    const text = data.getData("text/plain");
    if (text) content.text = text;
    for (const item of Array.from(data.items ?? [])) {
      if (item.kind === "file" && item.type === "image/png") {
        const file = item.getAsFile();
        if (file && file.size <= MAX_IMAGE_BYTES) content.png = new Uint8Array(await file.arrayBuffer());
      }
    }
    if (!isEmpty(content)) await this.setGuest(content);
  }

  /** Put `content` on the guest clipboard (skips a no-op). */
  async setGuest(content: ClipContent): Promise<void> {
    if (sameContent(content, this.lastGuest)) return;
    if (content.text && new TextEncoder().encode(content.text).byteLength > MAX_TEXT_BYTES) {
      this.options.onError?.("clipboard text over 1 MiB was not sent");
      delete content.text;
    }
    if (isEmpty(content)) return;
    const response = await this.options.api.computer.setClipboard({
      content: { text: content.text, imagePng: content.png, filePaths: [] },
    });
    this.generation = response.generation;
    this.lastGuest = content;
    this.lastPushed = content;
    this.options.onSynced?.("to-guest", content);
  }

  /** Push the host clipboard when the page may read it without a prompt. */
  async pushHostIfReadable(): Promise<void> {
    if (this.stopped || !navigator.clipboard?.read) return;
    try {
      const status = await navigator.permissions.query({ name: "clipboard-read" as PermissionName });
      if (status.state !== "granted") return;
    } catch {
      return; // no permissions API for the clipboard (Firefox, Safari)
    }
    try {
      const content = await readHost();
      if (content && !isEmpty(content) && !sameContent(content, this.lastPushed)) await this.setGuest(content);
    } catch {
      // focus raced the read; the next focus retries
    }
  }

  private schedule(delay: number): void {
    if (this.stopped) return;
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => {
      this.timer = null;
      void this.poll().finally(() => this.schedule(POLL_MS));
    }, delay);
  }

  private async poll(): Promise<void> {
    if (this.stopped || this.busy) return;
    if (typeof document !== "undefined" && document.visibilityState === "hidden") return;
    this.busy = true;
    try {
      const response = await this.options.api.computer.getClipboard({});
      const generation = response.generation;
      if (this.generation !== null && generation === this.generation) return;
      const first = this.generation === null;
      this.generation = generation;
      const content: ClipContent = {};
      if (response.content?.text) content.text = response.content.text;
      if (response.content?.imagePng?.byteLength) content.png = response.content.imagePng;
      if (first) {
        // What was already there when the viewer opened is not a copy.
        this.lastGuest = content;
        return;
      }
      if (isEmpty(content) || sameContent(content, this.lastGuest)) return;
      this.lastGuest = content;
      await this.writeHost(content);
    } catch (error) {
      this.options.onError?.(`clipboard: ${String((error as Error)?.message ?? error)}`);
    } finally {
      this.busy = false;
    }
  }

  private async writeHost(content: ClipContent): Promise<void> {
    const write = async () => {
      await writeHost(content);
      this.lastPushed = content;
      this.options.onSynced?.("to-host", content);
    };
    try {
      // Some engines neither resolve nor reject a write without a user
      // gesture: give up after a moment and offer the one-click copy.
      await Promise.race([
        write(),
        new Promise<never>((_, reject) => window.setTimeout(() => reject(new Error("clipboard write needs a gesture")), 1500)),
      ]);
    } catch {
      this.options.onNeedsGesture?.(write);
    }
  }
}

/** Reads the host clipboard through the async Clipboard API. */
export async function readHost(): Promise<ClipContent | null> {
  const content: ClipContent = {};
  if (navigator.clipboard.read) {
    const items = await navigator.clipboard.read();
    for (const item of items) {
      if (item.types.includes("image/png") && !content.png) {
        const blob = await item.getType("image/png");
        if (blob.size <= MAX_IMAGE_BYTES) content.png = new Uint8Array(await blob.arrayBuffer());
      }
      if (item.types.includes("text/plain") && content.text === undefined) {
        content.text = await (await item.getType("text/plain")).text();
      }
    }
    return content;
  }
  content.text = await navigator.clipboard.readText();
  return content;
}

/** Writes to the host clipboard (text and, where supported, PNG). */
export async function writeHost(content: ClipContent): Promise<void> {
  if (content.png && typeof ClipboardItem !== "undefined" && navigator.clipboard.write) {
    const parts: Record<string, Blob> = {
      "image/png": new Blob([content.png.slice()], { type: "image/png" }),
    };
    if (content.text) parts["text/plain"] = new Blob([content.text], { type: "text/plain" });
    await navigator.clipboard.write([new ClipboardItem(parts)]);
    return;
  }
  await navigator.clipboard.writeText(content.text ?? "");
}
