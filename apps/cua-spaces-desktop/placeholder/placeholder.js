// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Shown when ../cua-spaces-web/dist has not been built. It speaks the same
// contract as the web bridge (apps/cua-spaces-web/src/bridge/electron-channels.ts).
const desktop = window.cuaDesktop;
const info = document.getElementById("info");

function row(label, value) {
  const dt = document.createElement("dt");
  dt.textContent = label;
  const dd = document.createElement("dd");
  dd.textContent = value;
  info.append(dt, dd);
}

async function call(op, args = {}) {
  const reply = await desktop.invoke(`cua:${op}`, args);
  if (!reply.ok) throw new Error(reply.error.message);
  return reply.result;
}

if (!desktop) {
  row("bridge", "window.cuaDesktop is missing (opened outside the shell?)");
} else {
  const session = await call("session.get");
  const settings = await call("settings.get");
  row("platform", desktop.platform);
  row("origin", location.origin);
  row("theme", settings.values.theme);
  row("titlebar inset", getComputedStyle(document.documentElement).getPropertyValue("--titlebar-left-inset") || "0px");
  row("cua daemon", session.daemon?.connected ? `running ${session.daemon.version ?? ""}` : "not running");

  const spaces = await call("spaces.list");
  document.getElementById("source").textContent = `· ${spaces.length} Spaces`;
  const body = document.getElementById("spaces");
  for (const s of spaces) {
    const tr = document.createElement("tr");
    for (const v of [s.name, s.provider, s.os, s.powerState]) {
      const td = document.createElement("td");
      td.textContent = v ?? "";
      tr.append(td);
    }
    body.append(tr);
  }
}
