// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "This machine" as an unattended-access host (the SwiftUI app's
// HostModel.swift): the SDK's `Host` state as the core flattens it
// (`appHostState`), its relay machine id, relay sharing following the
// sign-in (`reconcileAccount`), the page's setup (`setUp`: relay setup with
// the account's token, signing in inline when there is none) and its
// buttons (`run`: the core's setting changes, stop, resume and remove,
// each failure worded with Retry). The SDK runs the host on every system:
// launchd on macOS, a scheduled task on Windows, systemd (or a process)
// on Linux.
import type { Native } from "../native/load";
import type { AppHostAccount, AppHostActionId, AppHostSetupRequest, AppHostState, AppHostSummaryInput, HostLike, HostStatus } from "../native/generated/index";
import { sdkErrorKind, words } from "./errors";
import { presentHostFailure, type HostSetupFailure } from "./host-failure";
import { sleep } from "./time";

/** Why relay setup could not get an account token (`HostSetupAuthError`); worded as "Sign in to Cua". */
export class HostSetupAuthError extends Error {
  static readonly signedOut = "Not signed in to Cua: sign in to set up this machine for access.";
  static readonly signInNotFinished = "Not signed in to Cua: the sign-in did not finish.";
  constructor(readonly reason: "signedOut" | "signInNotFinished") {
    super(HostSetupAuthError[reason]);
    this.name = "HostSetupAuthError";
  }
}

/** The account refused (signed out, or a session that can no longer refresh). */
export function isUnauthenticated(error: unknown): boolean {
  if (error instanceof HostSetupAuthError) return error.reason === "signedOut";
  return sdkErrorKind(error) === "Unauthenticated";
}

const TRANSIENT = new Set(["Http", "Timeout", "Transport"]);

export class HostModel {
  /** The host's state (null until it answers). */
  state: AppHostState | null = null;
  /** This install's relay machine id, once set up in relay mode. */
  machineId: string | null = null;
  error: string | null = null;
  /** What a running setup or Sign In waits for; null otherwise. */
  progress: string | null = null;
  /** The signed-in account (relay mode joins as it). */
  identity: string | null = null;
  /** The account's token (`true`: refresh it); null when signed out; throws when it could not be read. */
  accountToken: ((force: boolean) => Promise<string | null>) | null = null;
  /** The signed-in account's id, email and name, read without the network. */
  currentAccount: (() => AppHostAccount | null) | null = null;
  /** The account the page was last checked against. */
  account: AppHostAccount | null = null;
  /** Signs in to Cua (opens the browser and waits): true once signed in. */
  signIn: (() => Promise<boolean>) | null = null;
  /** Called whenever the state changes (the roster's entry follows it). */
  onChange: (() => void) | null = null;
  /** Waits before trying the account token again after a network error (ms). */
  tokenRetryDelays = [1000, 3000];
  /** A page button is running. */
  busy = false;
  /** The last page button that failed, in plain words with Retry; cleared when any button succeeds. */
  actionFailure: HostSetupFailure | null = null;
  /** What Retry runs again. */
  failedAction: AppHostActionId | null = null;
  /** Asks the system for Local Network access once this machine provides Spaces (macOS; `local-network.ts`). */
  localNetwork: { request(): void } | null = null;
  private askedLocalNetwork = false;
  private accountCheckedAt: number | null = null;
  private reconciling = false;

  static readonly signInProgress = "Sign in to Cua in your browser to continue. Setup finishes on its own after that.";

  constructor(
    private readonly native: Native,
    /** The SDK's `Host` (null until the launch made it). */
    public host: HostLike | null,
    /** The system the words name (`this Mac`, `this PC`, `this computer`). */
    readonly platform: NodeJS.Platform = process.platform,
  ) {}

  /** What the roster's This machine entry reads. */
  get summaryInput(): AppHostSummaryInput | undefined {
    return this.state ? this.native.appHostSummaryInput(this.state) : undefined;
  }

  apply(status: HostStatus): void {
    const next = this.native.appHostState(status);
    next.account = this.account ?? undefined;
    this.state = next;
    this.machineId = status.machineId ?? null;
    // Setup that provides Spaces, a launch on a machine that already does,
    // or Spaces turned on: ask now, while someone is at this machine, not
    // when the first Space boots.
    if (next.configured && next.provideSpaces) this.requestLocalNetwork();
    this.onChange?.();
  }

  /** Asks for Local Network access once (also when the first run ends; see `local-network.ts`). */
  requestLocalNetwork(): void {
    if (this.askedLocalNetwork || !this.localNetwork) return;
    this.askedLocalNetwork = true;
    this.localNetwork.request();
  }

  async refresh(): Promise<void> {
    if (!this.host) return;
    try {
      this.apply(await this.host.status());
    } catch (error) {
      this.error = words(error);
    }
  }

  /** One token read: null when signed out (or refused for good); a network error is tried again before it is reported. */
  async readToken(force: boolean): Promise<string | null> {
    if (!this.accountToken) return null;
    const waits = [...this.tokenRetryDelays];
    for (;;) {
      try {
        const token = await this.accountToken(force);
        return token ? token : null;
      } catch (error) {
        const kind = sdkErrorKind(error);
        if (kind === "Unauthenticated") return null;
        if (kind && TRANSIENT.has(kind) && waits.length) {
          await sleep(waits.shift()!);
          continue;
        }
        throw error;
      }
    }
  }

  // MARK: Setup

  /**
   * Host setup with a request the page's form built (the core validates it
   * again): relay setup with the account's token. Throws the worded
   * failure (`HostSetupFailure`), with the raw error as its details.
   */
  async setUp(request: AppHostSetupRequest): Promise<void> {
    const host = this.host;
    if (!host) throw new Error("This machine can't be set up for access in this build");
    try {
      const status = request.mode === "relay" ? await this.setUpRelay(host, request) : await this.setup(host, request, undefined);
      this.apply(status);
    } catch (error) {
      throw presentHostFailure(words(error), this.platform, isUnauthenticated(error) ? "signedOut" : undefined);
    } finally {
      this.setProgress(null);
    }
  }

  /** Relay setup always runs with a valid token: signed out signs in first, inline; a token the relay refuses is refreshed once, then signed in again. */
  private async setUpRelay(host: HostLike, request: AppHostSetupRequest): Promise<HostStatus> {
    // No account wired (tests): the host decides.
    if (!this.accountToken) return this.setup(host, request, undefined);
    const token = await this.validToken(false);
    try {
      return await this.setup(host, request, token);
    } catch (error) {
      if (!isUnauthenticated(error)) throw error;
      // The relay refused it (expired early, revoked, clock skew).
      return this.setup(host, request, await this.validToken(true));
    }
  }

  /** The SDK's `Host.setup` with the request the core validated (`appHostSetup`'s mapping). */
  private setup(host: HostLike, request: AppHostSetupRequest, token: string | undefined): Promise<HostStatus> {
    const r = this.native.appHostValidateSetup(request);
    return host.setup(
      {
        mode: r.mode,
        relayUrl: r.relayUrl,
        direct: r.direct,
        name: r.name,
        allow: r.allow ?? [],
        driverBin: undefined,
        runner: undefined,
        profile: r.profile,
        shareDesktop: r.shareDesktop,
        provideSpaces: r.provideSpaces,
      },
      token,
    );
  }

  /** The account's token, signing in first when there is none. */
  private async validToken(force: boolean): Promise<string> {
    const token = await this.readToken(force);
    if (token) return token;
    if (!this.signIn) throw new HostSetupAuthError("signedOut");
    this.setProgress(HostModel.signInProgress);
    const signedIn = await this.signIn();
    this.setProgress(null);
    if (!signedIn) throw new HostSetupAuthError("signInNotFinished");
    const again = await this.readToken(false);
    if (again) return again;
    throw new HostSetupAuthError("signedOut");
  }

  private setProgress(progress: string | null): void {
    if (this.progress === progress) return;
    this.progress = progress;
    this.onChange?.();
  }

  // MARK: Page buttons

  /** A page button (`host.action`); a failure stays in `actionFailure` with Retry. */
  async run(id: AppHostActionId): Promise<void> {
    const A = this.native.AppHostActionId;
    // Sharing over the relay needs a signed-in owner: Sign In, and Resume
    // while paused or signed out, sign in first (inline).
    if (id === A.SignIn || (id === A.ResumeSharing && this.needsSignInToShare)) {
      if (this.busy) return;
      this.busy = true;
      this.actionFailure = null;
      try {
        await this.signInThenReconcile();
      } finally {
        this.busy = false;
      }
      return;
    }
    const host = this.host;
    if (!host || this.busy || id === A.SetUp) return;
    this.busy = true;
    this.error = null;
    try {
      await this.perform(id, host);
      this.actionFailure = null;
      this.failedAction = null;
    } catch (error) {
      this.actionFailure = presentHostFailure(words(error), this.platform);
      this.failedAction = id;
      // The page shows what the failure left behind, not the state before the button.
      await this.refresh();
    } finally {
      this.busy = false;
    }
  }

  /** Runs the failed button again. */
  async retryFailedAction(): Promise<void> {
    if (this.failedAction !== null) await this.run(this.failedAction);
  }

  /** Resume must sign in first: relay sharing paused while signed out, or relay mode with nobody signed in. */
  private get needsSignInToShare(): boolean {
    const state = this.state;
    if (!this.accountToken || !state || state.mode !== "relay") return false;
    return state.pausedSignedOut || (this.account === null && this.accountCheckedAt !== null);
  }

  private async perform(id: AppHostActionId, host: HostLike): Promise<void> {
    const A = this.native.AppHostActionId;
    // The two settings: the core says what each switch changes.
    const change = this.native.appHostSettingChange(id);
    if (change) {
      this.apply(await host.configure({ shareDesktop: change.shareDesktop, provideSpaces: change.provideSpaces, maxSpaces: undefined, maxMacosVms: undefined }));
      return;
    }
    switch (id) {
      case A.StopSharing:
        this.apply(await host.stopSharing());
        break;
      case A.ResumeSharing:
        this.apply(await host.startSharing());
        break;
      case A.Remove:
        await host.remove();
        this.apply(await host.status());
        break;
      default:
        break;
    }
  }

  /** Sign In on the paused page (and Resume while signed out): the inline sign-in, then sharing follows it. */
  private async signInThenReconcile(): Promise<boolean> {
    // Offline (the token could not be read) is not "signed out": no sign-in then.
    let signedOut: boolean;
    try {
      signedOut = (await this.readToken(false)) === null;
    } catch {
      signedOut = false;
    }
    if (signedOut) {
      if (!this.signIn) return false;
      this.setProgress(HostModel.signInProgress);
      const done = await this.signIn();
      this.setProgress(null);
      if (!done) return false;
    }
    await this.reconcileAccount();
    return true;
  }

  /**
   * Relay sharing needs a signed-in owner (the core's `account_step`):
   * nobody signed in, or another account, pauses it; the owner signing in
   * again resumes it. Not knowing (offline, the credential vault) changes
   * nothing. At launch, after a sign-in or sign-out, and every `olderThanMs`
   * from the app's refresh.
   */
  async reconcileAccount(olderThanMs = 0): Promise<void> {
    if (!this.host || !this.accountToken || this.reconciling) return;
    if (olderThanMs > 0 && this.accountCheckedAt !== null && Date.now() - this.accountCheckedAt < olderThanMs) return;
    this.reconciling = true;
    try {
      let signedIn: AppHostAccount | null;
      try {
        signedIn = (await this.readToken(false)) === null ? null : (this.currentAccount?.() ?? { display: this.identity ?? undefined });
      } catch {
        return;
      }
      this.accountCheckedAt = Date.now();
      this.account = signedIn;
      if (!this.state) await this.refresh();
      else this.state = { ...this.state, account: signedIn ?? undefined };
      if (!this.state) return;
      try {
        switch (this.native.appHostAccountStep(this.state, signedIn ?? undefined)) {
          case this.native.AppHostAccountStep.Pause:
            this.apply(await this.host.pauseSignedOut());
            this.actionFailure = null;
            break;
          case this.native.AppHostAccountStep.Resume:
            if (signedIn) this.apply(await this.host.resumeSignedIn(this.native.appHostAccountKey(signedIn)));
            this.actionFailure = null;
            break;
          default:
            break;
        }
      } catch (error) {
        this.error = words(error);
        this.actionFailure = presentHostFailure(words(error), this.platform);
        this.failedAction = this.native.AppHostActionId.ResumeSharing;
      }
    } finally {
      this.reconciling = false;
    }
  }
}
