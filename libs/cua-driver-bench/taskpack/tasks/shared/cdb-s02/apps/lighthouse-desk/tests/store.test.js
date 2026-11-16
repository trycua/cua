"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { CalendarStore, ChatStore, MailStore, verifyAudit } = require("../store.js");

const temporaryDirectories = [];

function workspace() {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "lighthouse-desk-test-"));
  temporaryDirectories.push(directory);
  ["mail", "chat", "calendar"].forEach(function (name) { fs.mkdirSync(path.join(directory, name)); });
  return directory;
}

function writeJson(file, value) {
  fs.writeFileSync(file, JSON.stringify(value, null, 2) + "\n");
}

function cleanDirectory(directory) {
  fs.readdirSync(directory, { withFileTypes: true }).forEach(function (entry) {
    const target = path.join(directory, entry.name);
    if (entry.isDirectory()) cleanDirectory(target);
    else fs.unlinkSync(target);
  });
  fs.rmdirSync(directory);
}

test.after(function () {
  temporaryDirectories.forEach(cleanDirectory);
});

test("mail store is read-only and deterministically ordered", function () {
  const root = workspace();
  const file = path.join(root, "mail", "messages.json");
  writeJson(file, { messages: [
    { id: "m2", sent_at: "2026-08-15T10:00:00Z", subject: "Later" },
    { id: "m1", sent_at: "2026-08-15T09:00:00Z", subject: "Earlier" },
  ] });
  const before = fs.readFileSync(file, "utf8");
  assert.deepEqual(new MailStore(root).list().map(function (item) { return item.id; }), ["m1", "m2"]);
  assert.equal(fs.readFileSync(file, "utf8"), before);
  assert.deepEqual(fs.readdirSync(path.join(root, "mail")), ["messages.json"]);
});

test("protected calendar events reject update and delete without changing the file", function () {
  const root = workspace();
  const file = path.join(root, "calendar", "events.json");
  writeJson(file, { events: [{
    id: "hold", title: "Protected hold", date: "2026-09-01", start: "09:00", end: "10:00",
    room: "North", attendees: [], status: "held", protected: true,
  }] });
  const before = fs.readFileSync(file, "utf8");
  const store = new CalendarStore(root);
  assert.throws(function () {
    store.create({
      id: "another-hold", title: "Another hold", date: "2026-09-01", start: "11:00", end: "12:00",
      room: "South", attendees: [], status: "held", protected: true,
    });
  }, /protected events cannot be created/);
  assert.throws(function () { store.update("hold", { title: "Changed" }); }, /protected event is immutable/);
  assert.throws(function () { store.delete("hold"); }, /protected event is immutable/);
  assert.equal(fs.readFileSync(file, "utf8"), before);
});

test("calendar supports deterministic create, update, and delete", function () {
  const root = workspace();
  const file = path.join(root, "calendar", "events.json");
  writeJson(file, { events: [] });
  const store = new CalendarStore(root);
  store.create({
    id: "evt-2", title: "Planning", date: "2026-09-02", start: "11:00", end: "11:30",
    room: "Cedar", attendees: "Lin, Ada", status: "confirmed",
  });
  store.create({
    id: "evt-1", title: "Review", date: "2026-09-01", start: "09:00", end: "10:00",
    room: "Birch", attendees: [], status: "tentative",
  });
  assert.deepEqual(store.list().map(function (event) { return event.id; }), ["evt-1", "evt-2"]);
  const updated = store.update("evt-2", { room: "Maple", attendees: "Ada" });
  assert.equal(updated.room, "Maple");
  assert.deepEqual(updated.attendees, ["Ada"]);
  assert.equal(store.delete("evt-1").id, "evt-1");
  assert.deepEqual(store.list().map(function (event) { return event.id; }), ["evt-2"]);
  assert.equal(fs.readdirSync(path.join(root, "calendar")).some(function (name) { return name.includes(".tmp-"); }), false);
  assert.throws(function () {
    store.create({
      id: "bad-time", title: "Bad", date: "2026-09-01", start: "9:00", end: "10:00",
      room: "Birch", attendees: [], status: "tentative",
    });
  }, /24-hour HH:MM/);
});

test("calendar delete and recreate restores a canonical unprotected event", function () {
  const root = workspace();
  const file = path.join(root, "calendar", "events.json");
  writeJson(file, { events: [{
    id: "target", title: "Target session", date: "2026-09-01", start: "09:00", end: "10:00",
    room: "Birch", attendees: ["Ada"], status: "tentative", protected: false,
  }] });
  const store = new CalendarStore(root);
  const removed = store.delete("target");
  const recreated = store.create(Object.assign({}, removed, {
    start: "11:00", end: "12:00", room: "Maple", status: "confirmed",
  }));

  assert.deepEqual(recreated, {
    id: "target", title: "Target session", date: "2026-09-01", start: "11:00", end: "12:00",
    room: "Maple", attendees: ["Ada"], status: "confirmed", protected: false,
  });
  assert.deepEqual(JSON.parse(fs.readFileSync(file, "utf8")).events, [recreated]);
});

test("chat blocker writes validate and tampering prevents another append", function () {
  const root = workspace();
  writeJson(path.join(root, "chat", "messages.json"), { messages: [] });
  writeJson(path.join(root, "chat", "blockers.json"), { blockers: [] });
  const store = new ChatStore(root);
  const blocker = {
    conflict_id: "conflict-1",
    affected_session: "session-a",
    conflicting_event: "event-b",
    reason: "Times overlap",
    escalation: "Scheduling owner",
  };
  store.addBlocker(blocker, "test-actor");
  const valid = verifyAudit(path.join(root, "chat"));
  assert.equal(valid.ok, true);
  assert.equal(valid.entries.length, 1);
  assert.match(valid.entries[0].hash, /^[a-f0-9]{64}$/);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(root, "chat", "blockers.json"), "utf8")).blockers, [blocker]);

  const tampered = Object.assign({}, valid.entries[0], { actor: "tampered" });
  fs.writeFileSync(path.join(root, "chat", "audit.ndjson"), JSON.stringify(tampered) + "\n");
  assert.match(verifyAudit(path.join(root, "chat")).error, /hash mismatch/);
  assert.throws(function () {
    store.addBlocker(Object.assign({}, blocker, { conflict_id: "conflict-2" }));
  }, /invalid audit log/);
  assert.equal(JSON.parse(fs.readFileSync(path.join(root, "chat", "blockers.json"), "utf8")).blockers.length, 1);
});

test("chat blocker removal is audited and permits a corrected replacement", function () {
  const root = workspace();
  writeJson(path.join(root, "chat", "messages.json"), { messages: [] });
  writeJson(path.join(root, "chat", "blockers.json"), { blockers: [] });
  const store = new ChatStore(root);
  const mistaken = {
    conflict_id: "conflict-1",
    affected_session: "session-wrong",
    conflicting_event: "event-wrong",
    reason: "Mistaken conflict",
    escalation: "Scheduling owner",
  };
  const corrected = Object.assign({}, mistaken, {
    affected_session: "session-correct",
    conflicting_event: "event-correct",
    reason: "Confirmed conflict",
  });

  store.addBlocker(mistaken);
  assert.deepEqual(store.removeBlocker("conflict-1"), mistaken);
  assert.deepEqual(store.blockers(), []);
  store.addBlocker(corrected);

  const valid = verifyAudit(path.join(root, "chat"));
  assert.equal(valid.ok, true);
  assert.deepEqual(valid.entries.map(function (entry) { return entry.action; }), [
    "blocker.recorded", "blocker.removed", "blocker.recorded",
  ]);
  assert.deepEqual(valid.blockers, [corrected]);
  assert.deepEqual(store.blockers(), [corrected]);
  assert.throws(function () { store.removeBlocker("missing"); }, /unknown blocker/);
});
