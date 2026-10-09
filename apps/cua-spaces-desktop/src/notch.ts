// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The macOS notch: "Cua Spaces Notch.app" (libs/spaces-notch-swift, the
// SwiftUI app's own notch views), bundled in Contents/Helpers and launched by
// this app. The app core's notch model runs here (src/notch/model.ts, the
// SwiftUI app's NotchModel and NotchController over the same cua-spaces-ffi
// calls); the helper draws what it is sent and sends input and clicks back
// over its stdin and stdout (src/notch/protocol.ts). Nothing on Windows or
// Linux, or on a Mac older than the SwiftUI app's minimum (macOS 26).

import { existsSync } from "node:fs";
import { release } from "node:os";
import * as path from "node:path";
import { NotchModel, type NotchHost, type NotchModelOptions } from "./notch/model";
import { NotchProcess, type NotchProcessOptions } from "./notch/process";
import { NOTCH_PROTOCOL_VERSION, type HelperMessage, type HostMessage } from "./notch/protocol";

export type { NotchHost, NotchActions, NotchTeleport, WindowDragEvent } from "./notch/model";
export type { NotchCore } from "./notch/core";

/** The helper's bundle and executable name. */
export const HELPER_APP = "Cua Spaces Notch.app";
export const HELPER_EXECUTABLE = "Cua Spaces Notch";
/** Darwin 25 is macOS 26, the SwiftUI app's (and the helper's) minimum. */
export const MIN_DARWIN = 25;

/** Whether this Mac runs the notch. */
export function notchSupported(platform: NodeJS.Platform, osRelease: string): boolean {
  return platform === "darwin" && Number.parseInt(osRelease, 10) >= MIN_DARWIN;
}

/**
 * The helper's executable: Contents/Helpers in the packaged app (next to
 * Contents/Resources), `native/notch` (`pnpm notch`) in development.
 */
export function helperPath(o: { packaged: boolean; resourcesPath: string; appPath: string }): string {
  const bundle = o.packaged
    ? path.join(o.resourcesPath, "..", "Helpers", HELPER_APP)
    : path.join(o.appPath, "native", "notch", HELPER_APP);
  return path.join(bundle, "Contents", "MacOS", HELPER_EXECUTABLE);
}

/** The running notch: the app model feeds it. */
export interface Notch {
  /** The Spaces (the core's `AppSpace` records, as the roster has them). */
  setSpaces(spaces: readonly unknown[]): void;
  /** Live Keyvault sign-ins: the core's sharing label less dismissed copies, and the Space ids that carry the key. */
  setKeyvault(label: string | null | undefined, signedIn?: string[]): void;
  /** The hotspot, or a transfer (bytes; both unknown while one starts). */
  setActivity(hotspot: boolean, transfer?: { sent?: number | bigint; total?: number | bigint } | null): void;
  /** The "Spaces tab in the notch" setting (`!settings.menuBar`). */
  setShown(shown: boolean): void;
  /** The app became active: check the window-drag permission again. */
  recheckPermission(): void;
  /** Quits the helper. */
  stop(): void;
}

export interface StartNotchOptions extends NotchModelOptions {
  /** The helper's executable (`helperPath`). */
  path: string;
  /** Logs every protocol message both ways (`CUA_SPACES_NOTCH_DEBUG`; images left out). */
  trace?: (line: string) => void;
  /** Process options for tests (spawn, timers, backoff). */
  process?: Partial<Omit<NotchProcessOptions, "path" | "onMessage" | "onStart">>;
}

/** Starts the helper and the model behind it. */
export function startNotch(host: NotchHost, options: StartNotchOptions): Notch {
  const log = options.log ?? ((m: string) => console.warn(`[cua-spaces] notch: ${m}`));
  // The last thumbnail per Space, base64, for a restarted helper.
  const thumbnails = new Map<string, string>();
  let helper: NotchProcess | undefined;
  const trace = options.trace;
  const send = (m: HostMessage) => {
    trace?.(`-> ${describe(m)}`);
    helper?.send(m);
  };
  const model = new NotchModel(
    host,
    {
      state: (message) => send(message),
      thumbnail: (id, image) => {
        const b64 = image ? Buffer.from(image).toString("base64") : undefined;
        if (b64) thumbnails.set(id, b64);
        else thumbnails.delete(id);
        send({ type: "thumbnail", id, image: b64 });
      },
      ghost: (image) => send({ type: "ghost", image: image ? Buffer.from(image).toString("base64") : undefined }),
    },
    { ...options, log },
  );

  const onMessage = (m: HelperMessage) => {
    trace?.(`<- ${describe(m)}`);
    switch (m.type) {
      case "hello":
        if (m.v !== NOTCH_PROTOCOL_VERSION) log(`the helper speaks notch protocol ${m.v}, this app ${NOTCH_PROTOCOL_VERSION}`);
        return;
      case "screens":
        return model.screens(m.notch, m.primary);
      case "event":
        return model.input(m.event);
      case "stage":
        return model.stageChanged(m.open);
      case "error":
        return log(`helper: ${m.message}`);
      case "action":
        switch (m.action) {
          case "openSpace":
            return host.actions.openSpace(m.spaceId);
          case "openMain":
            return host.actions.openMain();
          case "openSettings":
            return host.actions.openSettings();
          case "openAccess":
            return host.actions.openAccess();
          case "dismissAccess":
            return host.actions.dismissAccess();
          case "openPermissionSettings":
            return model.openPermissionSettings(m.pane);
          case "drop":
            return host.actions.drop(m.spaceId, m.paths);
        }
    }
  };

  helper = new NotchProcess({
    ...options.process,
    path: options.path,
    log,
    onMessage,
    onStart: () => {
      trace?.(`started ${options.path} (pid ${helper?.pid ?? "?"})`);
      send({ type: "hello", v: NOTCH_PROTOCOL_VERSION, ...model.motion() });
      model.resend();
      for (const [id, image] of thumbnails) send({ type: "thumbnail", id, image });
    },
  });
  helper.start();
  model.recheckPermission();

  return {
    setSpaces: (spaces) => model.setSpaces(spaces),
    setKeyvault: (label, signedIn) => model.setKeyvault(label, signedIn),
    setActivity: (hotspot, transfer) => model.setActivity(hotspot, transfer),
    setShown: (shown) => model.setShown(shown),
    recheckPermission: () => model.recheckPermission(),
    stop: () => {
      model.dispose();
      helper?.stop();
    },
  };
}

/** A protocol message for the trace: images as their size, the rest as sent. */
export function describe(m: HostMessage | HelperMessage): string {
  return JSON.stringify(m, (key, value) => ((key === "image" || key === "svg") && typeof value === "string" ? `<${value.length} chars>` : value));
}

/**
 * The one wiring point to the app model: call it once the cua-spaces-ffi
 * bindings and the model are up, with the bindings as `host.core`, the
 * model's handlers as `host.actions` (and its thumbnails and the SDK's
 * teleport when there are), then feed the returned notch the Spaces, the
 * Keyvault sign-ins, the activity and the notch setting as they change.
 * Null where there is no notch (Windows, Linux, macOS before 26, or no
 * helper in a development build: run `pnpm notch`).
 */
export async function connectNotch(host: NotchHost): Promise<Notch | null> {
  if (!notchSupported(process.platform, release())) return null;
  // Loaded here so the rest of this file runs without Electron (tests).
  const { app, shell } = await import("electron");
  const executable = helperPath({ packaged: app.isPackaged, resourcesPath: process.resourcesPath, appPath: app.getAppPath() });
  if (!existsSync(executable)) {
    console.warn(`[cua-spaces] notch: no helper at ${executable} (pnpm notch)`);
    return null;
  }
  // Like the SwiftUI app, every click that shows a window brings the app
  // forward first (the helper is a separate, non-activating process).
  const forward =
    <A extends unknown[]>(f: (...args: A) => void) =>
    (...args: A) => {
      app.focus({ steal: true });
      f(...args);
    };
  const a = host.actions;
  const actions = {
    ...a,
    openSpace: forward(a.openSpace.bind(a)),
    openMain: forward(a.openMain.bind(a)),
    openSettings: forward(a.openSettings.bind(a)),
    openAccess: forward(a.openAccess.bind(a)),
    teleport: forward(a.teleport.bind(a)),
    drop: forward(a.drop.bind(a)),
  };
  const notch = startNotch({ ...host, actions }, {
    path: executable,
    openURL: (url) => void shell.openExternal(url),
    highlight: process.env.CUA_SPACES_NOTCH_HIGHLIGHT || undefined,
    // Diagnosing the notch (`CUA_SPACES_NOTCH_DEBUG=1`): the protocol both ways in the app's log.
    trace: process.env.CUA_SPACES_NOTCH_DEBUG ? (line) => console.log(`[cua-spaces] notch ${line}`) : undefined,
  });
  app.on("did-become-active", () => notch.recheckPermission());
  app.on("will-quit", () => notch.stop());
  return notch;
}
