// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { testSigner, verify } from "./helpers/signer";
import { RelayClient, RelayError } from "../src/data/live/relay";

type Call = { url: string; init: RequestInit };

function fake(responses: Record<string, { status?: number; body: unknown }>) {
  const calls: Call[] = [];
  const fetcher = async (url: string, init?: RequestInit) => {
    calls.push({ url, init: init ?? {} });
    const key = `${init?.method ?? "GET"} ${new URL(url).pathname}`;
    const r = responses[key];
    if (!r) return new Response("not found", { status: 404 });
    return new Response(typeof r.body === "string" ? r.body : JSON.stringify(r.body), { status: r.status ?? 200 });
  };
  return { fetcher, calls };
}

describe("RelayClient", () => {
  it("lists machines with the account bearer and the device session, filling defaults", async () => {
    const { fetcher, calls } = fake({ "GET /v1/machines": { body: { machines: [{ id: "m1", name: "Studio Mac", online: true }] } } });
    const r = new RelayClient("https://relay.test", fetcher);
    const ms = await r.machines("tok", "sess");
    expect(ms[0]).toMatchObject({ id: "m1", name: "Studio Mac", online: true, sharing: true, role: "owner", meta: {} });
    const h = calls[0]!.init.headers as Record<string, string>;
    expect(h.authorization).toBe("Bearer tok");
    expect(h["x-cua-device-session"]).toBe("sess");
  });

  it("raises the relay's own words on a refusal", async () => {
    const { fetcher } = fake({ "GET /v1/machines": { status: 403, body: { error: "this device is not enrolled" } } });
    await expect(new RelayClient("https://relay.test", fetcher).machines("tok")).rejects.toMatchObject({ message: "this device is not enrolled", status: 403 });
    await expect(new RelayClient("https://relay.test", fetcher).machines("tok")).rejects.toBeInstanceOf(RelayError);
  });

  it("registers a device with a signature the relay can verify and opens a session", async () => {
    const { fetcher, calls } = fake({
      "POST /v1/devices": { body: { device: { id: "dev_x", name: "This phone", state: "pending" }, code: "ABCD-1234" } },
      "POST /v1/devices/session": { body: { session: "s1", expires_at: 99 } },
    });
    const r = new RelayClient("https://relay.test", fetcher);
    const key = testSigner();
    const e = await r.registerDevice("tok", key, "This phone", "ios", 1700000000);
    expect(e.code).toBe("ABCD-1234");
    const body = JSON.parse(calls[0]!.init.body as string);
    expect(body).toMatchObject({ public_key: key.publicKey, name: "This phone", platform: "ios", ts: 1700000000, bootstrap: true });
    expect(verify(body.public_key, `cua-device-register/v1\n${key.id}\n1700000000`, body.sig)).toBe(true);
    const s = await r.deviceSession("tok", key, 1700000001);
    expect(s.session).toBe("s1");
    const sb = JSON.parse(calls[1]!.init.body as string);
    expect(sb.device_id).toBe(key.id);
    expect(verify(key.publicKey, `cua-device-session/v1\n${key.id}\n1700000001`, sb.sig)).toBe(true);
  });

  it("shares by replacing the allowlist and the view-only list", async () => {
    const { fetcher, calls } = fake({ "PATCH /v1/machines/space-1": { body: { id: "space-1" } } });
    await new RelayClient("https://relay.test", fetcher).patchMachine("tok", "space-1", { allow: ["a@x.io"], viewers: [] });
    expect(JSON.parse(calls[0]!.init.body as string)).toEqual({ allow: ["a@x.io"], viewers: [] });
  });
});
