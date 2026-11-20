// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The signed-in session: authorization code with PKCE in the system sign-in
 * sheet (ASWebAuthenticationSession, through expo-auth-session), tokens in
 * the Keychain, a refreshing access token. The shape follows `cua-auth`.
 */

import * as AuthSession from "expo-auth-session";
import * as SecureStore from "expo-secure-store";
import * as WebBrowser from "expo-web-browser";

import { signInFailure } from "../core/views";
import { config } from "./config";

WebBrowser.maybeCompleteAuthSession();

export interface Tokens {
  accessToken: string;
  refreshToken: string | null;
  /** Unix seconds. */
  expiresAt: number;
}

const KEY = "ai.cua.session.v1";
const SKEW_S = 60;

export class SignInError extends Error {
  constructor(
    message: string,
    readonly cancelled = false,
  ) {
    super(message);
    this.name = "SignInError";
  }
}

const now = () => Math.floor(Date.now() / 1000);

/** What the provider answers: the claims the app shows (`sub`, `email`, `name`), unverified (the relay verifies the token). */
export function claimsOf(token: string): Record<string, unknown> {
  try {
    const body = token.split(".")[1]!.replace(/-/g, "+").replace(/_/g, "/");
    const json = typeof atob === "function" ? atob(body.padEnd(body.length + ((4 - (body.length % 4)) % 4), "=")) : "";
    return JSON.parse(json) as Record<string, unknown>;
  } catch {
    return {};
  }
}

type Listener = (signedIn: boolean) => void;

class SessionManager {
  private tokens: Tokens | null = null;
  private loaded = false;
  private listeners = new Set<Listener>();
  private refreshing: Promise<Tokens> | null = null;

  subscribe(l: Listener): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }
  private emit() {
    for (const l of this.listeners) l(this.tokens !== null);
  }

  async load(): Promise<boolean> {
    if (!this.loaded) {
      this.loaded = true;
      try {
        const raw = await SecureStore.getItemAsync(KEY);
        this.tokens = raw ? (JSON.parse(raw) as Tokens) : null;
      } catch {
        this.tokens = null;
      }
    }
    return this.tokens !== null;
  }

  private async save(t: Tokens | null) {
    this.tokens = t;
    try {
      if (t) await SecureStore.setItemAsync(KEY, JSON.stringify(t), { keychainAccessible: SecureStore.AFTER_FIRST_UNLOCK_THIS_DEVICE_ONLY });
      else await SecureStore.deleteItemAsync(KEY);
    } catch {
      /* the session still works for this launch */
    }
    this.emit();
  }

  /** Local harness only: a token from the harness's endpoint for the configured account. */
  private async devSignIn(url: string): Promise<void> {
    try {
      const r = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ sub: config.devAccount }) });
      if (!r.ok) throw new Error(`the harness answered ${r.status}`);
      const t = (await r.json()) as { access_token: string; refresh_token?: string; expires_in?: number };
      await this.save({ accessToken: t.access_token, refreshToken: t.refresh_token ?? null, expiresAt: now() + (t.expires_in ?? 3600) });
    } catch (e) {
      throw new SignInError(signInFailure(String(e instanceof Error ? e.message : e), "offline")?.message ?? "Could not reach the local harness.");
    }
  }

  /** The system sign-in sheet; resolves when signed in, throws a `SignInError` in the core's words otherwise. */
  async signIn(): Promise<void> {
    if (config.devTokenUrl) return this.devSignIn(config.devTokenUrl);
    // Exactly the redirect registered for the phone's client, `ai.cua.spaces:/oauth/callback` (one slash).
    // `makeRedirectUri` builds `ai.cua.spaces://oauth/callback`, which the provider refuses ("Invalid parameter: redirect_uri").
    const redirectUri = `${config.redirectScheme}:/${config.redirectPath}`;
    let discovery: AuthSession.DiscoveryDocument;
    try {
      discovery = await AuthSession.fetchDiscoveryAsync(config.issuer);
    } catch (e) {
      throw new SignInError(signInFailure(String(e), "offline")?.message ?? "Could not reach cua.ai. Check the connection and try again.");
    }
    const request = new AuthSession.AuthRequest({
      clientId: config.clientId,
      redirectUri,
      scopes: config.scope.split(" "),
      usePKCE: true,
      responseType: AuthSession.ResponseType.Code,
    });
    const result = await request.promptAsync(discovery);
    if (result.type === "cancel" || result.type === "dismiss") throw new SignInError("Sign-in was cancelled.", true);
    if (result.type !== "success") {
      const detail = result.type === "error" ? (result.params?.error_description ?? result.params?.error ?? result.error?.message ?? "error") : result.type;
      throw new SignInError(signInFailure(String(detail))?.message ?? `Sign-in did not finish (${detail}).`);
    }
    try {
      const t = await AuthSession.exchangeCodeAsync(
        { clientId: config.clientId, code: result.params.code!, redirectUri, extraParams: { code_verifier: request.codeVerifier ?? "" } },
        discovery,
      );
      await this.save({ accessToken: t.accessToken, refreshToken: t.refreshToken ?? null, expiresAt: t.expiresIn ? now() + t.expiresIn : now() + 300 });
    } catch (e) {
      throw new SignInError(signInFailure(String(e instanceof Error ? e.message : e))?.message ?? "Sign-in did not finish.");
    }
  }

  async signOut(): Promise<void> {
    // The sign-in provider is told the refresh token is dead, so a copy of it that outlives the phone's keychain is useless.
    // Best effort and short: signing out never waits on the network.
    const refresh = this.tokens?.refreshToken ?? null;
    await this.save(null);
    if (!refresh || config.devTokenUrl) return;
    try {
      const discovery = await Promise.race([
        AuthSession.fetchDiscoveryAsync(config.issuer),
        new Promise<never>((_, no) => setTimeout(() => no(new Error("slow")), 4000)),
      ]);
      await AuthSession.revokeAsync({ clientId: config.clientId, token: refresh, tokenTypeHint: AuthSession.TokenTypeHint.RefreshToken }, discovery);
    } catch {
      // Offline or the provider has no revocation endpoint: the tokens are already gone from this phone.
    }
  }

  /** A fresh access token, refreshed shortly before it expires; throws when signed out. */
  async accessToken(): Promise<string> {
    await this.load();
    const t = this.tokens;
    if (!t) throw new SignInError("Not signed in.");
    if (t.expiresAt - SKEW_S > now()) return t.accessToken;
    if (!t.refreshToken) {
      await this.save(null);
      throw new SignInError("Your session ended. Sign in again.");
    }
    this.refreshing ??= (async () => {
      try {
        const discovery = await AuthSession.fetchDiscoveryAsync(config.issuer);
        const r = await AuthSession.refreshAsync({ clientId: config.clientId, refreshToken: t.refreshToken! }, discovery);
        const next = { accessToken: r.accessToken, refreshToken: r.refreshToken ?? t.refreshToken, expiresAt: r.expiresIn ? now() + r.expiresIn : now() + 300 };
        await this.save(next);
        return next;
      } finally {
        this.refreshing = null;
      }
    })();
    return (await this.refreshing).accessToken;
  }

  account(): { email: string | null; name: string | null } | null {
    if (!this.tokens) return null;
    const c = claimsOf(this.tokens.accessToken);
    return { email: typeof c.email === "string" ? c.email : null, name: typeof c.name === "string" ? c.name : null };
  }
}

export const session = new SessionManager();
