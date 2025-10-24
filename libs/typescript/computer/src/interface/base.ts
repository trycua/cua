/**
 * Base interface for computer control.
 */

import pino from 'pino';
import WebSocket from 'ws';
import type { ScreenSize } from '../types';

export type MouseButton = 'left' | 'middle' | 'right';

export interface CursorPosition {
  x: number;
  y: number;
}

export interface AccessibilityNode {
  role: string;
  title?: string;
  value?: string;
  description?: string;
  bounds?: {
    x: number;
    y: number;
    width: number;
    height: number;
  };
  children?: AccessibilityNode[];
}

/**
 * Base class for computer control interfaces.
 */
export abstract class BaseComputerInterface {
  protected ipAddress: string;
  protected username: string;
  protected password: string;
  protected closed = false;
  protected commandLock: Promise<unknown> = Promise.resolve();
  protected ws: WebSocket;
  protected apiKey?: string;
  protected vmName?: string;

  protected logger = pino({ name: 'computer.interface-base' });

  constructor(
    ipAddress: string,
    username = 'lume',
    password = 'lume',
    apiKey?: string,
    vmName?: string
  ) {
    this.ipAddress = ipAddress;
    this.username = username;
    this.password = password;
    this.apiKey = apiKey;
    this.vmName = vmName;

    // Initialize WebSocket with headers if needed
    const headers: { [key: string]: string } = {};
    if (this.apiKey && this.vmName) {
      headers['X-API-Key'] = this.apiKey;
      headers['X-VM-Name'] = this.vmName;
    }

    // Create the WebSocket instance
    this.ws = new WebSocket(this.wsUri, { headers });
  }

  /**
   * Get the WebSocket URI for connection.
   * Subclasses can override this to customize the URI.
   */
  protected get wsUri(): string {
    const protocol = this.apiKey ? 'wss' : 'ws';

    // Check if ipAddress already includes a port
    if (this.ipAddress.includes(':')) {
      return `${protocol}://${this.ipAddress}/ws`;
    }

    // Otherwise, append the default port
    const port = this.apiKey ? '8443' : '8000';
    return `${protocol}://${this.ipAddress}:${port}/ws`;
  }

  /**
   * Wait for interface to be ready.
   * @param timeout Maximum time to wait in seconds
   * @throws Error if interface is not ready within timeout
   */
  async waitForReady(timeout = 60): Promise<void> {
    const startTime = Date.now();

    while (Date.now() - startTime < timeout * 1000) {
      try {
        await this.connect();
        return;
      } catch (error) {
        console.log(error);
        // Wait a bit before retrying
        this.logger.error(`Error connecting to websocket: ${JSON.stringify(error)}`);
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
    }

    throw new Error(`Interface not ready after ${timeout} seconds`);
  }

  /**
   * Authenticate with the WebSocket server.
   * This should be called immediately after the WebSocket connection is established.
   */
  private async authenticate(): Promise<void> {
    if (!this.apiKey || !this.vmName) {
      // No authentication needed
      return;
    }

    this.logger.info('Performing authentication handshake...');
    const authMessage = {
      command: 'authenticate',
      params: {
        api_key: this.apiKey,
        container_name: this.vmName,
      },
    };

    return new Promise<void>((resolve, reject) => {
      const authHandler = (data: WebSocket.RawData) => {
        try {
          const authResult = JSON.parse(data.toString());
          if (!authResult.success) {
            const errorMsg = authResult.error || 'Authentication failed';
            this.logger.error(`Authentication failed: ${errorMsg}`);
            this.ws.close();
            reject(new Error(`Authentication failed: ${errorMsg}`));
          } else {
            this.logger.info('Authentication successful');
            this.ws.off('message', authHandler);
            resolve();
          }
        } catch (error) {
          this.ws.off('message', authHandler);
          reject(error);
        }
      };

      this.ws.on('message', authHandler);
      this.ws.send(JSON.stringify(authMessage));
    });
  }

  /**
   * Connect to the WebSocket server.
   */
  public async connect(): Promise<void> {
    // If the WebSocket is already open, check if we need to authenticate
    if (this.ws.readyState === WebSocket.OPEN) {
      this.logger.info('Websocket is open, ensuring authentication is complete.');
      return this.authenticate();
    }

    // If the WebSocket is closed or closing, reinitialize it
    if (this.ws.readyState === WebSocket.CLOSED || this.ws.readyState === WebSocket.CLOSING) {
      this.logger.info('Websocket is closed. Reinitializing connection.');
      const headers: { [key: string]: string } = {};
      if (this.apiKey && this.vmName) {
        headers['X-API-Key'] = this.apiKey;
        headers['X-VM-Name'] = this.vmName;
      }
      this.ws = new WebSocket(this.wsUri, { headers });
      return this.authenticate();
    }

    // Connect and authenticate
    return new Promise((resolve, reject) => {
      const onOpen = async () => {
        try {
          // Always authenticate immediately after connection
          await this.authenticate();
          resolve();
        } catch (error) {
          reject(error);
        }
      };

      // If already connecting, wait for it to complete then authenticate
      if (this.ws.readyState === WebSocket.CONNECTING) {
        this.ws.addEventListener('open', onOpen, { once: true });
        this.ws.addEventListener('error', (error) => reject(error), {
          once: true,
        });
        return;
      }

      // Set up event handlers
      this.ws.on('open', onOpen);

      this.ws.on('error', (error: Error) => {
        reject(error);
      });

      this.ws.on('close', () => {
        if (!this.closed) {
          // Attempt to reconnect
          setTimeout(() => this.connect(), 1000);
        }
      });
    });
  }

  /**
   * Send a command to the WebSocket server.
   */
  public async sendCommand(
    command: string,
    params: { [key: string]: unknown } = {}
  ): Promise<{ [key: string]: unknown }> {
    // Create a new promise for this specific command
    const commandPromise = new Promise<{ [key: string]: unknown }>((resolve, reject) => {
      // Chain it to the previous commands
      const executeCommand = async (): Promise<{
        [key: string]: unknown;
      }> => {
        if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
          await this.connect();
        }

        return new Promise<{ [key: string]: unknown }>((innerResolve, innerReject) => {
          const messageHandler = (data: WebSocket.RawData) => {
            try {
              const response = JSON.parse(data.toString());
              if (response.error) {
                innerReject(new Error(response.error));
              } else {
                innerResolve(response);
              }
            } catch (error) {
              innerReject(error);
            }
            this.ws.off('message', messageHandler);
          };

          this.ws.on('message', messageHandler);
          const wsCommand = { command, params };
          this.ws.send(JSON.stringify(wsCommand));
        });
      };

      // Add this command to the lock chain
      this.commandLock = this.commandLock.then(() => executeCommand().then(resolve, reject));
    });

    return commandPromise;
  }

  /**
   * Check if the WebSocket is connected.
   */
  public isConnected(): boolean {
    return this.ws && this.ws.readyState === WebSocket.OPEN;
  }

  /**
   * Close the interface connection.
   */
  disconnect(): void {
    this.closed = true;
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      this.ws.close();
    } else if (this.ws && this.ws.readyState === WebSocket.CONNECTING) {
      // If still connecting, terminate the connection attempt
      this.ws.terminate();
    }
  }

  /**
   * Force close the interface connection.
   * By default, this just calls close(), but subclasses can override
   * to provide more forceful cleanup.
   */
  forceClose(): void {
    this.disconnect();
  }

  // Mouse Actions
  abstract mouseDown(x?: number, y?: number, button?: MouseButton): Promise<void>;
  abstract mouseUp(x?: number, y?: number, button?: MouseButton): Promise<void>;
  abstract leftClick(x?: number, y?: number): Promise<void>;
  abstract rightClick(x?: number, y?: number): Promise<void>;
  abstract doubleClick(x?: number, y?: number): Promise<void>;
  abstract moveCursor(x: number, y: number): Promise<void>;
  abstract dragTo(x: number, y: number, button?: MouseButton, duration?: number): Promise<void>;
  abstract drag(
    path: Array<[number, number]>,
    button?: MouseButton,
    duration?: number
  ): Promise<void>;

  // Keyboard Actions
  abstract keyDown(key: string): Promise<void>;
  abstract keyUp(key: string): Promise<void>;
  abstract typeText(text: string): Promise<void>;
  abstract pressKey(key: string): Promise<void>;
  abstract hotkey(...keys: string[]): Promise<void>;

  // Scrolling Actions
  abstract scroll(x: number, y: number): Promise<void>;
  abstract scrollDown(clicks?: number): Promise<void>;
  abstract scrollUp(clicks?: number): Promise<void>;

  // Screen Actions
  abstract screenshot(): Promise<Buffer>;
  abstract getScreenSize(): Promise<ScreenSize>;
  abstract getCursorPosition(): Promise<CursorPosition>;

  // Window Management
  abstract open(target: string): Promise<void>;
  abstract launch(app: string, args?: string[]): Promise<number | undefined>;
  abstract getCurrentWindowId(): Promise<number | string>;
  abstract getApplicationWindows(app: string): Promise<Array<number | string>>;
  abstract getWindowName(windowId: number | string): Promise<string>;
  abstract getWindowSize(windowId: number | string): Promise<[number, number]>;
  abstract activateWindow(windowId: number | string): Promise<void>;
  abstract closeWindow(windowId: number | string): Promise<void>;

  // Desktop Actions
  abstract getDesktopEnvironment(): Promise<string>;
  abstract setWallpaper(path: string): Promise<void>;

  // Clipboard Actions
  abstract copyToClipboard(): Promise<string>;
  abstract setClipboard(text: string): Promise<void>;

  // File System Actions
  abstract fileExists(path: string): Promise<boolean>;
  abstract directoryExists(path: string): Promise<boolean>;
  abstract listDir(path: string): Promise<string[]>;
  abstract readText(path: string): Promise<string>;
  abstract writeText(path: string, content: string): Promise<void>;
  abstract readBytes(path: string): Promise<Buffer>;
  abstract writeBytes(path: string, content: Buffer): Promise<void>;
  abstract deleteFile(path: string): Promise<void>;
  abstract createDir(path: string): Promise<void>;
  abstract deleteDir(path: string): Promise<void>;
  abstract runCommand(command: string): Promise<[string, string]>;

  // Accessibility Actions
  abstract getAccessibilityTree(): Promise<AccessibilityNode>;
  abstract toScreenCoordinates(x: number, y: number): Promise<[number, number]>;
  abstract toScreenshotCoordinates(x: number, y: number): Promise<[number, number]>;
}
