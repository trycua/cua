"use strict";
const fs = require("fs");
const path = require("path");

const CANDIDATE_IDS = ["RWC-193", "RWC-201", "RWC-206", "RWC-212"];
const STATUSES = ["open", "duplicate"];
const EVIDENCE_FIELDS = ["wiki_ref", "repro_url", "log_ref"];

class CaseStore {
  constructor(workspace) {
    this.workspace = path.resolve(workspace);
    this.casesPath = path.join(this.workspace, "cases", "cases.json");
  }

  read() { return JSON.parse(fs.readFileSync(this.casesPath, "utf8")); }
  list() { return this.read().cases.map(function (item) { return item; }); }
  get(id) {
    const item = this.read().cases.find(function (entry) { return entry.id === id; });
    if (!item) { throw new Error("unknown case " + id); }
    return item;
  }

  update(id, changes) {
    if (!CANDIDATE_IDS.includes(id)) { throw new Error("case is not editable: " + id); }
    const keys = Object.keys(changes || {});
    keys.forEach(function (key) {
      if (!["status", "duplicate_of", "evidence"].includes(key)) {
        throw new Error("field is not editable: " + key);
      }
    });
    const document = this.read();
    const item = document.cases.find(function (entry) { return entry.id === id; });
    if (!item) { throw new Error("unknown case " + id); }
    if (changes.status !== undefined) {
      if (!STATUSES.includes(changes.status)) { throw new Error("invalid status " + changes.status); }
      item.status = changes.status;
    }
    if (changes.duplicate_of !== undefined) {
      if (changes.duplicate_of !== null && changes.duplicate_of !== "RWC-184") {
        throw new Error("invalid duplicate target");
      }
      item.duplicate_of = changes.duplicate_of;
    }
    if (changes.evidence !== undefined) {
      const evidence = changes.evidence;
      if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) {
        throw new Error("evidence must be an object");
      }
      const evidenceKeys = Object.keys(evidence);
      if (evidenceKeys.some(function (key) { return !EVIDENCE_FIELDS.includes(key); })) {
        throw new Error("invalid evidence field");
      }
      item.evidence = {
        wiki_ref: String(evidence.wiki_ref || ""),
        repro_url: String(evidence.repro_url || ""),
        log_ref: String(evidence.log_ref || ""),
      };
    }
    fs.writeFileSync(this.casesPath, JSON.stringify(document, null, 2) + "\n");
    return item;
  }
}

module.exports = { CaseStore, CANDIDATE_IDS, STATUSES, EVIDENCE_FIELDS };
