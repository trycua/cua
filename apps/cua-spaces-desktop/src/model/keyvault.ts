// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Keyvault (the SwiftUI app's KeyvaultModel.swift): the broker's overview
// through the app core's `KeyvaultClient`, and the core's views of it. The
// app never reads secret values (the broker has none to give), never runs
// its own presence prompt (the daemon asks for it when access widens: Touch
// ID on a Mac, Windows Hello, the desktop's prompt on Linux), and approves
// only the items the user ticks. The SwiftUI list's own state (search,
// grouping, icons, the approval sheet) belongs to the web page here.
import type { Native } from "../native/load";
import type {
  AppSpace,
  KeyvaultClientLike,
  KeyvaultOverview,
  KvCommand,
  KvDeleteConfirm,
  KvListView,
  KvOutcome,
  KvPage,
  KvSelection,
  KvSidebar,
} from "../native/generated/index";
import { sdkErrorKind, words } from "./errors";

/** What the user answered to the unlock prompt. */
export type UnlockAnswer = "deny" | "allow" | "neverAskAgain";

/** Items waiting for the user's confirmation to delete. */
export interface PendingDelete {
  ids: string[];
  confirm: KvDeleteConfirm;
}

/** What a secure credential form hands back: the passphrase (and its confirmation at setup), or null when cancelled. */
export type CredentialAnswer = { passphrase: string; confirm: string } | null;

/** The broker's sentence, without the SDK error kind in front ("invalid argument: ..."). */
export function brokerWords(error: unknown): string {
  const text = words(error);
  if (sdkErrorKind(error) === null) return text;
  const colon = text.indexOf(": ");
  if (colon < 0 || !/^[a-z ]*$/.test(text.slice(0, colon))) return text;
  return text.slice(colon + 2);
}

/** Two overviews are the same answer (the broker is asked again on every change; only a different answer is one). */
const sameOverview = (a: KeyvaultOverview, b: KeyvaultOverview) => JSON.stringify(a, replacer) === JSON.stringify(b, replacer);
const replacer = (_: string, v: unknown) => (typeof v === "bigint" ? v.toString() : v);

/** The overview of a broker that cannot be reached. */
export function unavailableOverview(): KeyvaultOverview {
  return {
    availability: "not_running",
    message: "The Cua daemon is not running, so the Keyvault is unavailable.",
    status: undefined,
    serverVerified: false,
    items: [],
    namesVisible: false,
    itemsTotal: 0,
    pending: [],
    grants: [],
    rules: [],
    deliveries: [],
    audit: [],
    auditVerification: undefined,
    partialErrors: [],
  };
}

export class KeyvaultModel {
  overview: KeyvaultOverview;
  /** The sidebar's pick (the bridge answers the list of the "All" category unless told). */
  selection: KvSelection;
  /** The delete waiting for the user's answer (the SwiftUI sheet's state). */
  deleteConfirm: PendingDelete | null = null;
  busy = false;
  error: string | null = null;
  /** Shown once: never again after the vault locks. */
  recoveryKey: string | null = null;
  /** Copies (import ids) the user dismissed from the notch. Dismiss hides the indicator and the tiles' key; it revokes and wipes nothing. */
  dismissed: string[] = [];
  /** The Access row to bring forward (a Space's "Signed in" badge). */
  focusKey: string | null = null;
  /** Told the sharing label after every refresh (and after a dismissal). */
  onSharing: ((label: string | null) => void) | null = null;
  /** Told the dismissed copies when they change (the app saves them). */
  onDismissed: ((ids: string[]) => void) | null = null;
  private readonly listeners = new Set<() => void>();
  private poll: NodeJS.Timeout | null = null;

  constructor(
    private readonly native: Native,
    readonly client: KeyvaultClientLike | null,
    private readonly clock: () => number = Date.now,
    overview?: KeyvaultOverview,
  ) {
    this.overview = overview ?? unavailableOverview();
    this.selection = new native.KvSelection.Category({ category: native.KvCategory.All });
  }

  /** Called when the overview, the busy flag or the dismissed copies change; returns the unsubscribe. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private changed(): void {
    for (const l of [...this.listeners]) l();
  }

  get nowMs(): bigint {
    return BigInt(this.clock());
  }

  get page(): KvPage {
    return this.native.kvPage(this.overview, this.nowMs);
  }

  get sidebar(): KvSidebar {
    return this.native.kvSidebar(this.overview, this.nowMs);
  }

  get list(): KvListView {
    return this.native.kvList(this.overview, this.selection, this.nowMs);
  }

  /** The always-visible signal while sign-ins are live in a Space (null when nothing is): the notch indicator and the menu bar line. */
  get sharingLabel(): string | null {
    return this.native.kvSharingLabel(this.overview, this.nowMs) ?? null;
  }

  /** The notch's indicator: the sharing label without dismissed copies. */
  get notchLabel(): string | null {
    return this.native.kvVisibleSharingLabel(this.overview, this.nowMs, this.dismissed) ?? null;
  }

  /** The ids of `spaces` signed in through the Keyvault: all of them (`notch: false`, the Spaces list), or less the dismissed copies. */
  signedIn(spaces: AppSpace[], notch = false): string[] {
    return this.native.kvSignedInSpaces(this.overview, this.nowMs, notch ? this.dismissed : [], spaces);
  }

  /** The Access row of `space`'s copies. */
  accessKey(space: AppSpace): string | null {
    return this.native.kvSpaceAccessKey(this.overview, this.nowMs, space) ?? null;
  }

  /** Hides `imports` (or every live copy) from the notch. */
  dismiss(imports?: string[]): void {
    const ids = imports ?? this.overview.deliveries.map((d) => d.importId);
    const next = [...this.dismissed, ...ids.filter((id) => !this.dismissed.includes(id))];
    if (next.length === this.dismissed.length) return;
    this.dismissed = next;
    this.onDismissed?.(next);
    this.onSharing?.(this.sharingLabel);
    this.changed();
  }

  /** "Never ask again" is on (Settings turns it back off); null before the broker says. */
  get unlockPromptShows(): boolean | null {
    const skip = this.overview.status?.skipUnlockPrompt;
    return skip === undefined ? null : !skip;
  }

  /** Auto-wipe of access given to Spaces (off by default; turning it off makes the daemon ask for presence). */
  get autoWipe(): boolean | null {
    return this.overview.status?.autoWipe ?? null;
  }

  setSkipUnlockPrompt = (on: boolean) => this.run(new this.native.KvCommand.SetSkipUnlockPrompt({ on }));
  setAutoWipe = (on: boolean) => this.run(new this.native.KvCommand.SetAutoWipe({ on }));
  setDisabled = (disabled: boolean) => this.run(new this.native.KvCommand.SetDisabled({ disabled }));
  /** Shows the items' names (the daemon asks for presence). */
  showItems = () => this.run(new this.native.KvCommand.Browse());

  // MARK: Locks (one prompt and one presence check for a batch)

  /** The core's unlock prompt for `count` items, or null when the user chose Never ask again. */
  unlockPrompt(count: number, name?: string) {
    return this.native.kvUnlockPrompt(this.overview, count, name) ?? null;
  }

  unlockCommand(ids: string[]): KvCommand {
    return this.native.kvUnlockCommand(ids);
  }

  /** The prompt's answers: Allow, Never ask again (stored in the vault, revertible in Settings), Deny. */
  async answerUnlock(ids: string[], answer: UnlockAnswer): Promise<void> {
    if (answer === "deny") return;
    if (answer === "neverAskAgain") await this.setSkipUnlockPrompt(true);
    await this.run(this.unlockCommand(ids));
  }

  /** Locks items (narrowing: no prompt, no presence check). */
  async lock(ids: string[]): Promise<void> {
    if (ids.length === 0) return;
    await this.run(this.native.kvLockCommand(ids));
  }

  /** A delete asks first, in the core's words (naming the Spaces whose copies are wiped too). */
  requestDelete(ids: string[]): void {
    if (ids.length === 0) return;
    const copies = this.native.kvLiveCopySpaces(this.overview, ids, this.nowMs);
    this.deleteConfirm = { ids, confirm: this.native.kvDeleteConfirm(ids.length, copies) };
  }

  /** Deletes the items and wipes their live copies in Spaces. */
  async confirmDelete(): Promise<void> {
    const pending = this.deleteConfirm;
    if (!pending) return;
    this.deleteConfirm = null;
    await this.run(this.native.kvDeleteCommand(pending.ids));
  }

  async deny(requestId: string): Promise<void> {
    await this.run(this.native.kvApprovalDenyCommand(this.native.kvApprovalOpen(requestId)));
  }

  // MARK: Refresh

  /** Re-reads the broker (never throws: unavailability is a page state). */
  async refresh(): Promise<void> {
    const client = this.client;
    if (!client) return;
    // Only a different answer is a change: the page asks again on every
    // change, and each ask reads the broker.
    const next = await client.overview();
    let moved = false;
    if (!sameOverview(next, this.overview)) {
      this.overview = next;
      moved = true;
    }
    // Forget dismissals of copies that were wiped or expired (only when the
    // broker answered: an unavailable page lists none).
    if (this.overview.availability === "ready") {
      const live = this.native.kvPruneDismissed(this.overview, this.nowMs, this.dismissed);
      if (live.join("\n") !== this.dismissed.join("\n")) {
        this.dismissed = live;
        this.onDismissed?.(live);
        moved = true;
      }
    }
    this.onSharing?.(this.sharingLabel);
    // The recovery key is shown once: never again after the vault locks.
    if (this.overview.availability === "locked") this.recoveryKey = null;
    if (moved) this.changed();
  }

  /** Reads the broker every `ms` while the app runs: the notch indicator and the menu line follow live deliveries, so access is never silent. Returns the stop. */
  startPoll(ms = 10_000): () => void {
    this.poll ??= setInterval(() => void this.refresh().catch((error) => console.warn(`[cua-spaces] Keyvault refresh: ${words(error)}`)), ms);
    this.poll.unref?.();
    return () => {
      if (this.poll) clearInterval(this.poll);
      this.poll = null;
    };
  }

  // MARK: Setup and unlock (the OS key store, or a passphrase, as the daemon allows)

  /**
   * Sets up or unlocks the vault the way the form offers. The OS key store
   * needs nothing typed (the daemon asks for presence); a passphrase is
   * asked for through `ask` (a secure form of this app, never the page) and
   * goes only to the broker over the verified Keyvault socket.
   */
  async submitCredential(ask: (form: NonNullable<KvPage["form"]>) => Promise<CredentialAnswer>): Promise<void> {
    const client = this.client;
    const form = this.page.form;
    if (!client || !form || this.busy) return;
    const { KvFormMode, KvMethod, KvCommand, KvOutcome_Tags } = this.native;
    let secret = "";
    if (form.method === KvMethod.Passphrase) {
      const answer = await ask(form);
      if (!answer) return;
      const check = this.native.kvPassphraseCheck(form.mode, answer.passphrase, answer.confirm);
      if (!check.canSubmit) {
        this.error = check.hint ?? "Check the passphrase.";
        this.changed();
        return;
      }
      secret = answer.passphrase;
    }
    this.busy = true;
    this.error = null;
    this.changed();
    try {
      if (form.method === KvMethod.TouchId) {
        const outcome = await client.execute(form.mode === KvFormMode.Setup ? new KvCommand.Setup() : new KvCommand.Unlock());
        if (form.mode === KvFormMode.Setup && outcome.tag === KvOutcome_Tags.RecoveryKey) this.recoveryKey = outcome.inner.key ?? null;
      } else if (form.mode === KvFormMode.Setup) {
        this.recoveryKey = (await client.setupWithPassphrase(secret)) ?? null;
      } else {
        await client.unlockWithPassphrase(secret);
      }
    } catch (error) {
      this.error = brokerWords(error);
    }
    secret = "";
    this.busy = false;
    await this.refresh();
    this.changed();
  }

  // MARK: Actions (each one broker request)

  /** Sends one command, then re-reads the broker. Returns what the broker answered (null when it failed: `error` says why, or with no client). */
  async run(command: KvCommand): Promise<KvOutcome | null> {
    const client = this.client;
    if (!client) return null;
    this.busy = true;
    this.error = null;
    this.changed();
    let result: KvOutcome | null = null;
    try {
      const outcome = await client.execute(command);
      if (outcome.tag === this.native.KvOutcome_Tags.RecoveryKey) this.recoveryKey = outcome.inner.key ?? null;
      result = outcome;
    } catch (error) {
      this.error = brokerWords(error);
    }
    this.busy = false;
    await this.refresh();
    this.changed();
    return result;
  }
}
