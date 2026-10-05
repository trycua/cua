#!/usr/bin/env node
const crypto = require("node:crypto");

const seed = Buffer.from(
  "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
  "hex",
);
const privateKey = crypto.createPrivateKey({
  key: Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"),
    seed,
  ]),
  format: "der",
  type: "pkcs8",
});
const publicDer = crypto.createPublicKey(privateKey).export({
  format: "der",
  type: "spki",
});

const chunks = [];
process.stdin.on("data", (chunk) => chunks.push(chunk));
process.stdin.on("end", () => {
  const message = Buffer.concat(chunks);
  process.stdout.write(
    JSON.stringify({
      public_key_base64: publicDer.subarray(-32).toString("base64"),
      signature: crypto.sign(null, message, privateKey).toString("base64"),
    }),
  );
});
