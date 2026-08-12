import {
  type MacOsPermissionStatus,
  openMacOsScreenRecordingSettings as openGeneratedSettings,
  requestMacOsPermissions as requestGeneratedPermissions,
} from './native/index.js';

/** Compatibility spelling retained for the Electron-specific entry point. */
export type MacOSPermissionStatus = MacOsPermissionStatus;

/** Call after Electron's app.whenReady(), from the Electron main process. */
export const requestMacOSPermissions = (): MacOSPermissionStatus => {
  return requestGeneratedPermissions();
};

export const hasRequiredMacOSPermissions = (status: MacOSPermissionStatus): boolean =>
  status.accessibility && status.screenRecording;

export const openMacOSScreenRecordingSettings = async (): Promise<void> =>
  openGeneratedSettings();
