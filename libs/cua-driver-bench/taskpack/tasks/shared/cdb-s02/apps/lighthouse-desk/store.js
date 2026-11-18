"use strict";

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const AUDIT_GENESIS = "signal-quay-chat-genesis";
const BLOCKER_FIELDS = ["conflict_id", "affected_session", "conflicting_event", "reason", "escalation"];
const EVENT_FIELDS = ["id", "title", "date", "start", "end", "room", "attendees", "status"];

function canonical(value) {
  if (Array.isArray(value)) {
    return "[" + value.map(canonical).join(",") + "]";
  }
  if (value && typeof value === "object") {
    return "{" + Object.keys(value).sort().map(function (key) {
      return JSON.stringify(key) + ":" + canonical(value[key]);
    }).join(",") + "}";
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return crypto.createHash("sha256").update(value, "utf8").digest("hex");
}

function compare(left, right) {
  const a = String(left === undefined || left === null ? "" : left);
  const b = String(right === undefined || right === null ? "" : right);
  return a < b ? -1 : (a > b ? 1 : 0);
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function collection(document, key) {
  if (Array.isArray(document)) {
    return document;
  }
  if (document && Array.isArray(document[key])) {
    return document[key];
  }
  throw new Error(path.basename(key) + " data must be an array or an object with a " + key + " array");
}

function atomicWriteJson(file, value) {
  const directory = path.dirname(file);
  const temporary = path.join(directory, "." + path.basename(file) + ".tmp-" + process.pid + "-" + crypto.randomBytes(6).toString("hex"));
  const data = JSON.stringify(value, null, 2) + "\n";
  const mode = fs.existsSync(file) ? fs.statSync(file).mode & 0o777 : 0o600;
  let descriptor;
  try {
    descriptor = fs.openSync(temporary, "wx", mode);
    fs.writeFileSync(descriptor, data, "utf8");
    fs.fsyncSync(descriptor);
    fs.closeSync(descriptor);
    descriptor = undefined;
    fs.renameSync(temporary, file);
  } catch (error) {
    if (descriptor !== undefined) {
      fs.closeSync(descriptor);
    }
    try { fs.unlinkSync(temporary); } catch (_cleanupError) { /* nothing to clean */ }
    throw error;
  }
}

function text(value, field) {
  const result = String(value === undefined || value === null ? "" : value).trim();
  if (!result) {
    throw new Error(field + " is required");
  }
  return result;
}

function clock(value, field) {
  const result = text(value, field);
  const match = /^(\d{2}):(\d{2})$/.exec(result);
  if (!match || Number(match[1]) > 23 || Number(match[2]) > 59) {
    throw new Error(field + " must use 24-hour HH:MM");
  }
  return result;
}

function normalizeAttendees(value) {
  const values = Array.isArray(value) ? value : String(value || "").split(",");
  return values.map(function (item) { return String(item).trim(); }).filter(Boolean);
}

function normalizeEvent(input) {
  const source = input && typeof input === "object" ? input : {};
  const event = {
    id: text(source.id, "id"),
    title: text(source.title, "title"),
    date: text(source.date, "date"),
    start: clock(source.start, "start"),
    end: clock(source.end, "end"),
    room: String(source.room || "").trim(),
    attendees: normalizeAttendees(source.attendees),
    status: text(source.status, "status"),
    protected: false,
  };
  if (event.end <= event.start) {
    throw new Error("end must be after start");
  }
  return event;
}

function orderedEvents(events) {
  return events.slice().sort(function (left, right) {
    return compare(left.date, right.date) || compare(left.start, right.start) || compare(left.id, right.id);
  });
}

class MailStore {
  constructor(workspace) {
    this.file = path.join(workspace, "mail", "messages.json");
  }

  list() {
    return collection(readJson(this.file), "messages").slice().sort(function (left, right) {
      return compare(left.sent_at || left.date || left.timestamp, right.sent_at || right.date || right.timestamp) ||
        compare(left.id, right.id);
    });
  }
}

class ChatStore {
  constructor(workspace) {
    this.directory = path.join(workspace, "chat");
    this.messagesPath = path.join(this.directory, "messages.json");
    this.blockersPath = path.join(this.directory, "blockers.json");
    this.auditPath = path.join(this.directory, "audit.ndjson");
  }

  messages() {
    return collection(readJson(this.messagesPath), "messages").slice().sort(function (left, right) {
      return compare(left.sent_at || left.timestamp || left.date, right.sent_at || right.timestamp || right.date) ||
        compare(left.id, right.id);
    });
  }

  blockers() {
    if (!fs.existsSync(this.blockersPath)) {
      return [];
    }
    return collection(readJson(this.blockersPath), "blockers").slice().sort(function (left, right) {
      return compare(left.conflict_id, right.conflict_id);
    });
  }

  auditEntries() {
    if (!fs.existsSync(this.auditPath)) {
      return [];
    }
    return fs.readFileSync(this.auditPath, "utf8").split("\n").filter(function (line) {
      return line.trim() !== "";
    }).map(function (line) { return JSON.parse(line); });
  }

  addBlocker(input, actor) {
    const source = input && typeof input === "object" ? input : {};
    const blocker = {};
    BLOCKER_FIELDS.forEach(function (field) { blocker[field] = text(source[field], field); });

    let document = { blockers: [] };
    if (fs.existsSync(this.blockersPath)) {
      document = readJson(this.blockersPath);
    }
    const blockers = collection(document, "blockers");
    if (blockers.some(function (item) { return item.conflict_id === blocker.conflict_id; })) {
      throw new Error("conflict_id already exists: " + blocker.conflict_id);
    }

    const audit = verifyAudit(this.directory);
    if (!audit.ok) {
      throw new Error("refusing to append to invalid audit log: " + audit.error);
    }
    if (canonical(audit.blockers) !== canonical(blockers)) {
      throw new Error("refusing to change blockers that do not match the audit log");
    }
    blockers.push(blocker);
    blockers.sort(function (left, right) { return compare(left.conflict_id, right.conflict_id); });

    this.appendBlockerAction("blocker.recorded", blocker, actor, audit, document);
    return blocker;
  }

  removeBlocker(conflictId, actor) {
    const id = text(conflictId, "conflict_id");
    if (!fs.existsSync(this.blockersPath)) {
      throw new Error("unknown blocker " + id);
    }
    const document = readJson(this.blockersPath);
    const blockers = collection(document, "blockers");
    const index = blockers.findIndex(function (item) { return item.conflict_id === id; });
    if (index === -1) {
      throw new Error("unknown blocker " + id);
    }
    const audit = verifyAudit(this.directory);
    if (!audit.ok) {
      throw new Error("refusing to append to invalid audit log: " + audit.error);
    }
    if (canonical(audit.blockers) !== canonical(blockers)) {
      throw new Error("refusing to change blockers that do not match the audit log");
    }
    const blocker = blockers.splice(index, 1)[0];
    this.appendBlockerAction("blocker.removed", blocker, actor, audit, document);
    return blocker;
  }

  appendBlockerAction(action, blocker, actor, audit, document) {
    const at = new Date().toISOString();
    const entry = {
      action: action,
      actor: actor || "signal-quay-chat",
      at: at,
      blocker: blocker,
      prev: audit.entries.length ? audit.entries[audit.entries.length - 1].hash : AUDIT_GENESIS,
    };
    entry.hash = sha256(entry.prev + "\n" + canonical({
      action: entry.action,
      actor: entry.actor,
      at: entry.at,
      blocker: entry.blocker,
    }));

    atomicWriteJson(this.blockersPath, document);
    fs.appendFileSync(this.auditPath, JSON.stringify(entry) + "\n", { encoding: "utf8", mode: 0o600 });
  }
}

class CalendarStore {
  constructor(workspace) {
    this.file = path.join(workspace, "calendar", "events.json");
  }

  read() {
    return readJson(this.file);
  }

  list() {
    return orderedEvents(collection(this.read(), "events"));
  }

  create(input) {
    if (input && input.protected === true) {
      throw new Error("protected events cannot be created");
    }
    const document = this.read();
    const events = collection(document, "events");
    const event = normalizeEvent(input);
    if (events.some(function (item) { return item.id === event.id; })) {
      throw new Error("event id already exists: " + event.id);
    }
    events.push(event);
    events.splice(0, events.length, ...orderedEvents(events));
    atomicWriteJson(this.file, document);
    return event;
  }

  update(id, changes) {
    const document = this.read();
    const events = collection(document, "events");
    const index = events.findIndex(function (event) { return event.id === id; });
    if (index === -1) {
      throw new Error("unknown event " + id);
    }
    if (events[index].protected === true) {
      throw new Error("protected event is immutable: " + id);
    }
    Object.keys(changes || {}).forEach(function (field) {
      if (EVENT_FIELDS.indexOf(field) === -1) {
        throw new Error("field is not editable: " + field);
      }
    });
    const normalized = normalizeEvent(Object.assign({}, events[index], changes));
    const updated = Object.assign({}, events[index], normalized);
    if (updated.id !== id && events.some(function (event) { return event.id === updated.id; })) {
      throw new Error("event id already exists: " + updated.id);
    }
    events[index] = updated;
    events.splice(0, events.length, ...orderedEvents(events));
    atomicWriteJson(this.file, document);
    return updated;
  }

  delete(id) {
    const document = this.read();
    const events = collection(document, "events");
    const index = events.findIndex(function (event) { return event.id === id; });
    if (index === -1) {
      throw new Error("unknown event " + id);
    }
    if (events[index].protected === true) {
      throw new Error("protected event is immutable: " + id);
    }
    const removed = events.splice(index, 1)[0];
    atomicWriteJson(this.file, document);
    return removed;
  }
}

function verifyAudit(directory) {
  const store = Object.create(ChatStore.prototype);
  store.auditPath = path.join(directory, "audit.ndjson");
  let entries;
  try {
    entries = store.auditEntries();
  } catch (error) {
    return { ok: false, entries: [], error: "unreadable audit log: " + error.message };
  }
  let previous = AUDIT_GENESIS;
  const live = new Map();
  for (let index = 0; index < entries.length; index += 1) {
    const entry = entries[index];
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
      return { ok: false, entries: entries, blockers: [], error: "malformed entry at " + index };
    }
    const entryKeys = Object.keys(entry).sort();
    if (canonical(entryKeys) !== canonical(["action", "actor", "at", "blocker", "hash", "prev"])) {
      return { ok: false, entries: entries, blockers: [], error: "malformed entry fields at " + index };
    }
    if ((entry.action !== "blocker.recorded" && entry.action !== "blocker.removed") ||
        typeof entry.actor !== "string" || !entry.actor || typeof entry.at !== "string" || !entry.at ||
        !entry.blocker || typeof entry.blocker !== "object" || Array.isArray(entry.blocker) ||
        canonical(Object.keys(entry.blocker).sort()) !== canonical(BLOCKER_FIELDS.slice().sort()) ||
        BLOCKER_FIELDS.some(function (field) { return typeof entry.blocker[field] !== "string" || !entry.blocker[field].trim(); })) {
      return { ok: false, entries: entries, blockers: [], error: "malformed entry payload at " + index };
    }
    if (entry.prev !== previous) {
      return { ok: false, entries: entries, blockers: [], error: "chain break at entry " + index };
    }
    const expected = sha256(previous + "\n" + canonical({
      action: entry.action,
      actor: entry.actor,
      at: entry.at,
      blocker: entry.blocker,
    }));
    if (entry.hash !== expected) {
      return { ok: false, entries: entries, blockers: [], error: "hash mismatch at entry " + index };
    }
    const id = entry.blocker.conflict_id;
    if (entry.action === "blocker.recorded") {
      if (live.has(id)) {
        return { ok: false, entries: entries, blockers: [], error: "duplicate live blocker at entry " + index };
      }
      live.set(id, entry.blocker);
    } else {
      if (!live.has(id) || canonical(live.get(id)) !== canonical(entry.blocker)) {
        return { ok: false, entries: entries, blockers: [], error: "removal does not match a live blocker at entry " + index };
      }
      live.delete(id);
    }
    previous = entry.hash;
  }
  const blockers = Array.from(live.values()).sort(function (left, right) {
    return compare(left.conflict_id, right.conflict_id);
  });
  return { ok: true, entries: entries, blockers: blockers, error: null };
}

module.exports = {
  AUDIT_GENESIS,
  BLOCKER_FIELDS,
  CalendarStore,
  ChatStore,
  EVENT_FIELDS,
  MailStore,
  atomicWriteJson,
  canonical,
  sha256,
  verifyAudit,
};
