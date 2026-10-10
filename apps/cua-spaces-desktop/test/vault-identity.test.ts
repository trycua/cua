// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The identity the Keyvault trusts, in the packages (packaging/sign-mac.cjs,
// the polkit policy, the entitlements): the daemon is signed as
// com.trycua.cua with the Swift app's entitlements, the app signs as
// com.trycua.spaces.macos, and both are names the Rust trust policy lists.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { createRequire } from "node:module";
import * as path from "node:path";
import { describe, expect, it } from "vitest";

const require = createRequire(import.meta.url);
interface FileOptions {
  entitlements?: string;
  hardenedRuntime?: boolean;
  additionalArguments?: string[];
}
const signer = require("../packaging/sign-mac.cjs") as {
  sign(opts: unknown, load?: () => unknown): Promise<void>;
  withDaemonIdentity(base: FileOptions, file: string): FileOptions;
  isDaemon(file: string): boolean;
  DAEMON_IDENTIFIER: string;
  DAEMON_ENTITLEMENTS: string;
};
const config = require("../electron-builder.config.cjs");

const repo = path.join(import.meta.dirname, "../../..");
const read = (file: string) => readFileSync(path.join(repo, file), "utf8");
const keys = (plist: string) => [...plist.matchAll(/<key>([^<]+)<\/key>/g)].map((m) => m[1]);

const app = "/dist/mac-arm64/Cua Spaces.app";
const daemon = `${app}/Contents/Resources/native/cua`;

describe("the bundled daemon's signature", () => {
  it("signs the daemon as com.trycua.cua with its own entitlements, hardened", () => {
    const base = { entitlements: "inherit.plist", hardenedRuntime: true, additionalArguments: ["--deep-check"] };
    const o = signer.withDaemonIdentity(base, daemon);
    expect(o.additionalArguments).toEqual(["--deep-check", "--identifier", "com.trycua.cua"]);
    expect(o.entitlements).toBe(signer.DAEMON_ENTITLEMENTS);
    expect(o.hardenedRuntime).toBe(true);
  });

  it("leaves every other file as electron-builder has it", () => {
    for (const file of [app, `${app}/Contents/MacOS/Cua Spaces`, `${app}/Contents/Resources/native/libcua_spaces_ffi.dylib`, `${app}/Contents/Resources/native/cua_node_runtime.node`, `${app}/Contents/Helpers/Cua Spaces Notch.app`, `${app}/Contents/Resources/web/cua`]) {
      const base = { entitlements: "x.plist", additionalArguments: [] };
      expect(signer.withDaemonIdentity(base, file)).toBe(base);
    }
    expect(signer.isDaemon(daemon)).toBe(true);
    expect(signer.isDaemon(`${daemon}.bak`)).toBe(false);
  });

  it("signs the whole app the default way, through the daemon's options", async () => {
    let received: { optionsForFile: (f: string) => FileOptions; app: string } | undefined;
    const opts = { app, identity: "Developer ID Application: Cua", optionsForFile: () => ({ entitlements: "inherit.plist", additionalArguments: [] }) };
    await signer.sign(opts, () => ({ signAsync: async (o: typeof received) => void (received = o) }));
    expect(received?.app).toBe(app);
    expect(received?.optionsForFile(daemon).additionalArguments).toEqual(["--identifier", "com.trycua.cua"]);
    expect(received?.optionsForFile(`${app}/Contents/MacOS/Cua Spaces`)).toEqual({ entitlements: "inherit.plist", additionalArguments: [] });
  });

  it("is what the mac build signs with", () => {
    expect(config.mac.sign).toBe(signer.sign);
    expect(config.mac.appId).toBe("com.trycua.spaces.macos");
  });

  it("names identifiers the Keyvault's trust policy lists", () => {
    const rust = read("libs/cua/crates/cua-keyvault/src/caller.rs");
    const listed = /pub const CUA_IDENTIFIERS: &\[&str\] = &\[([^\]]*)\]/.exec(rust)?.[1] ?? "";
    expect(listed).toContain(`"${signer.DAEMON_IDENTIFIER}"`);
    expect(listed).toContain(`"${config.mac.appId}"`);
    expect(rust).toContain('pub const CUA_TEAM_ID: &str = "YCK386LBJ7"');
  });

  it("grants the daemon what the Swift app's does, and nothing more", () => {
    const ours = readFileSync(signer.DAEMON_ENTITLEMENTS, "utf8");
    const swift = read("apps/cua-spaces-macos/Support/cua.entitlements");
    expect(keys(ours)).toEqual(keys(swift));
    expect(keys(ours)).toEqual(["com.apple.security.automation.apple-events"]);
  });

  it("gives the app no keychain access group: the vault key is the daemon's item", () => {
    const entitlements = readFileSync(path.join(import.meta.dirname, "../packaging/entitlements.mac.plist"), "utf8");
    expect(keys(entitlements)).not.toContain("keychain-access-groups");
    expect(keys(entitlements)).not.toContain("com.apple.application-identifier");
    // The hardened runtime the broker requires of a first-party caller is on in a signed build.
    expect(config.mac.hardenedRuntime).toBe(false); // unsigned, as this test loads it
  });
});

describe("the Linux policy", () => {
  it("is the action the daemon asks for, answered by the user's own password, every time", () => {
    const policy = readFileSync(path.join(import.meta.dirname, "../packaging/ai.cua.spaces.policy"), "utf8");
    const rust = read("libs/cua/crates/cua-teleport/src/biometric.rs");
    const action = /pub const POLKIT_ACTION: &str = "([^"]+)"/.exec(rust)?.[1];
    expect(action).toBe("ai.cua.spaces.keyvault");
    expect(policy).toContain(`<action id="${action}">`);
    expect(policy).toContain("<allow_active>auth_self</allow_active>");
    expect(policy).not.toContain("auth_self_keep");
    expect(policy).not.toContain("auth_admin");
  });
});

describe("the Windows daemon's signature", () => {
  const configPath = require.resolve("../electron-builder.config.cjs");
  const azure = { AZURE_SIGNING_ENDPOINT: "https://eus.codesigning.azure.net", AZURE_SIGNING_ACCOUNT: "acct", AZURE_SIGNING_PROFILE: "profile", AZURE_SIGNING_PUBLISHER: "Cua AI, Inc." };

  /** The config as CI loads it with `env`. */
  function load(env: Record<string, string>) {
    const saved = { ...process.env };
    for (const k of Object.keys(azure)) delete process.env[k];
    Object.assign(process.env, env);
    delete require.cache[configPath];
    try {
      return require(configPath);
    } finally {
      process.env = saved;
      delete require.cache[configPath];
    }
  }

  /** An unpacked Windows app with a native layer, and the files `signIf` was asked to sign. */
  async function pack(env: Record<string, string>) {
    const out = mkdtempSync(path.join(tmpdir(), "cua-afterpack-"));
    try {
      const native = path.join(out, "resources", "native");
      mkdirSync(native, { recursive: true });
      for (const f of ["cua_spaces_ffi.dll", "cua_node_runtime.node", "cua.exe"]) writeFileSync(path.join(native, f), "x");
      const signed: string[] = [];
      const packager = { signIf: async (file: string) => void signed.push(path.relative(out, file).split(path.sep).join("/")) };
      await load(env).afterPack({ electronPlatformName: "win32", appOutDir: out, packager });
      return signed;
    } finally {
      rmSync(out, { recursive: true, force: true });
    }
  }

  it("signs the daemon and its libraries with the app's signer, which electron-builder does not do for extraResources", async () => {
    expect((await pack(azure)).sort()).toEqual(["resources/native/cua.exe", "resources/native/cua_node_runtime.node", "resources/native/cua_spaces_ffi.dll"]);
  });

  it("signs nothing in a build with no signing set up", async () => {
    expect(await pack({})).toEqual([]);
  });
});
