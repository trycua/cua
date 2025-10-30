import type { Argv } from "yargs";
import { ensureApiKeyInteractive } from "../auth";
import { http } from "../http";
import { printVmList, openInBrowser } from "../util";
import { WEBSITE_URL } from "../config";
import type { VmItem } from "../util";
import { clearApiKey } from "../storage";

export function registerVmCommands(y: Argv) {
  return y.command(
    "vm",
    "VM commands",
    (yv) =>
      yv
        .command(
          "list",
          "List VMs",
          () => {},
          async (_argv: Record<string, unknown>) => {
            const token = await ensureApiKeyInteractive();
            const res = await http("/v1/vms", { token });
            if (res.status === 401) {
              clearApiKey();
              console.error("Unauthorized. Try 'cua auth login' again.");
              process.exit(1);
            }
            if (!res.ok) {
              console.error(`Request failed: ${res.status}`);
              process.exit(1);
            }
            const data = (await res.json()) as VmItem[];
            printVmList(data);
          }
        )
        .command(
          "start <name>",
          "Start a VM",
          (y) => y.positional("name", { type: "string", describe: "VM name" }),
          async (argv: Record<string, unknown>) => {
            const token = await ensureApiKeyInteractive();
            const name = String((argv as any).name);
            const res = await http(`/v1/vms/${encodeURIComponent(name)}/start`, { token, method: "POST" });
            if (res.status === 204) { console.log("Start accepted"); return; }
            if (res.status === 404) { console.error("VM not found"); process.exit(1); }
            if (res.status === 401) { clearApiKey(); console.error("Unauthorized. Try 'cua auth login' again."); process.exit(1); }
            console.error(`Unexpected status: ${res.status}`); process.exit(1);
          }
        )
        .command(
          "stop <name>",
          "Stop a VM",
          (y) => y.positional("name", { type: "string", describe: "VM name" }),
          async (argv: Record<string, unknown>) => {
            const token = await ensureApiKeyInteractive();
            const name = String((argv as any).name);
            const res = await http(`/v1/vms/${encodeURIComponent(name)}/stop`, { token, method: "POST" });
            if (res.status === 202) { const body = (await res.json().catch(() => ({}))) as { status?: string }; console.log(body.status ?? "stopping"); return; }
            if (res.status === 404) { console.error("VM not found"); process.exit(1); }
            if (res.status === 401) { clearApiKey(); console.error("Unauthorized. Try 'cua auth login' again."); process.exit(1); }
            console.error(`Unexpected status: ${res.status}`); process.exit(1);
          }
        )
        .command(
          "restart <name>",
          "Restart a VM",
          (y) => y.positional("name", { type: "string", describe: "VM name" }),
          async (argv: Record<string, unknown>) => {
            const token = await ensureApiKeyInteractive();
            const name = String((argv as any).name);
            const res = await http(`/v1/vms/${encodeURIComponent(name)}/restart`, { token, method: "POST" });
            if (res.status === 202) { const body = (await res.json().catch(() => ({}))) as { status?: string }; console.log(body.status ?? "restarting"); return; }
            if (res.status === 404) { console.error("VM not found"); process.exit(1); }
            if (res.status === 401) { clearApiKey(); console.error("Unauthorized. Try 'cua auth login' again."); process.exit(1); }
            console.error(`Unexpected status: ${res.status}`); process.exit(1);
          }
        )
        .command(
          "vnc <name>",
          "Open NoVNC for a VM in your browser",
          (y) => y.positional("name", { type: "string", describe: "VM name" }),
          async (argv: Record<string, unknown>) => {
            const token = await ensureApiKeyInteractive();
            const name = String((argv as any).name);
            const listRes = await http("/v1/vms", { token });
            if (listRes.status === 401) { clearApiKey(); console.error("Unauthorized. Try 'cua auth login' again."); process.exit(1); }
            if (!listRes.ok) { console.error(`Request failed: ${listRes.status}`); process.exit(1); }
            const vms = (await listRes.json()) as VmItem[];
            const vm = vms.find(v => v.name === name);
            if (!vm) { console.error("VM not found"); process.exit(1); }
            const url = `https://${vm.name}.containers.cloud.trycua.com/vnc.html?autoconnect=true&password=${encodeURIComponent(vm.password)}`;
            console.log(`Opening NoVNC: ${url}`);
            await openInBrowser(url);
          }
        )
        .command(
          "chat <name>",
          "Open CUA dashboard playground for a VM",
          (y) => y.positional("name", { type: "string", describe: "VM name" }),
          async (argv: Record<string, unknown>) => {
            const token = await ensureApiKeyInteractive();
            const name = String((argv as any).name);
            const listRes = await http("/v1/vms", { token });
            if (listRes.status === 401) { clearApiKey(); console.error("Unauthorized. Try 'cua auth login' again."); process.exit(1); }
            if (!listRes.ok) { console.error(`Request failed: ${listRes.status}`); process.exit(1); }
            const vms = (await listRes.json()) as VmItem[];
            const vm = vms.find(v => v.name === name);
            if (!vm) { console.error("VM not found"); process.exit(1); }
            const host = `${vm.name}.containers.cloud.trycua.com`;
            const base = WEBSITE_URL.replace(/\/$/, "");
            const url = `${base}/dashboard/playground?host=${encodeURIComponent(host)}&id=${encodeURIComponent(vm.name)}&name=${encodeURIComponent(vm.name)}&vnc_password=${encodeURIComponent(vm.password)}&fullscreen=true`;
            console.log(`Opening Playground: ${url}`);
            await openInBrowser(url);
          }
        )
        .demandCommand(1, "Specify a vm subcommand")
  );
}
