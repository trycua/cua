import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig, type UserConfig } from "tsdown";

// The crash-reporting DSN (a public ingest key) comes from
// config/spaces-crash-reporting.json, so every shell reads one file. A build
// can override it with CUA_SPACES_CRASH_DSN; an empty DSN leaves crash
// reporting off.
function crashConfig(): string {
  let dsn = "";
  try {
    dsn = String(JSON.parse(readFileSync(new URL("../../config/spaces-crash-reporting.json", import.meta.url), "utf8")).dsn ?? "");
  } catch {
    // No file: off.
  }
  return JSON.stringify({ dsn: (process.env.CUA_SPACES_CRASH_DSN ?? dsn).trim() });
}

// Everything ships as CommonJS: a sandboxed preload cannot load ESM, and CJS
// lets boot.cjs turn on the compile cache before main.cjs loads.
const shared: UserConfig = {
  outDir: "dist-electron",
  format: "cjs",
  platform: "node",
  target: "node24",
  sourcemap: true,
  dts: false,
  tsconfig: "tsconfig.build.json",
  // The web sources this shell bundles (../cua-spaces-web/src) import packages
  // the release build installs only here: look in this project's node_modules
  // when the importing file's own directories have none.
  inputOptions: { resolve: { modules: ["node_modules", fileURLToPath(new URL("./node_modules", import.meta.url))] } },
  fixedExtension: true,
  // @sentry/electron stays a runtime dependency, packaged beside electron-updater.
  deps: { neverBundle: ["electron", "electron-updater", /^@sentry\//, "./main.cjs"] },
  define: { __CUA_CRASH_CONFIG__: JSON.stringify(crashConfig()) },
};

export default defineConfig([
  { ...shared, entry: { boot: "src/boot.ts", main: "src/main.ts" }, clean: false },
  // The sandboxed preload can only require "electron", so it is its own
  // self-contained bundle with no shared chunks.
  { ...shared, entry: { preload: "src/preload.ts" }, clean: false },
  // The passphrase form's preload (src/passphrase.ts), the same way.
  { ...shared, entry: { "passphrase-preload": "src/passphrase-preload.ts" }, clean: false },
]);
