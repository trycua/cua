// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The viewer: connects to one sandbox desktop and wires the media session,
 * input, clipboard, audio, file drop, folder sharing and the toolbar.
 */

import { ConnectError, Code } from "@connectrpc/connect";

import { createApi, endpointFromLocation, mediaSocketUrl, type Api } from "./api";
import { probe, type BrowserSupport } from "./capabilities";
import { ClipboardSync } from "./clipboard";
import { MediaSession, type AudioConfigMessage, type MediaTicket } from "./core/mediaSession";
import type { InteractiveInputEvent } from "./core/mediaWire";
import type { MessageInitShape } from "@bufbuild/protobuf";
import { AudioCodec, MediaCodec, SessionPolicy, type OpenMediaRequestSchema, type OpenMediaResponse } from "./gen/cua/env/v1/stream_pb";
import type { FolderShare } from "./folderSync";
import { InputController, chord } from "./input";
import { MicUplink, type UplinkGrant } from "./mic";
import { ViewerPresence } from "./presence";
import { SkillRecorder } from "./recorder";
import { canShareFolders, ensurePermission, forgetShare, savedShares, startShare, type SavedShare } from "./share";
import { joinGuest, safeName, uploadBlob } from "./transfer";
import { icon } from "./icons";
import { OsFamily } from "./gen/cua/env/v1/system_pb";

type ScaleMode = "fit" | "actual";

interface ViewerState {
  scale: ScaleMode;
  muted: boolean;
  mic: boolean;
  clipboard: boolean;
  stats: boolean;
}

const RESIZE_DEBOUNCE_MS = 400;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function errorText(error: unknown): string {
  if (error instanceof ConnectError) {
    if (error.code === Code.Unauthenticated) return "This viewer link has expired or is not valid. Open a new one (for example with `cua sb view`).";
    if (error.code === Code.PermissionDenied) return `Not allowed: ${error.rawMessage}`;
    if (error.code === Code.Unavailable || error.code === Code.Unknown) return `Cannot reach the sandbox (${error.rawMessage}).`;
    return error.rawMessage;
  }
  return String((error as Error)?.message ?? error);
}

export class Viewer {
  readonly api: Api;
  private support!: BrowserSupport;
  private session: MediaSession | null = null;
  private input: InputController | null = null;
  private clipboard: ClipboardSync | null = null;
  private mic: MicUplink | null = null;
  private uplink: UplinkGrant | null = null;
  private recorder: SkillRecorder | null = null;
  private readonly shares = new Map<string, FolderShare>();
  /** Clipboard syncs this page made (the stats hook and tests read it). */
  private readonly clipboardLog: Array<{ direction: string; text?: string; png?: number }> = [];
  private media: OpenMediaResponse | null = null;
  private displayId = "primary";
  private guestOs: OsFamily = OsFamily.UNSPECIFIED;
  private frameSize = { width: 0, height: 0 };
  private resizeTimer: number | null = null;
  private statsTimer: number | null = null;
  private lastCounters = { at: 0, frames: 0, bytes: 0 };
  private readonly state: ViewerState;
  /** Presence cursors over the stream (none when the Space has no presence). */
  private presence: ViewerPresence | null = null;

  // DOM
  readonly root: HTMLElement;
  readonly stage: HTMLElement;
  readonly canvas: HTMLCanvasElement;
  private readonly toolbar: HTMLElement;
  private readonly overlay: HTMLElement;
  private readonly toasts: HTMLElement;
  private readonly statsBox: HTMLElement;
  private readonly chips: HTMLElement;
  private readonly dropZone: HTMLElement;
  private readonly statusDot: HTMLElement;
  private readonly title: HTMLElement;
  private readonly buttons = new Map<string, HTMLButtonElement>();

  constructor(root: HTMLElement) {
    const endpoint = endpointFromLocation(window.location, safeSession(), (url) => history.replaceState(null, "", url));
    this.api = createApi(endpoint);
    const params = endpoint.params;
    this.state = {
      scale: params.get("scale") === "actual" ? "actual" : "fit",
      muted: params.get("audio") === "off",
      mic: false,
      clipboard: params.get("clipboard") !== "off",
      stats: params.get("stats") === "1",
    };
    this.displayId = params.get("display") ?? "primary";

    this.root = root;
    root.classList.add("cua-viewer");
    this.stage = el("div", "cua-stage");
    this.canvas = el("canvas", "cua-screen");
    this.canvas.width = 1280;
    this.canvas.height = 800;
    this.stage.appendChild(this.canvas);
    this.toolbar = el("div", "cua-toolbar cua-chrome");
    this.overlay = el("div", "cua-overlay cua-chrome");
    this.toasts = el("div", "cua-toasts cua-chrome");
    this.statsBox = el("pre", "cua-stats cua-chrome");
    this.chips = el("div", "cua-chips cua-chrome");
    this.dropZone = el("div", "cua-drop cua-chrome");
    this.statusDot = el("span", "cua-dot");
    this.title = el("span", "cua-title", "Sandbox");
    root.append(this.stage, this.toolbar, this.chips, this.overlay, this.toasts, this.statsBox, this.dropZone);
    this.buildToolbar();
    this.applyScale();
    window.addEventListener("resize", () => this.onResize());
    document.addEventListener("fullscreenchange", () => this.onFullscreen());
  }

  // ---------------------------------------------------------------- start

  async start(): Promise<void> {
    this.status("connecting", "Connecting to the sandbox…");
    this.support = await probe();
    if (!this.support.secureContext) {
      this.toast("This page is not a secure context (https or localhost), so the browser disables fast video, audio, clipboard and folder sharing. The viewer falls back to PNG frames.", "warn");
    }
    try {
      const caps = await this.api.system.getCapabilities({});
      this.guestOs = caps.os?.family ?? OsFamily.UNSPECIFIED;
      this.title.textContent = caps.hostname || "Sandbox";
      document.title = `${caps.hostname || "Sandbox"} · Cua viewer`;
      const targets = await this.api.stream.listTargets({ includeWindows: false });
      const displays = targets.targets.filter((t) => t.target.case === "display");
      if (displays.length === 0) throw new Error("the sandbox has no display to view (is it headless?)");
      const wanted = displays.find((t) => t.target.case === "display" && t.target.value.id === this.displayId) ?? displays[0]!;
      if (wanted.target.case === "display") this.displayId = wanted.target.value.id || "primary";
      if (!wanted.available) throw new Error(wanted.limitation || "the display cannot be streamed right now");
      const uplinkWanted = targets.audioUplinkAllowed && this.support.microphone && this.support.audioWorklet;
      const first = await this.openMedia(uplinkWanted);
      this.attach(first);
    } catch (error) {
      this.status("failed", errorText(error));
      return;
    }
    this.startClipboard();
    this.wireDrop();
    void this.offerResume();
    this.startRecorderIfAsked();
    this.exposeTestHooks();
  }

  private request(uplink: boolean): MessageInitShape<typeof OpenMediaRequestSchema> {
    const params = this.api.endpoint.params;
    const forcePng = params.get("codec") === "png";
    const codecs = !forcePng && this.support.h264 ? [MediaCodec.H264, MediaCodec.PNG] : [MediaCodec.PNG];
    const audioOk = this.support.audioWorklet && params.get("audio") !== "none";
    return {
      target: { target: { case: "displayId", value: this.displayId } },
      codecs,
      maxFps: Number(params.get("fps") ?? 60) || 60,
      maxDimension: this.wantedDimension(),
      policy: params.get("view_only") === "1" ? SessionPolicy.VIEW_ONLY : SessionPolicy.ALLOW_ACTIVATION,
      ticketTtl: { seconds: 600n, nanos: 0 },
      audio: audioOk
        ? {
            enabled: true,
            sourceIds: [],
            encoding: { codecs: this.support.opusDecode ? [AudioCodec.OPUS] : [AudioCodec.PCM_S16LE], frameMs: 20 },
            uplink: uplink
              ? {
                  enabled: true,
                  encoding: { codecs: this.support.opusEncode ? [AudioCodec.OPUS] : [AudioCodec.PCM_S16LE], channels: 1, frameMs: 20 },
                  virtualSourceName: "",
                  setDefaultInput: true,
                }
              : undefined,
          }
        : undefined,
    };
  }

  private async openMedia(uplink: boolean): Promise<MediaTicket> {
    const response = await this.api.stream.openMedia(this.request(uplink));
    this.media = response;
    const grant = response.audio?.uplink;
    if (grant?.granted && grant.encoding) {
      this.uplink = {
        trackId: grant.trackId,
        codec: grant.encoding.codec === AudioCodec.OPUS ? "opus" : "pcm",
        sampleRate: grant.encoding.sampleRateHz || 48000,
        channels: grant.encoding.channels || 1,
        frameMs: grant.encoding.frameMs || 20,
        bitrateKbps: grant.encoding.bitrateKbps || 32,
        configEpoch: this.uplink?.configEpoch ?? 0,
      };
    } else {
      this.uplink = null;
      if (grant && !grant.granted && grant.deniedReason) this.buttons.get("mic")?.setAttribute("title", `Microphone unavailable: ${grant.deniedReason}`);
    }
    this.updateButtons();
    const expires = response.ticketExpiresAt;
    return {
      wsUrl: mediaSocketUrl(this.api.endpoint.baseUrl, response.wsPath),
      ticketExpiresAt: expires ? new Date(Number(expires.seconds) * 1000).toISOString() : undefined,
      mediaSessionId: response.mediaSessionId,
    };
  }

  private attach(ticket: MediaTicket): void {
    const interactive = this.media?.policy !== SessionPolicy.VIEW_ONLY;
    const session = new MediaSession({
      canvas: this.canvas,
      ticket,
      reopen: () => this.openMedia(Boolean(this.uplink)),
      interactive,
      wireInput: false,
      audio: this.support.audioWorklet,
      onStatus: (status, detail) => {
        if (status === "streaming") this.status("streaming");
        else if (status === "reconnecting") this.status("reconnecting", "Reconnecting…");
        else if (status === "ended") this.status("failed", detail ? `The stream ended: ${detail}` : "The stream ended.");
        else if (status === "failed") this.status("failed", detail ?? "The stream failed.");
      },
      onGeometry: (width, height) => {
        this.frameSize = { width, height };
        this.applyScale();
      },
      onFrame: (width, height) => {
        if (width !== this.frameSize.width || height !== this.frameSize.height) {
          this.frameSize = { width, height };
          this.applyScale();
        }
      },
      onAudioConfig: (config) => this.onAudioConfig(config),
      onServerError: (payload) => {
        if (payload.code === "audio_uplink_denied") this.toast("The sandbox refused the microphone.", "warn");
      },
    });
    session.setAudioMuted(this.state.muted);
    this.session = session;
    session.start();
    if (!this.presence) {
      const presence = new ViewerPresence(this.api, this.canvas, this.stage, interactive, this.displayId, Date.now, () =>
        this.statusDot.dataset.state === "streaming",
      );
      this.presence = presence;
      void presence.start();
      window.addEventListener("pagehide", () => presence.stop());
    }
    if (interactive) {
      this.input = new InputController({
        surface: this.stage,
        screen: () => this.canvas,
        emit: (event) => session.queueInput(event),
        keyOptions: () => ({ metaAsControl: this.metaAsControl() }),
        onPaste: (event) => (this.state.clipboard ? this.clipboard?.fromPasteEvent(event) : undefined),
        onCopyShortcut: () => this.clipboard?.soon(),
        onAction: (event) => this.recorder?.add(event),
      });
      this.stage.addEventListener("pointerdown", () => this.input?.focus());
      this.input.focus();
    }
  }

  private metaAsControl(): boolean {
    const mac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
    return mac && this.guestOs !== OsFamily.MACOS && this.api.endpoint.params.get("meta") !== "meta";
  }

  // ---------------------------------------------------------------- audio

  private onAudioConfig(config: AudioConfigMessage): void {
    if (config.direction !== "up" || !this.uplink) return;
    this.uplink = {
      ...this.uplink,
      codec: config.codec === "opus" ? "opus" : "pcm",
      sampleRate: config.sample_rate_hz || this.uplink.sampleRate,
      channels: config.channels || this.uplink.channels,
      frameMs: config.frame_ms || this.uplink.frameMs,
      bitrateKbps: config.bitrate_kbps || this.uplink.bitrateKbps,
      configEpoch: config.config_epoch & 0xff,
      trackId: config.track_id,
    };
    this.mic?.reconfigure(this.uplink);
  }

  private async toggleMic(): Promise<void> {
    if (this.mic) {
      this.mic.stop();
      this.mic = null;
      this.state.mic = false;
      this.updateButtons();
      return;
    }
    if (!this.uplink || !this.session) {
      this.toast("The microphone is not available for this sandbox.", "warn");
      return;
    }
    if (!MicUplink.supported(this.uplink.codec)) {
      this.toast("This browser cannot send the microphone in the format the sandbox asked for.", "warn");
      return;
    }
    const session = this.session;
    const mic = new MicUplink(this.uplink, (packet) => session.sendBinary(packet), (type, payload) => session.sendControl(type, payload));
    try {
      await mic.start();
      this.mic = mic;
      this.state.mic = true;
    } catch (error) {
      this.toast(`Microphone: ${errorText(error)}`, "warn");
    }
    this.updateButtons();
  }

  // ---------------------------------------------------------------- clipboard

  private startClipboard(): void {
    if (!this.state.clipboard || this.media?.policy === SessionPolicy.VIEW_ONLY) return;
    const sync = new ClipboardSync({
      api: this.api,
      onSynced: (direction, content) => {
        this.clipboardLog.push({ direction, text: content.text, png: content.png?.byteLength });
        if (this.clipboardLog.length > 50) this.clipboardLog.shift();
      },
      onNeedsGesture: (write) => {
        this.chip("clipboard", "Sandbox clipboard changed", "Copy to this computer", async () => {
          await write();
          this.removeChip("clipboard");
        });
      },
      onError: (message) => {
        if (/not allowed|permission/i.test(message)) {
          sync.stop();
          this.clipboard = null;
          this.state.clipboard = false;
          this.updateButtons();
        }
      },
    });
    this.clipboard = sync;
    sync.start();
  }

  // ---------------------------------------------------------------- files

  private filesRoot(): string | null {
    return this.api.endpoint.params.get("files");
  }

  private wireDrop(): void {
    const root = this.filesRoot();
    if (!root) return;
    let depth = 0;
    const show = (on: boolean) => this.dropZone.classList.toggle("on", on);
    this.dropZone.textContent = `Drop files to upload them to ${pretty(joinGuest(root, "Downloads"), root)}`;
    this.root.addEventListener("dragenter", (event) => {
      if (!event.dataTransfer?.types.includes("Files")) return;
      event.preventDefault();
      depth += 1;
      show(true);
    });
    this.root.addEventListener("dragleave", () => {
      depth = Math.max(0, depth - 1);
      if (depth === 0) show(false);
    });
    this.root.addEventListener("dragover", (event) => event.preventDefault());
    this.root.addEventListener("drop", (event) => {
      event.preventDefault();
      depth = 0;
      show(false);
      const items = Array.from(event.dataTransfer?.items ?? []);
      const all = items.map((item) => item.webkitGetAsEntry?.()).filter((e): e is FileSystemEntry => Boolean(e));
      // App bundles are teleported (the Cua Spaces app's picker), never
      // uploaded as a pile of files: the same rule as its hasAppPath.
      const entries = all.filter((e) => !isAppBundle(e.name));
      const plain = Array.from(event.dataTransfer?.files ?? [])
        .filter((file) => !isAppBundle(file.name))
        .map((file) => ({ file, rel: safeName(file.name) }));
      if (entries.length < all.length) {
        this.toast("Apps are not uploaded. To bring an app into the sandbox, drop it on the Space in the Cua Spaces app (Teleport).", "warn");
      }
      // Entries keep folder structure; when an engine cannot read them
      // (script-made drags), the plain file list still uploads.
      void this.uploadEntries(entries).then((n) => (n === 0 && plain.length ? this.uploadFiles(plain) : undefined));
    });
  }

  private async uploadEntries(entries: FileSystemEntry[]): Promise<number> {
    const files: Array<{ file: File; rel: string }> = [];
    const walk = async (entry: FileSystemEntry, prefix: string): Promise<void> => {
      if (files.length > 10_000) return;
      if (entry.isFile) {
        const file = await new Promise<File>((resolve, reject) => (entry as FileSystemFileEntry).file(resolve, reject));
        files.push({ file, rel: prefix + safeName(entry.name) });
      } else if (entry.isDirectory) {
        const reader = (entry as FileSystemDirectoryEntry).createReader();
        for (;;) {
          const batch = await new Promise<FileSystemEntry[]>((resolve, reject) => reader.readEntries(resolve, reject));
          if (batch.length === 0) break;
          for (const child of batch) await walk(child, `${prefix}${safeName(entry.name)}/`);
        }
      }
    };
    for (const entry of entries) {
      try {
        await walk(entry, "");
      } catch {
        // unreadable entry: the caller falls back to the plain file list
      }
    }
    if (files.length) await this.uploadFiles(files);
    return files.length;
  }

  async uploadFiles(files: Array<{ file: File; rel: string }>): Promise<string[]> {
    const root = this.filesRoot();
    if (!root || files.length === 0) return [];
    const dest = joinGuest(root, "Downloads");
    const total = files.reduce((n, f) => n + f.file.size, 0);
    let done = 0;
    const uploaded: string[] = [];
    const chip = this.chip("upload", `Uploading ${files.length} file${files.length === 1 ? "" : "s"}…`);
    for (const { file, rel } of files) {
      const path = joinGuest(dest, rel);
      try {
        const base = done;
        await uploadBlob(this.api, path, file, (p) => {
          chip.querySelector(".cua-chip-text")!.textContent = `Uploading ${rel}: ${Math.round(((base + p.done) / Math.max(1, total)) * 100)}%`;
        });
        uploaded.push(path);
      } catch (error) {
        this.toast(`Upload of ${rel} failed: ${errorText(error)}`, "warn");
      }
      done += file.size;
    }
    this.removeChip("upload");
    if (uploaded.length) this.toast(`Uploaded ${uploaded.length} file${uploaded.length === 1 ? "" : "s"} to ${pretty(dest, root)}`);
    return uploaded;
  }

  private pickUpload(): void {
    const input = el("input");
    input.type = "file";
    input.multiple = true;
    input.onchange = () => void this.uploadFiles(Array.from(input.files ?? []).map((file) => ({ file, rel: safeName(file.name) })));
    input.click();
  }

  // ---------------------------------------------------------------- folder sharing

  private async offerResume(): Promise<void> {
    if (!this.filesRoot() || !canShareFolders()) return;
    for (const saved of await savedShares(this.api.endpoint.baseUrl)) {
      this.chip(`resume:${saved.key}`, `Shared folder “${saved.name}” is paused`, "Resume", async () => {
        this.removeChip(`resume:${saved.key}`);
        if (!(await ensurePermission(saved.handle))) {
          this.toast("The browser did not allow writing to that folder.", "warn");
          return;
        }
        await this.share(saved.handle, saved.guestPath, saved);
      }, async () => {
        await forgetShare(saved.key);
        this.removeChip(`resume:${saved.key}`);
      });
    }
  }

  private confirmShare(): void {
    const root = this.filesRoot();
    if (!root) return;
    const dialog = this.dialog(
      "Share a folder with this sandbox",
      [
        "Pick a folder on this computer. Its contents are copied into the sandbox and kept in sync both ways:",
        "• changes made in the sandbox are written back to your folder, including new, edited and deleted files;",
        "• if a file changes on both sides at once, both versions are kept (the sandbox copy is saved next to yours);",
        "• only the folder you pick is shared, and sharing stops when you close this page or click Stop.",
        `In the sandbox it appears under ${pretty(joinGuest(root, "Shared"), root)}/<folder name>. Your browser will ask to let this page edit the folder.`,
      ],
      [
        ["Cancel", () => dialog.remove()],
        [
          "Choose folder…",
          async () => {
            dialog.remove();
            try {
              const picker = (globalThis as unknown as { showDirectoryPicker: (o: object) => Promise<FileSystemDirectoryHandle> }).showDirectoryPicker;
              const handle = await picker({ mode: "readwrite", id: "cua-share" });
              if (!(await ensurePermission(handle))) {
                this.toast("The browser did not allow writing to that folder.", "warn");
                return;
              }
              await this.share(handle, joinGuest(root, "Shared", safeName(handle.name)), null);
            } catch (error) {
              if ((error as DOMException)?.name !== "AbortError") this.toast(`Folder sharing: ${errorText(error)}`, "warn");
            }
          },
        ],
      ],
    );
  }

  /** Starts sharing `handle` as the guest directory `guestPath`. */
  async share(handle: FileSystemDirectoryHandle, guestPath: string, saved: SavedShare | null): Promise<FolderShare> {
    const root = this.filesRoot() ?? "";
    const key = guestPath;
    this.shares.get(key)?.stop();
    const label = `${handle.name} ↔ ${pretty(guestPath, root)}`;
    this.chip(`share:${key}`, `Syncing ${label}`, "Stop", async () => {
      this.shares.get(key)?.stop();
      this.shares.delete(key);
      this.removeChip(`share:${key}`);
      this.updateButtons();
    });
    const share = await startShare(this.api, handle, guestPath, saved, {
      onState: (state) => {
        const chip = this.chips.querySelector(`[data-chip="share:${CSS.escape(key)}"] .cua-chip-text`);
        if (!chip) return;
        const conflicts = state.conflicts ? ` · ${state.conflicts} conflict${state.conflicts === 1 ? "" : "s"} kept` : "";
        chip.textContent =
          state.status === "syncing"
            ? `Syncing ${label}…`
            : state.status === "error"
              ? `${label}: ${state.detail ?? "error"}`
              : `${label} · in sync${conflicts}`;
      },
      onError: (message) => this.toast(`Folder sync: ${message}`, "warn"),
    });
    this.shares.set(key, share);
    this.updateButtons();
    return share;
  }

  // ---------------------------------------------------------------- recording

  private startRecorderIfAsked(): void {
    const params = this.api.endpoint.params;
    const url = params.get("record_url");
    if (params.get("autorecord") !== "true" || !url) return;
    if (!this.support.mediaRecorder) {
      this.toast("This browser cannot record the screen (no MediaRecorder).", "warn");
      return;
    }
    this.recorder = new SkillRecorder(this.canvas, url, params.get("record_format") !== "webm");
    this.chip("record", "Recording a demonstration", "Stop recording", async () => {
      const recorder = this.recorder;
      this.recorder = null;
      this.removeChip("record");
      if (!recorder) return;
      const chip = this.chip("record-send", "Sending the recording…");
      try {
        const bytes = await recorder.finish();
        chip.remove();
        this.toast(`Recording sent (${Math.round(bytes / 1024)} KiB). You can close this tab.`);
      } catch (error) {
        chip.remove();
        this.toast(errorText(error), "warn");
      }
    });
  }

  // ---------------------------------------------------------------- scaling

  private wantedDimension(): number {
    if (this.state.scale === "actual") return 0;
    const dpr = window.devicePixelRatio || 1;
    const long = Math.max(this.stage.clientWidth || window.innerWidth, this.stage.clientHeight || window.innerHeight);
    return Math.max(640, Math.round(long * dpr));
  }

  private applyScale(): void {
    const { width, height } = this.frameSize.width ? this.frameSize : { width: this.canvas.width, height: this.canvas.height };
    this.root.dataset.scale = this.state.scale;
    if (this.state.scale === "actual") {
      const dpr = window.devicePixelRatio || 1;
      this.canvas.style.width = `${width / dpr}px`;
      this.canvas.style.height = `${height / dpr}px`;
      return;
    }
    const sw = this.stage.clientWidth || window.innerWidth;
    const sh = this.stage.clientHeight || window.innerHeight;
    const k = Math.min(sw / width, sh / height);
    this.canvas.style.width = `${Math.floor(width * k)}px`;
    this.canvas.style.height = `${Math.floor(height * k)}px`;
  }

  private onResize(): void {
    this.applyScale();
    if (this.resizeTimer !== null) window.clearTimeout(this.resizeTimer);
    this.resizeTimer = window.setTimeout(() => {
      this.resizeTimer = null;
      void this.pushDimension();
    }, RESIZE_DEBOUNCE_MS);
  }

  private async pushDimension(): Promise<void> {
    const id = this.session?.id;
    if (!id) return;
    try {
      await this.api.stream.setPreferences({ mediaSessionId: id, maxDimension: this.wantedDimension() });
    } catch {
      // older drivers: frames keep their size and the browser scales
    }
  }

  private setScale(mode: ScaleMode): void {
    this.state.scale = mode;
    this.applyScale();
    void this.pushDimension();
    this.updateButtons();
  }

  private async toggleFullscreen(): Promise<void> {
    if (document.fullscreenElement) {
      await document.exitFullscreen().catch(() => {});
      return;
    }
    await this.root.requestFullscreen?.().catch(() => {});
  }

  private onFullscreen(): void {
    const keyboard = (navigator as unknown as { keyboard?: { lock?: () => Promise<void>; unlock?: () => void } }).keyboard;
    if (document.fullscreenElement) void keyboard?.lock?.().catch(() => {});
    else keyboard?.unlock?.();
    this.updateButtons();
    this.onResize();
  }

  // ---------------------------------------------------------------- toolbar

  private button(id: string, label: string, svg: string, onClick: () => void): HTMLButtonElement {
    const b = el("button", "cua-btn");
    b.type = "button";
    b.title = label;
    b.setAttribute("aria-label", label);
    b.dataset.id = id;
    b.innerHTML = svg;
    b.addEventListener("click", (event) => {
      event.stopPropagation();
      onClick();
      if (id !== "keys" && id !== "more") this.input?.focus();
    });
    this.buttons.set(id, b);
    return b;
  }

  private buildToolbar(): void {
    const handle = el("div", "cua-handle");
    handle.append(this.statusDot, this.title);
    const menu = el("div", "cua-menu");
    const keys = el("div", "cua-popover");
    const combos: Array<[string, string[]]> = [
      ["Ctrl+Alt+Del", ["control", "alt", "delete"]],
      ["Super", ["meta"]],
      ["Alt+Tab", ["alt", "tab"]],
      ["Alt+F4", ["alt", "f4"]],
      ["Ctrl+Alt+T", ["control", "alt", "t"]],
      ["Esc", ["escape"]],
      ["Print Screen", ["printscreen"]],
    ];
    for (const [label, sequence] of combos) {
      const item = el("button", "cua-item", label);
      item.type = "button";
      item.addEventListener("click", () => {
        for (const event of chord(sequence)) this.sendInput(event);
        keys.classList.remove("open");
        this.input?.focus();
      });
      keys.appendChild(item);
    }
    const keysWrap = el("div", "cua-pop");
    keysWrap.append(
      this.button("keys", "Send keys", icon.keyboard, () => keys.classList.toggle("open")),
      keys,
    );
    menu.append(
      this.button("scale", "Scale: fit to window", icon.fit, () => this.setScale(this.state.scale === "fit" ? "actual" : "fit")),
      this.button("fullscreen", "Full screen", icon.fullscreen, () => void this.toggleFullscreen()),
      keysWrap,
      this.button("clipboard", "Clipboard sync", icon.clipboard, () => this.toggleClipboard()),
      this.button("upload", "Upload files", icon.upload, () => this.pickUpload()),
      this.button("share", "Share a folder", icon.folder, () => this.confirmShare()),
      this.button("audio", "Sound", icon.speaker, () => {
        this.state.muted = !this.state.muted;
        this.session?.setAudioMuted(this.state.muted);
        this.updateButtons();
      }),
      this.button("mic", "Microphone", icon.mic, () => void this.toggleMic()),
      this.button("stats", "Stats", icon.stats, () => {
        this.state.stats = !this.state.stats;
        this.updateButtons();
      }),
    );
    this.toolbar.append(handle, menu);
    this.updateButtons();
  }

  private sendInput(event: InteractiveInputEvent): void {
    this.session?.queueInput(event);
    this.recorder?.add(event);
  }

  private toggleClipboard(): void {
    this.state.clipboard = !this.state.clipboard;
    if (this.state.clipboard) this.startClipboard();
    else {
      this.clipboard?.stop();
      this.clipboard = null;
    }
    this.updateButtons();
  }

  private updateButtons(): void {
    const set = (id: string, on: boolean | null, label?: string, svg?: string) => {
      const b = this.buttons.get(id);
      if (!b) return;
      if (on === null) b.hidden = true;
      else {
        b.hidden = false;
        b.classList.toggle("on", on);
      }
      if (label) {
        b.title = label;
        b.setAttribute("aria-label", label);
      }
      if (svg) b.innerHTML = svg;
    };
    const files = Boolean(this.filesRoot());
    const viewOnly = this.media?.policy === SessionPolicy.VIEW_ONLY;
    set("scale", this.state.scale === "actual", this.state.scale === "fit" ? "Scale: fit to window (click for 1:1)" : "Scale: 1:1 (click to fit)", this.state.scale === "fit" ? icon.fit : icon.actual);
    set("fullscreen", Boolean(document.fullscreenElement), document.fullscreenElement ? "Exit full screen" : "Full screen");
    set("keys", viewOnly ? null : false);
    set("clipboard", viewOnly || this.api.endpoint.params.get("clipboard") === "0" ? null : this.state.clipboard, this.state.clipboard ? "Clipboard sync on" : "Clipboard sync off");
    set("upload", files && !viewOnly ? false : null);
    set("share", files && !viewOnly && canShareFolders() ? this.shares.size > 0 : null, this.shares.size ? "Share another folder" : "Share a folder");
    set("audio", !this.state.muted, this.state.muted ? "Sound off" : "Sound on", this.state.muted ? icon.speakerOff : icon.speaker);
    set("mic", this.uplink ? this.state.mic : null, this.state.mic ? "Microphone on" : "Microphone off");
    set("stats", this.state.stats);
    this.statsBox.hidden = !this.state.stats;
    if (this.state.stats && this.statsTimer === null) this.statsTimer = window.setInterval(() => this.renderStats(), 1000);
    if (!this.state.stats && this.statsTimer !== null) {
      window.clearInterval(this.statsTimer);
      this.statsTimer = null;
    }
  }

  private renderStats(): void {
    const s = this.session?.stats;
    if (!s) return;
    const now = performance.now();
    const dt = (now - (this.lastCounters.at || now)) / 1000 || 1;
    const fps = (s.framesDecoded - this.lastCounters.frames) / dt;
    const kbps = ((s.bytesReceived - this.lastCounters.bytes) * 8) / 1000 / dt;
    this.lastCounters = { at: now, frames: s.framesDecoded, bytes: s.bytesReceived };
    const audio = this.session?.audioStats() ?? [];
    this.statsBox.textContent = [
      `${this.frameSize.width}×${this.frameSize.height} ${s.codec || "?"} · ${fps.toFixed(0)} fps · ${kbps.toFixed(0)} kbit/s`,
      `decode ${s.decodeMs.toFixed(1)} ms · frames ${s.framesDecoded} · keyframes ${s.keyframes}`,
      ...audio.map((a) => `audio #${a.trackId}: ${a.packets} pkts, lost ${a.lost}, jitter ${a.jitterMs.toFixed(1)} ms, buffer ${a.bufferTargetMs.toFixed(0)} ms`),
      this.mic ? `mic: ${this.mic.packetsSent} pkts sent` : "",
    ]
      .filter(Boolean)
      .join("\n");
  }

  // ---------------------------------------------------------------- chrome

  private status(kind: "connecting" | "streaming" | "reconnecting" | "failed", message?: string): void {
    this.statusDot.dataset.state = kind;
    this.root.dataset.status = kind;
    this.overlay.replaceChildren();
    this.overlay.hidden = kind === "streaming";
    if (kind === "streaming") return;
    const box = el("div", "cua-overlay-box");
    if (kind !== "failed") box.appendChild(el("div", "cua-spinner"));
    box.appendChild(el("p", undefined, message ?? ""));
    if (kind === "failed") {
      const retry = el("button", "cua-primary", "Reconnect");
      retry.type = "button";
      retry.onclick = () => window.location.reload();
      box.appendChild(retry);
    }
    this.overlay.appendChild(box);
  }

  toast(message: string, kind: "info" | "warn" = "info"): void {
    const t = el("div", `cua-toast ${kind}`, message);
    this.toasts.appendChild(t);
    window.setTimeout(() => t.remove(), kind === "warn" ? 9000 : 5000);
  }

  private chip(id: string, text: string, action?: string, onAction?: () => void | Promise<void>, onDismiss?: () => void | Promise<void>): HTMLElement {
    this.removeChip(id);
    const chip = el("div", "cua-chip");
    chip.dataset.chip = id;
    chip.appendChild(el("span", "cua-chip-text", text));
    if (action && onAction) {
      const b = el("button", "cua-chip-btn", action);
      b.type = "button";
      b.onclick = () => void onAction();
      chip.appendChild(b);
    }
    if (onDismiss) {
      const x = el("button", "cua-chip-x", "×");
      x.type = "button";
      x.title = "Forget";
      x.onclick = () => void onDismiss();
      chip.appendChild(x);
    }
    this.chips.appendChild(chip);
    return chip;
  }

  private removeChip(id: string): void {
    this.chips.querySelector(`[data-chip="${CSS.escape(id)}"]`)?.remove();
  }

  private dialog(title: string, lines: string[], actions: Array<[string, () => void | Promise<void>]>): HTMLElement {
    const backdrop = el("div", "cua-dialog-backdrop cua-chrome");
    const dialog = el("div", "cua-dialog");
    dialog.setAttribute("role", "dialog");
    dialog.setAttribute("aria-modal", "true");
    dialog.appendChild(el("h2", undefined, title));
    for (const line of lines) dialog.appendChild(el("p", undefined, line));
    const row = el("div", "cua-dialog-actions");
    actions.forEach(([label, run], i) => {
      const b = el("button", i === actions.length - 1 ? "cua-primary" : "cua-secondary", label);
      b.type = "button";
      b.onclick = () => void run();
      row.appendChild(b);
    });
    dialog.appendChild(row);
    backdrop.appendChild(dialog);
    this.root.appendChild(backdrop);
    return backdrop;
  }

  // ---------------------------------------------------------------- tests

  /** Hooks the browser e2e drives (the page's own origin only). */
  private exposeTestHooks(): void {
    (window as unknown as { __cuaViewer: unknown }).__cuaViewer = {
      viewer: this,
      support: () => this.support,
      stats: () => ({
        ...this.session?.stats,
        audio: this.session?.audioStats() ?? [],
        mic: this.mic?.packetsSent ?? 0,
        micDebug: this.mic?.debug ?? null,
        frame: this.frameSize,
      }),
      input: (event: InteractiveInputEvent) => this.sendInput(event),
      share: (handle: FileSystemDirectoryHandle, guestPath: string) => this.share(handle, guestPath, null),
      shares: () => [...this.shares.values()].map((s) => s.status),
      syncNow: async () => {
        for (const share of this.shares.values()) await share.syncOnce();
      },
      clipboard: () => this.clipboard,
      clipboardLog: () => [...this.clipboardLog],
      toggleMic: async () => {
        await this.toggleMic();
        return { on: Boolean(this.mic), toasts: [...this.toasts.querySelectorAll(".cua-toast")].map((t) => t.textContent) };
      },
      upload: (files: File[]) => this.uploadFiles(files.map((file) => ({ file, rel: safeName(file.name) }))),
    };
  }
}

/** An application bundle or launcher (`.app`, `.desktop`, `.lnk`). */
export function isAppBundle(name: string): boolean {
  return /\.(app|desktop|lnk)\/?$/i.test(name);
}

/** `~`-relative display of a guest path under the files root. */
function pretty(path: string, root: string): string {
  if (root && (path === root || path.startsWith(`${root}/`))) {
    const home = /^\/(home\/[^/]+|root|Users\/[^/]+)$/.test(root);
    if (home) return `~${path.slice(root.length)}`;
  }
  return path;
}

function safeSession(): Storage | null {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}
