// CuaTestHarness.Electron — minimal Electron host loading the shared
// index.html that CuaTestHarness.WebView also loads. cua-driver's `page`
// tool routes through CDP when --remote-debugging-port is set, so we
// expose one here on a configurable port.

const { app, BrowserWindow } = require('electron');
const path = require('path');
const sentinelMode = process.env.CUA_E2E_SENTINEL === '1';
if (process.env.CUA_E2E_USER_DATA_DIR) {
  app.setPath('userData', process.env.CUA_E2E_USER_DATA_DIR);
}

// Validate CUA_ELECTRON_CDP_PORT before forwarding to Chromium —
// remote-debugging-port=0 means "pick an ephemeral port" which would
// break our fixed-port expectation in the harness tests, and a
// non-numeric value silently disables CDP.
const rawCdpPort = process.env.CUA_ELECTRON_CDP_PORT ?? '9223';
const cdpPortNum = Number(rawCdpPort);
if (!Number.isInteger(cdpPortNum) || cdpPortNum < 1 || cdpPortNum > 65535) {
  throw new Error(
    `Invalid CUA_ELECTRON_CDP_PORT: "${rawCdpPort}". Expected an integer in 1-65535.`
  );
}
const CDP_PORT = String(cdpPortNum);
app.commandLine.appendSwitch('remote-debugging-port', CDP_PORT);

let mainWindow;

function createWindow() {
  const fixedTitle = sentinelMode
    ? `CuaTestHarness Sentinel [cdp=${CDP_PORT}]`
    : `CuaTestHarness Electron [cdp=${CDP_PORT}]`;
  mainWindow = new BrowserWindow({
    width: sentinelMode ? 1280 : 940,
    height: sentinelMode ? 900 : 780,
    x: sentinelMode ? 0 : undefined,
    y: sentinelMode ? 0 : undefined,
    title: fixedTitle,
    show: false,
    alwaysOnTop: sentinelMode,
    autoHideMenuBar: true,
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: !sentinelMode,
      preload: path.join(__dirname, 'preload.js'),
    },
  });

  // Override the page's <title> with our deterministic harness title so
  // cua-driver tests can find the window by substring match. Without this,
  // Electron syncs window.title to document.title which would be
  // 'cua-driver Web Harness' (the page's title).
  mainWindow.on('page-title-updated', e => e.preventDefault());
  mainWindow.setTitle(fixedTitle);
  mainWindow.webContents.setWindowOpenHandler(() => ({
    action: 'allow',
    overrideBrowserWindowOptions: {
      width: 320,
      height: 220,
      show: false,
      autoHideMenuBar: true,
      title: 'CuaTestHarness Child Window',
    },
  }));
  mainWindow.webContents.on('did-create-window', child => {
    child.once('ready-to-show', () => {
      child.showInactive();
    });
  });

  mainWindow
    .loadFile(path.join(__dirname, 'web', 'index.html'))
    .then(() => {
      // Re-set after page-load — Electron syncs window.title to
      // document.title once the load finishes, which would override
      // our fixedTitle and break the harness-window-discovery test.
      if (mainWindow && !mainWindow.isDestroyed()) {
        mainWindow.setTitle(fixedTitle);
        if (sentinelMode) {
          mainWindow.setAlwaysOnTop(true);
          mainWindow.maximize();
          mainWindow.show();
          mainWindow.focus();
        } else {
          // Xvfb/Openbox can keep a showInactive window inspectable through
          // AT-SPI while never mapping it onto the captured root desktop.
          // Show it normally; background cells subsequently foreground the
          // occlusion sentinel before taking their desktop snapshot.
          mainWindow.show();
          mainWindow.focus();
        }
      }
    })
    .catch(err => {
      // Fail deterministically rather than leaving the harness window
      // up with no content — the integration tests would then time out
      // waiting for the DOM markers to render.
      console.error('Failed to load harness web/index.html:', err);
      app.exit(1);
    });
}

app.whenReady().then(() => {
  createWindow();
  app.on('activate', () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow();
  });
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') app.quit();
});
