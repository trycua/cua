// cua bridge: a byte pipe between an MCP client on stdio and the host.
//
//   node bridge.mjs <bridge_dir>
//
// The harness starts this as a stdio MCP server. It knows nothing about the
// protocol: every byte on stdin is appended to <bridge_dir>/<id>.in, and every
// byte the host appends to <bridge_dir>/<id>.out is copied to stdout. The
// host (the cua daemon, over the Space's authenticated spacesd channel) reads
// the .in file, answers with its own MCP server as the persistent agent that
// owns this run, and appends to .out. No token or network path to the host
// ever enters the guest.

import fs from "node:fs";
import path from "node:path";

const dir = process.argv[2];
if (!dir) {
  console.error("usage: bridge.mjs <bridge_dir>");
  process.exit(2);
}
fs.mkdirSync(dir, { recursive: true, mode: 0o700 });
const id = `${Date.now()}-${process.pid}`;
const inFile = path.join(dir, `${id}.in`);
const outFile = path.join(dir, `${id}.out`);
fs.writeFileSync(inFile, "", { mode: 0o600 });
fs.writeFileSync(outFile, "", { mode: 0o600 });

process.stdin.on("data", (chunk) => fs.appendFileSync(inFile, chunk));
process.stdin.on("end", () => {
  fs.appendFileSync(path.join(dir, `${id}.closed`), "");
  setTimeout(() => process.exit(0), 200);
});

let offset = 0;
const pump = () => {
  let size = 0;
  try {
    size = fs.statSync(outFile).size;
  } catch {
    return;
  }
  if (size <= offset) return;
  const fd = fs.openSync(outFile, "r");
  try {
    const buf = Buffer.alloc(size - offset);
    const n = fs.readSync(fd, buf, 0, buf.length, offset);
    offset += n;
    process.stdout.write(buf.subarray(0, n));
  } finally {
    fs.closeSync(fd);
  }
};
setInterval(pump, 50);
