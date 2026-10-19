import { defineConfig, type UserConfig } from "tsdown";

// Everything ships as CommonJS: a sandboxed preload cannot load ESM, and CJS
// lets boot.cjs turn on the compile cache before main.cjs loads.
const shared: UserConfig = {
  outDir: "dist-electron",
  format: "cjs",
  platform: "node",
  target: "node24",
  sourcemap: true,
  dts: false,
  fixedExtension: true,
  deps: { neverBundle: ["electron", "electron-updater", "./main.cjs"] },
};

export default defineConfig([
  { ...shared, entry: { boot: "src/boot.ts", main: "src/main.ts" }, clean: false },
  // The sandboxed preload can only require "electron", so it is its own
  // self-contained bundle with no shared chunks.
  { ...shared, entry: { preload: "src/preload.ts" }, clean: false },
  // The passphrase form's preload (src/passphrase.ts), the same way.
  { ...shared, entry: { "passphrase-preload": "src/passphrase-preload.ts" }, clean: false },
]);
