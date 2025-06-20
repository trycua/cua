import { defineConfig } from "tsdown";

export default defineConfig([
  {
    entry: ["./src/index.ts"],
    platform: "node",
    dts: true,
    external: ["child_process", "util"],
  },
]);
