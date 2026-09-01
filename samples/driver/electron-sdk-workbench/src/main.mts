import { app, BrowserWindow, ipcMain, type IpcMainInvokeEvent } from 'electron';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import type { CuaDriverLike, CuaDriverSessionLike } from '@trycua/cua-driver';

import {
  assertAllowedTool,
  parseArgumentsJson,
  serializeToolResult,
  validateSessionRequest,
  type PermissionModeName,
} from './lib.mjs';

const moduleDirectory = path.dirname(fileURLToPath(import.meta.url));
const rendererFile = path.join(moduleDirectory, 'renderer', 'index.html');

let mainWindow: BrowserWindow | undefined;
let runtime: CuaDriverLike | undefined;
let activeSession: CuaDriverSessionLike | undefined;
let activeSessionName: string | undefined;
let activeMode: PermissionModeName | undefined;
let shuttingDown = false;
let sdkPromise: Promise<typeof import('@trycua/cua-driver')> | undefined;

function loadSdk(): Promise<typeof import('@trycua/cua-driver')> {
  sdkPromise ??= import('@trycua/cua-driver');
  return sdkPromise;
}

function assertTrustedRenderer(event: IpcMainInvokeEvent): void {
  const frameUrl = event.senderFrame?.url;
  if (!frameUrl || frameUrl !== new URL(`file://${rendererFile}`).href) {
    throw new Error('Rejected IPC from an untrusted renderer.');
  }
}

function registerHandler(channel: string, handler: (payload: unknown) => unknown | Promise<unknown>): void {
  ipcMain.handle(channel, async (event, payload) => {
    assertTrustedRenderer(event);
    try {
      return await handler(payload);
    } catch (error) {
      const nativeReason = error !== null && typeof error === 'object' && 'inner' in error
        ? (error as { inner?: { reason?: unknown } }).inner?.reason
        : undefined;
      throw new Error(typeof nativeReason === 'string' ? nativeReason : String(error));
    }
  });
}

async function ensureRuntime(): Promise<CuaDriverLike> {
  if (runtime) return runtime;
  const sdk = await loadSdk();
  const authorization = sdk.RuntimeAuthorizationOptions.new({
    allowedModes: [
      sdk.SessionPermissionMode.Standard,
      sdk.SessionPermissionMode.Bounded,
      sdk.SessionPermissionMode.Unrestricted,
    ],
    compatibilityMode: sdk.SessionPermissionMode.Standard,
    unrestrictedAcknowledged: true,
    maxSessionTtlSeconds: 3_600n,
    maxIdleTtlSeconds: 900n,
  });
  runtime = sdk.CuaDriver.createConfigured(
    sdk.ConfiguredDriverOptions.new({ claudeCodeCompatibility: false, authorization }),
  );
  return runtime;
}

async function status() {
  const driver = await ensureRuntime();
  const metadata = await driver.metadata();
  return {
    metadata: {
      ...metadata,
      executionMode: await driver.executionMode(),
    },
    session: activeSessionName
      ? { name: activeSessionName, mode: activeMode }
      : undefined,
  };
}

async function permissions(request = false) {
  if (process.platform !== 'darwin') {
    return { platform: process.platform, required: false, accessibility: true, screenRecording: true };
  }
  const sdk = await loadSdk();
  const current = request
    ? (await import('@trycua/cua-driver/electron')).requestMacOSPermissions()
    : sdk.currentMacOsPermissionStatus();
  return {
    platform: process.platform,
    required: true,
    accessibility: current.accessibility,
    screenRecording: current.screenRecording,
    ready: current.accessibility && current.screenRecording,
  };
}

async function closeSession(): Promise<void> {
  if (!activeSession) return;
  const session = activeSession;
  const name = activeSessionName;
  activeSession = undefined;
  activeSessionName = undefined;
  activeMode = undefined;
  try {
    if (name) await session.endSession({ session: name });
  } finally {
    session.close();
  }
}

async function shutdown(): Promise<void> {
  if (shuttingDown) return;
  shuttingDown = true;
  try {
    await closeSession();
    if (runtime) {
      await runtime.shutdown();
      (runtime as unknown as { uniffiDestroy(): void }).uniffiDestroy();
      runtime = undefined;
    }
  } finally {
    shuttingDown = false;
  }
}

function installIpcHandlers(): void {
  registerHandler('cua:status', status);
  registerHandler('cua:permissions:get', () => permissions());
  registerHandler('cua:permissions:request', () => permissions(true));
  registerHandler('cua:permissions:open-settings', async () => {
    if (process.platform === 'darwin') {
      await (await import('@trycua/cua-driver/electron')).openMacOSScreenRecordingSettings();
    }
    return permissions();
  });
  registerHandler('cua:session:start', async (payload) => {
    if (activeSession) throw new Error('End the current session before starting another one.');
    const request = validateSessionRequest(payload);
    const sdk = await loadSdk();
    const driver = await ensureRuntime();
    const options = sdk.TrustedSessionOptions.new({
      publicSession: request.name,
      mode: request.mode === 'bounded'
        ? sdk.SessionPermissionMode.Bounded
        : request.mode === 'unrestricted'
          ? sdk.SessionPermissionMode.Unrestricted
          : sdk.SessionPermissionMode.Standard,
      ttlSeconds: 3_600n,
      idleTtlSeconds: 900n,
      ...(request.manifestPath ? { boundedManifestPath: request.manifestPath } : {}),
    });
    const session = sdk.createTrustedSession(driver, options);
    try {
      await session.startSession(
        sdk.StartSessionInput.new({ session: request.name, captureScope: sdk.CaptureScope.Desktop }),
      );
      activeSession = session;
      activeSessionName = request.name;
      activeMode = request.mode;
      return status();
    } catch (error) {
      session.close();
      throw error;
    }
  });
  registerHandler('cua:session:end', async () => {
    await closeSession();
    return status();
  });
  registerHandler('cua:tool:invoke', async (payload) => {
    if (!activeSession || !activeSessionName) throw new Error('Start a session first.');
    if (payload === null || typeof payload !== 'object') throw new Error('Invalid tool request.');
    const request = payload as Record<string, unknown>;
    assertAllowedTool(request.tool);
    const args = parseArgumentsJson(request.argumentsJson);
    if (args.session !== undefined && args.session !== activeSessionName) {
      throw new Error('Tool session must match the active trusted session.');
    }
    args.session = activeSessionName;
    const result = await activeSession.callTool(request.tool, JSON.stringify(args));
    return serializeToolResult(result);
  });
}

async function createWindow(): Promise<void> {
  mainWindow = new BrowserWindow({
    width: 1280,
    height: 860,
    minWidth: 980,
    minHeight: 680,
    title: 'Cua Driver Workbench',
    backgroundColor: '#0b0c0f',
    titleBarStyle: 'hiddenInset',
    webPreferences: {
      preload: path.join(moduleDirectory, 'preload.cjs'),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      webSecurity: true,
    },
  });
  mainWindow.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  mainWindow.webContents.on('will-navigate', (event) => event.preventDefault());
  mainWindow.webContents.session.setPermissionRequestHandler((_webContents, _permission, callback) => callback(false));
  await mainWindow.loadFile(rendererFile);
}

void app.whenReady().then(async () => {
  await ensureRuntime();
  installIpcHandlers();
  await createWindow();
}).catch((error: unknown) => {
  console.error('[workbench] startup failed', error);
  app.quit();
});

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) void createWindow();
});

app.on('window-all-closed', () => {
  void shutdown().finally(() => app.quit());
});

app.on('before-quit', () => {
  void shutdown();
});
