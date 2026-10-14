// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Folder shares the user started, remembered per sandbox in IndexedDB (the
 * directory handle and the sync base), so a reload can resume after one
 * click (browsers ask for write permission again after a reload).
 */

import type { Api } from "./api";
import { FolderShare, LocalSide, RemoteSide, loadBase, storeBase, type ShareEvents, type StoredBase } from "./folderSync";

const DB = "cua-spacesd-html5";
const STORE = "shares";

export interface SavedShare {
  key: string;
  baseUrl: string;
  guestPath: string;
  name: string;
  handle: FileSystemDirectoryHandle;
  base: StoredBase | null;
}

function open(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE, { keyPath: "key" });
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function tx<T>(mode: IDBTransactionMode, run: (store: IDBObjectStore) => IDBRequest<T>): Promise<T> {
  const db = await open();
  try {
    return await new Promise<T>((resolve, reject) => {
      const request = run(db.transaction(STORE, mode).objectStore(STORE));
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
  } finally {
    db.close();
  }
}

export async function savedShares(baseUrl: string): Promise<SavedShare[]> {
  if (typeof indexedDB === "undefined") return [];
  try {
    const all = await tx<SavedShare[]>("readonly", (s) => s.getAll() as IDBRequest<SavedShare[]>);
    return all.filter((share) => share.baseUrl === baseUrl);
  } catch {
    return [];
  }
}

export async function saveShare(share: SavedShare): Promise<void> {
  try {
    await tx("readwrite", (s) => s.put(share));
  } catch {
    // private mode: the share still runs, it just is not remembered
  }
}

export async function forgetShare(key: string): Promise<void> {
  try {
    await tx("readwrite", (s) => s.delete(key));
  } catch {
    // nothing stored
  }
}

/** Whether this browser can share a folder (File System Access API). */
export function canShareFolders(): boolean {
  return typeof (globalThis as { showDirectoryPicker?: unknown }).showDirectoryPicker === "function";
}

export async function ensurePermission(handle: FileSystemDirectoryHandle): Promise<boolean> {
  const h = handle as FileSystemDirectoryHandle & {
    queryPermission?: (o: { mode: string }) => Promise<PermissionState>;
    requestPermission?: (o: { mode: string }) => Promise<PermissionState>;
  };
  if (!h.queryPermission) return true; // OPFS and engines without the prompt
  if ((await h.queryPermission({ mode: "readwrite" })) === "granted") return true;
  return (await h.requestPermission?.({ mode: "readwrite" })) === "granted";
}

/** Starts (or resumes) a share and keeps its record up to date. */
export async function startShare(
  api: Api,
  handle: FileSystemDirectoryHandle,
  guestPath: string,
  saved: SavedShare | null,
  events: ShareEvents,
): Promise<FolderShare> {
  const key = `${api.endpoint.baseUrl}|${guestPath}`;
  const record: SavedShare = saved ?? {
    key,
    baseUrl: api.endpoint.baseUrl,
    guestPath,
    name: handle.name,
    handle,
    base: null,
  };
  const share = new FolderShare(new LocalSide(handle), new RemoteSide(api, guestPath), loadBase(record.base), {
    ...events,
    onSynced: (base, applied) => {
      record.base = storeBase(base);
      void saveShare(record);
      events.onSynced?.(base, applied);
    },
  });
  await saveShare(record);
  await share.start();
  return share;
}
