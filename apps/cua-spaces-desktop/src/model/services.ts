// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The account and the usage-telemetry switch (the SwiftUI app's
// AppServices.swift), behind small interfaces so tests never touch the real
// session or telemetry config. Words and rows are the core's.
import type { Native } from "../native/load";
import type { AppHostAccount, AppTelemetryInput, AppTelemetrySignal, AuthIdentity, AuthLike, TelemetryStatus } from "../native/generated/index";

/** A started sign-in: the code to confirm (device flow), then the identity. */
export interface SignInAttempt {
  userCode: string | null;
  /** The page the sign-in finishes on (to open it again). */
  url: string | null;
  wait(): Promise<string | null>;
}

/** The signed-in account's claims, for the presence name and id. */
export interface AccountProfile {
  name?: string;
  email?: string;
  username?: string;
  subject?: string;
}

/** The Cua account. */
export interface AccountRunning {
  /** The signed-in identity, read without network access. */
  identity(): string | null;
  profile(): AccountProfile | null;
  /** Starts a sign-in (opens the browser). */
  beginSignIn(): Promise<SignInAttempt>;
  signOut(): Promise<void>;
  /** The account's token (`force`: refresh it); null when signed out. */
  accessToken(force: boolean): Promise<string | null>;
}

const display = (i: AuthIdentity | undefined) => i?.display ?? i?.email ?? i?.username ?? null;

/** The cua SDK's auth: the same session as `cua auth login`. */
export class LiveAccount implements AccountRunning {
  constructor(
    private readonly auth: AuthLike,
    private readonly openUrl: (url: string) => void,
  ) {}

  private status() {
    try {
      const s = this.auth.status();
      return s.loggedIn ? s : null;
    } catch {
      return null;
    }
  }

  identity(): string | null {
    return display(this.status()?.identity);
  }

  profile(): AccountProfile | null {
    const i = this.status()?.identity;
    return i ? { name: i.name, email: i.email, username: i.username, subject: i.subject } : null;
  }

  async beginSignIn(): Promise<SignInAttempt> {
    const attempt = await this.auth.beginLogin(undefined);
    const url = attempt.url();
    if (url) this.openUrl(url);
    return {
      userCode: attempt.userCode() ?? null,
      url: url || null,
      wait: async () => display(await attempt.wait()),
    };
  }

  async signOut(): Promise<void> {
    await this.auth.logout();
  }

  async accessToken(force: boolean): Promise<string | null> {
    try {
      return await this.auth.accessToken(force);
    } catch (error) {
      // Signed out is no token; anything else (offline, the credential
      // vault) is the error itself, never a silent "not signed in".
      if ((error as { tag?: unknown }).tag === "Unauthenticated") return null;
      throw error;
    }
  }
}

/** What relay sharing is checked against: the account's id, email and name. */
export function hostAccount(account: AccountRunning): AppHostAccount | null {
  const who = account.identity();
  if (!who) return null;
  const p = account.profile();
  const name = p?.name?.trim() ? p.name : undefined;
  return { id: p?.subject, email: p?.email, display: name ?? who };
}

/** The usage-telemetry switch, and where the app's usage events go. */
export interface TelemetryRunning {
  status(): AppTelemetryInput;
  setEnabled(on: boolean): AppTelemetryInput;
  /** Records signals the app core derived (`appTelemetry*`). */
  record(signals: AppTelemetrySignal[]): void;
}

/** The SDK's telemetry setting (`$CUA_HOME/config.toml`, the same switch as
 * `cua telemetry off`); the environment wins and locks it. */
export class LiveTelemetry implements TelemetryRunning {
  constructor(private readonly native: Native) {}

  static input(s: TelemetryStatus): AppTelemetryInput {
    const locked = s.sourceKind !== "config" && s.sourceKind !== "default";
    return { enabled: s.enabled, lockedBy: locked ? s.source : undefined, noticeShown: s.noticeShown };
  }

  status(): AppTelemetryInput {
    return LiveTelemetry.input(this.native.telemetryStatus());
  }

  setEnabled(on: boolean): AppTelemetryInput {
    return LiveTelemetry.input(this.native.telemetrySetEnabled(on));
  }

  record(signals: AppTelemetrySignal[]): void {
    if (signals.length) this.native.appTelemetryRecord(signals);
  }
}
