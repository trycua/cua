/**
 * Shared Cua Driver runtime for the Eve tools in `agent/tools/`.
 *
 * Eve runs authored tools in the app runtime, so this module imports the
 * in-process `@trycua/cua-driver` SDK directly. It is modeled on
 * `../../../agent-sdks/native-tools.ts`: every mutation is followed by a fresh
 * observation, and an action whose outcome is unknown is reported as such
 * instead of being retried.
 */

import {
  ActionResult,
  ActionTarget,
  ClickButton,
  ClickInput,
  ClickPosition,
  CuaDriver,
  GetDesktopStateInput,
  InputDeliveryMode,
  PressKeyInput,
  ToolResult,
  TypeTextInput,
} from '@trycua/cua-driver';
import { toolOutput, toolOutputPart } from 'eve/tools';

/** JSON-serializable tool result. Eve persists tool outputs durably. */
export type DesktopObservation = {
  text: string;
  images: { dataBase64: string; mimeType: string }[];
  isError: boolean;
};

const TIMEOUT_MS = 30_000;

let driver: ReturnType<typeof CuaDriver.create> | undefined;
const desktopTarget = new ActionTarget.Desktop({ displayId: 'primary' });

/**
 * One Driver client per app-runtime process. The client owns one implicit
 * Driver lifecycle session, which every Eve session in this process shares.
 * `eve dev` can load a new runtime generation after a source change; that
 * generation creates its own client on first use.
 */
function client(): ReturnType<typeof CuaDriver.create> {
  driver ??= CuaDriver.create(undefined);
  return driver;
}

export async function observe(signal?: AbortSignal): Promise<DesktopObservation> {
  const result = await bounded(
    client().getDesktopState(GetDesktopStateInput.new({})),
    'get_desktop_state',
    signal
  );
  return fromToolResult(result);
}

export async function click(
  x: number,
  y: number,
  signal?: AbortSignal
): Promise<DesktopObservation> {
  return await mutateThenObserve(
    () =>
      client().click(
        ClickInput.new({
          position: new ClickPosition.Coordinates({ x, y }),
          target: desktopTarget,
          deliveryMode: InputDeliveryMode.Foreground,
          button: ClickButton.Left,
          count: 1,
        })
      ),
    signal
  );
}

export async function typeText(text: string, signal?: AbortSignal): Promise<DesktopObservation> {
  return await mutateThenObserve(
    () => client().typeText(TypeTextInput.new({ text, target: desktopTarget })),
    signal
  );
}

export async function pressKey(key: string, signal?: AbortSignal): Promise<DesktopObservation> {
  return await mutateThenObserve(
    () => client().pressKey(PressKeyInput.new({ key, target: desktopTarget })),
    signal
  );
}

/**
 * Projects an observation into model content parts: the Driver's text summary
 * plus each screenshot as an image part. The full output stays available to
 * channels and hooks on `action.result`.
 */
export function toModelContent(output: DesktopObservation) {
  return toolOutput.content([
    toolOutputPart.text(output.text),
    ...output.images.map((image) =>
      toolOutputPart.file(image.dataBase64, { mediaType: image.mimeType })
    ),
  ]);
}

async function mutateThenObserve(
  operation: () => Promise<ToolResult | ActionResult>,
  signal?: AbortSignal
): Promise<DesktopObservation> {
  let unknownDetail: string | undefined;
  try {
    const result = await bounded(operation(), 'desktop action', signal);
    if ('isError' in result && result.isError) {
      unknownDetail =
        `Action reported an error and its outcome may be unknown (${result.text}). ` +
        'A fresh observation follows. Do not retry until the observation proves ' +
        'the action did not land.';
    }
  } catch (error) {
    if (signal?.aborted) throw error;
    unknownDetail =
      `Action outcome is unknown (${String(error)}). A fresh observation follows. ` +
      'Do not retry until the observation proves the action did not land.';
  }

  const observation = await observe(signal);
  if (unknownDetail) {
    observation.text = `${unknownDetail}\n\n${observation.text}`;
  }
  return observation;
}

/**
 * Bounds how long a tool call waits for the Driver. A timeout or turn
 * cancellation stops the wait; it does not cancel work the Driver already
 * started, which is why a mutation's outcome is then treated as unknown.
 */
async function bounded<T>(operation: Promise<T>, label: string, signal?: AbortSignal): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let onAbort: (() => void) | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} timed out`)), TIMEOUT_MS);
        if (signal) {
          onAbort = () => reject(new Error(`${label} cancelled`));
          if (signal.aborted) onAbort();
          else signal.addEventListener('abort', onAbort, { once: true });
        }
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
    if (signal && onAbort) signal.removeEventListener('abort', onAbort);
  }
}

function fromToolResult(result: ToolResult): DesktopObservation {
  return {
    text: result.text,
    images: result.images.map((image) => ({
      dataBase64: image.dataBase64,
      mimeType: image.mimeType,
    })),
    isError: result.isError,
  };
}
