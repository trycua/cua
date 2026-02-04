#!/usr/bin/env node
import { spawn } from "child_process";
import { fileURLToPath } from "url";
import { dirname, join } from "path";

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const args = ["--no-warnings", join(__dirname, "..", "src", "cuabot.tsx"), ...process.argv.slice(2)];

const child = spawn("npx", ["tsx", ...args], {
  stdio: "inherit",
  shell: process.platform === "win32"
});

child.on("exit", (code) => {
  process.exit(code ?? 0);
});
