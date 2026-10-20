"use strict";

const path = require("path");
const { app, BrowserWindow, ipcMain } = require("electron");
const { CalendarStore, ChatStore, MailStore } = require("./store.js");

app.commandLine.appendSwitch("force-renderer-accessibility");

const SURFACES = {
  mail: { name: "Copperfield Mail", width: 1040, height: 720 },
  chat: { name: "Signal Quay Chat", width: 1000, height: 760 },
  calendar: { name: "Daymark Calendar", width: 1160, height: 800 },
};

function flag(name) {
  const prefix = "--" + name + "=";
  const argument = process.argv.find(function (item) { return item.startsWith(prefix); });
  return argument ? argument.slice(prefix.length) : null;
}

const surface = flag("surface");
const workspaceFlag = flag("workspace");
if (!Object.prototype.hasOwnProperty.call(SURFACES, surface)) {
  throw new Error("Lighthouse Desk needs --surface=mail|chat|calendar");
}
if (!workspaceFlag) {
  throw new Error("Lighthouse Desk needs --workspace=<path>");
}

const workspace = path.resolve(workspaceFlag);
const product = SURFACES[surface];
const stores = {
  mail: new MailStore(workspace),
  chat: new ChatStore(workspace),
  calendar: new CalendarStore(workspace),
};
const store = stores[surface];

app.setName(product.name);
app.setPath("userData", path.join(workspace, ".runtime", "electron-" + surface));

function requireSurface(expected) {
  if (surface !== expected) {
    throw new Error("operation is unavailable in " + product.name);
  }
}

ipcMain.handle("surface:info", function () {
  return { surface: surface, name: product.name };
});
ipcMain.handle("mail:list", function () { requireSurface("mail"); return store.list(); });
ipcMain.handle("chat:list", function () { requireSurface("chat"); return store.messages(); });
ipcMain.handle("chat:blockers:list", function () { requireSurface("chat"); return store.blockers(); });
ipcMain.handle("chat:blocker", function (_event, blocker) {
  requireSurface("chat");
  return store.addBlocker(blocker, "signal-quay-chat");
});
ipcMain.handle("chat:blocker:remove", function (_event, conflictId) {
  requireSurface("chat");
  return store.removeBlocker(conflictId, "signal-quay-chat");
});
ipcMain.handle("calendar:list", function () { requireSurface("calendar"); return store.list(); });
ipcMain.handle("calendar:create", function (_event, event) { requireSurface("calendar"); return store.create(event); });
ipcMain.handle("calendar:update", function (_event, id, changes) { requireSurface("calendar"); return store.update(id, changes); });
ipcMain.handle("calendar:delete", function (_event, id) { requireSurface("calendar"); return store.delete(id); });

function createWindow() {
  const window = new BrowserWindow({
    width: product.width,
    height: product.height,
    title: product.name,
    backgroundColor: "#f6f7fb",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      partition: "persist:lighthouse-" + surface,
    },
  });
  window.setMenuBarVisibility(false);
  window.loadFile(path.join(__dirname, "renderer", "index.html"), { query: { surface: surface } });
}

app.whenReady().then(createWindow);
app.on("window-all-closed", function () { app.quit(); });
