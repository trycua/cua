// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react-native";
import { Linking } from "react-native";

import { Composer } from "../src/features/thread/Composer";
import { setVoiceForTests } from "../src/features/thread/dictation";
import { ThemeProvider } from "../src/theme/theme";
import { installFixtureCore, uninstallCore } from "./helpers/core";

beforeAll(installFixtureCore);
afterAll(uninstallCore);
afterEach(() => setVoiceForTests(undefined));

type Handlers = {
  onResult: ((e: { text: string; final: boolean }) => void)[];
  onEnd: (() => void)[];
  onError: ((e: { kind: string; message: string }) => void)[];
};

/** A speech module that records what the app asks of it and lets the test play the recognizer. */
function fakeVoice(status: { microphone: string; speech: string; recognizer?: boolean; onDevice?: boolean }) {
  const h: Handlers = { onResult: [], onEnd: [], onError: [] };
  let state = { microphone: status.microphone, speech: status.speech };
  const mod = {
    available: true,
    status: jest.fn(async () => ({ ...state, recognizer: status.recognizer ?? true, onDevice: status.onDevice ?? true, listening: false })),
    requestPermissions: jest.fn(async () => {
      state = { microphone: "granted", speech: "granted" };
      return state;
    }),
    start: jest.fn(async () => {}),
    stop: jest.fn(async () => {}),
    addListener: jest.fn((event: keyof Handlers, cb: never) => {
      (h[event] as unknown[]).push(cb);
      return { remove: () => void (h[event] as unknown[]).splice((h[event] as unknown[]).indexOf(cb), 1) };
    }),
  };
  setVoiceForTests(mod as never);
  return {
    mod,
    say: (text: string, final = false) => act(() => h.onResult.forEach((f) => f({ text, final }))),
    end: () => act(() => h.onEnd.forEach((f) => f())),
    error: (kind: string) => act(() => h.onError.forEach((f) => f({ kind, message: kind }))),
  };
}

const composer = async () => {
  const onSend = jest.fn();
  await render(
    <ThemeProvider force="light">
      <Composer placeholder="Message checkout-api" mentionables={[]} running={false} onSend={onSend} onStop={jest.fn()} onSendWithScreen={jest.fn()} />
    </ThemeProvider>,
  );
  return onSend;
};

describe("dictation into the message box (the phone's own speech recognizer)", () => {
  it("holds to talk: listens while the button is down, types what it hears, replaces a changed guess, and stops on release", async () => {
    const v = fakeVoice({ microphone: "granted", speech: "granted" });
    await composer();
    const mic = await screen.findByTestId("composer-mic");
    expect(mic.props.accessibilityLabel).toBe("Dictate");
    await fireEvent(mic, "pressIn");
    await waitFor(() => expect(v.mod.start).toHaveBeenCalledTimes(1));
    await v.say("fix the login");
    expect(screen.getByTestId("composer-input").props.value).toBe("fix the login");
    await v.say("fix the login test");
    expect(screen.getByTestId("composer-input").props.value).toBe("fix the login test");
    expect(screen.getByTestId("voice-status")).toBeTruthy();
    expect(screen.getByText("Dictation stays on this phone.")).toBeTruthy();
    await fireEvent(mic, "pressOut");
    await waitFor(() => expect(v.mod.stop).toHaveBeenCalled());
    await v.end();
    // What was heard stays in the box, ready to send.
    expect(screen.getByTestId("composer-input").props.value).toBe("fix the login test");
    expect(screen.getByLabelText("Send")).toBeTruthy();
  });

  it("puts the transcript after what is already typed, with a space between", async () => {
    const v = fakeVoice({ microphone: "granted", speech: "granted" });
    await composer();
    await fireEvent.changeText(screen.getByTestId("composer-input"), "please");
    await fireEvent(screen.getByTestId("composer-input"), "selectionChange", { nativeEvent: { selection: { start: 6, end: 6 } } });
    await fireEvent(screen.getByTestId("composer-mic"), "pressIn");
    await waitFor(() => expect(v.mod.start).toHaveBeenCalled());
    await v.say("add a test");
    expect(screen.getByTestId("composer-input").props.value).toBe("please add a test");
  });

  it("asks for microphone and speech access on the first press and listens from the next", async () => {
    const v = fakeVoice({ microphone: "undetermined", speech: "undetermined" });
    await composer();
    const mic = await screen.findByTestId("composer-mic");
    expect(mic.props.accessibilityHint).toBe("Asks for microphone and speech access, then listens.");
    await fireEvent(mic, "pressIn");
    await waitFor(() => expect(v.mod.requestPermissions).toHaveBeenCalled());
    expect(v.mod.start).not.toHaveBeenCalled();
    await fireEvent(mic, "pressOut");
    await fireEvent(screen.getByTestId("composer-mic"), "pressIn");
    await waitFor(() => expect(v.mod.start).toHaveBeenCalled());
  });

  it("when access was refused, says where to turn it on and does not listen", async () => {
    const v = fakeVoice({ microphone: "denied", speech: "granted" });
    const open = jest.spyOn(Linking, "openSettings").mockResolvedValue();
    await composer();
    expect(await screen.findByText("Microphone access is off. Turn it on in Settings to dictate.")).toBeTruthy();
    await fireEvent(screen.getByTestId("composer-mic"), "pressIn");
    expect(v.mod.start).not.toHaveBeenCalled();
    await fireEvent.press(screen.getByText("Open Settings"));
    expect(open).toHaveBeenCalled();
    open.mockRestore();
  });

  it("says plainly when there is no recognizer for the language, and when a press heard nothing", async () => {
    const none = fakeVoice({ microphone: "granted", speech: "granted", recognizer: false });
    await composer();
    expect(await screen.findByText("Dictation isn't available for this language right now.")).toBeTruthy();
    expect(none.mod.start).not.toHaveBeenCalled();
  });

  it("offers Try again after an error", async () => {
    const v = fakeVoice({ microphone: "granted", speech: "granted" });
    await composer();
    await fireEvent(await screen.findByTestId("composer-mic"), "pressIn");
    await waitFor(() => expect(v.mod.start).toHaveBeenCalledTimes(1));
    await v.error("no-speech");
    expect(await screen.findByText("Didn't hear anything. Try again.")).toBeTruthy();
    await fireEvent.press(screen.getByText("Try again"));
    await waitFor(() => expect(v.mod.start).toHaveBeenCalledTimes(2));
  });

  it("has no microphone button where there is no speech module (Android here)", async () => {
    setVoiceForTests(null);
    await composer();
    expect(screen.queryByTestId("composer-mic")).toBeNull();
    expect(screen.getByLabelText(/Send, type a message first/)).toBeTruthy();
  });

  it("is reachable without holding: VoiceOver's activate starts and stops it", async () => {
    const v = fakeVoice({ microphone: "granted", speech: "granted" });
    await composer();
    const mic = await screen.findByTestId("composer-mic");
    await fireEvent(mic, "accessibilityAction", { nativeEvent: { actionName: "activate" } });
    await waitFor(() => expect(v.mod.start).toHaveBeenCalled());
  });
});
