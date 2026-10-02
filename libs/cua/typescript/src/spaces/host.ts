/**
 * `@trycua/cua/spaces/host` — the host presentation adapter. **Explicitly unstable.**
 *
 * This is the one thing the SDK itself cannot do: put a Space on
 * the *operator's own Mac desktop* — pinned as picture-in-picture, opened in a
 * viewer window, or a single Space window streamed back bidirectionally over
 * rcdp.
 *
 * It talks to the loopback control server inside the Cua Spaces desktop app,
 * discovered through `$CUA_HOME/spaces-control.json` (default `~/.cua`; `{port, token}`). That means
 * all of the following, and you should assume all of it will change:
 *
 *  - it only works on the machine where the app is running, as the user who
 *    owns that home directory;
 *  - the port is ephemeral and the token is regenerated on every app launch;
 *  - a Local Space's rcdp token rotates on every rcdpd restart, so a token read
 *    once and cached will start failing — `streamWindow` takes a fresh one;
 *  - the control server resolves `local:<vm>` ids only. `pin` and `openViewer`
 *    with any other id are rejected by the app with HTTP 400, and this adapter
 *    says so rather than reporting success.
 *
 * Every method throws `host_unavailable` when the app is not running. None of
 * them degrades to a silent no-op — a `forget_rcdp_token` with zero callers is
 * exactly the kind of thing this codebase has shipped before.
 */

import { SpacesError, excerpt } from './errors.js';

export interface ControlEndpoint {
  port: number;
  token: string;
}

export interface HostPresentationOptions {
  /** Override the discovery file. Mostly for tests. */
  controlFile?: string;
  /** Inject a fetch (tests, or a different agent). */
  fetch?: (input: string, init?: RequestInit) => Promise<Response>;
}

/** `$CUA_HOME` when set and non-empty, else `~/.cua` (the Rust core's rule). */
export function cuaHome(
  home: string,
  env: Record<string, string | undefined> = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {},
): string {
  const set = env.CUA_HOME;
  return set ? set : `${home.replace(/[\\/]+$/, '')}/.cua`;
}

/** Read `{port, token}` from the app's control file. */
export async function readControlEndpoint(controlFile?: string): Promise<ControlEndpoint> {
  const { readFile } = await import('node:fs/promises');
  const { homedir } = await import('node:os');
  const { join } = await import('node:path');
  const path = controlFile ?? join(cuaHome(homedir()), 'spaces-control.json');
  let raw: string;
  try {
    raw = await readFile(path, 'utf8');
  } catch (cause) {
    throw new SpacesError(
      'host_unavailable',
      `the Cua Spaces app does not appear to be running: cannot read ${path}. ` +
        `Host presentation (PiP, viewer, window stream) needs the desktop app on this machine.`,
      { cause },
    );
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (cause) {
    throw SpacesError.protocol(`${path} is not valid JSON`, { cause, detail: excerpt(raw, 120) });
  }
  const record = parsed as { port?: unknown; token?: unknown };
  if (typeof record.port !== 'number' || typeof record.token !== 'string' || !record.token) {
    throw SpacesError.protocol(`${path} does not contain a usable {port, token}`);
  }
  return { port: record.port, token: record.token };
}

export class HostPresentation {
  constructor(private readonly options: HostPresentationOptions = {}) {}

  /** Pin a Local Space as picture-in-picture on this Mac's desktop. */
  async pin(spaceId: string): Promise<void> {
    this.assertLocal(spaceId, 'pin');
    await this.post('/pip/pin', { space_id: spaceId });
  }

  async unpin(spaceId: string): Promise<void> {
    await this.post('/pip/unpin', { space_id: spaceId });
  }

  /** Open the full viewer window for a Local Space. */
  async openViewer(spaceId: string): Promise<void> {
    this.assertLocal(spaceId, 'openViewer');
    await this.post('/viewer/open', { space_id: spaceId });
  }

  /**
   * Stream one window of a Local Space back to a bidirectional window here.
   *
   * `rcdpToken` must be read fresh: a Local Space's rcdp token rotates on every
   * rcdpd restart, so a cached one silently stops working.
   *
   * `replica: true` opens an *additional* stream for the same target rather
   * than focusing the existing one. Note the real limit: only one page per app
   * can hardware-decode H.264, so a second concurrent stream falls back to an
   * uncompressed codec and is visibly heavier.
   */
  async streamWindow(input: {
    spaceId: string;
    windowId: string;
    rcdpToken: string;
    appName?: string;
    title?: string;
    replica?: boolean;
  }): Promise<void> {
    this.assertLocal(input.spaceId, 'streamWindow');
    if (!input.windowId) throw SpacesError.usage('streamWindow requires a windowId');
    if (!input.rcdpToken) {
      throw SpacesError.usage(
        'streamWindow requires a fresh rcdpToken; the token rotates on every rcdpd restart',
      );
    }
    await this.post('/window/stream', {
      space_id: input.spaceId,
      window_id: input.windowId,
      rcdp_token: input.rcdpToken,
      app_name: input.appName ?? '',
      title: input.title ?? '',
      replica: input.replica ?? false,
    });
  }

  /** Is the desktop app reachable right now? */
  async available(): Promise<boolean> {
    try {
      await readControlEndpoint(this.options.controlFile);
      return true;
    } catch {
      return false;
    }
  }

  private assertLocal(spaceId: string, method: string): void {
    if (!spaceId.startsWith('local:') && !spaceId.startsWith('space://local/')) {
      throw SpacesError.usage(
        `${method} accepts a Local Space id (local:<vm>, or legacy space://local/<vm>) only — ` +
          `the app's control server rejects other ids with HTTP 400. Stream any Space with ` +
          `space.openStream() / space.attachStream() instead.`,
      );
    }
  }

  private async post(path: string, body: unknown): Promise<void> {
    const endpoint = await readControlEndpoint(this.options.controlFile);
    const doFetch = this.options.fetch ?? globalThis.fetch;
    const url = `http://127.0.0.1:${endpoint.port}${path}`;
    let response: Response;
    try {
      response = await doFetch(url, {
        method: 'POST',
        headers: {
          authorization: `Bearer ${endpoint.token}`,
          'content-type': 'application/json',
        },
        body: JSON.stringify(body),
      });
    } catch (cause) {
      throw new SpacesError(
        'host_unavailable',
        `the Cua Spaces control server at 127.0.0.1:${endpoint.port} did not answer; the app may ` +
          `have restarted, which also invalidates the token in the control file`,
        { cause },
      );
    }
    const text = await response.text();
    if (!response.ok) {
      throw new SpacesError('http', `POST ${path} -> HTTP ${response.status}`, {
        status: response.status,
        target: path,
        detail: excerpt(text, 200),
      });
    }
    // The control server answers `{"ok":true}`. Anything else means the route
    // did not do what we asked, and reporting success would be a lie.
    if (!text.includes('"ok"') && !text.includes('"active"')) {
      throw SpacesError.protocol(
        `POST ${path} returned HTTP 200 without an ok acknowledgement, so the effect is unproven`,
        { detail: excerpt(text, 200) },
      );
    }
  }
}
