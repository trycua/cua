"use strict";

const path = require("node:path");
const { app, BrowserWindow, ipcMain } = require("electron");
const { MailStore } = require("./store.js");

app.commandLine.appendSwitch("force-renderer-accessibility");

function flag(name) {
  const prefix = "--" + name + "=";
  const argument = process.argv.find(function (item) { return item.startsWith(prefix); });
  return argument ? argument.slice(prefix.length) : null;
}

const workspaceFlag = flag("workspace");
if (!workspaceFlag) {
  throw new Error("Ledger Lantern Mail needs --workspace=<path>");
}

const workspace = path.resolve(workspaceFlag);
const store = new MailStore(workspace);

app.setName("Ledger Lantern Mail");
app.setPath("userData", path.join(workspace, ".runtime", "ledger-lantern-mail"));

ipcMain.handle("mail:list", function () { return store.list(); });
ipcMain.handle("mail:attachment", function (_event, messageId, attachmentId) {
  return store.attachment(messageId, attachmentId);
});

function createWindow() {
  const window = new BrowserWindow({
    width: 1180,
    height: 780,
    minWidth: 860,
    minHeight: 620,
    title: "Ledger Lantern Mail",
    backgroundColor: "#f3efe6",
    webPreferences: {
      preload: path.join(__dirname, "preload.js"),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      partition: "persist:ledger-lantern-mail",
    },
  });
  window.setMenuBarVisibility(false);
  window.loadFile(path.join(__dirname, "renderer", "index.html"));
}

app.whenReady().then(createWindow);
app.on("window-all-closed", function () { app.quit(); });
