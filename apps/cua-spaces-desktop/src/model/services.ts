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
  /** `openUrl` opens a page in this machine's browser: false when there is none (or it would not start). */
  constructor(
    private readonly auth: AuthLike,
    private readonly openUrl: (url: string) => Promise<boolean>,
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

  /** The browser on this machine finishes the sign-in. Without one (a Linux
   * box with no browser), the device flow instead, as `cua auth login
   * --remote` does: its page opens on any device, with the code the page
   * shows. */
  async beginSignIn(): Promise<SignInAttempt> {
    let attempt = await this.auth.beginLogin(undefined);
    const opened = attempt.url() ? await this.openUrl(attempt.url()) : false;
    if (!opened && attempt.method() === "browser") attempt = await this.auth.beginLogin("device");
    const url = attempt.url();
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

/** The one-time notice that usage data is now linked to the signed-in
 * account (CUA-1881), as the page shows it: the SDK's words, and whether it
 * is due now. */
export interface LinkNoticeView {
  due: boolean;
  title: string;
  body: string;
  okLabel: string;
  turnOffLabel: string;
  detailsLabel: string;
  docsUrl: string;
}

/** The usage-telemetry switch, and where the app's usage events go. */
export interface TelemetryRunning {
  status(): AppTelemetryInput;
  setEnabled(on: boolean): AppTelemetryInput;
  /** Records signals the app core derived (`appTelemetry*`). */
  record(signals: AppTelemetrySignal[]): void;
  /** The first-run notice was on screen and the person moved on (left
   * Welcome, finished the first run, or used the switch beside the note):
   * from here usage events may be sent (the SwiftUI app's
   * `appTelemetryWelcomeLeft`). */
  acknowledgeNotice(): void;
  /** The one-time linking notice for machines that saw the older
   * "anonymous" wording (`telemetryLinkNotice`, `telemetryLinkNoticeDue`). */
  linkNotice(): LinkNoticeView;
  /** The person closed it, either button (`telemetryAcknowledgeLinkNotice`):
   * linking starts at the next launch, never in this process. */
  acknowledgeLinkNotice(): void;
}

/** Signals held while the first-run notice is still on screen. */
const MAX_HELD = 200;

/** The SDK's telemetry setting (`$CUA_HOME/config.toml`, the same switch as
 * `cua telemetry off`); the environment wins and locks it.
 *
 * The SDK sends nothing from a machine before the app showed its first-run
 * notice (`acknowledgeNotice`). Until then signals wait here, in order, so
 * the first session's run start, sign-in and first Space still count once
 * Welcome is left; turning usage data off drops them. */
export class LiveTelemetry implements TelemetryRunning {
  private held: AppTelemetrySignal[] = [];
  private noticeShown = false;

  constructor(private readonly native: Native) {}

  static input(s: TelemetryStatus): AppTelemetryInput {
    const locked = s.sourceKind !== "config" && s.sourceKind !== "default";
    return { enabled: s.enabled, lockedBy: locked ? s.source : undefined, noticeShown: s.noticeShown };
  }

  status(): AppTelemetryInput {
    return LiveTelemetry.input(this.native.telemetryStatus());
  }

  setEnabled(on: boolean): AppTelemetryInput {
    if (!on) this.held = [];
    return LiveTelemetry.input(this.native.telemetrySetEnabled(on));
  }

  record(signals: AppTelemetrySignal[]): void {
    if (!signals.length) return;
    if (this.shown()) {
      this.native.appTelemetryRecord(signals);
      return;
    }
    this.held.push(...signals);
    if (this.held.length > MAX_HELD) this.held.splice(0, this.held.length - MAX_HELD);
  }

  acknowledgeNotice(): void {
    const s = this.native.telemetryStatus();
    if (!s.noticeShown) {
      try {
        // Keeps the switch as it is, writes the notice marker, then records
        // the `app_launched` that waited for it.
        this.native.appTelemetryWelcomeLeft(s.enabled);
      } catch (error) {
        console.warn(`[cua-spaces] usage notice not recorded: ${error instanceof Error ? error.message : String(error)}`);
        return;
      }
    }
    this.noticeShown = true;
    const held = this.held.splice(0);
    if (held.length && s.enabled) this.native.appTelemetryRecord(held);
  }

  linkNotice(): LinkNoticeView {
    const n = this.native.telemetryLinkNotice();
    return {
      due: this.native.telemetryLinkNoticeDue(),
      title: n.title,
      body: n.body,
      okLabel: n.okLabel,
      turnOffLabel: n.turnOffLabel,
      detailsLabel: n.detailsLabel,
      docsUrl: n.docsUrl,
    };
  }

  acknowledgeLinkNotice(): void {
    this.native.telemetryAcknowledgeLinkNotice();
  }

  private shown(): boolean {
    if (!this.noticeShown) this.noticeShown = this.native.telemetryStatus().noticeShown;
    return this.noticeShown;
  }
}
