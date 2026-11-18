"use strict";
const { contextBridge, ipcRenderer } = require("electron");
contextBridge.exposeInMainWorld("rillwatch", {
  list: function () { return ipcRenderer.invoke("cases:list"); },
  get: function (id) { return ipcRenderer.invoke("cases:get", id); },
  update: function (id, changes) { return ipcRenderer.invoke("cases:update", id, changes); },
  workspace: function () { return ipcRenderer.invoke("cases:workspace"); },
});
