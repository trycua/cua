// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// electron-builder config for Cua Spaces.
//
// Builds are unsigned unless the signing environment below is present, so a
// local `pnpm dist:mac` works on any machine.
//
// macOS, Developer ID + notarization (set all of these to turn it on):
//   CSC_LINK / CSC_KEY_PASSWORD   Developer ID Application .p12 (or CSC_NAME
//                                 for a keychain identity)
//   APPLE_API_KEY, APPLE_API_KEY_ID, APPLE_API_ISSUER   App Store Connect key
//     (or APPLE_ID, APPLE_APP_SPECIFIC_PASSWORD, APPLE_TEAM_ID)
//
// Windows, Azure Artifact (Trusted) Signing (set the three AZURE_SIGNING_*
// to turn it on); the TrustedSigning PowerShell module signs with whatever
// Azure credential the environment has: `azure/login` (OIDC) in CI, or
// AZURE_TENANT_ID + AZURE_CLIENT_ID + AZURE_CLIENT_SECRET locally:
//   AZURE_SIGNING_ENDPOINT        e.g. https://eus.codesigning.azure.net
//   AZURE_SIGNING_ACCOUNT         signing account name
//   AZURE_SIGNING_PROFILE         certificate profile name
//   AZURE_SIGNING_PUBLISHER       optional: the certificate's CN; when set,
//                                 electron-updater accepts only installers
//                                 signed by it
//
// Native layer: every package needs `pnpm native -- --target <triple>` for
// its arch first (both mac triples for universal); `afterPack` refuses a
// package without it unless CUA_SPACES_ALLOW_NO_NATIVE=1 (a shell-only QA
// build, which cannot start).
//
// Electron fuses (packaging/fuses.cjs) are flipped in every packaged build;
// `pnpm check:fuses` reads them back. CUA_SPACES_NO_FUSES=1 leaves them as
// Electron ships them, for a local QA build that needs --inspect.
//
// Version: CUA_SPACES_VERSION (CI passes the tag's version, e.g.
// 0.8.0-beta.1), else package.json, which Release Please keeps on the
// cua-spaces version. CFBundleVersion (and the Windows FileVersion) is
// X.Y.Z.<CUA_SPACES_BUILD_NUMBER, default 0>, the Swift app's scheme
// (build-release.sh --build-number), so Sparkle orders the two apps' builds.
//
// Updates (electron-updater, see src/updater.ts): a generic feed on the
// rolling `cua-spaces-latest` release of GITHUB_REPOSITORY (default
// trycua/cua), baked into app-update.yml, so a build from another
// repository (<owner>/<repo>) updates from that repository. GitHub's own
// provider cannot serve this monorepo: it wants semver tags and reads the
// repository-wide latest release. An X.Y.Z-suffix version is channel `beta`
// (beta*.yml), anything else `latest`.
// The CI (cd-cua-spaces.yml) uploads the feed files with their URLs made
// absolute (scripts/feed-urls.mjs); scripts pass `--publish never`.
//
// macOS bundle id: com.trycua.spaces.macos, the Swift app's, with its
// usage descriptions and Sparkle public key (read from
// ../cua-spaces-macos/Support/Info.plist), so one Sparkle update can
// replace the Swift app in place (docs/sparkle-cutover.md). Windows and
// Linux keep ai.cua.spaces.desktop.
const fs = require("node:fs");
const path = require("node:path");

const env = process.env;

// Every file electron-builder writes itself (the Linux packages' desktop
// entry, apparmor profile and package type, after `afterPack`) is writable by
// its owner only, whatever the build machine's umask (002 on many Linux
// desktops); `afterPack` sets the modes of what it copied
// (packaging/linux-permissions.cjs).
try {
  process.umask(0o022);
} catch {
  // A worker thread (a test runner's) has no umask of its own; packaging never runs there.
}
const icons = path.join(__dirname, "../cua-spaces/src-tauri/icons");
const webDist = path.join(__dirname, "../cua-spaces-web/dist");
// The macOS notch helper (`pnpm notch`, src/notch.ts), universal.
const notchApp = path.join(__dirname, "native/notch/Cua Spaces Notch.app");
const pkg = require("./package.json");
const { electronFuses } = require("./packaging/fuses.cjs");
const { sign: signMac } = require("./packaging/sign-mac.cjs");
const { normalizeModes } = require("./packaging/linux-permissions.cjs");

const version = env.CUA_SPACES_VERSION || pkg.version;
const semver = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/.exec(version);
if (!semver) throw new Error(`CUA_SPACES_VERSION must look like 1.2.3 or 1.2.3-suffix, got ${version}`);
const prerelease = Boolean(semver[4]);
const buildNumber = env.CUA_SPACES_BUILD_NUMBER || "0";
if (!/^\d+$/.test(buildNumber)) throw new Error("CUA_SPACES_BUILD_NUMBER must be a number");
const buildVersion = `${semver[1]}.${semver[2]}.${semver[3]}.${buildNumber}`;

const repository = env.GITHUB_REPOSITORY || "trycua/cua";
if (!/^[\w.-]+\/[\w.-]+$/.test(repository)) throw new Error(`GITHUB_REPOSITORY must be owner/repo, got ${repository}`);
const feedUrl = `https://github.com/${repository}/releases/download/cua-spaces-latest`;

// The Swift app's Info.plist strings this app must carry as well.
const swiftPlist = fs.readFileSync(path.join(__dirname, "../cua-spaces-macos/Support/Info.plist"), "utf8");
const swiftInfo = (key) => {
  const match = new RegExp(`<key>${key}</key>\\s*<string>([^<]*)</string>`).exec(swiftPlist);
  if (!match) throw new Error(`${key} is missing from cua-spaces-macos/Support/Info.plist`);
  const entities = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };
  return match[1].replace(/&(amp|lt|gt|quot|apos);/g, (_, name) => entities[name]);
};

// Linux desktop entry, WM_CLASS and Wayland app id; src/main.ts sets the same.
const linuxId = "cua-spaces";

const macSigning = Boolean(env.CSC_LINK || env.CSC_NAME);
const macNotarize =
  macSigning &&
  Boolean((env.APPLE_API_KEY && env.APPLE_API_KEY_ID && env.APPLE_API_ISSUER) || (env.APPLE_ID && env.APPLE_TEAM_ID));
const azureSigning = ["AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_SIGNING_PROFILE"].every((k) => env[k]);

/**
 * The native layer of the arch being packed (`pnpm native`): the
 * cua-spaces-ffi library, its N-API runtime and the bundled `cua`, as
 * `Resources/native`. Each platform names its own directory (electron-builder's
 * `${platform}` is the build machine's). A mac universal build packs both
 * arches' directories, and @electron/universal merges the two with lipo.
 */
const nativeResources = (platform) => ({ from: `native/${platform}-\${arch}`, to: "native", filter: ["**/*"] });

/** The native directory's files on a platform (src/native/location.ts). */
function nativeLayout(platform) {
  const libraryFile = { darwin: "libcua_spaces_ffi.dylib", win32: "cua_spaces_ffi.dll", linux: "libcua_spaces_ffi.so" }[platform];
  return { libraryFile, files: [libraryFile, "cua_node_runtime.node", platform === "win32" ? "cua.exe" : "cua"] };
}

/** @type {import("electron-builder").Configuration} */
module.exports = {
  appId: "ai.cua.spaces.desktop",
  productName: "Cua Spaces",
  copyright: "Copyright © 2026 Cua AI, Inc.",
  directories: { output: "dist", buildResources: "packaging" },
  // `name` is the npm name, @cua/cua-spaces-desktop, which electron-builder
  // would turn into the Windows install folder (%LOCALAPPDATA%\Programs\@cuacua-spaces-desktop)
  // and the updater cache name. The app name, and so the user data folder,
  // comes from productName and does not change.
  extraMetadata: { name: linuxId, version, desktopName: `${linuxId}.desktop` },
  buildVersion,
  asar: true,
  compression: "normal",
  files: ["package.json", "dist-electron/**/*", "placeholder/**/*", "!**/*.map"],
  extraResources: [
    // The built web UI sits next to the asar, served by the cua-spaces:// handler.
    ...(fs.existsSync(path.join(webDist, "index.html"))
      ? [{ from: webDist, to: "web", filter: ["**/*", "!**/*.map"] }]
      : []),
  ],
  // Every package carries its native layer: without it the app cannot start.
  afterPack: async (context) => {
    const { libraryFile, files } = nativeLayout(context.electronPlatformName);
    const resources =
      context.electronPlatformName === "darwin"
        ? path.join(context.appOutDir, `${context.packager.appInfo.productFilename}.app`, "Contents", "Resources")
        : path.join(context.appOutDir, "resources");
    const missing = files.filter((f) => !fs.existsSync(path.join(resources, "native", f)));
    if (missing.length && env.CUA_SPACES_ALLOW_NO_NATIVE !== "1") {
      throw new Error(
        `the package has no native layer (${missing.join(", ")} missing under ${path.join(resources, "native")}); ` +
          `run \`pnpm native -- --target <triple>\` for this arch first (libcua_spaces_ffi: ${libraryFile})`,
      );
    }
    // Linux: folders 755 and files 755 or 644, whatever the build machine's
    // umask, so the deb's /opt/Cua Spaces is writable by root alone and the
    // Keyvault trusts its `cua` (packaging/linux-permissions.cjs).
    if (context.electronPlatformName === "linux") normalizeModes(context.appOutDir);
    // Windows: electron-builder signs the app's executables and app.asar.unpacked,
    // not extraResources. The Keyvault knows Cua by its Authenticode publisher
    // (cua-keyvault `windows_signing`), so the daemon and the libraries beside it
    // are signed here, with the same signer, before the installer is made.
    if (context.electronPlatformName === "win32" && azureSigning) {
      for (const f of files) await context.packager.signIf(path.join(resources, "native", f));
    }
  },
  electronLanguages: ["en"],
  electronFuses: env.CUA_SPACES_NO_FUSES === "1" ? null : electronFuses({ resetAdHocSignature: !macSigning }),

  mac: {
    appId: "com.trycua.spaces.macos",
    category: "public.app-category.developer-tools",
    // Used by the zip; the dmg sets its own name below.
    artifactName: "Cua-Spaces-${version}-${arch}-mac.${ext}",
    icon: path.join(icons, "icon.icns"),
    target: [
      { target: "dmg", arch: ["arm64", "universal"] },
      { target: "zip", arch: ["arm64", "universal"] },
    ],
    // null skips signing entirely; with CSC_* set electron-builder picks the
    // Developer ID identity itself.
    identity: macSigning ? undefined : null,
    hardenedRuntime: macSigning,
    gatekeeperAssess: false,
    // The menu bar item's template icon (src/tray.ts), as the Swift app ships it.
    extraResources: [{ from: icons, to: "tray", filter: ["tray-template.png", "tray-template@2x.png"] }, nativeResources("darwin")],
    entitlements: "packaging/entitlements.mac.plist",
    entitlementsInherit: "packaging/entitlements.mac.inherit.plist",
    // The default signing, but the bundled `cua` as com.trycua.cua with its
    // own entitlements: the identity the Keyvault trusts (packaging/sign-mac.cjs).
    sign: signMac,
    extendInfo: {
      NSAppleEventsUsageDescription: swiftInfo("NSAppleEventsUsageDescription"),
      NSLocalNetworkUsageDescription: swiftInfo("NSLocalNetworkUsageDescription"),
      // Not used by this app (no Sparkle inside). Sparkle refuses an update
      // that drops the key the old app had, so the cutover build keeps it.
      SUPublicEDKey: swiftInfo("SUPublicEDKey"),
      // The full version, as the Swift app records it (About, telemetry).
      CuaVersion: version,
    },
    notarize: macNotarize,
    // The notch helper sits in Contents/Helpers, where signing reaches it:
    // electron-builder signs every nested .app and Mach-O with the app's
    // identity, hardened runtime and (inherited) entitlements, so it
    // notarizes with the app. It is already universal, so the universal
    // merge keeps it as is.
    extraFiles: fs.existsSync(path.join(notchApp, "Contents/Info.plist"))
      ? [{ from: notchApp, to: "Helpers/Cua Spaces Notch.app" }]
      : [],
  },
  dmg: { sign: false, artifactName: "Cua-Spaces-${version}-${arch}.dmg" },

  win: {
    icon: path.join(icons, "icon.ico"),
    // Tray icon (src/tray.ts), used as is.
    extraResources: [{ from: icons, to: "tray", filter: ["icon.ico"] }, nativeResources("win32")],
    target: [{ target: "nsis", arch: ["x64", "arm64"] }],
    ...(azureSigning
      ? {
          azureSignOptions: {
            endpoint: env.AZURE_SIGNING_ENDPOINT,
            codeSigningAccountName: env.AZURE_SIGNING_ACCOUNT,
            certificateProfileName: env.AZURE_SIGNING_PROFILE,
            ...(env.AZURE_SIGNING_PUBLISHER ? { publisherName: env.AZURE_SIGNING_PUBLISHER } : {}),
          },
        }
      : {}),
  },
  nsis: {
    oneClick: true,
    perMachine: false,
    // Stops the cua daemon before files are written or removed, and on
    // uninstall removes launch at login and the CLI the app put on PATH.
    include: "packaging/installer.nsh",
    artifactName: "Cua-Spaces-Setup-${version}-${arch}.exe",
  },

  linux: {
    executableName: linuxId,
    // 32 to 512 px, symlinks to the Cua Spaces icons so each hicolor size is installed.
    icon: "packaging/linux-icons",
    // 32x32 for the tray; 128x128@2x (256 px) as the window icon (src/icon.ts).
    extraResources: [{ from: icons, to: "tray", filter: ["32x32.png", "128x128@2x.png"] }, nativeResources("linux")],
    category: "Development",
    // The desktop entry's Comment and the package description (electron-builder
    // writes the description over an entry's own Comment).
    description: "Run apps and agents in Cua Spaces",
    // Name the .desktop file after desktopName so it matches StartupWMClass
    // and the app id Electron reports.
    syncDesktopName: true,
    desktop: {
      entry: {
        Name: "Cua Spaces",
        StartupWMClass: linuxId,
        Keywords: "cua;spaces;sandbox;agents;",
      },
    },
    target: [
      { target: "AppImage", arch: ["x64", "arm64"] },
      { target: "deb", arch: ["x64", "arm64"] },
    ],
    maintainer: "Cua AI, Inc. <founders@trycua.com>",
    synopsis: "Cua Spaces",
  },
  // The static AppImage runtime: no FUSE 2 or libz.so on the host. The
  // default runtime's arm64 build links the unversioned libz.so, which stock
  // distros only ship with zlib1g-dev, so it does not start there.
  toolsets: { appimage: "1.0.3" },
  appImage: { artifactName: "Cua-Spaces-${version}-${arch}.AppImage" },
  // The deb also installs the polkit action the Keyvault's confirmation asks
  // the desktop for (packaging/ai.cua.spaces.policy; the AppImage cannot, so
  // there the confirmation is unavailable and the Keyvault stays off).
  deb: {
    artifactName: "cua-spaces_${version}_${arch}.deb",
    fpm: [`${path.join(__dirname, "packaging/ai.cua.spaces.policy")}=/usr/share/polkit-1/actions/ai.cua.spaces.policy`],
  },

  // Feed for electron-updater (written to app-update.yml); see the header.
  publish: [{ provider: "generic", url: feedUrl, channel: prerelease ? "beta" : "latest" }],
};
