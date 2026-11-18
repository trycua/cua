// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Two-way folder sharing between a local folder (File System Access API:
 * `showDirectoryPicker` handle) and a guest directory (`FilesystemService`).
 *
 * The engine keeps a *base*: what each file looked like on both sides after
 * the last time it was in sync. Each pass scans both sides and plans with
 * the classic three-way rule:
 *
 * - changed on one side only: copy it to the other side;
 * - deleted on one side, unchanged on the other: delete it there too;
 * - deleted on one side, changed on the other: the change wins (restored);
 * - changed on both sides: a conflict. Both versions are kept: the local
 *   one stays at the path on both sides and the guest's version is saved
 *   next to it as `name (sandbox copy YYYY-MM-DD HHMMSS).ext`;
 * - new on both sides with the same size: compared by content; equal files
 *   are adopted, different ones are a conflict.
 *
 * Passes run on change notifications (a guest watcher, and the local
 * `FileSystemObserver` where the browser has it) and on a timer otherwise.
 * A side that suddenly looks empty while the base is not (an unmounted
 * disk, a failed scan) never propagates deletions.
 */

import type { Api } from "./api";
import { FileType } from "./gen/cua/env/v1/filesystem_pb";
import { joinGuest, readInto, uploadBlob } from "./transfer";

// ---------------------------------------------------------------- planner

export interface FileMeta {
  size: number;
  /** Modification time in whole milliseconds. */
  mtime: number;
}

export interface Snapshot {
  /** Relative path (`/`-separated) to metadata. */
  files: Map<string, FileMeta>;
  /** Relative directory paths. */
  dirs: Set<string>;
}

export interface BaseEntry {
  local: FileMeta;
  remote: FileMeta;
}

export interface Base {
  files: Map<string, BaseEntry>;
  dirs: Set<string>;
}

export type Action =
  | { kind: "push"; path: string }
  | { kind: "pull"; path: string }
  | { kind: "delete-remote"; path: string }
  | { kind: "delete-local"; path: string }
  | { kind: "conflict"; path: string }
  | { kind: "compare"; path: string }
  | { kind: "forget"; path: string }
  | { kind: "mkdir-remote"; path: string }
  | { kind: "mkdir-local"; path: string }
  | { kind: "rmdir-remote"; path: string }
  | { kind: "rmdir-local"; path: string };

export function emptyBase(): Base {
  return { files: new Map(), dirs: new Set() };
}

function changed(now: FileMeta, then: FileMeta | undefined): boolean {
  return !then || now.size !== then.size || now.mtime !== then.mtime;
}

/** Plans one pass. Pure: the tests drive it with plain maps. */
export function plan(local: Snapshot, remote: Snapshot, base: Base): Action[] {
  const actions: Action[] = [];
  const paths = new Set([...local.files.keys(), ...remote.files.keys(), ...base.files.keys()]);
  // A side that looks wiped while we have history is not trusted to delete.
  const localWiped = local.files.size === 0 && local.dirs.size === 0 && base.files.size > 0;
  const remoteWiped = remote.files.size === 0 && remote.dirs.size === 0 && base.files.size > 0;
  for (const path of [...paths].sort()) {
    const l = local.files.get(path);
    const r = remote.files.get(path);
    const b = base.files.get(path);
    const lc = l !== undefined && changed(l, b?.local);
    const rc = r !== undefined && changed(r, b?.remote);
    if (l && r) {
      if (b) {
        if (lc && rc) actions.push({ kind: "conflict", path });
        else if (lc) actions.push({ kind: "push", path });
        else if (rc) actions.push({ kind: "pull", path });
      } else if (l.size === r.size) {
        actions.push({ kind: "compare", path });
      } else {
        actions.push({ kind: "conflict", path });
      }
    } else if (l && !r) {
      if (!b || lc) actions.push({ kind: "push", path });
      else if (!remoteWiped) actions.push({ kind: "delete-local", path });
    } else if (!l && r) {
      if (!b || rc) actions.push({ kind: "pull", path });
      else if (!localWiped) actions.push({ kind: "delete-remote", path });
    } else if (b) {
      actions.push({ kind: "forget", path });
    }
  }
  // Directories (files create their parents; this covers empty ones).
  const dirs = new Set([...local.dirs, ...remote.dirs, ...base.dirs]);
  const deepestFirst = [...dirs].sort((a, b) => b.split("/").length - a.split("/").length || b.localeCompare(a));
  for (const dir of deepestFirst) {
    const inLocal = local.dirs.has(dir);
    const inRemote = remote.dirs.has(dir);
    const known = base.dirs.has(dir);
    if (inLocal && !inRemote) {
      if (known && !remoteWiped) actions.push({ kind: "rmdir-local", path: dir });
      else actions.push({ kind: "mkdir-remote", path: dir });
    } else if (!inLocal && inRemote) {
      if (known && !localWiped) actions.push({ kind: "rmdir-remote", path: dir });
      else actions.push({ kind: "mkdir-local", path: dir });
    }
  }
  return actions;
}

/** `name (sandbox copy 2026-09-24 101500).ext` */
export function conflictName(path: string, at: Date): string {
  const slash = path.lastIndexOf("/");
  const dir = slash >= 0 ? path.slice(0, slash + 1) : "";
  const name = path.slice(slash + 1);
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : "";
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}${pad(at.getMinutes())}${pad(at.getSeconds())}`;
  return `${dir}${stem} (sandbox copy ${stamp})${ext}`;
}

/** Files never synced (OS litter). */
export function ignored(name: string): boolean {
  return (
    name === ".DS_Store" ||
    name === "Thumbs.db" ||
    name === "desktop.ini" ||
    name.startsWith("._") ||
    name.endsWith(".crswap") ||
    name.startsWith(".~lock.")
  );
}

// ---------------------------------------------------------------- sides

/** Most entries a share may hold (per side). */
export const MAX_ENTRIES = 20_000;

/** The local side: a File System Access directory handle. */
export class LocalSide {
  constructor(readonly root: FileSystemDirectoryHandle) {}

  async scan(): Promise<Snapshot> {
    const files = new Map<string, FileMeta>();
    const dirs = new Set<string>();
    let count = 0;
    const walk = async (dir: FileSystemDirectoryHandle, prefix: string): Promise<void> => {
      for await (const [name, handle] of (dir as unknown as AsyncIterable<[string, FileSystemHandle]>)) {
        if (ignored(name)) continue;
        if (++count > MAX_ENTRIES) throw new Error(`the shared folder has more than ${MAX_ENTRIES} entries`);
        const rel = prefix ? `${prefix}/${name}` : name;
        if (handle.kind === "directory") {
          dirs.add(rel);
          await walk(handle as FileSystemDirectoryHandle, rel);
        } else {
          const file = await (handle as FileSystemFileHandle).getFile();
          files.set(rel, { size: file.size, mtime: Math.floor(file.lastModified) });
        }
      }
    };
    await walk(this.root, "");
    return { files, dirs };
  }

  private async dir(path: string, create: boolean): Promise<FileSystemDirectoryHandle> {
    let dir = this.root;
    for (const part of path.split("/").filter(Boolean)) {
      dir = await dir.getDirectoryHandle(part, { create });
    }
    return dir;
  }

  private split(path: string): [string, string] {
    const slash = path.lastIndexOf("/");
    return slash >= 0 ? [path.slice(0, slash), path.slice(slash + 1)] : ["", path];
  }

  async file(path: string): Promise<File> {
    const [parent, name] = this.split(path);
    return (await (await this.dir(parent, false)).getFileHandle(name)).getFile();
  }

  /** Writes a file from chunks; returns its new metadata. */
  async write(path: string, fill: (sink: (chunk: Uint8Array) => Promise<void>) => Promise<unknown>): Promise<FileMeta> {
    const [parent, name] = this.split(path);
    const handle = await (await this.dir(parent, true)).getFileHandle(name, { create: true });
    const writable = await handle.createWritable();
    try {
      await fill(async (chunk) => {
        await writable.write(chunk.slice());
      });
      await writable.close();
    } catch (error) {
      await writable.abort().catch(() => {});
      throw error;
    }
    const file = await handle.getFile();
    return { size: file.size, mtime: Math.floor(file.lastModified) };
  }

  async remove(path: string, directory: boolean): Promise<void> {
    const [parent, name] = this.split(path);
    try {
      await (await this.dir(parent, false)).removeEntry(name, { recursive: false });
    } catch (error) {
      const code = (error as DOMException)?.name;
      // Gone already is fine; a non-empty directory is left alone.
      if (code === "NotFoundError") return;
      if (directory && code === "InvalidModificationError") return;
      throw error;
    }
  }

  async mkdir(path: string): Promise<void> {
    await this.dir(path, true);
  }
}

function tsMillis(ts: { seconds: bigint; nanos: number } | undefined): number {
  if (!ts) return 0;
  return Number(ts.seconds) * 1000 + Math.floor(ts.nanos / 1_000_000);
}

/** The guest side: a directory through `FilesystemService`. */
export class RemoteSide {
  constructor(
    readonly api: Api,
    readonly root: string,
  ) {}

  path(rel: string): string {
    return joinGuest(this.root, rel);
  }

  async scan(): Promise<Snapshot> {
    const files = new Map<string, FileMeta>();
    const dirs = new Set<string>();
    let pageToken = "";
    const prefix = this.root.replace(/\/+$/, "") + "/";
    for (let page = 0; page < 1000; page++) {
      const response = await this.api.files.listDir({
        path: this.root,
        depth: 64,
        includeHidden: true,
        pageSize: 1000,
        pageToken,
      });
      for (const entry of response.entries) {
        if (!entry.path.startsWith(prefix)) continue;
        const rel = entry.path.slice(prefix.length);
        if (rel.split("/").some(ignored)) continue;
        if (files.size + dirs.size >= MAX_ENTRIES) throw new Error(`the guest folder has more than ${MAX_ENTRIES} entries`);
        if (entry.type === FileType.DIRECTORY) dirs.add(rel);
        else if (entry.type === FileType.FILE) files.set(rel, { size: Number(entry.size), mtime: tsMillis(entry.modifiedAt) });
      }
      pageToken = response.nextPageToken;
      if (!pageToken) break;
    }
    return { files, dirs };
  }

  async push(rel: string, blob: Blob): Promise<FileMeta> {
    const entry = await uploadBlob(this.api, this.path(rel), blob);
    if (entry) return { size: Number(entry.size), mtime: tsMillis(entry.modifiedAt) };
    const stat = await this.api.files.stat({ path: this.path(rel) });
    return { size: Number(stat.entry?.size ?? 0), mtime: tsMillis(stat.entry?.modifiedAt) };
  }

  async stat(rel: string): Promise<FileMeta | null> {
    try {
      const stat = await this.api.files.stat({ path: this.path(rel) });
      return { size: Number(stat.entry?.size ?? 0), mtime: tsMillis(stat.entry?.modifiedAt) };
    } catch {
      return null;
    }
  }

  async read(rel: string, sink: (chunk: Uint8Array) => Promise<void>): Promise<void> {
    await readInto(this.api, this.path(rel), sink);
  }

  async remove(rel: string, directory: boolean): Promise<void> {
    try {
      await this.api.files.remove({ path: this.path(rel), recursive: false, missingOk: true });
    } catch (error) {
      // A directory that is not empty stays.
      if (!directory) throw error;
    }
  }

  async mkdir(rel: string): Promise<void> {
    await this.api.files.makeDir({ path: this.path(rel), parents: true });
  }
}

// ---------------------------------------------------------------- engine

export interface ShareEvents {
  onState?: (state: ShareState) => void;
  onError?: (message: string) => void;
  /** After every pass (the base is what to persist). */
  onSynced?: (base: Base, applied: Action[]) => void;
}

export interface ShareState {
  status: "idle" | "syncing" | "error" | "stopped";
  lastSync: number;
  files: number;
  conflicts: number;
  detail?: string;
}

async function sha256(chunks: AsyncIterable<Uint8Array> | Uint8Array[]): Promise<string> {
  const parts: Uint8Array[] = [];
  let total = 0;
  for await (const chunk of chunks as AsyncIterable<Uint8Array>) {
    parts.push(chunk);
    total += chunk.byteLength;
  }
  const all = new Uint8Array(total);
  let at = 0;
  for (const p of parts) {
    all.set(p, at);
    at += p.byteLength;
  }
  const digest = await crypto.subtle.digest("SHA-256", all);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

/** Largest file compared by content when both sides created it. */
const MAX_COMPARE_BYTES = 256 * 1024 * 1024;
const REMOTE_POLL_MS = 1000;
const LOCAL_POLL_MS = 3000;
const FULL_PASS_MS = 15_000;

export class FolderShare {
  base: Base;
  private running = false;
  private again = false;
  private stopped = false;
  private watcherId: string | null = null;
  private timers: number[] = [];
  private observer: { disconnect(): void } | null = null;
  private conflicts = 0;
  private state: ShareState = { status: "idle", lastSync: 0, files: 0, conflicts: 0 };

  constructor(
    readonly local: LocalSide,
    readonly remote: RemoteSide,
    base: Base | null,
    private readonly events: ShareEvents = {},
    private readonly now: () => Date = () => new Date(),
  ) {
    this.base = base ?? emptyBase();
  }

  get status(): ShareState {
    return this.state;
  }

  /** Starts watching both sides and runs the first pass. */
  async start(): Promise<void> {
    this.stopped = false;
    await this.remote.mkdir("");
    try {
      const watcher = await this.remote.api.files.createWatcher({ path: this.remote.root, recursive: true });
      this.watcherId = watcher.watcherId;
    } catch {
      this.watcherId = null; // periodic passes still cover guest changes
    }
    this.watchLocal();
    this.every(REMOTE_POLL_MS, () => void this.pollRemote());
    this.every(FULL_PASS_MS, () => this.trigger());
    await this.syncOnce();
  }

  stop(): void {
    this.stopped = true;
    for (const t of this.timers.splice(0)) window.clearInterval(t);
    this.observer?.disconnect();
    this.observer = null;
    if (this.watcherId) void this.remote.api.files.removeWatcher({ watcherId: this.watcherId }).catch(() => {});
    this.watcherId = null;
    this.setState({ status: "stopped" });
  }

  /** Schedules a pass (coalesced). */
  trigger(): void {
    if (this.stopped) return;
    if (this.running) {
      this.again = true;
      return;
    }
    void this.syncOnce();
  }

  private every(ms: number, fn: () => void): void {
    this.timers.push(window.setInterval(fn, ms));
  }

  private watchLocal(): void {
    const Observer = (globalThis as unknown as { FileSystemObserver?: new (cb: () => void) => { observe(h: FileSystemHandle, o: { recursive: boolean }): Promise<void>; disconnect(): void } }).FileSystemObserver;
    if (Observer) {
      try {
        const observer = new Observer(() => this.trigger());
        void observer.observe(this.local.root, { recursive: true }).catch(() => {
          this.every(LOCAL_POLL_MS, () => this.trigger());
        });
        this.observer = observer;
        return;
      } catch {
        // fall through to polling
      }
    }
    this.every(LOCAL_POLL_MS, () => this.trigger());
  }

  private async pollRemote(): Promise<void> {
    if (!this.watcherId || this.stopped) return;
    try {
      const events = await this.remote.api.files.getWatcherEvents({ watcherId: this.watcherId, maxEvents: 256 });
      if (events.events.length > 0 || events.overflowed) this.trigger();
    } catch {
      this.watcherId = null; // expired: the full passes carry on
    }
  }

  private setState(patch: Partial<ShareState>): void {
    this.state = { ...this.state, ...patch, conflicts: this.conflicts };
    this.events.onState?.(this.state);
  }

  /** One full pass. */
  async syncOnce(): Promise<Action[]> {
    if (this.running) {
      this.again = true;
      return [];
    }
    this.running = true;
    const applied: Action[] = [];
    try {
      do {
        this.again = false;
        this.setState({ status: "syncing" });
        const [local, remote] = await Promise.all([this.local.scan(), this.remote.scan()]);
        const actions = plan(local, remote, this.base);
        for (const action of actions) {
          if (this.stopped) break;
          try {
            await this.apply(action, local, remote);
            applied.push(action);
          } catch (error) {
            this.events.onError?.(`${action.kind} ${action.path}: ${String((error as Error)?.message ?? error)}`);
          }
        }
        this.setState({ status: "idle", lastSync: Date.now(), files: this.base.files.size });
        this.events.onSynced?.(this.base, applied);
      } while (this.again && !this.stopped);
    } catch (error) {
      const message = String((error as Error)?.message ?? error);
      this.setState({ status: "error", detail: message });
      this.events.onError?.(message);
    } finally {
      this.running = false;
    }
    return applied;
  }

  private async pushFile(path: string): Promise<BaseEntry> {
    const file = await this.local.file(path);
    const remote = await this.remote.push(path, file);
    const again = await this.local.file(path);
    return { local: { size: again.size, mtime: Math.floor(again.lastModified) }, remote };
  }

  private async pullFile(path: string, target = path): Promise<BaseEntry> {
    const local = await this.local.write(target, (sink) => this.remote.read(path, sink));
    const remote = (await this.remote.stat(path)) ?? { size: local.size, mtime: 0 };
    return { local, remote };
  }

  private async apply(action: Action, local: Snapshot, remote: Snapshot): Promise<void> {
    const { path } = action;
    switch (action.kind) {
      case "push":
        this.base.files.set(path, await this.pushFile(path));
        break;
      case "pull":
        this.base.files.set(path, await this.pullFile(path));
        break;
      case "delete-remote":
        await this.remote.remove(path, false);
        this.base.files.delete(path);
        break;
      case "delete-local":
        await this.local.remove(path, false);
        this.base.files.delete(path);
        break;
      case "forget":
        this.base.files.delete(path);
        break;
      case "compare": {
        const l = local.files.get(path)!;
        const r = remote.files.get(path)!;
        if (l.size <= MAX_COMPARE_BYTES) {
          const file = await this.local.file(path);
          const localHash = await sha256([new Uint8Array(await file.arrayBuffer())]);
          const chunks: Uint8Array[] = [];
          await this.remote.read(path, async (c) => {
            chunks.push(c);
          });
          const remoteHash = await sha256(chunks);
          if (localHash === remoteHash) {
            this.base.files.set(path, { local: l, remote: r });
            break;
          }
        }
        await this.conflict(path);
        break;
      }
      case "conflict":
        await this.conflict(path);
        break;
      case "mkdir-remote":
        await this.remote.mkdir(path);
        this.base.dirs.add(path);
        break;
      case "mkdir-local":
        await this.local.mkdir(path);
        this.base.dirs.add(path);
        break;
      case "rmdir-remote":
        await this.remote.remove(path, true);
        this.base.dirs.delete(path);
        break;
      case "rmdir-local":
        await this.local.remove(path, true);
        this.base.dirs.delete(path);
        break;
    }
    // Remember every directory both sides now have.
    for (const dir of local.dirs) if (remote.dirs.has(dir)) this.base.dirs.add(dir);
  }

  /** Keeps both: the guest's version is saved as a sibling copy locally
   * (the next pass uploads it), the local version wins at the path. */
  private async conflict(path: string): Promise<void> {
    const copy = conflictName(path, this.now());
    await this.pullFile(path, copy);
    this.base.files.set(path, await this.pushFile(path));
    this.conflicts += 1;
  }
}

// ---------------------------------------------------------------- persistence

/** Serializable form of a base (IndexedDB / JSON). */
export interface StoredBase {
  files: Array<[string, BaseEntry]>;
  dirs: string[];
}

export function storeBase(base: Base): StoredBase {
  return { files: [...base.files.entries()], dirs: [...base.dirs] };
}

export function loadBase(stored: StoredBase | undefined | null): Base {
  if (!stored) return emptyBase();
  return { files: new Map(stored.files), dirs: new Set(stored.dirs) };
}
