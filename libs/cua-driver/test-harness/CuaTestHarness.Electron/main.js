// CuaTestHarness.Electron — minimal Electron host loading the shared
// index.html that CuaTestHarness.WebView also loads. cua-driver's `page`
// tool routes through CDP when --remote-debugging-port is set, so we
// expose one here on a configurable port.

const { app, BrowserWindow } = require('electron');
const path = require('path');

const CDP_PORT = process.env.CUA_ELECTRON_CDP_PORT || '9223';
app.commandLine.appendSwitch('remote-debugging-port', CDP_PORT);

let mainWindow;

function createWindow() {
  const fixedTitle = `CuaTestHarness Electron [cdp=${CDP_PORT}]`;
  mainWindow = new BrowserWindow({
    width: 940,
    height: 780,
    title: fixedTitle,
    autoHideMenuBar: true,
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
    },
  });

  // Override the page's <title> with our deterministic harness title so
  // cua-driver tests can find the window by substring match. Without this,
  // Electron syncs window.title to document.title which would be
  // 'cua-driver Web Harness' (the page's title).
  mainWindow.on('page-title-updated', e => e.preventDefault());
  mainWindow.setTitle(fixedTitle);

  mainWindow.loadFile(path.join(__dirname, 'web', 'index.html')).then(() => {
    mainWindow.setTitle(fixedTitle);
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
