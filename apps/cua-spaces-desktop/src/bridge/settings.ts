// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings (`settings.get`, `settings.choose`): the core's Settings page
// (`appSettingsPage`, with Storage after General while Cua Volume is on)
// and Experiments, and what a row's choice does (the SwiftUI host's
// `settings()` and `AppModel.choose` / `press`): the notch or the menu bar,
// where new Spaces start, auto-connect, the built-in Lume and gVisor
// runtimes, launch at login, usage data, the update channel, the
// experiments, Welcome's "Show again", the coding agents (`model.agents`)
// and the Keyvault's switches (`model.keyvault`).
import type { AppLoginItemStatus, AppSettingsInput, AppTeamInput, AppTelemetryInput } from "../native/generated/index";
import type { AppModel } from "../model/app-model";
import { words } from "../model/errors";
import { bounded } from "../model/time";
import { string } from "./args";
import { consentMayHaveChanged } from "../crash-reporting";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { hostParts, type HostParts } from "./host-parts";
import { encode } from "./value";
import { cliSetting } from "./cli";

function telemetryInput(model: AppModel): AppTelemetryInput | undefined {
  try {
    return model.telemetry?.status();
  } catch {
    return undefined;
  }
}

/** Settings, General (and Storage while Cua Volume is on). */
export function settingsPage(model: AppModel, parts: HostParts) {
  // The coding agents' rows (Settings, AI agents).
  const coding = model.agents.coding;
  const login = parts.loginItem;
  const input: AppSettingsInput = {
    identity: model.identity ?? undefined,
    apiKeyClient: undefined,
    signIn: model.signIn,
    canSignOut: model.account !== null && model.identity !== null,
    menuBar: model.settings.menuBar,
    telemetry: telemetryInput(model),
    agents: coding.rows ?? undefined,
    agentsBusy: coding.busy,
    agentsPending: coding.pending,
    // Teams from the Spaces API (read by `settings.get`).
    teams: model.teams.input(),
    loginItem:
      login.status === null
        ? undefined
        : {
            status: login.status as AppLoginItemStatus,
            busy: login.busy,
            error: login.error ?? undefined,
            providesSpaces: Boolean(model.host.state?.configured && model.host.state.provideSpaces),
            runsAgents: login.runsAgents,
          },
    experiments: model.settings.experiments,
    keyvaultAutoWipe: model.keyvault.autoWipe ?? undefined,
    keyvaultUnlockPrompt: model.keyvault.unlockPromptShows ?? undefined,
    keyvaultSiteIcons: model.settings.keyvaultSiteIcons,
    keyvaultProtection: model.keyvault.page.protection,
    autoConnect: model.settings.autoConnect,
    // Settings, Advanced: every kind of Space's runtime row ("Checking…" until read).
    runtimes: model.runtimesInput(),
    // Settings, General's Show tips (the tips themselves live in `AppSettings.tips`).
    showTips: !model.settings.tips.off,
  };
  const page = model.native.appSettingsPage(input);
  return model.native.appSettingsWithStorage(page, parts.storage.section, model.settings.experiments);
}

/** The SwiftUI app's New UI switch ("Use classic UI", and the older `experiment:web_ui`): this app is the web UI, so it (and its note) is not offered here. */
export const WEB_UI_ROWS = new Set(["experiment:classic_ui", "experiment:classic_ui-note", "experiment:web_ui", "experiment:web_ui-note"]);

export function experimentsPage(model: AppModel) {
  const page = model.native.appExperimentsPage(model.settings.experiments);
  return { ...page, sections: page.sections.map((s) => ({ ...s, rows: s.rows.filter((r) => !WEB_UI_ROWS.has(r.id)) })) };
}

/** `settings.get`'s answer: General, Experiments, the update channel (null:
 * no updater), the global shortcut (null: none here) and the account app. */
export function settingsState(model: AppModel, parts: HostParts) {
  return {
    page: encode(settingsPage(model, parts)),
    experiments: encode(experimentsPage(model)),
    updateChannel: parts.updates.updater === null ? null : parts.updates.channelWord,
    globalShortcut: parts.system?.globalShortcut?.() ?? null,
    // The account app (CUA_ENV, CUA_ACCOUNT_APP_URL): the page's core has no
    // environment, so Upgrade's billing fallback names this one.
    accountUrl: model.native.appAccountAppUrl(),
    // The CLI this app installs and whether it is here (`cli.ts`).
    cli: cliSetting(model),
    // Settings, Advanced's runtimes as read (`runtimes::RuntimesInput`): the page's core lays out every kind's row.
    runtimes: encode(model.runtimesInput()),
    // The app core's own settings the page changes (`settings.update`).
    tips: { off: model.settings.tips.off, done: [...model.settings.tips.done] },
  };
}

/** `settings.update`'s fields, as the page sends them (each optional). */
export function settingsUpdate(args: Record<string, unknown>): Partial<AppModel["settings"]> {
  const patch: Partial<AppModel["settings"]> = {};
  if ("tips" in args) {
    const t = args.tips;
    if (!t || typeof t !== "object" || Array.isArray(t)) throw Failure.badArgs("tips: {off, done}");
    const { off, done } = t as { off?: unknown; done?: unknown };
    if (typeof off !== "boolean" || !Array.isArray(done) || !done.every((d) => typeof d === "string")) throw Failure.badArgs("tips: {off: boolean, done: string[]}");
    patch.tips = { off, done: done as string[] };
  }
  if (!Object.keys(patch).length) throw Failure.badArgs("tips");
  return patch;
}

/** A switch in Settings, Experiments: saved and recorded. Off hides; nothing is undone. */
export function chooseExperiment(model: AppModel, row: string, option: string): void {
  const before = model.settings.experiments;
  const after = model.native.appExperimentsChoose(before, row, option);
  const signals = model.native.appTelemetryExperimentsChanged(before, after);
  if (!signals.length) return;
  model.settings = { ...model.settings, experiments: after };
  model.saveSettings();
  model.telemetry?.record(signals);
}

export function settingsMethods(ctx: BridgeContext): Handlers {
  const { model } = ctx;
  const parts = () => hostParts(ctx);
  const save = (patch: Partial<AppModel["settings"]>) => {
    model.settings = { ...model.settings, ...patch };
    model.saveSettings();
  };

  /** What Settings shows that the host reads (the Swift app's `loadSettings`): the runtimes, the login item. */
  const load = async () => {
    parts().loginItem.read();
    if (!model.servicesIn) return;
    // Teams: the account's organizations (a failure shows in its row).
    await bounded(10, () => model.teams.reload());
    if (model.agents.coding.rows === null) await bounded(10, () => model.agents.coding.reload());
    const [lume, linux] = await Promise.all([bounded(5, () => model.backend.lumeSource()), bounded(5, () => model.backend.linuxSource())]);
    if (lume !== null) model.lumeSource = lume;
    if (linux !== null) model.linuxSource = linux;
    // What is installed for each kind and the doctor: in the background, the page hears `settings.changed`.
    refreshRuntimes();
  };

  /** Reads the runtimes again unless a read is recent or running. */
  let runtimesReadAt = 0;
  let runtimesReading = false;
  const refreshRuntimes = (force = false) => {
    if (!model.servicesIn || runtimesReading || (!force && Date.now() - runtimesReadAt < 15_000)) return;
    runtimesReading = true;
    void model
      .readRuntimes()
      .then(() => {
        runtimesReadAt = Date.now();
        model.changed("settings");
      })
      .catch(() => {})
      .finally(() => {
        runtimesReading = false;
      });
  };

  /** A choice row changed (`AppModel.choose`). */
  const choose = async (row: string, option: string): Promise<void> => {
    // Settings, Advanced's runtime rows: the setting the core names, written; the runtimes read again.
    const change = row.startsWith("runtime") ? model.native.appRuntimesChoose(model.runtimesInput(), row, option) : undefined;
    if (change) {
      await model.backend.setRuntimeSetting(change.key, change.value ?? null);
      await bounded(15, () => model.readRuntimes());
      const [lume, linux] = await Promise.all([model.backend.lumeSource(), model.backend.linuxSource()]);
      model.lumeSource = lume;
      model.linuxSource = linux;
      model.changed("settings");
      return;
    }
    switch (row) {
      case "team-current":
        // Settings, Teams: the team to work in (saved; the option is its id).
        save({ currentTeamId: option });
        return;
      case "notch":
        // "Spaces tab in the notch" (shown) or the menu bar only (hide); the notch follows `settings.changed`.
        save({ menuBar: option === "hide" });
        return;
      case "auto-connect":
        save({ autoConnect: option === "on" });
        return;
      case "global-shortcut": {
        // The keys that bring the app forward from any app (Mac). The option is the accelerator.
        const system = parts().system;
        if (!system?.setGlobalShortcut) throw Failure.failed("This computer has no shortcut to change.");
        try {
          system.setGlobalShortcut(option);
        } catch (e) {
          throw Failure.failed(e instanceof Error ? e.message : String(e));
        }
        model.changed("settings");
        return;
      }
      case "launch-at-login": {
        const login = parts().loginItem;
        login.set(option === "on");
        model.changed("settings");
        if (login.error) throw Failure.failed(login.error);
        return;
      }
      case "keyvault-auto-wipe":
        // Off asks for presence in the daemon.
        await model.keyvault.setAutoWipe(option === "on");
        if (model.keyvault.error) throw Failure.failed(model.keyvault.error);
        return;
      case "keyvault-unlock-prompt":
        // On shows the prompt; off is the stored "Never ask again".
        await model.keyvault.setSkipUnlockPrompt(option !== "on");
        if (model.keyvault.error) throw Failure.failed(model.keyvault.error);
        return;
      case "keyvault-site-icons":
        save({ keyvaultSiteIcons: option === "on" });
        return;
      // General's Show tips and Reset tips: the core's (`settings::choose`).
      case "tips":
      case "reset-tips":
        model.settings = model.native.appSettingsChoose(model.settings, row, option);
        model.saveSettings();
        return;
      case "telemetry":
        model.telemetry?.setEnabled(option === "on");
        // Off also stops crash reports (crash-reporting.ts).
        consentMayHaveChanged();
        // The switch sits beside the usage note (Welcome, Settings): the
        // notice was seen.
        model.telemetry?.acknowledgeNotice();
        model.changed("settings");
        return;
      default:
        return;
    }
  };

  return {
    // "Add a machine" and "Share": the account's Teams (Settings, Teams) and the current one.
    "teams.list": async () => {
      if (model.servicesIn) await bounded(10, () => model.teams.reload());
      const input = model.teams.input();
      const state = input.state as { tag: string; inner?: { teams?: AppTeamInput[] } };
      const teams = state.tag === "Ready" ? (state.inner?.teams ?? []) : [];
      return {
        teams: teams.map((t) => ({ id: t.id, alias: t.alias, name: t.name, role: t.role, plan: t.plan })),
        currentTeamId: input.currentTeamId ?? null,
      };
    },
    "settings.get": async () => {
      await load();
      return settingsState(model, parts());
    },
    "settings.choose": async (args) => {
      const row = string(args, "row");
      // An empty option only for a runtime path ("Reset to detected").
      const option = row.startsWith("runtime-path:") && args.option === "" ? "" : string(args, "option");
      if (row === "update-channel") {
        // Settings → About's channel (saved in `AppSettings`).
        const updates = parts().updates;
        if (!updates.updater) throw Failure.unsupported("Updates are off in this build");
        updates.choose(option);
      } else if (row.startsWith("experiment:")) {
        if (WEB_UI_ROWS.has(row)) throw Failure.unsupported("This app is the new UI");
        chooseExperiment(model, row, option);
      } else if (row.startsWith("agent:")) {
        // An AI agents row: configure it, or remove what cua added.
        await model.agents.coding.press(row);
      } else if (row === "welcome") {
        // General's "Show again": the first run is due again.
        parts().onboarding.restart();
      } else {
        try {
          await choose(row, option);
        } catch (error) {
          if (error instanceof Failure) throw error;
          throw Failure.failed(words(error));
        }
      }
      return settingsState(model, parts());
    },
    // Settings, Advanced: what a typed path is, before it is saved.
    "runtimes.check": async (args) => {
      try {
        return encode(await model.backend.runtimeCheckPath(string(args, "kind"), string(args, "path")));
      } catch (error) {
        throw Failure.failed(words(error));
      }
    },
    // Settings, Advanced's "Browse…".
    "runtimes.browse": async (args, caller) => {
      string(args, "kind");
      if (!ctx.ui.chooseProgram) throw Failure.unsupported("This build has no file picker");
      return { path: (await ctx.ui.chooseProgram(caller.window)) ?? null };
    },
    // The page's tips (`AppSettings.tips`), saved as given.
    "settings.update": (args) => {
      save(settingsUpdate(args));
      return settingsState(model, parts());
    },
  };
}
