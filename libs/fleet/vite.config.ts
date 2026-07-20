import { defineConfig } from "vite"
import react from "@vitejs/plugin-react"

// /api/k8s and /api/orch are served by the cyclops-cs backend sidecar
// (Keycloak SSO + OPA), which isn't reachable from a laptop. In dev,
// route them through the deployed cyclops-cs Tailscale ingress so the
// in-cluster nginx forwards to the sidecar.
const ORCH_API = process.env.ORCH_API ?? "https://cyclops-cs.tail204509.ts.net"

export default defineConfig({
  plugins: [react()],
  // @novnc/novnc (1.7+) uses top-level await; es2020 (esbuild's default here)
  // rejects it. es2022 is supported by all evergreen browsers the app targets.
  build: { target: "es2022" },
  esbuild: { target: "es2022" },
  server: {
    port: 5180,
    proxy: {
      // Upstream cyclops-ctrl is retired — every /api route is served by
      // the cyclops-cs backend behind the deployed ingress.
      "/api": {
        target: ORCH_API,
        changeOrigin: true,
        secure: true,
      },
    },
  },
  optimizeDeps: {
    include: ["@cloudscape-design/components"],
    // Dep pre-bundling (dev server) has its OWN esbuild target that ignores
    // build.target/esbuild.target above — it defaults to es2020, which rejects
    // @novnc/novnc's top-level await and crashes `vite dev` (Playwright then
    // sees ERR_CONNECTION_REFUSED). Pin it to es2022 too.
    esbuildOptions: { target: "es2022" },
  },
})
