import { defineConfig } from "vite"

export default defineConfig({
  root: "examples",
  assetsInclude: ["**/*.wasm"],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: "browser.js",
      },
    },
  },
  server: {
    fs: {
      allow: [".."],
    },
  },
})
