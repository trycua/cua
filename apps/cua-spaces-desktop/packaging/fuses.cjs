// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Electron fuses for packaged builds (https://github.com/electron/fuses).
// electron-builder flips them in the packaged Electron binary right before
// signing (`electronFuses` in electron-builder.config.cjs). `pnpm dev` and
// `pnpm start` run the stock Electron from node_modules and are unaffected.
// scripts/check-fuses.mjs reads them back from a packaged build.

/** electron-builder's `electronFuses` names, with the fuse wire index of each. */
const RELEASE_FUSES = {
  // ELECTRON_RUN_AS_NODE would turn the app into a plain Node runtime.
  runAsNode: { index: 0, on: false },
  // Cookies on disk encrypted with the OS key store (Keychain, DPAPI, libsecret).
  enableCookieEncryption: { index: 1, on: true },
  // NODE_OPTIONS and NODE_EXTRA_CA_CERTS.
  enableNodeOptionsEnvironmentVariable: { index: 2, on: false },
  // --inspect, --inspect-brk and SIGUSR1.
  enableNodeCliInspectArguments: { index: 3, on: false },
  // app.asar checked against the hash electron-builder embeds (Info.plist on
  // macOS, an INTEGRITY resource on Windows). Linux has no check: the fuse is
  // set but does nothing there.
  enableEmbeddedAsarIntegrityValidation: { index: 4, on: true },
  // Only resources/app.asar, never a loose resources/app folder.
  onlyLoadAppFromAsar: { index: 5, on: true },
  // The app is served from cua-spaces://, never file://.
  grantFileProtocolExtraPrivileges: { index: 7, on: false },
};

/** `electronFuses` for electron-builder. `resetAdHocSignature`: re-sign an
 * unsigned macOS build ad hoc after the flip, so it still launches. */
function electronFuses({ resetAdHocSignature }) {
  const out = { resetAdHocDarwinSignature: resetAdHocSignature };
  for (const [name, { on }] of Object.entries(RELEASE_FUSES)) out[name] = on;
  return out;
}

module.exports = { RELEASE_FUSES, electronFuses };
