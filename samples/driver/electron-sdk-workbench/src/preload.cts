const { contextBridge, ipcRenderer } = require('electron');

const invoke = (channel: string, payload?: unknown) => ipcRenderer.invoke(channel, payload);

contextBridge.exposeInMainWorld('cuaWorkbench', Object.freeze({
  getStatus: () => invoke('cua:status'),
  getPermissions: () => invoke('cua:permissions:get'),
  requestPermissions: () => invoke('cua:permissions:request'),
  openScreenRecordingSettings: () => invoke('cua:permissions:open-settings'),
  startSession: (request: unknown) => invoke('cua:session:start', request),
  endSession: () => invoke('cua:session:end'),
  invokeTool: (request: unknown) => invoke('cua:tool:invoke', request),
}));
