// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { request } from "node:http";
import { Readable } from "node:stream";

/**
 * A `fetch` over node's `http`, for the opt-in harness run: the Expo test preset replaces the global fetch with one that
 * cannot read a response here. Loopback http only (the harness), enough of `Response` for the relay client and Connect.
 */
export function nodeFetch(
  input: unknown,
  init: { method?: string; headers?: Record<string, string> | Headers; body?: unknown; signal?: AbortSignal } = {},
): Promise<Response> {
  const url = new URL(String(typeof input === "object" && input && "url" in input ? (input as { url: string }).url : input));
  if (url.protocol !== "http:" || !["127.0.0.1", "localhost"].includes(url.hostname))
    throw new Error(`the harness fetch only reaches loopback http, not ${url.origin}`);
  const headers: Record<string, string> = {};
  const h = init.headers;
  if (h && typeof (h as Headers).forEach === "function") (h as Headers).forEach((v, k) => (headers[k] = v));
  else Object.assign(headers, h ?? {});
  return new Promise((resolve, reject) => {
    const req = request(url, { method: init.method ?? "GET", headers }, (res) => {
      const rh = new Map<string, string>();
      for (const [k, v] of Object.entries(res.headers)) rh.set(k.toLowerCase(), Array.isArray(v) ? v.join(", ") : String(v ?? ""));
      const headersObj = {
        get: (k: string) => rh.get(k.toLowerCase()) ?? null,
        has: (k: string) => rh.has(k.toLowerCase()),
        forEach: (f: (v: string, k: string) => void) => rh.forEach((v, k) => f(v, k)),
        entries: () => rh.entries(),
      };
      const web = Readable.toWeb(res) as ReadableStream<Uint8Array>;
      const status = res.statusCode ?? 0;
      const bytes = async () => {
        const chunks: Buffer[] = [];
        for await (const c of res) chunks.push(c as Buffer);
        return Buffer.concat(chunks);
      };
      resolve({
        ok: status >= 200 && status < 300,
        status,
        statusText: res.statusMessage ?? "",
        headers: headersObj,
        body: web,
        url: url.href,
        text: async () => (await bytes()).toString("utf8"),
        json: async () => JSON.parse((await bytes()).toString("utf8")),
        arrayBuffer: async () => {
          const b = await bytes();
          return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
        },
      } as unknown as Response);
    });
    req.on("error", reject);
    init.signal?.addEventListener("abort", () => req.destroy(new Error("aborted")));
    const b = init.body;
    if (b !== undefined && b !== null) req.write(typeof b === "string" ? b : Buffer.from(b as Uint8Array));
    req.end();
  });
}
