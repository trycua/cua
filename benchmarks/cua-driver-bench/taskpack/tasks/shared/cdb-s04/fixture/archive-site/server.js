"use strict";
const fs = require("fs"); const http = require("http"); const path = require("path");
const ROOT = __dirname; const HOST = process.env.CDB_S04_HOST || "127.0.0.1"; const PORT = Number(process.env.CDB_S04_PORT || 4184);
function loadRecords() { return JSON.parse(fs.readFileSync(path.join(ROOT, "data", "records.json"), "utf8")).records; }
function inspect(records, recordId, view) {
  const record = records.find(function (item) { return item.id === recordId; });
  if (!record) { return { status: 404, payload: { ok: false, error: "record not found", record: recordId, view: view } }; }
  if (view !== "overlay" && view !== "summary") { return { status: 400, payload: { ok: false, error: "view must be overlay or summary", record: record.id, view: view } }; }
  if (view === "overlay" && record.legacy === true && record.layers.length === 0) { return { status: 200, payload: { ok: false, record: record.id, title: record.title, view: view, legacy: record.legacy, layers: record.layers, message: "No panel selected", defect: "legacy-empty-layers" } }; }
  if (view === "overlay") { return { status: 200, payload: { ok: true, record: record.id, title: record.title, view: view, legacy: record.legacy, layers: record.layers, message: "Overlay ready", defect: null } }; }
  return { status: 200, payload: { ok: true, record: record.id, title: record.title, view: view, legacy: record.legacy, layers: record.layers, message: record.summary || "No curator summary available", defect: null } };
}
function escapeHtml(value) { return String(value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/\"/g, "&quot;"); }
function renderPage(result) { const p = result.payload; return ["<!doctype html>", "<html lang=\"en\"><head><meta charset=\"utf-8\"><title>Morrowglass Archive</title><link rel=\"stylesheet\" href=\"/static/styles.css\"></head><body>", "<header><h1>Morrowglass Archive</h1><p>Record inspection console</p></header><main>", "<form id=\"inspect-form\" action=\"/inspect\" method=\"get\"><label>Record <input name=\"record\" value=\"" + escapeHtml(p.record || "") + "\"></label><label>View <select name=\"view\"><option value=\"overlay\"" + (p.view === "overlay" ? " selected" : "") + ">Overlay</option><option value=\"summary\"" + (p.view === "summary" ? " selected" : "") + ">Summary</option></select></label><button type=\"submit\">Inspect</button></form>", "<section id=\"result\" data-ok=\"" + String(Boolean(p.ok)) + "\"><p class=\"record-id\">" + escapeHtml(p.record || "") + " · " + escapeHtml(p.view || "") + "</p><h2>" + escapeHtml(p.title || p.error || "Inspection") + "</h2><p id=\"message\">" + escapeHtml(p.message || p.error || "") + "</p>" + (p.defect ? "<p class=\"diagnostic\">Diagnostic: <code>" + escapeHtml(p.defect) + "</code></p>" : "") + (Array.isArray(p.layers) ? "<p>Layers: " + p.layers.length + "</p>" : "") + "</section>", "</main><script src=\"/static/app.js\"></script></body></html>"].join("\n"); }
function send(response, status, type, body) { response.writeHead(status, { "Content-Type": type, "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" }); response.end(body); }
function sendFile(response, file, type) { fs.readFile(file, function (error, body) { if (error) { send(response, 404, "text/plain; charset=utf-8", "not found"); return; } send(response, 200, type, body); }); }
function createServer() { return http.createServer(function (request, response) {
  let url; try { url = new URL(request.url, "http://127.0.0.1"); } catch (_error) { send(response, 400, "text/plain; charset=utf-8", "bad request"); return; }
  if (request.method !== "GET") { send(response, 405, "text/plain; charset=utf-8", "method not allowed"); return; }
  if (url.pathname === "/healthz") { send(response, 200, "application/json; charset=utf-8", JSON.stringify({ ok: true, records: loadRecords().length })); return; }
  if (url.pathname === "/api/inspect" || url.pathname === "/inspect") { const result = inspect(loadRecords(), url.searchParams.get("record") || "", url.searchParams.get("view") || ""); if (url.pathname === "/api/inspect") { send(response, result.status, "application/json; charset=utf-8", JSON.stringify(result.payload)); } else { send(response, result.status, "text/html; charset=utf-8", renderPage(result)); } return; }
  if (url.pathname === "/static/app.js") { sendFile(response, path.join(ROOT, "public", "app.js"), "application/javascript; charset=utf-8"); return; }
  if (url.pathname === "/static/styles.css") { sendFile(response, path.join(ROOT, "public", "styles.css"), "text/css; charset=utf-8"); return; }
  if (url.pathname === "/") { response.writeHead(302, { Location: "/inspect?record=vesper-17&view=overlay", "Cache-Control": "no-store" }); response.end(); return; }
  send(response, 404, "text/plain; charset=utf-8", "not found");
}); }
if (require.main === module) { const server = createServer(); server.listen(PORT, HOST, function () { process.stdout.write("Morrowglass Archive listening on http://" + HOST + ":" + PORT + "/\n"); }); function shutdown() { server.close(function () { process.exit(0); }); } process.on("SIGINT", shutdown); process.on("SIGTERM", shutdown); }
module.exports = { createServer, inspect, loadRecords, renderPage };
