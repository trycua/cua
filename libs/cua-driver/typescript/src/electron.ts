import Electron from 'electron';
import { DataType, load, open } from 'ffi-rs';

const CORE_GRAPHICS_LIBRARY = 'cua-driver-embedded-core-graphics';
const CORE_GRAPHICS_PATH = '/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics';
const SCREEN_RECORDING_SETTINGS_URL =
  'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture';

let coreGraphicsLoaded = false;

export interface MacOSPermissionStatus {
  readonly accessibility: boolean;
  readonly screenRecording: boolean;
}

const requestScreenRecording = (): boolean => {
  if (!coreGraphicsLoaded) {
    open({ library: CORE_GRAPHICS_LIBRARY, path: CORE_GRAPHICS_PATH });
    coreGraphicsLoaded = true;
  }
  return load({
    library: CORE_GRAPHICS_LIBRARY,
    funcName: 'CGRequestScreenCaptureAccess',
    retType: DataType.Boolean,
    paramsType: [],
    paramsValue: [],
  }) as boolean;
};

/** Call after Electron's app.whenReady(), from the Electron main process. */
export const requestMacOSPermissions = (): MacOSPermissionStatus => {
  if (process.platform !== 'darwin') {
    return { accessibility: true, screenRecording: true };
  }
  return {
    accessibility: Electron.systemPreferences.isTrustedAccessibilityClient(true),
    screenRecording: requestScreenRecording(),
  };
};

export const hasRequiredMacOSPermissions = (status: MacOSPermissionStatus): boolean =>
  status.accessibility && status.screenRecording;

export const openMacOSScreenRecordingSettings = async (): Promise<void> => {
  if (process.platform === 'darwin')
    await Electron.shell.openExternal(SCREEN_RECORDING_SETTINGS_URL);
};
