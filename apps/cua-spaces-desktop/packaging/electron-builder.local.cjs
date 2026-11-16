// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// electron-builder config for "Cua Spaces Local" (Windows): the release config
// with the Local names, no installer and no update feed. scripts/install-local.ps1
// builds with it (`electron-builder --win dir --config packaging/electron-builder.local.cjs`)
// and installs the unpacked app beside a released Cua Spaces: another app id
// (so another AppUserModelID, taskbar group and Run entry), another
// executable (`Cua Spaces Local.exe`, which is how src/channel.ts knows it is
// Local), another URL scheme (`cua-local://`), another output folder, and no
// `app-update.yml`, so electron-updater has nothing to check.
//
// The runtime half (CUA_CHANNEL=local, CUA_HOME=~/.cua-local, user data
// folder) is src/channel.ts's. Always unsigned.

const base = require("../electron-builder.config.cjs");

/** @type {import("electron-builder").Configuration} */
module.exports = {
  ...base,
  appId: "ai.cua.spaces.desktop.local",
  productName: "Cua Spaces Local",
  directories: { ...base.directories, output: "dist-local" },
  protocols: [{ name: "Cua Local", schemes: ["cua-local"] }],
  // Never a feed: a Local build is not updated from the release channel.
  publish: null,
  win: {
    ...base.win,
    // Unsigned, whatever the environment has.
    azureSignOptions: undefined,
    target: [{ target: "dir", arch: [process.env.CUA_SPACES_LOCAL_ARCH || "x64"] }],
  },
  // The installer's is the released app's; there is no installer here.
  nsis: undefined,
  // Release-only checks and signing hooks stay; the executable name follows productName.
  extraMetadata: { ...base.extraMetadata, name: "cua-spaces-local" },
  afterSign: undefined,
};
