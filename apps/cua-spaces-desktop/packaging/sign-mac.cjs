// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// electron-builder's macOS signer (`mac.sign`): the default signing, with one
// file signed differently. The bundled `cua` (the daemon the app runs, and
// the one that holds the Keyvault) is signed with the identifier
// `com.trycua.cua` and its own entitlements, as the SwiftUI app's release does
// (apps/cua-spaces-macos/scripts/build-release.sh).
//
// That identifier is what the Keyvault trusts. The app and the daemon check
// each other's code signature against the requirement "Apple-anchored, team
// YCK386LBJ7, identifier com.trycua.spaces.macos or com.trycua.cua"
// (cua-keyvault, `TrustPolicy::production`), and the vault key sits in a
// keychain item that only that same requirement may read without a prompt. A
// bare `cua` is signed under its file name, which satisfies neither: the app
// would refuse the daemon as an impostor, and a vault the SwiftUI app made
// would ask for the keychain password again.
const fs = require("node:fs");
const path = require("node:path");

const DAEMON_IDENTIFIER = "com.trycua.cua";
const DAEMON_ENTITLEMENTS = path.join(__dirname, "entitlements.mac.cua.plist");

/** The bundled daemon: `Contents/Resources/native/cua`. */
const isDaemon = (file) => file.split(path.sep).join("/").endsWith("/Contents/Resources/native/cua");

/** electron-builder's options for `file`, but for the daemon its own identifier and entitlements. */
function withDaemonIdentity(base, file) {
  if (!isDaemon(file)) return base;
  return {
    ...base,
    entitlements: DAEMON_ENTITLEMENTS,
    hardenedRuntime: true,
    additionalArguments: [...(base.additionalArguments ?? []), "--identifier", DAEMON_IDENTIFIER],
  };
}

/** `@electron/osx-sign`, which electron-builder signs with (it is not a dependency of this project). */
function osxSign() {
  const builder = require.resolve("electron-builder/package.json");
  const lib = require.resolve("app-builder-lib/package.json", { paths: [path.dirname(builder)] });
  return require(require.resolve("@electron/osx-sign", { paths: [path.dirname(lib)] }));
}

/**
 * Signs with `load()`'s `signAsync` (`@electron/osx-sign` in a build, a stand-in in tests).
 * @param {import("@electron/osx-sign/dist/cjs/types").SignOptions} opts
 */
async function signWith(opts, load) {
  if (!fs.existsSync(DAEMON_ENTITLEMENTS)) throw new Error(`missing ${DAEMON_ENTITLEMENTS}`);
  const optionsForFile = opts.optionsForFile;
  const { signAsync } = load();
  await signAsync({ ...opts, optionsForFile: (file) => withDaemonIdentity(optionsForFile ? optionsForFile(file) : {}, file) });
}

/**
 * electron-builder's `mac.sign` hook. It is called as `sign(opts, packager)`;
 * the packager is not used.
 * @param {import("@electron/osx-sign/dist/cjs/types").SignOptions} opts
 */
async function sign(opts) {
  await signWith(opts, osxSign);
}

module.exports = { sign, signWith, withDaemonIdentity, isDaemon, DAEMON_IDENTIFIER, DAEMON_ENTITLEMENTS };
