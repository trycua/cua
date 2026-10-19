// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Where the viewer talks to, and with what credential.
 *
 * The page is always served one level below a spacesd-shaped origin:
 *
 * - embedded: `http://host:3211/viewer/` (base `/`);
 * - Fleet signed service URL: `/api/signed-svc/<token>/viewer/`;
 * - standalone multi-sandbox server: `/s/<id>/viewer/`.
 *
 * So the base is the page's parent directory in every mode, and gRPC-Web
 * (`<base>cua.env.v1.X/Y`), the media socket (`<base>media`) and signed
 * file URLs (`<base>files`) resolve against it. The credential is a viewer
 * ticket from the URL fragment (`#ticket=`), which never reaches a server
 * or proxy log. It is moved to sessionStorage and scrubbed from the address
 * bar, so a reload keeps working and the ticket does not linger in history.
 */

import { createClient, type Client, type Interceptor } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";

import { ComputerService } from "./gen/cua/env/v1/computer_pb";
import { FilesystemService } from "./gen/cua/env/v1/filesystem_pb";
import { PresenceService } from "./gen/cua/env/v1/presence_pb";
import { StreamService } from "./gen/cua/env/v1/stream_pb";
import { SystemService } from "./gen/cua/env/v1/system_pb";

/** Header the viewer sends its ticket in (the Fleet gateway strips
 * `authorization`, so the alternate header works on every path). */
export const ENV_AUTHORIZATION = "x-cua-env-authorization";
const STORAGE_KEY = "cua.viewer.ticket";

export interface Endpoint {
  /** Base URL with a trailing slash. */
  baseUrl: string;
  /** Viewer ticket, or null when the spacesd needs none (open loopback). */
  ticket: string | null;
  /** Options from the fragment/query (`scale`, `audio`, `record_*`, ...). */
  params: URLSearchParams;
}

interface LocationLike {
  href: string;
  hash: string;
  search: string;
}

interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** The base URL for a viewer page URL: its parent directory. */
export function baseUrlFor(pageUrl: string): string {
  const url = new URL(pageUrl);
  url.hash = "";
  url.search = "";
  // `/x/viewer/` and `/x/viewer/index.html` both have the base `/x/`.
  const path = url.pathname.replace(/[^/]*$/, "");
  url.pathname = path.replace(/[^/]+\/$/, "");
  return url.toString();
}

/**
 * Reads the endpoint from the page location. `#base=` overrides the base
 * (development against a remote spacesd). The ticket comes from the
 * fragment, else from this tab's sessionStorage.
 */
export function endpointFromLocation(
  location: LocationLike,
  storage: StorageLike | null,
  scrub?: (urlWithoutTicket: string) => void,
): Endpoint {
  const fragment = new URLSearchParams(location.hash.replace(/^#/, ""));
  const query = new URLSearchParams(location.search);
  const params = new URLSearchParams(query);
  for (const [key, value] of fragment) {
    if (key !== "ticket") params.set(key, value);
  }
  let ticket = fragment.get("ticket");
  const base = fragment.get("base") ?? query.get("base");
  const baseUrl = base ? new URL(base.endsWith("/") ? base : `${base}/`, location.href).toString() : baseUrlFor(location.href);
  const key = `${STORAGE_KEY}:${baseUrl}`;
  if (ticket) {
    try {
      storage?.setItem(key, ticket);
    } catch {
      // storage disabled: the ticket still works for this page load
    }
    if (scrub) {
      fragment.delete("ticket");
      const url = new URL(location.href);
      const rest = fragment.toString();
      url.hash = rest ? `#${rest}` : "";
      scrub(url.toString());
    }
  } else {
    try {
      ticket = storage?.getItem(key) ?? null;
    } catch {
      ticket = null;
    }
  }
  return { baseUrl, ticket: ticket || null, params };
}

/** `ws(s)://` URL of the media socket from `OpenMediaResponse.ws_path`. */
export function mediaSocketUrl(baseUrl: string, wsPath: string): string {
  const url = new URL(wsPath.replace(/^\//, ""), baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

/** Absolute URL of a spacesd path (for `/files` signed URLs). */
export function spacesdUrl(baseUrl: string, path: string): string {
  return new URL(path.replace(/^\//, ""), baseUrl).toString();
}

export interface Api {
  endpoint: Endpoint;
  system: Client<typeof SystemService>;
  stream: Client<typeof StreamService>;
  computer: Client<typeof ComputerService>;
  files: Client<typeof FilesystemService>;
  presence: Client<typeof PresenceService>;
}

export function createApi(endpoint: Endpoint, fetchImpl?: typeof fetch): Api {
  const auth: Interceptor = (next) => async (request) => {
    if (endpoint.ticket) request.header.set(ENV_AUTHORIZATION, `Bearer ${endpoint.ticket}`);
    return next(request);
  };
  const transport = createGrpcWebTransport({
    baseUrl: endpoint.baseUrl.replace(/\/$/, ""),
    interceptors: [auth],
    // Binary protobuf: tonic-web speaks `application/grpc-web+proto`.
    useBinaryFormat: true,
    fetch: fetchImpl ?? ((input, init) => globalThis.fetch(input, { ...init, credentials: "omit" })),
  });
  return {
    endpoint,
    system: createClient(SystemService, transport),
    stream: createClient(StreamService, transport),
    computer: createClient(ComputerService, transport),
    files: createClient(FilesystemService, transport),
    presence: createClient(PresenceService, transport),
  };
}
