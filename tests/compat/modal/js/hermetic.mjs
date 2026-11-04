// The unmodified `modal` npm SDK against the gateway over mock Spaces
// (the mock's process vocabulary: echo, fail N, cat, ticker, sleep MS).
//
//   node hermetic.mjs <env.json>
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";

Object.assign(process.env, JSON.parse(readFileSync(process.argv[2], "utf8")));
const modal = await import("modal");
const { ModalClient } = modal;

let passed = 0;
const failures = [];
async function check(name, fn) {
  try {
    await fn();
    passed++;
    console.log(`PASS ${name}`);
  } catch (e) {
    failures.push(name);
    console.log(`FAIL ${name}: ${e?.constructor?.name}: ${e?.message}`);
    console.log(e?.stack);
  }
}
function eq(a, b, what) {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${what ?? "value"}: ${JSON.stringify(a)} !== ${JSON.stringify(b)}`);
}
async function rejects(cls, fn) {
  try {
    await fn();
  } catch (e) {
    if (!(e instanceof cls)) throw new Error(`expected ${cls.name}, got ${e?.constructor?.name}: ${e?.message}`);
    return e;
  }
  throw new Error(`expected ${cls.name}`);
}

const client = new ModalClient();
const app = await client.apps.fromName("js-hermetic", { createIfMissing: true });
const image = client.images.fromRegistry("ghcr.io/trycua/linux:24.04");
const create = (command, params = {}) =>
  client.sandboxes.create(app, image, { command, timeoutMs: 120_000, ...params });

const sb = await create(["sleep", "600000"]);
try {
  await check("exec_stdout_and_wait", async () => {
    const p = await sb.exec(["echo", "hi", "there"]);
    eq(await p.stdout.readText(), "hi there\n");
    eq(await p.wait(), 0);
  });
  await check("exec_exit_code_and_stderr", async () => {
    const p = await sb.exec(["fail", "7"]);
    eq(await p.wait(), 7);
    eq(await p.stderr.readText(), "failed\n");
  });
  await check("exec_stdin_eof", async () => {
    const p = await sb.exec(["cat"]);
    await p.stdin.writeText("alpha\n");
    await p.stdin.writeText("beta\n");
    await p.stdin.close();
    eq(await p.stdout.readText(), "alpha\nbeta\n");
    eq(await p.wait(), 0);
  });
  await check("exec_binary_mode", async () => {
    const p = await sb.exec(["echo", "a"], { mode: "binary" });
    eq(Array.from(await p.stdout.readBytes()), [97, 10]);
  });
  await check("filesystem_roundtrip_and_errors", async () => {
    const fs = sb.filesystem;
    await fs.writeText("héllo\n", "/tmp/js-fs/a.txt");
    eq(await fs.readText("/tmp/js-fs/a.txt"), "héllo\n");
    const st = await fs.stat("/tmp/js-fs/a.txt");
    eq([st.type, st.size], ["file", 7]);
    eq((await fs.listFiles("/tmp/js-fs")).map((e) => e.name), ["a.txt"]);
    await rejects(modal.SandboxFilesystemNotFoundError, () => fs.readText("/tmp/js-fs/missing"));
    await fs.remove("/tmp/js-fs/a.txt");
  });
  await check("tags_and_list", async () => {
    const s = await create(["sleep", "600000"], { tags: { suite: "js" } });
    try {
      eq(await s.getTags(), { suite: "js" });
      await s.setTags({ suite: "js", n: "2" });
      eq((await s.getTags()).n, "2");
      let found = false;
      for await (const x of client.sandboxes.list({ appId: app.appId, tags: { suite: "js" } })) {
        if (x.sandboxId === s.sandboxId) found = true;
      }
      if (!found) throw new Error("not listed");
    } finally {
      await s.terminate();
    }
  });
  await check("fromId_and_terminate", async () => {
    const s = await create(["sleep", "600000"]);
    const again = await client.sandboxes.fromId(s.sandboxId);
    const p = await again.exec(["echo", "via id"]);
    eq(await p.stdout.readText(), "via id\n");
    eq(await s.terminate({ wait: true }), 137);
  });
  await check("entrypoint_output", async () => {
    const s = await create(["echo", "from the entrypoint"]);
    eq(await s.stdout.readText(), "from the entrypoint\n");
    eq(await s.wait(), 0);
  });
  await check("filesystem_dirs_copy_watch_api", async () => {
    const fs = sb.filesystem;
    await fs.makeDirectory("/tmp/js-fs2/sub/deep", { createParents: true });
    await fs.writeBytes(new Uint8Array([0, 255, 1]), "/tmp/js-fs2/sub/b.bin");
    eq(Array.from(await fs.readBytes("/tmp/js-fs2/sub/b.bin")), [0, 255, 1]);
    await rejects(modal.SandboxFilesystemDirectoryNotEmptyError, () => fs.remove("/tmp/js-fs2/sub"));
    await fs.remove("/tmp/js-fs2", { recursive: true });
    await rejects(modal.SandboxFilesystemNotFoundError, () => fs.stat("/tmp/js-fs2"));
  });
  await check("logs_and_poll_and_tunnels", async () => {
    eq(await sb.poll(), null);
    const s = await create(["echo", "js-log-line"], { encryptedPorts: [8080] });
    try {
      await s.wait();
      const t = await s.tunnels();
      if (!t[8080] || !t[8080].url.startsWith("https://")) throw new Error(`tunnel: ${JSON.stringify(t)}`);
    } finally {
      await s.terminate();
    }
  });
  await check("connect_token", async () => {
    const t = await sb.createConnectToken({ port: 8080, userMetadata: "m" });
    if (!t.url.startsWith("https://8080-") || !t.token) throw new Error(JSON.stringify(t));
  });
  await check("snapshot_filesystem_and_sandbox_from_it", async () => {
    const img = await sb.snapshotFilesystem();
    const s = await client.sandboxes.create(app, img, { command: ["sleep", "600000"], timeoutMs: 60_000 });
    await s.terminate();
  });
  await check("experimental_name_and_memory_snapshot", async () => {
    const s = await create(["sleep", "600000"], { experimentalEnableSnapshot: true });
    try {
      await s.experimentalSetName("js-named");
      const again = await client.sandboxes.experimentalFromName(app.name, "js-named");
      eq(again.sandboxId, s.sandboxId);
      const snap = await s.experimentalSnapshot();
      if (!snap.snapshotId) throw new Error("no snapshot id");
      const restored = await client.sandboxes.experimentalFromSnapshot(snap);
      if (restored.sandboxId === s.sandboxId) throw new Error("the restore is the same sandbox");
      await restored.terminate();
      let n = 0;
      for await (const x of client.sandboxes.experimentalList({ appId: app.appId })) if (x.sandboxId === s.sandboxId) n++;
      eq(n, 1);
    } finally {
      await s.terminate();
    }
  });
  await check("logs_fetch_and_tail", async () => {
    const s = await create(["echo", "js-log-line"]);
    try {
      await s.wait();
      let text = "";
      for await (const e of s.logs.tail({ entries: 10 })) text += e.message;
      if (!text.includes("js-log-line")) throw new Error(`tail: ${text}`);
    } finally {
      await s.terminate();
    }
  });
  await check("snapshot_directory_and_unmount_errors", async () => {
    await rejects(Error, () => sb.unmountImage("/not-mounted"));
    await rejects(Error, () => sb.waitUntilReady(1_000).then(() => { throw new Error("no probe, no wait"); }));
  });
  await check("named_lookup_and_listing", async () => {
    const s = await create(["sleep", "600000"], { name: "js-lookup" });
    try {
      eq((await client.sandboxes.fromName(app.name, "js-lookup")).sandboxId, s.sandboxId);
    } finally {
      await s.terminate();
    }
    await rejects(modal.NotFoundError, () => client.sandboxes.fromName(app.name, "js-lookup"));
  });
  await check("volumes_secrets_queues", async () => {
    const vol = await client.volumes.fromName("js-vol", { createIfMissing: true });
    const again = await client.volumes.fromName("js-vol");
    eq(again.volumeId, vol.volumeId);
    const ro = vol.withMountOptions({ readOnly: true });
    if (!ro) throw new Error("no view");
    await client.volumes.delete("js-vol");
    await rejects(modal.NotFoundError, () => client.volumes.fromName("js-vol"));
    const sec = await client.secrets.fromObject({ A: "1" });
    // A Secret is resolved when a Sandbox that uses it is created.
    const withSecret = await create(["sleep", "600000"], { secrets: [sec] });
    await withSecret.terminate();
    const named = await client.secrets.fromName("js-secret").catch((e) => e);
    if (!(named instanceof modal.NotFoundError)) throw new Error("expected NotFoundError for an unknown secret");
    const q = await client.queues.fromName("js-q", { createIfMissing: true });
    await q.put(1);
    await q.putMany([2, 3]);
    eq(await q.len(), 3);
    eq(await q.get(), 1);
    eq(await q.getMany(5, { timeoutMs: 100 }), [2, 3]);
    await client.queues.delete("js-q");
  });
  await check("images_registry_and_dockerfile", async () => {
    const img = client.images.fromRegistry("ghcr.io/trycua/linux:24.04").dockerfileCommands(["RUN echo js-layer", "ENV JSV=1"]);
    const built = await img.build(app);
    if (!built.imageId.startsWith("im-")) throw new Error(built.imageId);
    const s = await client.sandboxes.create(app, built, { command: ["sleep", "600000"], timeoutMs: 60_000 });
    await s.terminate();
  });
  await check("unsupported_is_a_modal_error", async () => {
    await rejects(Error, () => create(["sleep", "1"], { outboundCidrAllowlist: ["10.0.0.0/8"] }));
  });
} finally {
  await sb.terminate();
}
console.log(`${passed} passed, ${failures.length} failed: ${JSON.stringify(failures)}`);
process.exit(failures.length ? 1 : 0);
