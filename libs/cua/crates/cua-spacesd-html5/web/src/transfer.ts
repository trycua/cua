// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * File transfer over `FilesystemService`: resumable chunked uploads
 * (`BeginUpload` / `UploadChunk` / `CommitUpload`, the gRPC-Web form of
 * `WriteFile`) and streamed reads (`ReadFile`). Memory stays bounded by one
 * chunk in both directions.
 */

import { create } from "@bufbuild/protobuf";

import type { Api } from "./api";
import { type EntryInfo, WriteFileHeaderSchema, WriteMode } from "./gen/cua/env/v1/filesystem_pb";

const DEFAULT_CHUNK = 1024 * 1024;

export interface Progress {
  path: string;
  done: number;
  total: number;
}

/** Uploads `blob` to the guest `path` (parents created, replaced). */
export async function uploadBlob(
  api: Api,
  path: string,
  blob: Blob,
  onProgress?: (progress: Progress) => void,
  signal?: AbortSignal,
): Promise<EntryInfo | undefined> {
  const begin = await api.files.beginUpload(
    {
      header: create(WriteFileHeaderSchema, {
        path,
        mode: WriteMode.OVERWRITE,
        createParents: true,
        expectedSize: BigInt(blob.size),
      }),
    },
    { signal },
  );
  const uploadId = begin.uploadId;
  const chunk = Math.max(64 * 1024, Math.min(begin.maxChunkBytes || DEFAULT_CHUNK, 4 * 1024 * 1024));
  let offset = Number(begin.receivedBytes);
  try {
    while (offset < blob.size) {
      if (signal?.aborted) throw new DOMException("aborted", "AbortError");
      const end = Math.min(blob.size, offset + chunk);
      const data = new Uint8Array(await blob.slice(offset, end).arrayBuffer());
      const response = await api.files.uploadChunk({ uploadId, offset: BigInt(offset), data }, { signal });
      offset = Number(response.receivedBytes);
      onProgress?.({ path, done: offset, total: blob.size });
    }
    const committed = await api.files.commitUpload({ uploadId, sha256: "" }, { signal });
    onProgress?.({ path, done: blob.size, total: blob.size });
    return committed.entry;
  } catch (error) {
    await api.files.abortUpload({ uploadId }).catch(() => {});
    throw error;
  }
}

/** Streams the guest file at `path` into `sink` (chunk by chunk). */
export async function readInto(
  api: Api,
  path: string,
  sink: (chunk: Uint8Array) => Promise<void> | void,
  signal?: AbortSignal,
): Promise<EntryInfo | undefined> {
  let entry: EntryInfo | undefined;
  for await (const message of api.files.readFile({ path, chunkSize: DEFAULT_CHUNK }, { signal })) {
    switch (message.message.case) {
      case "entry":
        entry = message.message.value;
        break;
      case "chunk":
        await sink(message.message.value.data);
        break;
      default:
        break;
    }
  }
  return entry;
}

/** Reads a whole (small) guest file. */
export async function readAll(api: Api, path: string, maxBytes = 64 * 1024 * 1024): Promise<Uint8Array> {
  const parts: Uint8Array[] = [];
  let total = 0;
  await readInto(api, path, (chunk) => {
    total += chunk.byteLength;
    if (total > maxBytes) throw new Error(`${path} is larger than ${maxBytes} bytes`);
    parts.push(chunk);
  });
  const out = new Uint8Array(total);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.byteLength;
  }
  return out;
}

/** Joins guest path segments with `/`. */
export function joinGuest(...parts: string[]): string {
  return parts
    .filter((p) => p !== "")
    .map((p, i) => (i === 0 ? p.replace(/\/+$/, "") : p.replace(/^\/+|\/+$/g, "")))
    .join("/");
}

/** A file name safe on every guest OS: no separators or control chars. */
export function safeName(name: string): string {
  const cleaned = name.replace(/[\\/\u0000-\u001f]/g, "_").trim();
  return cleaned === "" || cleaned === "." || cleaned === ".." ? "_" : cleaned;
}
