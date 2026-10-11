"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { MailStore } = require("../store.js");

const temporaryDirectories = [];

function workspace(document, files) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "ledger-lantern-mail-test-"));
  temporaryDirectories.push(root);
  const mail = path.join(root, "mail");
  const attachments = path.join(mail, "attachments");
  fs.mkdirSync(attachments, { recursive: true });
  fs.writeFileSync(path.join(mail, "messages.json"), JSON.stringify(document, null, 2) + "\n");
  Object.entries(files || {}).forEach(function (entry) {
    fs.writeFileSync(path.join(attachments, entry[0]), entry[1]);
  });
  return root;
}

function cleanDirectory(directory) {
  fs.readdirSync(directory, { withFileTypes: true }).forEach(function (entry) {
    const target = path.join(directory, entry.name);
    if (entry.isDirectory()) cleanDirectory(target);
    else fs.unlinkSync(target);
  });
  fs.rmdirSync(directory);
}

test.after(function () { temporaryDirectories.forEach(cleanDirectory); });

test("messages are newest-first and every returned record is read-only", function () {
  const root = workspace({ messages: [
    { id: "m1", from: "Older Sender", from_address: "older@example.test", to: "Desk", sent_at: "2026-01-01T09:00:00Z", subject: "Older", body: "First", labels: [], attachments: [] },
    { id: "m2", from: "Newer Sender", from_address: "newer@example.test", to: "Desk", sent_at: "2026-01-02T09:00:00Z", subject: "Newer", body: "Second", labels: ["invoice"], attachments: [] },
  ] });
  const before = fs.readFileSync(path.join(root, "mail", "messages.json"), "utf8");
  const messages = new MailStore(root).list();
  assert.deepEqual(messages.map(function (message) { return message.id; }), ["m2", "m1"]);
  assert.equal(messages.every(function (message) { return message.read_only === true; }), true);
  assert.equal(fs.readFileSync(path.join(root, "mail", "messages.json"), "utf8"), before);
});

test("attachment reads are message-scoped and do not change source files", function () {
  const metadata = { id: "a1", name: "invoice.txt", media_type: "text/plain", file: "invoice.txt" };
  const root = workspace({ messages: [
    { id: "m1", from: "Vendor", from_address: "vendor@example.test", to: "Desk", sent_at: "2026-01-01T09:00:00Z", subject: "Invoice", body: "Attached.", labels: [], attachments: [metadata] },
    { id: "m2", from: "Other", from_address: "other@example.test", to: "Desk", sent_at: "2026-01-01T10:00:00Z", subject: "Other", body: "No attachment.", labels: [], attachments: [] },
  ] }, { "invoice.txt": "INVOICE A1\nTOTAL $12.34\n" });
  const file = path.join(root, "mail", "attachments", "invoice.txt");
  const before = fs.readFileSync(file, "utf8");
  const store = new MailStore(root);
  assert.deepEqual(store.attachment("m1", "a1"), {
    id: "a1", name: "invoice.txt", media_type: "text/plain", content: before, read_only: true,
  });
  assert.throws(function () { store.attachment("m2", "a1"); }, /unknown attachment/);
  assert.throws(function () { store.attachment("missing", "a1"); }, /unknown message/);
  assert.equal(fs.readFileSync(file, "utf8"), before);
});

test("attachment paths cannot escape the fixture attachment directory", function () {
  const root = workspace({ messages: [{
    id: "m1", from: "Vendor", from_address: "vendor@example.test", to: "Desk", sent_at: "2026-01-01T09:00:00Z", subject: "Invoice", body: "Attached.", labels: [],
    attachments: [{ id: "escape", name: "outside.txt", media_type: "text/plain", file: "../messages.json" }],
  }] });
  assert.throws(function () { new MailStore(root).attachment("m1", "escape"); }, /must stay inside/);
});

test("attachment symlinks cannot escape the fixture attachment directory", {
  skip: process.platform === "win32" ? "creating symlinks requires extra Windows privileges" : false,
}, function () {
  const root = workspace({ messages: [{
    id: "m1", from: "Vendor", from_address: "vendor@example.test", to: "Desk", sent_at: "2026-01-01T09:00:00Z", subject: "Invoice", body: "Attached.", labels: [],
    attachments: [{ id: "link", name: "link.txt", media_type: "text/plain", file: "link.txt" }],
  }] });
  fs.symlinkSync(path.join(root, "mail", "messages.json"), path.join(root, "mail", "attachments", "link.txt"));
  assert.throws(function () { new MailStore(root).attachment("m1", "link"); }, /bounded regular file/);
});

test("benchmark fixture has four target invoices, one support record, and one decoy", function () {
  const fixture = path.resolve(__dirname, "../../../fixture");
  const store = new MailStore(fixture);
  const messages = store.list();
  assert.equal(messages.length, 6);
  assert.deepEqual(messages.flatMap(function (message) {
    return message.attachments.map(function (attachment) { return attachment.id; });
  }).sort(), ["att-decoy-menu", "att-nar-773", "att-ocp-4821", "att-ocp-4827", "att-receiving-log", "att-svl-119"]);

  const targetText = [
    store.attachment("LLM-104", "att-ocp-4821").content,
    store.attachment("LLM-109", "att-ocp-4827").content,
    store.attachment("LLM-113", "att-svl-119").content,
    store.attachment("LLM-118", "att-nar-773").content,
  ].join("\n");
  ["OCP-4821", "$1,248.60", "OCP-4827", "$846.50", "SVL-119", "$392.40", "NAR-773", "$275.00"].forEach(function (expected) {
    assert.match(targetText, new RegExp(expected.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  });
  assert.doesNotMatch(store.attachment("LLM-121", "att-receiving-log").content, /\$[0-9]/);
  assert.match(store.attachment("LLM-125", "att-decoy-menu").content, /Autumn open-house menu/);
});
