// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { create, fromBinary, toBinary, type DescMessage, type MessageShape } from "@bufbuild/protobuf";

import { CoordinateSpace } from "../../src/data/live/gen/cua/env/v1/common_pb";
import { PointerRequestSchema, PointerResponseSchema, KeyboardRequestSchema, KeyboardResponseSchema } from "../../src/data/live/gen/cua/env/v1/computer_pb";
import { CallThreadToolRequestSchema, CallThreadToolResponseSchema } from "../../src/data/live/gen/cua/env/v1/host_pb";
import { CreateViewerTicketRequestSchema, CreateViewerTicketResponseSchema } from "../../src/data/live/gen/cua/env/v1/system_pb";

/** A fake relay plus fake host daemon for tests: REST machines, and gRPC-Web for the three services the phone calls. */

const frame = (flag: number, payload: Uint8Array) => {
  const out = new Uint8Array(5 + payload.length);
  out[0] = flag;
  new DataView(out.buffer).setUint32(1, payload.length);
  out.set(payload, 5);
  return out;
};
const unframe = (b: Uint8Array) => b.slice(5, 5 + new DataView(b.buffer, b.byteOffset).getUint32(1));

export interface Seen {
  url: string;
  method?: string;
  restBody?: unknown;
  headers: Record<string, string>;
  tool?: { space: string; name: string; args: Record<string, unknown> };
  pointer?: ReturnType<typeof create<typeof PointerRequestSchema>>;
  keyboard?: ReturnType<typeof create<typeof KeyboardRequestSchema>>;
  ticketTtl?: number;
  ticketPolicy?: number;
}

export function fakeRelay(opts: {
  machines: unknown[];
  /** The computer refuses touch with these words (gRPC PermissionDenied). */ touchRefused?: string;
  /** The relay refuses the thread calls with these words (gRPC PermissionDenied). */ threadRefused?: string;
  tools: (name: string, args: Record<string, unknown>) => unknown;
  isError?: (name: string) => boolean;
  /** Other REST calls (push registrations): an answer, or undefined to fall through. */ rest?: (
    method: string,
    path: string,
    body: unknown,
  ) => { status?: number; body?: unknown } | undefined;
}) {
  const seen: Seen[] = [];
  const grpc = <S extends DescMessage>(schema: S, msg: MessageShape<S>) =>
    new Response(new Uint8Array([...frame(0, toBinary(schema, msg)), ...frame(0x80, new TextEncoder().encode("grpc-status: 0\r\n"))]), {
      status: 200,
      headers: { "content-type": "application/grpc-web+proto" },
    });

  const fetcher = async (input: string | URL | Request, init?: RequestInit): Promise<Response> => {
    const url = String(input);
    const headers = Object.fromEntries(new Headers(init?.headers as HeadersInit).entries());
    const path = new URL(url).pathname;
    const entry: Seen = { url, headers };
    seen.push(entry);
    if (path === "/v1/machines") return new Response(JSON.stringify({ machines: opts.machines }), { status: 200 });
    if (opts.rest && typeof init?.body === "string") {
      entry.method = init.method;
      entry.restBody = JSON.parse(init.body);
    }
    if (opts.rest) {
      entry.method = init?.method;
      const r = opts.rest(init?.method ?? "GET", path, typeof init?.body === "string" ? JSON.parse(init.body) : undefined);
      if (r) return new Response(r.body === undefined || r.status === 204 ? null : JSON.stringify(r.body), { status: r.status ?? 200 });
    }
    const body = init?.body ? new Uint8Array(init.body as ArrayBuffer) : new Uint8Array();
    if (opts.threadRefused && path.endsWith("/cua.env.v1.HostThreadService/CallThreadTool")) {
      return new Response(null, {
        status: 200,
        headers: { "content-type": "application/grpc-web+proto", "grpc-status": "7", "grpc-message": encodeURIComponent(opts.threadRefused) },
      });
    }
    if (path.endsWith("/cua.env.v1.HostThreadService/CallThreadTool")) {
      const req = fromBinary(CallThreadToolRequestSchema, unframe(body));
      const args = JSON.parse(req.argumentsJson || "{}") as Record<string, unknown>;
      entry.tool = { space: req.space, name: req.name, args };
      const out = opts.tools(req.name, args);
      return grpc(
        CallThreadToolResponseSchema,
        create(CallThreadToolResponseSchema, {
          contentJson: JSON.stringify([{ type: "text", text: JSON.stringify(out) }]),
          isError: opts.isError?.(req.name) ?? false,
        }),
      );
    }
    if (path.endsWith("/cua.env.v1.SystemService/CreateViewerTicket")) {
      const req = fromBinary(CreateViewerTicketRequestSchema, unframe(body));
      entry.ticketTtl = Number(req.ttl?.seconds ?? 0);
      entry.ticketPolicy = req.policy;
      return grpc(CreateViewerTicketResponseSchema, create(CreateViewerTicketResponseSchema, { ticket: "tkt", viewerPath: "/viewer/#ticket=tkt" }));
    }
    if (opts.touchRefused && (path.endsWith("/cua.env.v1.ComputerService/Pointer") || path.endsWith("/cua.env.v1.ComputerService/Keyboard"))) {
      return new Response(null, {
        status: 200,
        headers: { "content-type": "application/grpc-web+proto", "grpc-status": "7", "grpc-message": encodeURIComponent(opts.touchRefused) },
      });
    }
    if (path.endsWith("/cua.env.v1.ComputerService/Pointer")) {
      entry.pointer = fromBinary(PointerRequestSchema, unframe(body)) as never;
      return grpc(PointerResponseSchema, create(PointerResponseSchema, {}));
    }
    if (path.endsWith("/cua.env.v1.ComputerService/Keyboard")) {
      entry.keyboard = fromBinary(KeyboardRequestSchema, unframe(body)) as never;
      return grpc(KeyboardResponseSchema, create(KeyboardResponseSchema, {}));
    }
    return new Response("not found", { status: 404 });
  };
  return { fetcher, seen, NORMALIZED: CoordinateSpace.NORMALIZED };
}
