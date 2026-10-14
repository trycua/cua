// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Keyvault (`keyvault.*`): the broker's overview and the core's page
// views, unlock and lock, approvals, grants and the Access rows (the SwiftUI
// host's `KeyvaultModel` and its bridge methods). The page never gets a
// secret value or a passphrase: the broker has no value to give, and a
// passphrase is typed in a window of this app (`ui.askPassphrase`).
import type { Native } from "../native/load";
import type { KeyvaultOverview, KvCommand } from "../native/generated/index";
import type { AppModel } from "../model/app-model";
import { object, string, strings } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Caller, type Handlers } from "./host";
import { encode } from "./value";

/** The broker's redacted overview, for the web UI's own vault list (the core's `list` view does not carry the items). Items lose `blob`, so nothing but names and policy crosses the bridge. */
export function redactedOverview(overview: KeyvaultOverview): unknown {
  const out = encode(overview) as Record<string, unknown> | null;
  if (!out) return null;
  if (Array.isArray(out.items)) {
    out.items = out.items.map((item) => {
      const { blob: _blob, ...rest } = item as Record<string, unknown>;
      return rest;
    });
  }
  return out;
}

/** What the page shows of the Keyvault (`keyvault.get`, and every action's answer). */
export function keyvaultState(model: AppModel) {
  const kv = model.keyvault;
  return {
    availability: kv.overview.availability,
    page: encode(kv.page),
    sidebar: encode(kv.sidebar),
    list: encode(kv.list),
    overview: redactedOverview(kv.overview),
    dismissed: kv.dismissed,
    busy: kv.busy,
    error: kv.error,
  };
}

export function keyvaultMethods({ model, ui, platform }: BridgeContext): Handlers {
  const kv = model.keyvault;
  const native: Native = model.native;

  /**
   * The daemon asks for presence (Windows Hello, the desktop's password
   * prompt) in a dialog of the system's, which shows over the foreground
   * window: bring the app there first, as the SwiftUI app does for its
   * keychain prompts. (Touch ID's sheet is the system's on a Mac and needs no
   * help.)
   */
  const foreground = () => {
    if (platform !== "darwin") ui.activate();
  };

  const failure = (fallback: string) => Failure.failed(kv.error ?? fallback);

  /** The Keyvault after an action, or the action's error. */
  const result = () => {
    if (kv.error) throw Failure.failed(kv.error);
    return keyvaultState(model);
  };

  /** `requestId`, which must be waiting in the broker's overview. */
  const pendingRequest = (args: BridgeArgs) => {
    const id = string(args, "requestId");
    if (!kv.overview.pending.some((p) => p.id === id)) throw Failure.notFound(`no waiting request ${id}`);
    return id;
  };

  /**
   * The core's unlock prompt (unless the user chose Never ask again) as a
   * native alert, then the same unlock command the SwiftUI list sends; the
   * daemon asks for presence once for the batch.
   */
  const unlockItems = async (ids: string[], name: string | undefined, caller: Caller) => {
    if (ids.length === 0) throw Failure.badArgs("ids: non-empty");
    const prompt = kv.unlockPrompt(ids.length, name);
    if (!prompt) {
      foreground();
      await kv.run(kv.unlockCommand(ids));
      return;
    }
    const answer = (await ui.askUnlock?.(caller.window, prompt)) ?? "deny";
    if (answer === "deny") throw new Failure("cancelled", "Unlock cancelled");
    foreground();
    await kv.answerUnlock(ids, answer);
  };

  /** Delete: the core's confirmation (it says when copies in Spaces are wiped too) as a native alert, then the same delete the SwiftUI list sends. Declined: `cancelled`, nothing changes. */
  const deleteItems = async (ids: string[], caller: Caller) => {
    if (ids.length === 0) throw Failure.badArgs("ids: non-empty");
    kv.requestDelete(ids);
    const pending = kv.deleteConfirm;
    if (!pending) throw Failure.badArgs("ids: none of these items");
    // The page's alert replaces the sheet the Keyvault list would show.
    kv.deleteConfirm = null;
    const confirmed = (await ui.askDelete?.(caller.window, pending.confirm)) ?? false;
    if (!confirmed) throw new Failure("cancelled", "Delete cancelled");
    kv.deleteConfirm = pending;
    await kv.confirmDelete();
  };

  /** The commands an Access row sends: revoke a grant, remove a rule, wipe a Space's copies. */
  const accessCommand = (c: BridgeArgs): KvCommand => {
    switch (c.type) {
      case "revoke-grant":
        return new native.KvCommand.RevokeGrant({ id: string(c, "id") });
      case "remove-rule":
        return new native.KvCommand.RemoveRule({ id: string(c, "id") });
      case "release":
        return new native.KvCommand.Release({ target: string(c, "target") });
      default:
        throw Failure.badArgs("command: revoke-grant, remove-rule or release");
    }
  };

  /** Sets up or unlocks the way the form offers; a passphrase is asked for in this app's own form, and `cancelled` says the user closed it. */
  const credential = async (caller: Caller, cancelled: string): Promise<void> => {
    let asked = true;
    foreground();
    await kv.submitCredential(async (form) => {
      const answer = (await ui.askPassphrase?.(caller.window, form)) ?? null;
      asked = answer !== null;
      return answer;
    });
    if (!asked) throw new Failure("cancelled", cancelled);
  };

  /**
   * Unlocks (or sets up) the vault: the OS key store confirms with the
   * daemon's presence prompt; a passphrase vault asks in this app's form (a
   * passphrase never crosses the bridge).
   */
  const unlockVault = async (caller: Caller) => {
    const form = kv.page.form;
    if (!form) return;
    if (form.method !== native.KvMethod.TouchId && !ui.askPassphrase) {
      throw new Failure("native_only", "Enter the passphrase in the Cua Spaces window");
    }
    await credential(caller, "Unlock cancelled");
  };

  /** "Set up Keyvault" on the web page: the core's setup form. Answers the recovery key to show once. */
  const setUpVault = async (caller: Caller): Promise<string | null> => {
    const form = kv.page.form;
    if (!form || form.mode !== native.KvFormMode.Setup) throw Failure.failed("Keyvault is already set up");
    if (form.method !== native.KvMethod.TouchId && !ui.askPassphrase) {
      throw new Failure("native_only", "Choose a passphrase in the Cua Spaces window");
    }
    kv.recoveryKey = null;
    await credential(caller, "Set up cancelled");
    if (kv.error) throw Failure.failed(kv.error);
    return kv.recoveryKey;
  };

  return {
    // The broker, read again (the page asks when it shows the Keyvault and
    // after `keyvault.changed`); not while launching.
    "keyvault.get": async () => {
      if (model.servicesIn) await kv.refresh();
      return keyvaultState(model);
    },
    "keyvault.lock": async (args) => {
      await kv.lock(strings(args, "ids"));
      return result();
    },
    "keyvault.unlock": async (args, caller) => {
      await unlockItems(strings(args, "ids"), typeof args.name === "string" ? args.name : undefined, caller);
      return result();
    },
    "keyvault.unlockVault": async (_args, caller) => {
      await unlockVault(caller);
      return result();
    },
    "keyvault.setup": async (_args, caller) => ({ recoveryKey: await setUpVault(caller) }),
    "keyvault.setDisabled": async (args) => {
      if (typeof args.disabled !== "boolean") throw Failure.badArgs("disabled: boolean");
      foreground();
      await kv.setDisabled(args.disabled);
      return result();
    },
    // The daemon asks for presence; the page sends the item ids the user
    // ticked (null: all, as asked).
    "keyvault.approve": async (args) => {
      const requestId = pendingRequest(args);
      const items = args.items === undefined || args.items === null ? null : strings(args, "items");
      foreground();
      const outcome = await kv.run(new native.KvCommand.Approve({ requestId, items: items ?? undefined }));
      if (outcome?.tag !== native.KvOutcome_Tags.Granted) throw failure("Approve did not complete");
      return encode(outcome.inner.grant);
    },
    "keyvault.deny": async (args) => {
      await kv.deny(pendingRequest(args));
      if (kv.error) throw Failure.failed(kv.error);
      return null;
    },
    "keyvault.revokeGrant": async (args) => {
      const outcome = await kv.run(new native.KvCommand.RevokeGrant({ id: string(args, "id") }));
      if (outcome?.tag !== native.KvOutcome_Tags.Revoked) throw failure("Revoke did not complete");
      return outcome.inner.count;
    },
    // The names, behind the daemon's presence check (the list's "Show Items").
    "keyvault.showItems": async () => {
      foreground();
      await kv.showItems();
      return result();
    },
    "keyvault.delete": async (args, caller) => {
      await deleteItems(strings(args, "ids"), caller);
      return result();
    },
    // An Access row's own command; nothing else runs from the page.
    "keyvault.run": async (args) => {
      await kv.run(accessCommand(object(args, "command")));
      return result();
    },
    "keyvault.dismiss": (args) => {
      kv.dismiss(strings(args, "imports"));
      return keyvaultState(model);
    },
  };
}
