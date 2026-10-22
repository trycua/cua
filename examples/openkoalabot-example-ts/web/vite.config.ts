import { resolve } from "node:path"
import { defineConfig } from "vite"

// `npm run dev:web` serves the page with /api and /events proxied to the
// local server (`npm run serve`, default port 4780).
const server = process.env.OPENKOALABOTS_SERVER ?? "http://127.0.0.1:4780"

export default defineConfig({
  root: __dirname,
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
  optimizeDeps: { exclude: ["@trycua/cua"] },
  server: {
    host: "127.0.0.1",
    fs: { allow: [resolve(__dirname, "../../..")] },
    proxy: { "/api": server, "/events": { target: server.replace(/^http/, "ws"), ws: true } },
  },
})
