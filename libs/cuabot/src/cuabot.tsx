#!/usr/bin/env node
/**
 * CuaBot CLI
 */

import { startServer, stopServer, getServerInfo, setSessionName as setServerSessionName } from "./cuabotd.js";
import { ensureServerRunning, setSessionName as setClientSessionName } from "./client.js";
import { getDefaultAgent, AGENTS, AgentId, getAliasIgnored, getTelemetryEnabled } from "./settings.js";
import { runOnboarding } from "./onboarding.js";
import { sendTelemetryToServer } from "./telemetry.js";
import { execSync } from "child_process";

function isCuabotInPath(): boolean {
  try {
    const cmd = process.platform === "win32" ? "where cuabot" : "which cuabot";
    execSync(cmd, { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

const args = process.argv.slice(2);

// Parse --name / -n flag from args and return [sessionName, remainingArgs]
function parseSessionName(inputArgs: string[]): [string | null, string[]] {
  const remaining: string[] = [];
  let sessionName: string | null = null;

  for (let i = 0; i < inputArgs.length; i++) {
    const arg = inputArgs[i];
    if (arg === "--name" || arg === "-n") {
      sessionName = inputArgs[i + 1] || null;
      i++; // Skip the next arg (the name value)
    } else {
      remaining.push(arg);
    }
  }

  return [sessionName, remaining];
}

// Set session name in both server and client modules
function setSessionName(name: string | null): void {
  setServerSessionName(name);
  setClientSessionName(name);
}

// Track if CLI telemetry was sent this invocation
let cliTelemetrySent = false;

async function sendCliTelemetry(port: number): Promise<void> {
  if (cliTelemetrySent || !getTelemetryEnabled()) return;
  cliTelemetrySent = true;

  await sendTelemetryToServer(port, {
    type: "cli_invocation",
    timestamp: Date.now(),
    cli_args: args,
    cwd: process.cwd(),
  });
}

async function getClient() {
  const { CuaBotClient } = await import("./client.js");
  const port = await ensureServerRunning();
  sendCliTelemetry(port); // Fire and forget
  return new CuaBotClient(port);
}

async function runCommand(shellCommand: string) {
  const WebSocket = (await import("ws")).default;
  const port = await ensureServerRunning();
  sendCliTelemetry(port); // Fire and forget

  const cols = process.stdout.columns || 80;
  const rows = process.stdout.rows || 24;
  const wsUrl = `ws://localhost:${port}/?command=${encodeURIComponent(shellCommand)}&cols=${cols}&rows=${rows}`;

  const ws = new WebSocket(wsUrl);

  ws.on("open", () => {
    if (process.stdin.isTTY) process.stdin.setRawMode(true);
    process.stdin.resume();
  });

  ws.on("message", (data: Buffer) => {
    try {
      const msg = JSON.parse(data.toString());
      if (msg.type === "stdout" || msg.type === "stderr") {
        process.stdout.write(Buffer.from(msg.data, "base64"));
      } else if (msg.type === "exit") {
        cleanup();
        process.exit(msg.code || 0);
      } else if (msg.type === "error") {
        console.error(`\nError: ${msg.message}`);
        cleanup();
        process.exit(1);
      }
    } catch {
      process.stdout.write(data);
    }
  });

  const onStdinData = (data: Buffer) => {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: "stdin", data: data.toString("base64") }));
    }
  };
  process.stdin.on("data", onStdinData);

  const onResize = () => {
    if (ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: "resize", cols: process.stdout.columns || 80, rows: process.stdout.rows || 24 }));
    }
  };
  process.stdout.on("resize", onResize);

  const cleanup = () => {
    process.stdin.removeListener("data", onStdinData);
    process.stdout.removeListener("resize", onResize);
    if (process.stdin.isTTY) process.stdin.setRawMode(false);
    ws.close();
  };

  ws.on("close", () => { cleanup(); process.exit(0); });
  ws.on("error", (err: Error & { code?: string }) => {
    console.error(`WebSocket error: ${err.message || err.code || err}`);
    cleanup();
    process.exit(1);
  });
  process.on("SIGINT", () => { cleanup(); process.exit(0); });
}

async function runAgent(agentId: string, extraArgs: string[] = []) {
  const port = await ensureServerRunning();
  sendCliTelemetry(port); // Fire and forget

  let shellCommand: string;
  if (agentId === "claude") {
    shellCommand = `claude --mcp-config /home/user/.mcp.json --append-system-prompt-file /home/user/CLAUDE.md ${extraArgs.join(" ")}`.trim();
  } else {
    const agent = AGENTS[agentId as AgentId];
    shellCommand = `${agent?.command || agentId} ${extraArgs.join(" ")}`.trim();
  }

  await runCommand(shellCommand);
}

async function main() {
  // Parse session name first
  const [sessionName, remainingArgs] = parseSessionName(args);
  setSessionName(sessionName);

  const flag = remainingArgs[0];

  // Flag commands
  if (flag?.startsWith("--")) {
    const flagArgs = remainingArgs.slice(1);

    switch (flag) {
      case "--help":
      case "-h": {
        console.log(`cuabot - Computer Use Agent Bot

Usage:
  cuabot                     Run default agent (or setup if not configured)
  cuabot <agent>             Run agent (claude, gemini, codex, aider, openclaw, vibe)
  cuabot <command>           Run any command in the sandbox

Options:
  -n, --name <name>          Use a named session (allows multiple instances)

Commands:
  --serve [port]             Start server (auto-finds available port)
  --stop                     Stop server
  --status                   Server status
  --reset [all|sandbox|settings]  Reset (default: all)
  --screenshot [path]        Take screenshot
  --bash <command>           Execute bash command
  --click <x> <y> [button]   Click at coordinates
  --doubleclick <x> <y>      Double-click
  --move <x> <y>             Move mouse
  --mousedown <x> <y>        Press mouse button
  --mouseup <x> <y>          Release mouse button
  --drag <x1> <y1> <x2> <y2> Drag
  --scroll <x> <y> <dx> <dy> Scroll
  --type <text>              Type text
  --key <key>                Press key
  --keydown <key>            Key down
  --keyup <key>              Key up
  --help                     Show this help`);
        process.exit(0);
      }

      case "--serve": {
        const port = flagArgs[0] ? parseInt(flagArgs[0], 10) : undefined;
        await startServer(port);
        break;
      }

      case "--stop": {
        const stopped = await stopServer();
        process.exit(stopped ? 0 : 1);
      }

      case "--status": {
        const info = getServerInfo();
        if (info) {
          const nameSuffix = sessionName ? ` [${sessionName}]` : "";
          console.log(`Server${nameSuffix} running on port ${info.port} (PID: ${info.pid})`);
        } else {
          console.log("Server not running");
        }
        process.exit(0);
      }

      case "--reset": {
        const { execSync } = await import("child_process");
        const { rmSync } = await import("fs");
        const { homedir } = await import("os");
        const { join } = await import("path");

        const target = flagArgs[0] || "all";
        const CONTAINER_NAME = "cuabot-xpra";
        const IMAGE_NAME = "trycua/cuabot:latest";
        const CONFIG_DIR = join(homedir(), ".cuabot");

        if (target === "sandbox" || target === "all") {
          console.log("Resetting sandbox...");
          try { await stopServer(); } catch {}
          try { execSync(`docker rm -f ${CONTAINER_NAME}`, { stdio: "ignore" }); } catch {}
          try { execSync(`docker rmi ${IMAGE_NAME}`, { stdio: "ignore" }); } catch {}
          console.log("  ✓ Container and image removed");
        }

        if (target === "settings" || target === "all") {
          console.log("Resetting settings...");
          try { rmSync(CONFIG_DIR, { recursive: true, force: true }); } catch {}
          console.log("  ✓ ~/.cuabot removed");
        }

        if (target !== "all" && target !== "sandbox" && target !== "settings") {
          console.error("Usage: cuabot --reset [all|sandbox|settings]");
          process.exit(1);
        }

        console.log("Done.");
        process.exit(0);
      }

      case "--screenshot": {
        const client = await getClient();
        const base64 = await client.screenshot();
        const { writeFileSync } = await import("fs");
        const outputPath = flagArgs[0] || "screenshot.jpg";
        writeFileSync(outputPath, Buffer.from(base64, "base64"));
        console.log(`Screenshot saved to ${outputPath}`);
        process.exit(0);
      }

      case "--bash": {
        const cmd = flagArgs.join(" ");
        if (!cmd) {
          console.error("Usage: cuabot --bash <command>");
          process.exit(1);
        }
        const client = await getClient();
        const { stdout, stderr } = await client.bash(cmd);
        if (stdout) console.log(stdout);
        if (stderr) console.error(stderr);
        process.exit(0);
      }

      case "--click": {
        const [x, y, button] = flagArgs;
        if (!x || !y) { console.error("Usage: cuabot --click <x> <y> [button]"); process.exit(1); }
        const client = await getClient();
        await client.click(parseInt(x), parseInt(y), (button as any) || "left");
        console.log(`Clicked at ${x},${y}`);
        process.exit(0);
      }

      case "--doubleclick": {
        const [x, y] = flagArgs;
        if (!x || !y) { console.error("Usage: cuabot --doubleclick <x> <y>"); process.exit(1); }
        const client = await getClient();
        await client.doubleClick(parseInt(x), parseInt(y));
        console.log(`Double-clicked at ${x},${y}`);
        process.exit(0);
      }

      case "--move": {
        const [x, y] = flagArgs;
        if (!x || !y) { console.error("Usage: cuabot --move <x> <y>"); process.exit(1); }
        const client = await getClient();
        await client.mouseMove(parseInt(x), parseInt(y));
        console.log(`Moved to ${x},${y}`);
        process.exit(0);
      }

      case "--mousedown": {
        const [x, y, button] = flagArgs;
        if (!x || !y) { console.error("Usage: cuabot --mousedown <x> <y> [button]"); process.exit(1); }
        const client = await getClient();
        await client.mouseDown(parseInt(x), parseInt(y), (button as any) || "left");
        console.log(`Mouse down at ${x},${y}`);
        process.exit(0);
      }

      case "--mouseup": {
        const [x, y, button] = flagArgs;
        if (!x || !y) { console.error("Usage: cuabot --mouseup <x> <y> [button]"); process.exit(1); }
        const client = await getClient();
        await client.mouseUp(parseInt(x), parseInt(y), (button as any) || "left");
        console.log(`Mouse up at ${x},${y}`);
        process.exit(0);
      }

      case "--drag": {
        const [x1, y1, x2, y2] = flagArgs;
        if (!x1 || !y1 || !x2 || !y2) { console.error("Usage: cuabot --drag <x1> <y1> <x2> <y2>"); process.exit(1); }
        const client = await getClient();
        await client.drag(parseInt(x1), parseInt(y1), parseInt(x2), parseInt(y2));
        console.log(`Dragged from ${x1},${y1} to ${x2},${y2}`);
        process.exit(0);
      }

      case "--scroll": {
        const [x, y, dx, dy] = flagArgs;
        if (!x || !y || !dy) { console.error("Usage: cuabot --scroll <x> <y> <dx> <dy>"); process.exit(1); }
        const client = await getClient();
        await client.scroll(parseInt(x), parseInt(y), parseInt(dx || "0"), parseInt(dy));
        console.log(`Scrolled at ${x},${y}`);
        process.exit(0);
      }

      case "--type": {
        const text = flagArgs.join(" ");
        if (!text) { console.error("Usage: cuabot --type <text>"); process.exit(1); }
        const client = await getClient();
        await client.type(text);
        console.log(`Typed: ${text}`);
        process.exit(0);
      }

      case "--key": {
        const key = flagArgs[0];
        if (!key) { console.error("Usage: cuabot --key <key>"); process.exit(1); }
        const client = await getClient();
        await client.keyPress(key);
        console.log(`Pressed: ${key}`);
        process.exit(0);
      }

      case "--keydown": {
        const key = flagArgs[0];
        if (!key) { console.error("Usage: cuabot --keydown <key>"); process.exit(1); }
        const client = await getClient();
        await client.keyDown(key);
        console.log(`Key down: ${key}`);
        process.exit(0);
      }

      case "--keyup": {
        const key = flagArgs[0];
        if (!key) { console.error("Usage: cuabot --keyup <key>"); process.exit(1); }
        const client = await getClient();
        await client.keyUp(key);
        console.log(`Key up: ${key}`);
        process.exit(0);
      }

      default: {
        console.error(`Unknown flag: ${flag}`);
        console.error(`Run 'cuabot --help' for usage`);
        process.exit(1);
      }
    }
    return;
  }

  // No args: run default agent or onboarding
  if (remainingArgs.length === 0) {
    const defaultAgent = getDefaultAgent();
    const needsAliasSetup = !isCuabotInPath() && !getAliasIgnored();

    // Show onboarding if no default agent or alias needs setup
    if (!defaultAgent || needsAliasSetup) {
      runOnboarding();
      return;
    }

    try {
      await runAgent(defaultAgent);
    } catch (err) {
      // If dependencies fail, show onboarding
      if (err instanceof Error && err.message.includes("Missing dependencies")) {
        runOnboarding();
        return;
      }
      throw err;
    }
    return;
  }

  // cuabot <agent id> - run agent if it's a known agent ID
  const firstArg = remainingArgs[0];
  if (firstArg in AGENTS) {
    await runAgent(firstArg, remainingArgs.slice(1));
    return;
  }

  // cuabot <command> - run command in sandbox
  await runCommand(remainingArgs.join(" "));
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
