"use strict";
const os = require("os"); const path = require("path"); const { app, BrowserWindow, ipcMain } = require("electron"); const { ManualStore } = require("./store.js");
app.commandLine.appendSwitch("force-renderer-accessibility");
function workspaceDirectory() { const flag = process.argv.find(function (arg) { return arg.startsWith("--workspace="); }); const value = flag ? flag.slice("--workspace=".length) : (process.env.CDB_S04_WORKSPACE || path.join(os.homedir(), "cdb-s04")); return path.resolve(value); }
const store = new ManualStore(workspaceDirectory());
function createWindow() { const window = new BrowserWindow({ width: 1080, height: 720, title: "Margin Lantern Manual", webPreferences: { preload: path.join(__dirname, "preload.js"), contextIsolation: true, nodeIntegration: false, sandbox: true } }); window.setMenuBarVisibility(false); window.loadFile(path.join(__dirname, "renderer", "index.html")); }
ipcMain.handle("manual:list", function () { return store.list(); }); ipcMain.handle("manual:get", function (_event, id) { return store.get(id); }); ipcMain.handle("manual:workspace", function () { return store.workspace; });
app.whenReady().then(createWindow); app.on("window-all-closed", function () { app.quit(); });
