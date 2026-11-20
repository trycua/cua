"use strict";

const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("ledgerLantern", Object.freeze({
  listMessages: function () { return ipcRenderer.invoke("mail:list"); },
  openAttachment: function (messageId, attachmentId) {
    return ipcRenderer.invoke("mail:attachment", messageId, attachmentId);
  },
}));
