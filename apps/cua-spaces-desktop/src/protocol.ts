// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { app, net, protocol } from "electron";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import * as path from "node:path";

export const APP_SCHEME = "cua-spaces";
export const APP_HOST = "app";
export const APP_ORIGIN = `${APP_SCHEME}://${APP_HOST}`;

/** Dev server origin, set by `pnpm dev` (scripts/dev.mjs). Unset in packaged builds. */
export const devServerUrl = (): URL | null => {
  const raw = process.env.CUA_SPACES_DEV_URL;
  return raw && !app.isPackaged ? new URL(raw) : null;
};

/** Must run before `app.whenReady()`. */
export function registerSchemePrivileges(): void {
  protocol.registerSchemesAsPrivileged([
    {
      scheme: APP_SCHEME,
      privileges: {
        standard: true,
        secure: true,
        supportFetchAPI: true,
        corsEnabled: true,
        stream: true,
        // Custom schemes skip Chromium's code cache unless they opt in.
        codeCache: !devServerUrl(),
      },
    },
  ]);
}

function contentSecurityPolicy(dev: URL | null): string {
  // The renderer reaches the cua daemon's loopback listener and the relay
  // directly (see README "Data path"); Vite HMR needs its WebSocket in dev.
  const connect = ["'self'", "http://127.0.0.1:*", "ws://127.0.0.1:*", "https:", "wss:"];
  if (dev) connect.push(dev.origin, `ws://${dev.host}`);
  return [
    "default-src 'self'",
    // 'unsafe-inline' covers the pre-mount theme script in index.html and
    // the React Refresh preamble in dev. 'wasm-unsafe-eval' covers the core
    // wasm module.
    "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'",
    "style-src 'self' 'unsafe-inline'",
    `img-src 'self' ${APP_SCHEME}: data: blob: https:`,
    `media-src 'self' ${APP_SCHEME}: blob: https:`,
    `font-src 'self' ${APP_SCHEME}: data:`,
    `connect-src ${connect.join(" ")}`,
    "worker-src 'self' blob:",
    "frame-src 'none'",
    "object-src 'none'",
    "base-uri 'self'",
    "form-action 'self'",
  ].join("; ");
}

/** Where the built web UI lives, falling back to the bundled placeholder page. */
export function resolveWebRoot(): string {
  const candidates = app.isPackaged
    ? [path.join(process.resourcesPath, "web")]
    : [path.resolve(app.getAppPath(), "../cua-spaces-web/dist")];
  for (const dir of candidates) {
    if (existsSync(path.join(dir, "index.html"))) return dir;
  }
  return path.join(app.getAppPath(), "placeholder");
}

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".map": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
  ".wasm": "application/wasm",
  ".mp4": "video/mp4",
  ".webm": "video/webm",
  ".txt": "text/plain; charset=utf-8",
};

function withCsp(res: Response, csp: string): Response {
  const headers = new Headers(res.headers);
  headers.set("Content-Security-Policy", csp);
  return new Response(res.body, { status: res.status, statusText: res.statusText, headers });
}

// Serves files from `root`. Paths that are not files fall back to index.html
// so the SPA router handles them, except asset-shaped misses, which 404.
async function serveAsset(request: Request, root: string): Promise<Response> {
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response(null, { status: 405 });
  }
  let pathname: string;
  try {
    pathname = decodeURIComponent(new URL(request.url).pathname);
  } catch {
    return new Response(null, { status: 400 });
  }
  if (pathname.includes("\0")) return new Response(null, { status: 400 });
  const base = path.resolve(root);
  let file = path.resolve(base, `.${pathname}`);
  if (file !== base && !file.startsWith(base + path.sep)) return new Response(null, { status: 404 });

  const info = await stat(file).catch(() => null);
  if (!info?.isFile()) {
    const wantsHtml = request.headers.get("accept")?.includes("text/html") ?? false;
    if (path.extname(file) !== "" && !wantsHtml) return new Response(null, { status: 404 });
    file = path.join(base, "index.html");
  }
  const body = await readFile(file).catch(() => null);
  if (!body) return new Response(null, { status: 404 });
  return new Response(request.method === "HEAD" ? null : new Uint8Array(body), {
    headers: {
      "content-type": MIME[path.extname(file).toLowerCase()] ?? "application/octet-stream",
    },
  });
}

const DROPPED_HEADERS = new Set([
  "host",
  "origin",
  "referer",
  "connection",
  "content-length",
  "accept-encoding",
  "upgrade-insecure-requests",
]);

async function proxy(request: Request, target: URL, fallbackRoot: string): Promise<Response> {
  const url = new URL(request.url);
  const headers = new Headers();
  request.headers.forEach((value, name) => {
    if (!DROPPED_HEADERS.has(name) && !name.startsWith("sec-fetch-")) headers.set(name, value);
  });
  const init: RequestInit & { duplex?: "half" } = { method: request.method, headers };
  if (request.method !== "GET" && request.method !== "HEAD") {
    init.body = request.body;
    init.duplex = "half";
  }
  try {
    return await net.fetch(new URL(`${url.pathname}${url.search}`, target).toString(), init);
  } catch {
    // Dev server not running: show the bundled placeholder so the shell
    // still comes up, instead of a blank error page.
    return serveAsset(request, fallbackRoot);
  }
}

/** Call after `app.whenReady()`. */
export function handleAppProtocol(): { webRoot: string; dev: URL | null } {
  const dev = devServerUrl();
  const webRoot = resolveWebRoot();
  const placeholder = path.join(app.getAppPath(), "placeholder");
  const csp = contentSecurityPolicy(dev);
  protocol.handle(APP_SCHEME, async (request) => {
    if (new URL(request.url).host !== APP_HOST) return new Response(null, { status: 404 });
    const res = dev ? await proxy(request, dev, placeholder) : await serveAsset(request, webRoot);
    return withCsp(res, csp);
  });
  return { webRoot: dev ? dev.origin : webRoot, dev };
}
