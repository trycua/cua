// An in-memory SpacePort / SpacesPort: scripted agent runs (their events as
// the agent_events tool writes them), files and teleport. The app model
// folds those events with the SDK's real AgentTranscript.
// The server's accepts_message rule (cua-agents `accepts_message`): a
// follow-up now starts the next turn. The app never derives it; only this
// fake server does.
const ACCEPTS = new Set(["idle", "crashed", "failed"])

export class FakeSpace {
  constructor(info = { id: "space://direct/fake:1", name: "fake", provider: "direct", spacesdVersion: "0", features: ["presence"] }) {
    this._info = info
    this.runs = new Map()
    this.calls = []
    this.failStatus = false
    this.sendVerified = true
    /** Events a run writes before its first prompt (install progress). */
    this.startEvents = []
    this.manifest = { app: "firefox", displayName: "Firefox", scope: "full", items: [{ relativePath: "prefs.js", label: "Prefs", estimatedBytes: 1n, isSensitive: false, isCheckedByDefault: true }, { relativePath: "cookies.sqlite", label: "Cookies", estimatedBytes: 1n, isSensitive: true, isCheckedByDefault: true }], totalEstimatedBytes: 2n, notes: [] }
  }
  id() { return this._info.id }
  info() { return this._info }
  supports(f) { return this._info.features.includes(f) }
  async agentStart(agent, prompt) {
    this.calls.push("agentStart")
    const runId = `run-${this.runs.size + 1}`
    this.runs.set(runId, { agent, status: "running", events: [], turn: 1, polls: 0, pending: [`did: ${prompt}`] })
    for (const e of this.startEvents) this.emit(this.runs.get(runId), { turn: 0, ...e })
    this.emit(this.runs.get(runId), { kind: "turn_started", text: prompt })
    return { runId, agent, space: this.id(), processTag: `tag/${runId}`, notes: ["no host credentials copied"], json: "{}" }
  }
  async agentStatus(runId, tail) {
    this.calls.push("agentStatus")
    if (this.failStatus) throw new Error("probe failed")
    const r = this.runs.get(runId)
    if (!r) throw new Error("no run")
    r.polls += 1
    if (r.status === "running" && r.polls >= 2) {
      // A pending string is an agent message; an object is an event as the
      // agent_events tool writes it (kind, text, tool_*, ...).
      for (const p of r.pending) this.emit(r, typeof p === "string" ? { kind: "message", text: p } : p)
      this.emit(r, { kind: "turn_ended", stop_reason: "end_turn" })
      r.pending = []
      r.status = "idle"
    }
    return this.row(runId, r, r.status)
  }
  /** The SDK's AgentRunStatus as agent_status and agent_list both publish
   * it: `acceptsMessage` comes from the server, on list rows too. */
  row(runId, r, reason) {
    return { runId, agent: r.agent, status: r.status, reason, acceptsMessage: ACCEPTS.has(r.status), json: "{}" }
  }
  emit(r, e) {
    r.events.push({ seq: r.events.length + 1, ts_ms: 0, turn: r.turn, ...e })
    r.events.at(-1).seq = r.events.length
  }
  async agentEvents(runId, cursor, max) {
    this.calls.push("agentEvents")
    if (this.failStatus) throw new Error("probe failed")
    const r = this.runs.get(runId)
    if (!r) throw new Error("no run")
    const from = Number(cursor ?? 0n)
    const events = r.events.slice(from, from + (max ?? 100))
    return JSON.stringify({ run_id: runId, status: r.status, events, cursor: from + events.length, caught_up: from + events.length >= r.events.length })
  }
  async agentMessage(runId, text) {
    this.calls.push("agentMessage")
    const r = this.runs.get(runId)
    if (!ACCEPTS.has(r.status)) return { runId, ok: false, reason: r.status, json: "{}" }
    r.status = "running"
    r.polls = 0
    r.turn += 1
    this.emit(r, { kind: "turn_started", text })
    r.pending = [`continued: ${text}`]
    return { runId, ok: true, reason: "a new turn", json: "{}" }
  }
  async agentStop(runId) {
    this.calls.push("agentStop")
    this.runs.get(runId).status = "stopped"
    return { runId, ok: true, reason: "stopped", json: "{}" }
  }
  async agentList() {
    this.calls.push("agentList")
    return [...this.runs.entries()].map(([runId, r]) => this.row(runId, r, ""))
  }
  async sendFile(localPath, options) {
    this.calls.push("sendFile")
    this.lastSend = { localPath, options }
    return { source: localPath, destination: "", dest: "", kind: "file", files: [{ path: `/home/u/Downloads/${options.targetDirectory}/f`, size: 1n, sha256: "x" }], bytes: 1n, skippedByIgnorefiles: [], skippedByGuest: [], verified: this.sendVerified }
  }
  async teleportManifest() { return this.manifest }
  async teleport(app, scope, approver) {
    const d = approver.approve(this.manifest)
    if (!d) throw new Error("TeleportRefused: the approver declined")
    if (this.manifest.items.some((i) => i.isSensitive) && !d.acknowledgeSensitive) throw new Error("TeleportRefused: sensitive")
    this.lastDecision = d
    return { app, space: this.id(), method: "import_session", transferredPaths: [], bundleBytes: 10n, bundleSha256: "", imported: [app], skipped: [], launched: false }
  }
  async joinPresence(identity) {
    const events = []
    return {
      me: async () => ({ participantId: `p-${identity.id}`, principalId: "", displayName: identity.displayName, color: "", kind: identity.agent ? "agent" : "human" }),
      roster: async () => [],
      updateCursor: async () => {},
      nextEvent: async () => events.shift(),
      leave: async () => {},
      _events: events,
    }
  }
}

export class FakeSpaces {
  constructor() { this.byId = new Map(); this.deleted = [] }
  async list() { return [...this.byId.values()].map((s) => s.info()) }
  async add(url, token, name) {
    const s = new FakeSpace({ id: `space://direct/${url}`, name: name ?? url, provider: "direct", spacesdVersion: "0", features: ["presence", "desktop_stream"] })
    this.byId.set(s.id(), s)
    return s.info()
  }
  async delete_(id) { this.byId.delete(id); this.deleted.push(id); return `Deleted ${id}` }
  async remove(id) { this.byId.delete(id) }
  async space(id) { const s = this.byId.get(id); if (!s) throw new Error("not found"); return s }
}
