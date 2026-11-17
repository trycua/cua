// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// electron-updater as the CommonJS main bundle loads it. The bundle keeps
// `import("electron-updater")` (the package is never bundled), and Node's
// `import()` of that CommonJS package names only what its export lexer
// sees: `autoUpdater` is a getter it cannot see, so it is only on
// `default`. Check Now then failed on Windows and Linux with "Cannot set
// properties of undefined (setting 'channel')".

type ElectronUpdater = typeof import("electron-updater");

/** The package's exports, whichever way `load` hands them over. */
export async function electronUpdater(load: () => Promise<unknown> = () => import("electron-updater")): Promise<ElectronUpdater> {
  const mod = (await load()) as Partial<ElectronUpdater> & { default?: ElectronUpdater };
  if ("autoUpdater" in mod) return mod as ElectronUpdater;
  if (mod.default && "autoUpdater" in mod.default) return mod.default;
  throw new Error("electron-updater has no autoUpdater");
}
