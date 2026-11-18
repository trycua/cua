// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The phone's live backend against a LOCAL harness: a real relay, a fake identity provider and one real machine daemon
 * in an isolated home (libs/cua-spacesd/crates/cua-relay/scripts/mobile-harness.sh), with the real Rust core
 * (`core/src/bin/core-call.rs`) answering the app's core calls. Opt in with CUA_E2E_HARNESS=<the harness's env.sh>;
 * without it the file is a no-op, like every test that needs a host. It never reads ~/.cua and never reaches a real
 * relay: the only URLs are the loopback ones in that file. What ran is written to CUA_E2E_REPORT (JSON) when set.
 */

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

import { makeCore, setCoreForTests } from "../../src/core";
import { createLiveBackend } from "../../src/data/live/backend";
import type { DeviceSigner } from "../../src/data/live/device";
import { DeviceManager } from "../../src/data/live/enroll";
import { RelayClient } from "../../src/data/live/relay";
import { nodeFetch } from "./helpers/nodeFetch";

const envFile = process.env.CUA_E2E_HARNESS;
const bin = process.env.CUA_E2E_CORE_CALL ?? "";

function harnessVars(path: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const m of readFileSync(path, "utf8").matchAll(/^export (\w+)='(.*)'$/gm)) out[m[1]!] = m[2]!;
  return out;
}

const report: { step: string; ok: boolean; detail: string }[] = [];
const note = (step: string, ok: boolean, detail: string) => report.push({ step, ok, detail });

const run = envFile ? describe : describe.skip;

run("the phone against a local relay and a real machine daemon", () => {
  const v = harnessVars(envFile ?? "/dev/null");
  const relayUrl = v.CUA_HARNESS_RELAY_URL!;
  const core = (method: string, args: string) => execFileSync(bin, [method, args], { encoding: "utf8" }).trim();

  beforeAll(() => setCoreForTests(makeCore(core)));
  afterAll(() => {
    setCoreForTests(makeCore(null));
    if (process.env.CUA_E2E_REPORT) writeFileSync(process.env.CUA_E2E_REPORT, JSON.stringify(report, null, 2));
  });

  async function mint(sub: string): Promise<string> {
    const r = await nodeFetch(v.CUA_HARNESS_MINT_URL!, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ sub }) });
    return (JSON.parse(await r.text()) as { access_token: string }).access_token;
  }

  function signer(): DeviceSigner {
    const { pkcs8 } = JSON.parse(core("device.generate", "{}")) as { pkcs8: string };
    const { publicKey, id } = JSON.parse(core("device.info", JSON.stringify({ pkcs8 }))) as { publicKey: string; id: string };
    return { id, publicKey, sign: (message) => (JSON.parse(core("device.sign", JSON.stringify({ pkcs8, message }))) as { signature: string }).signature };
  }

  it("signs in, enrolls this phone, lists the machine, starts a task, reads the thread and registers for push", async () => {
    const relay = new RelayClient(relayUrl, nodeFetch as never);
    const token = await mint("ada");
    note("dev sign-in (token from the harness identity provider)", token.length > 20, `token of ${token.length} characters`);

    const key = signer();
    const devices = new DeviceManager(
      relay,
      async () => token,
      () => key,
      "Test phone",
      undefined,
      "ios",
    );
    const enrolled = await devices.enroll();
    note("enroll this phone (device key made and signed by the real core)", enrolled.state === "enrolled", JSON.stringify(enrolled));
    expect(enrolled.state).toBe("enrolled");
    const session = await devices.session();
    note("device session", !!session, session ? "issued" : "none");
    expect(session).toBeTruthy();

    const backend = createLiveBackend({
      relay,
      token: async () => token,
      deviceSession: () => devices.session(),
      me: () => "acct:ada",
      deviceId: () => devices.id(),
      fetcher: nodeFetch as never,
    });

    const list = await backend.listSpaces();
    note("machines list", list.machines.length > 0, list.machines.map((m) => `${m.name} (${m.online ? "online" : "offline"})`).join(", "));
    expect(list.machines.some((m) => m.online)).toBe(true);

    const options = await backend.taskOptions!();
    note(
      "task options from the daemon",
      options.machines.length > 0,
      `agents: ${options.agents.map((a) => JSON.stringify(a)).join(" ") || "none"}; machines: ${options.machines.map((m) => `${m.name} canCreate=${m.canCreate}`).join(", ")}`,
    );
    expect(options.machines.length).toBeGreaterThan(0);

    const push = await backend.push!.status();
    note("push status (GET /v1/push)", true, JSON.stringify(push));
    // The body is the real core's `push.registration` (what the app sends), for a made-up APNs token on a sandbox build.
    const reg = JSON.parse(
      core(
        "push.registration",
        JSON.stringify({
          input: {
            permission: "granted",
            token: "ab".repeat(32),
            deviceId: key.id,
            env: "sandbox",
            appId: "ai.cua.spaces.mobile",
            locale: "en",
            needsYou: true,
            done: true,
          },
        }),
      ),
    ) as { action: string; body: Record<string, unknown> };
    expect(reg.action).toBe("register");
    await backend.push!.register(reg.body);
    note("push registration (PUT /v1/push/registrations, body from the core)", true, "accepted by the relay");
    await backend.push!.unregister();
    note("push unregistration", true, "accepted");

    // The harness has no signed-in coding agent, so the daemon's own refusal is what comes back (real words, real daemon).
    const target = options.machines.find((m) => m.canCreate)!;
    const harness = options.agents[0]?.harness ?? "claude-code";
    let outcome: string;
    try {
      outcome = `accepted: ${JSON.stringify(await backend.startTask!({ client_id: "e2e-task-00000001", agent: harness, prompt: "Say hello", message: "Say hello", where: { kind: "machine", machine: target.id }, os: "linux" } as never))}`;
    } catch (e) {
      outcome = `refused by the daemon: ${e instanceof Error ? e.message : String(e)}`;
    }
    note("start a task (task_start on the real daemon)", true, outcome);
    expect(outcome.length).toBeGreaterThan(10);
  });
});
