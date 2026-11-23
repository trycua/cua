// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor } from "@testing-library/react-native";

import { ThreadScreen } from "../src/features/thread/ThreadScreen";
import { ToolGroup } from "../src/features/thread/cards";
import { ThemeProvider } from "../src/theme/theme";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { renderWithApp } from "./helpers/render";

jest.mock("expo-router", () => ({ useRouter: () => ({ push: jest.fn() }) }));

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const call = (id: string, cat: string, line: string, state: "running" | "done" | "failed", detail = "") => ({ id, cat, line, detail, state, atMs: 1 });

describe("tool rows in the thread: one muted phrase, short rows, the state of each call", () => {
  it("says a call is running or failed, in the core's words, and opens the detail on a tap", async () => {
    await render(
      <ThemeProvider force="light">
        <ToolGroup
          calls={[
            call("a", "read", "Read README.md", "done"),
            call("b", "ran", "Ran npm test", "running", "$ npm test"),
            call("c", "ran", "Ran make", "failed", "exit 2"),
          ]}
        />
      </ThemeProvider>,
    );
    const group = screen.getByTestId("tool-group");
    expect(group.props.accessibilityLabel).toBe("Read 1 file, ran 2 commands. Show details");
    expect(screen.getByText("Read 1 file, ran 2 commands")).toBeTruthy();
    await fireEvent.press(group);
    expect(screen.getByLabelText("Ran npm test, Running")).toBeTruthy();
    expect(screen.getByLabelText("Ran make, Failed")).toBeTruthy();
    expect(screen.queryByText("$ npm test")).toBeNull();
    await fireEvent.press(screen.getByLabelText("Ran npm test, Running"));
    expect(screen.getByText("$ npm test")).toBeTruthy();
  });

  it("shows the sample agent's rows from the same feed a live daemon sends, and new rows appear as an agent works", async () => {
    await renderWithApp(<ThreadScreen spaceId="relay:sample-ledger" onBack={() => {}} onFullStream={() => {}} />);
    await screen.findByText(/Matched 41 of 44/);
    expect(screen.queryByTestId("tool-group")).toBeNull();
    await fireEvent.changeText(screen.getByTestId("composer-input"), "@bookkeeper status?");
    await fireEvent.press(screen.getByTestId("composer-send"));
    // The reply's calls arrive through the long poll after the message that asked.
    await waitFor(() => expect(screen.getByText("Read 1 file, ran 2 commands")).toBeTruthy(), { timeout: 8_000 });
  });
});
