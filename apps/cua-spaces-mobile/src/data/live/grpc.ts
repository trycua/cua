// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * gRPC-Web to a machine through the relay (`<relay>/m/<id>/<service>/<method>`),
 * with Connect-ES and the types the HTML5 viewer is generated from. The relay
 * checks the account bearer (and this device's session) and forwards a
 * relay-signed identity; no machine token is ever on the phone.
 */

import { createClient, type Interceptor } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { fetch as expoFetch } from "expo/fetch";

import { ComputerService } from "./gen/cua/env/v1/computer_pb";
import { HostThreadService } from "./gen/cua/env/v1/host_pb";
import { SystemService } from "./gen/cua/env/v1/system_pb";

export interface Credentials {
  /** The account's access token and this device's session, fresh on each call. */
  headers(): Promise<Record<string, string>>;
}

export type FetchLike = (input: string, init?: RequestInit) => Promise<Response>;

export function transportFor(baseUrl: string, creds: Credentials, extra: Record<string, string> = {}, fetcher?: FetchLike) {
  const auth: Interceptor = (next) => async (req) => {
    const h = await creds.headers();
    for (const [k, v] of Object.entries({ ...h, ...extra })) req.header.set(k, v);
    return next(req);
  };
  return createGrpcWebTransport({
    baseUrl,
    interceptors: [auth],
    // expo/fetch streams response bodies; React Native's own fetch does not.
    fetch: (fetcher ?? (expoFetch as unknown as FetchLike)) as typeof fetch,
  });
}

export const clients = (t: ReturnType<typeof transportFor>) => ({
  thread: createClient(HostThreadService, t),
  computer: createClient(ComputerService, t),
  system: createClient(SystemService, t),
});
