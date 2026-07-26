/** Perceive, act, and externally verify with the same-process TypeScript SDK. */

import { randomUUID } from 'node:crypto';

import {
  CaptureScope,
  CuaDriver,
  DesktopScope,
  EndSessionInput,
  GetDesktopStateInput,
  PressKeyInput,
  StartSessionInput,
  TypeTextInput,
} from '@trycua/cua-driver';

const fixture = process.env.CUA_FIXTURE_URL ?? 'http://127.0.0.1:8765';
const timeoutMs = 10_000;

async function bounded<T>(promise: Promise<T>, label: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} timed out`)), timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

async function resetFixture(): Promise<void> {
  const response = await fetch(`${fixture}/reset`, { method: 'POST' });
  if (!response.ok) throw new Error(`fixture reset failed: ${response.status}`);
}

async function waitForSubmission(token: string): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const response = await fetch(`${fixture}/state`);
    const state = (await response.json()) as { submitted: string | null };
    if (state.submitted === token) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error('the fixture did not observe the submitted value');
}

const token = `cua-${randomUUID().slice(0, 10)}`;
const session = `native-typescript-${randomUUID().slice(0, 10)}`;
await resetFixture();

const driver = CuaDriver.create(undefined);
let started = false;
let mutationOutcomeUnknown = false;

try {
  await bounded(
    driver.startSession(
      StartSessionInput.new({
        session,
        captureScope: CaptureScope.Desktop,
      })
    ),
    'start_session'
  );
  started = true;

  const before = await bounded(
    driver.getDesktopState(GetDesktopStateInput.new({ session })),
    'get_desktop_state'
  );
  if (before.isError || before.images.length === 0) {
    throw new Error(before.text || 'desktop capture returned no image');
  }
  console.log(`perceived desktop: ${before.images[0]?.mimeType}`);

  try {
    const typed = await bounded(
      driver.typeText(
        TypeTextInput.new({
          text: token,
          scope: DesktopScope.Desktop,
          session,
        })
      ),
      'type_text'
    );
    if (typed.isError) throw new Error(typed.text);

    const submitted = await bounded(
      driver.pressKey(
        PressKeyInput.new({
          key: 'ENTER',
          scope: DesktopScope.Desktop,
          session,
        })
      ),
      'press_key'
    );
    if (submitted.isError) throw new Error(submitted.text);
  } catch {
    mutationOutcomeUnknown = true;
  }

  // A timed-out mutation may still have landed. Observe the independent
  // fixture before considering any retry.
  await waitForSubmission(token);
  const after = await bounded(
    driver.getDesktopState(GetDesktopStateInput.new({ session })),
    'post-action get_desktop_state'
  );
  if (after.isError || after.images.length === 0) {
    throw new Error(after.text || 'post-action capture returned no image');
  }
  console.log(`verified submitted value: ${token}`);
  if (mutationOutcomeUnknown) {
    console.log('the driver response was uncertain; the external postcondition resolved it');
  }
} finally {
  try {
    if (started) {
      await driver.endSession(EndSessionInput.new({ session }));
    }
  } finally {
    try {
      await driver.shutdown();
    } finally {
      // shutdown() drains the runtime; this generated destructor releases the
      // native UniFFI handle immediately instead of waiting for JavaScript GC.
      (driver as unknown as { uniffiDestroy(): void }).uniffiDestroy();
    }
  }
}
