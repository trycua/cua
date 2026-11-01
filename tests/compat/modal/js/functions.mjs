// The unmodified `modal` npm SDK calling Python Functions through the
// gateway (the Python suite deployed "hermetic-deployed" first). Inputs and
// outputs are CBOR, which Python Functions accept.
//
//   node functions.mjs <env.json>
import { readFileSync } from "node:fs";

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
function eq(a, b) {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${JSON.stringify(a)} !== ${JSON.stringify(b)}`);
}

const client = new ModalClient();
const square = await client.functions.fromName("hermetic-deployed", "square");
await check("remote", async () => eq(await square.remote([6]), 36));
await check("spawn_and_get", async () => {
  const call = await square.spawn([7]);
  eq(await call.get(), 49);
  const again = await client.functionCalls.fromId(call.functionCallId);
  eq(await again.get(), 49);
});
await check("remote_error_is_a_remote_error", async () => {
  const boom = await client.functions.fromName("hermetic-deployed", "boom");
  try {
    await boom.remote(["js-boom"]);
  } catch (e) {
    if (!(e instanceof modal.RemoteError)) throw new Error(`${e?.constructor?.name}: ${e?.message}`);
    return;
  }
  throw new Error("expected an error");
});
await check("stats", async () => {
  const s = await square.getCurrentStats();
  if (typeof s.backlog !== "number") throw new Error(JSON.stringify(s));
});
await check("missing_function", async () => {
  try {
    await client.functions.fromName("hermetic-deployed", "nope");
  } catch (e) {
    if (!(e instanceof modal.NotFoundError)) throw e;
    return;
  }
  throw new Error("expected NotFoundError");
});
console.log(`${passed} passed, ${failures.length} failed: ${JSON.stringify(failures)}`);
process.exit(failures.length ? 1 : 0);
