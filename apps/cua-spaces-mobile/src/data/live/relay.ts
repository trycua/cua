// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The relay's REST directory as the phone uses it (`cua-host::relay`): the
 * account's machines, this device's enrollment and session, and sharing.
 * Plain JSON over `fetch` (an injected one in tests).
 */

import { registerMessage, sessionMessage, type DeviceSigner } from "./device";

export interface RelayMachine {
  id: string;
  name: string;
  owner: { id: string; email?: string | null; name?: string | null };
  /** `owner` or `shared`. */
  role: string;
  online: boolean;
  sharing: boolean;
  version: string;
  url: string;
  host?: string | null;
  meta: Record<string, string>;
  allow?: string[];
  viewers?: string[];
  /** `false` for a computer nobody enrolled has confirmed: shown as new, never used. */
  confirmed: boolean;
}

/** An enrolled device of the account, as `GET /v1/devices` lists it. */
export interface RelayDevice {
  id: string;
  name: string;
  platform: string;
  phone: boolean;
  state: string;
  current: boolean;
  lastSeen: number | null;
  capabilities: { v: number; canHostSpaces: boolean; images: string[]; kinds: string[]; desktop: boolean } | null;
}

export type DeviceState = "enrolled" | "pending" | "expired" | "revoked";

export interface Enrollment {
  device: { id: string; name: string; state: DeviceState; code_expires?: number | null };
  code?: string | null;
  enforce_after?: number;
  ttl_secs?: number;
}

export class RelayError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
    this.name = "RelayError";
  }
}

type FetchLike = (url: string, init?: RequestInit) => Promise<Response>;

/** The words the relay keeps in a computer's `meta` (`cua-host`: META_OS, META_PROVIDES_SPACES, META_PHONE_FEED; the cloud word is `cua-sandbox-core`'s). */
export const META = { os: "cua.host.os", provides: "cua.host.spaces", cloud: "cua.cloud.provider", phoneFeed: "cua.host.phone" } as const;

export class RelayClient {
  constructor(
    readonly base: string,
    private readonly fetcher: FetchLike = (u, i) => fetch(u, i),
  ) {}

  machineUrl(id: string): string {
    return `${this.base}/m/${id}`;
  }

  private async call<T>(method: string, path: string, token: string, body?: unknown, deviceSession?: string | null): Promise<T> {
    const headers: Record<string, string> = { authorization: `Bearer ${token}`, accept: "application/json" };
    if (deviceSession) headers["x-cua-device-session"] = deviceSession;
    if (body !== undefined) headers["content-type"] = "application/json";
    const r = await this.fetcher(`${this.base}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
    const text = await r.text();
    if (!r.ok) {
      let msg = text;
      try {
        const j = JSON.parse(text) as { error?: string; message?: string };
        msg = j.error ?? j.message ?? text;
      } catch {
        /* plain text */
      }
      throw new RelayError(msg || `the relay answered ${r.status}`, r.status);
    }
    return (text ? JSON.parse(text) : {}) as T;
  }

  async machines(token: string, deviceSession?: string | null): Promise<RelayMachine[]> {
    const r = await this.call<{ machines?: Partial<RelayMachine>[] }>("GET", "/v1/machines", token, undefined, deviceSession);
    return (r.machines ?? []).map((m) => ({
      id: m.id ?? "",
      name: m.name ?? "",
      owner: m.owner ?? { id: "" },
      role: m.role ?? "owner",
      online: m.online ?? false,
      sharing: m.sharing ?? true,
      version: m.version ?? "",
      url: m.url ?? "",
      host: m.host ?? null,
      meta: m.meta ?? {},
      allow: m.allow,
      viewers: m.viewers,
      confirmed: m.confirmed ?? true,
    }));
  }

  /** `GET /v1/devices`: the account's devices, the phones among them (for the pool's Computers list). */
  async devices(token: string, deviceSession?: string | null): Promise<RelayDevice[]> {
    type Wire = {
      id?: string;
      name?: string;
      platform?: string;
      phone?: boolean;
      state?: string;
      current?: boolean;
      last_seen?: number | null;
      capabilities?: { v?: number; can_host_spaces?: boolean; images?: string[]; kinds?: string[]; desktop?: boolean } | null;
    };
    const r = await this.call<{ devices?: Wire[] } | Wire[]>("GET", "/v1/devices", token, undefined, deviceSession);
    const list = Array.isArray(r) ? r : (r.devices ?? []);
    return list.map((d) => ({
      id: d.id ?? "",
      name: d.name ?? "",
      platform: d.platform ?? "",
      phone: d.phone === true,
      state: d.state ?? "",
      current: d.current === true,
      lastSeen: d.last_seen ?? null,
      capabilities: d.capabilities
        ? {
            v: d.capabilities.v ?? 0,
            canHostSpaces: d.capabilities.can_host_spaces === true,
            images: d.capabilities.images ?? [],
            kinds: d.capabilities.kinds ?? [],
            desktop: d.capabilities.desktop === true,
          }
        : null,
    }));
  }

  /** `PATCH /v1/machines/{id}`: replace the allowlist (editors) and view-only list (owner). */
  patchMachine(
    token: string,
    id: string,
    patch: { allow?: string[]; viewers?: string[]; sharing?: boolean; name?: string },
    deviceSession?: string | null,
  ): Promise<RelayMachine> {
    return this.call("PATCH", `/v1/machines/${id}`, token, patch, deviceSession);
  }

  /** `GET /v1/push`: whether this relay can send push (a provider, the APNs key, is set up). 404 where it has no push. */
  async pushStatus(token: string, deviceSession?: string | null): Promise<{ configured: boolean }> {
    const r = await this.call<{ configured?: boolean }>("GET", "/v1/push", token, undefined, deviceSession);
    return { configured: r.configured === true };
  }

  /** `PUT /v1/push/registrations/{device}`: tell the relay where to push for this phone (APNs token and what to send). */
  async registerPush(token: string, deviceId: string, body: Record<string, unknown>, deviceSession?: string | null): Promise<void> {
    await this.call("PUT", `/v1/push/registrations/${encodeURIComponent(deviceId)}`, token, body, deviceSession);
  }

  /** `POST /v1/devices/revoke-phones`: every phone of the account is removed (sessions, tickets, alerts). The ids that went. */
  async revokeAllPhones(token: string, deviceSession?: string | null): Promise<string[]> {
    const r = await this.call<{ revoked?: string[] }>("POST", "/v1/devices/revoke-phones", token, {}, deviceSession);
    return r.revoked ?? [];
  }

  /** `DELETE /v1/push/registrations/{device}`: stop pushing to this phone. */
  async unregisterPush(token: string, deviceId: string, deviceSession?: string | null): Promise<void> {
    await this.call("DELETE", `/v1/push/registrations/${encodeURIComponent(deviceId)}`, token, undefined, deviceSession);
  }

  registerDevice(
    token: string,
    key: DeviceSigner,
    name: string,
    platform: string,
    nowSecs: number,
    capabilities?: Record<string, unknown>,
  ): Promise<Enrollment> {
    return this.call("POST", "/v1/devices", token, {
      public_key: key.publicKey,
      name,
      platform,
      machine_id: null,
      ts: nowSecs,
      sig: key.sign(registerMessage(key.id, nowSecs)),
      bootstrap: true,
      ...(capabilities ? { capabilities } : {}),
    });
  }

  async deviceSession(token: string, key: DeviceSigner, nowSecs: number): Promise<{ session: string; expires_at: number }> {
    return this.call("POST", "/v1/devices/session", token, { device_id: key.id, ts: nowSecs, sig: key.sign(sessionMessage(key.id, nowSecs)) });
  }
}
