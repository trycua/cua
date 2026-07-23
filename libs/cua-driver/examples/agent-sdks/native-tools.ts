/** Narrow agent-tool adapter over the same-process Cua Driver TypeScript SDK. */

import { randomUUID } from 'node:crypto';

import {
  CaptureScope,
  ClickButton,
  ClickInput,
  CuaDriver,
  DesktopScope,
  EndSessionInput,
  GetDesktopStateInput,
  PressKeyInput,
  StartSessionInput,
  ToolResult,
  TypeTextInput,
} from '@trycua/cua-driver';

type McpContent =
  | { type: 'text'; text: string }
  | { type: 'image'; data: string; mimeType: string };

export type NativeToolResult = {
  content: McpContent[];
  isError?: boolean;
};

export class NativeDesktopTools {
  readonly driver = CuaDriver.create(undefined);
  readonly session = `claude-native-${randomUUID().slice(0, 12)}`;
  private started = false;

  constructor(private readonly timeoutMs = 30_000) {}

  async start(): Promise<void> {
    await this.bounded(
      this.driver.startSession(
        StartSessionInput.new({
          session: this.session,
          captureScope: CaptureScope.Desktop,
        })
      ),
      'start_session'
    );
    this.started = true;
  }

  async close(): Promise<void> {
    try {
      if (this.started) {
        await this.bounded(
          this.driver.endSession(EndSessionInput.new({ session: this.session })),
          'end_session'
        );
        this.started = false;
      }
    } finally {
      try {
        await this.driver.shutdown();
      } finally {
        // Generated UniFFI bindings expose deterministic handle release
        // separately from asynchronous runtime shutdown.
        (this.driver as unknown as { uniffiDestroy(): void }).uniffiDestroy();
      }
    }
  }

  async observe(): Promise<NativeToolResult> {
    const result = await this.bounded(
      this.driver.getDesktopState(GetDesktopStateInput.new({ session: this.session })),
      'get_desktop_state'
    );
    return this.content(result);
  }

  async click(x: number, y: number): Promise<NativeToolResult> {
    return await this.mutateThenObserve(() =>
      this.driver.click(
        ClickInput.new({
          x,
          y,
          scope: DesktopScope.Desktop,
          session: this.session,
          button: ClickButton.Left,
          count: 1,
        })
      )
    );
  }

  async typeText(text: string): Promise<NativeToolResult> {
    return await this.mutateThenObserve(() =>
      this.driver.typeText(
        TypeTextInput.new({
          text,
          scope: DesktopScope.Desktop,
          session: this.session,
        })
      )
    );
  }

  async pressKey(key: string): Promise<NativeToolResult> {
    return await this.mutateThenObserve(() =>
      this.driver.pressKey(
        PressKeyInput.new({
          key,
          scope: DesktopScope.Desktop,
          session: this.session,
        })
      )
    );
  }

  private async mutateThenObserve(operation: () => Promise<ToolResult>): Promise<NativeToolResult> {
    let unknownDetail: string | undefined;
    try {
      const result = await this.bounded(operation(), 'desktop action');
      if (result.isError) {
        unknownDetail =
          `Action reported an error and its outcome may be unknown (${result.text}). ` +
          'A fresh observation follows. Do not retry until the observation proves ' +
          'the action did not land.';
      }
    } catch (error) {
      unknownDetail =
        `Action outcome is unknown (${String(error)}). A fresh observation follows. ` +
        'Do not retry until the observation proves the action did not land.';
    }

    const observation = await this.observe();
    if (unknownDetail) {
      observation.content.unshift({ type: 'text', text: unknownDetail });
    }
    return observation;
  }

  private async bounded<T>(operation: Promise<T>, label: string): Promise<T> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        operation,
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new Error(`${label} timed out`)), this.timeoutMs);
        }),
      ]);
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  }

  private content(result: ToolResult): NativeToolResult {
    return {
      content: [
        { type: 'text', text: result.text },
        ...result.images.map((image) => ({
          type: 'image' as const,
          data: image.dataBase64,
          mimeType: image.mimeType,
        })),
      ],
      isError: result.isError,
    };
  }
}
