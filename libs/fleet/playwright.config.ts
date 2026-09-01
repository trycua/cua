import { defineConfig } from "@playwright/test"

const localChromiumExecutable = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  retries: 0,
  workers: process.env.CI ? 2 : undefined,
  use: {
    baseURL: "http://localhost:5180",
    headless: process.env.CUA_E2E_HEADED !== "1",
    launchOptions: localChromiumExecutable
      ? { executablePath: localChromiumExecutable }
      : undefined,
  },
  webServer: {
    command: "VITE_CUA_LOCAL_VISUAL_PREVIEW=true npm run dev",
    port: 5180,
    reuseExistingServer: true,
    timeout: 30_000,
  },
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
})
