"use strict";

const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("lighthouse", Object.freeze({
  info: function () { return ipcRenderer.invoke("surface:info"); },
  mail: Object.freeze({
    list: function () { return ipcRenderer.invoke("mail:list"); },
  }),
  chat: Object.freeze({
    list: function () { return ipcRenderer.invoke("chat:list"); },
    listBlockers: function () { return ipcRenderer.invoke("chat:blockers:list"); },
    recordBlocker: function (blocker) { return ipcRenderer.invoke("chat:blocker", blocker); },
    removeBlocker: function (conflictId) { return ipcRenderer.invoke("chat:blocker:remove", conflictId); },
  }),
  calendar: Object.freeze({
    list: function () { return ipcRenderer.invoke("calendar:list"); },
    create: function (event) { return ipcRenderer.invoke("calendar:create", event); },
    update: function (id, changes) { return ipcRenderer.invoke("calendar:update", id, changes); },
    delete: function (id) { return ipcRenderer.invoke("calendar:delete", id); },
  }),
}));
