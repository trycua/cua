type WorkbenchStatus = {
  metadata: {
    driverVersion: string;
    contractVersion: string;
    pid: number;
    embedded: boolean;
    executionMode: number;
  };
  session?: { name: string; mode: string };
};

type PermissionStatus = {
  platform: string;
  required: boolean;
  accessibility: boolean;
  screenRecording: boolean;
  ready?: boolean;
};

type ToolResult = {
  text: string;
  isError: boolean;
  errorCode?: string;
  degraded: boolean;
  structured?: unknown;
  raw?: unknown;
  images: Array<{ mimeType: string; dataUrl: string }>;
};

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const statusDot = byId<HTMLSpanElement>('runtime-dot');
const runtimeLabel = byId<HTMLSpanElement>('runtime-label');
const metadataEl = byId<HTMLDivElement>('metadata');
const permissionEl = byId<HTMLDivElement>('permission-status');
const sessionPill = byId<HTMLDivElement>('session-pill');
const sessionName = byId<HTMLInputElement>('session-name');
const modeSelect = byId<HTMLSelectElement>('permission-mode');
const manifestField = byId<HTMLDivElement>('manifest-field');
const manifestPath = byId<HTMLInputElement>('manifest-path');
const startButton = byId<HTMLButtonElement>('start-session');
const endButton = byId<HTMLButtonElement>('end-session');
const toolSelect = byId<HTMLSelectElement>('tool-select');
const argsEditor = byId<HTMLTextAreaElement>('arguments-json');
const runButton = byId<HTMLButtonElement>('run-tool');
const output = byId<HTMLPreElement>('output');
const screenshot = byId<HTMLImageElement>('screenshot');
const screenshotEmpty = byId<HTMLDivElement>('screenshot-empty');
const activity = byId<HTMLDivElement>('activity');

const examples: Record<string, object> = {
  list_apps: {},
  list_windows: { on_screen_only: true },
  launch_app: { bundle_id: 'com.apple.calculator' },
  get_window_state: { pid: 0, window_id: 0, include_screenshot: true },
  get_desktop_state: {},
  click: { pid: 0, window_id: 0, x: 320, y: 240, delivery_mode: 'background' },
  type_text: { pid: 0, window_id: 0, x: 320, y: 240, text: 'Hello from Electron', delivery_mode: 'background' },
  scroll: { pid: 0, window_id: 0, x: 320, y: 240, direction: 'down', amount: 3 },
  press_key: { pid: 0, window_id: 0, key: 'ENTER', delivery_mode: 'background' },
};

function pretty(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

function log(message: string, kind: 'info' | 'success' | 'error' = 'info'): void {
  const item = document.createElement('div');
  item.className = `activity-item ${kind}`;
  const time = new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  const timeEl = document.createElement('span');
  timeEl.textContent = time;
  const messageEl = document.createElement('strong');
  messageEl.textContent = message;
  item.append(timeEl, messageEl);
  activity.prepend(item);
}

function setBusy(button: HTMLButtonElement, busy: boolean, busyText: string): void {
  if (busy) button.dataset.label = button.textContent ?? '';
  button.disabled = busy;
  button.textContent = busy ? busyText : button.dataset.label ?? button.textContent;
}

async function refreshStatus(): Promise<void> {
  const [status, permissions] = await Promise.all([
    window.cuaWorkbench.getStatus() as Promise<WorkbenchStatus>,
    window.cuaWorkbench.getPermissions() as Promise<PermissionStatus>,
  ]);
  statusDot.classList.add('ready');
  runtimeLabel.textContent = `Embedded runtime · v${status.metadata.driverVersion}`;
  metadataEl.innerHTML = `
    <div><span>Runtime</span><strong>Embedded</strong></div>
    <div><span>Native PID</span><strong>${status.metadata.pid}</strong></div>
    <div><span>Contract</span><strong>${status.metadata.contractVersion}</strong></div>
  `;
  const permissionReady = permissions.accessibility && permissions.screenRecording;
  permissionEl.innerHTML = permissions.required
    ? `<span class="permission ${permissions.accessibility ? 'ok' : ''}">Accessibility</span>
       <span class="permission ${permissions.screenRecording ? 'ok' : ''}">Screen capture</span>`
    : '<span class="permission ok">No platform grants required</span>';
  byId<HTMLButtonElement>('request-permissions').hidden = !permissions.required || permissionReady;
  byId<HTMLButtonElement>('open-settings').hidden = !permissions.required || permissions.screenRecording;
  const active = Boolean(status.session);
  sessionPill.textContent = status.session ? `${status.session.name} · ${status.session.mode}` : 'No active session';
  sessionPill.classList.toggle('active', active);
  startButton.disabled = active;
  endButton.disabled = !active;
  runButton.disabled = !active;
  sessionName.disabled = active;
  modeSelect.disabled = active;
  manifestPath.disabled = active;
}

async function act(button: HTMLButtonElement, busyText: string, task: () => Promise<void>): Promise<void> {
  setBusy(button, true, busyText);
  try {
    await task();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    log(message, 'error');
    output.textContent = message;
  } finally {
    setBusy(button, false, busyText);
  }
}

modeSelect.addEventListener('change', () => {
  manifestField.hidden = modeSelect.value !== 'bounded';
});

toolSelect.addEventListener('change', () => {
  argsEditor.value = pretty(examples[toolSelect.value] ?? {});
});

startButton.addEventListener('click', () => void act(startButton, 'Starting…', async () => {
  await window.cuaWorkbench.startSession({
    name: sessionName.value,
    mode: modeSelect.value,
    manifestPath: manifestPath.value,
  });
  log(`Started ${sessionName.value}`, 'success');
  await refreshStatus();
}));

endButton.addEventListener('click', () => void act(endButton, 'Ending…', async () => {
  await window.cuaWorkbench.endSession();
  log('Session ended', 'success');
  await refreshStatus();
}));

runButton.addEventListener('click', () => void act(runButton, 'Running…', async () => {
  const result = await window.cuaWorkbench.invokeTool({
    tool: toolSelect.value,
    argumentsJson: argsEditor.value,
  }) as ToolResult;
  output.textContent = pretty({
    text: result.text,
    isError: result.isError,
    errorCode: result.errorCode,
    degraded: result.degraded,
    structured: result.structured,
    raw: result.raw,
  });
  const image = result.images[0];
  if (image) {
    screenshot.src = image.dataUrl;
    screenshot.hidden = false;
    screenshotEmpty.hidden = true;
  }
  log(`${toolSelect.value} ${result.isError ? 'returned an error' : 'completed'}`, result.isError ? 'error' : 'success');
}));

byId<HTMLButtonElement>('request-permissions').addEventListener('click', () => void act(
  byId<HTMLButtonElement>('request-permissions'),
  'Requesting…',
  async () => { await window.cuaWorkbench.requestPermissions(); await refreshStatus(); },
));

byId<HTMLButtonElement>('open-settings').addEventListener('click', () => void window.cuaWorkbench.openScreenRecordingSettings());

window.addEventListener('DOMContentLoaded', () => {
  sessionName.value = `electron-${new Date().toISOString().slice(11, 19).replaceAll(':', '')}`;
  argsEditor.value = pretty(examples.list_windows);
  void refreshStatus().then(() => log('TypeScript SDK runtime ready', 'success')).catch((error) => {
    runtimeLabel.textContent = 'Runtime unavailable';
    output.textContent = String(error);
    log(String(error), 'error');
  });
});
