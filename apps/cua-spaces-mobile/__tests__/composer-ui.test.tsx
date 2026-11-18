// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen } from "@testing-library/react-native";

import { Composer } from "../src/features/thread/Composer";
import { ThemeProvider } from "../src/theme/theme";

import { installFixtureCore, uninstallCore } from "./helpers/core";

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const members = [
  { handle: "claude", name: "Claude Code", kind: "agent" as const, note: null },
  { handle: "sam", name: "Sam", kind: "person" as const, note: null },
];

async function setup(over: Partial<React.ComponentProps<typeof Composer>> = {}) {
  const props = {
    placeholder: "Message checkout-api",
    mentionables: members,
    running: false,
    onSend: jest.fn(),
    onStop: jest.fn(),
    onSendWithScreen: jest.fn(),
    ...over,
  };
  await render(
    <ThemeProvider force="light">
      <Composer {...props} />
    </ThemeProvider>,
  );
  return props;
}

describe("the composer", () => {
  it("has three button states: idle until there is text, Send with text, Stop while an agent runs and the box is empty", async () => {
    const p = await setup({ running: true });
    expect(screen.getByLabelText("Stop")).toBeTruthy();
    await fireEvent.press(screen.getByLabelText("Stop"));
    expect(p.onStop).toHaveBeenCalled();
  });

  it("sends with Send while idle", async () => {
    const p = await setup();
    await fireEvent.changeText(screen.getByTestId("composer-input"), "hello");
    expect(screen.getByLabelText("Send")).toBeTruthy();
    await fireEvent.press(screen.getByLabelText("Send"));
    expect(p.onSend).toHaveBeenCalledWith("hello", "send");
  });

  it("while an agent runs, the send button is Queue, with the queued count, and Steer is the other choice", async () => {
    const p = await setup({ running: true, queued: 2, canSteer: true });
    await fireEvent.changeText(screen.getByTestId("composer-input"), "and then open a PR");
    expect(screen.getByLabelText("Queue")).toBeTruthy();
    expect(screen.getByText("Queue")).toBeTruthy();
    expect(screen.getByTestId("queued-line").props.children).toBe("2 messages queued");
    await fireEvent.press(screen.getByLabelText("Queue"));
    expect(p.onSend).toHaveBeenCalledWith("and then open a PR", "queue");
    await fireEvent.changeText(screen.getByTestId("composer-input"), "stop, do this instead");
    const send = screen.getByTestId("composer-send");
    // The non-gesture route: the same accessibility action Steer has on the button.
    await fireEvent(send, "accessibilityAction", { nativeEvent: { actionName: "steer" } });
    expect(p.onSend).toHaveBeenLastCalledWith("stop, do this instead", "steer");
  });

  it("offers no Steer where the daemon cannot interrupt", async () => {
    await setup({ running: true, canSteer: false });
    await fireEvent.changeText(screen.getByTestId("composer-input"), "hello");
    expect(screen.getByTestId("composer-send").props.accessibilityActions).toBeUndefined();
  });

  it("is disabled while empty", async () => {
    const p = await setup();
    await fireEvent.press(screen.getByTestId("composer-send"));
    expect(p.onSend).not.toHaveBeenCalled();
    expect(screen.getByLabelText(/Send, type a message first/).props.accessibilityState.disabled).toBe(true);
  });

  it("opens a mention picker on @ and inserts the handle", async () => {
    await setup();
    const box = screen.getByTestId("composer-input");
    await fireEvent.changeText(box, "@");
    await fireEvent(box, "selectionChange", { nativeEvent: { selection: { start: 1, end: 1 } } });
    await fireEvent.press(screen.getByLabelText("Mention Claude Code"));
    expect(screen.getByTestId("composer-input").props.value).toBe("@claude ");
  });

  it("explains why it cannot post for a viewer", async () => {
    await setup({ readOnly: "You can watch this Space." });
    expect(screen.getByText("You can watch this Space.")).toBeTruthy();
    expect(screen.queryByTestId("composer-input")).toBeNull();
  });
});
