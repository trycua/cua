"use strict";
const { contextBridge, ipcRenderer } = require("electron");
contextBridge.exposeInMainWorld("manual", { list: function () { return ipcRenderer.invoke("manual:list"); }, get: function (id) { return ipcRenderer.invoke("manual:get", id); }, workspace: function () { return ipcRenderer.invoke("manual:workspace"); } });
