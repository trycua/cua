// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen } from "@testing-library/react-native";

import { ChildSpaceRow } from "../src/features/thread/bubbles";
import { ThemeProvider } from "../src/theme/theme";

const mockPush = jest.fn();
jest.mock("expo-router", () => ({ useRouter: () => ({ push: mockPush }) }));

const row = (state: "creating" | "ready" | "failed") => (
  <ThemeProvider force="light">
    <ChildSpaceRow
      space={{
        kind: "child_space",
        space: "local:mac",
        key: "mac:500",
        name: "mac",
        os: "macOS",
        agent: "claude",
        state,
        phase: "Cloning the disk",
        error: "no disk",
        startedMs: 500,
        endedMs: state === "creating" ? null : 900,
      }}
    />
  </ThemeProvider>
);

beforeEach(() => mockPush.mockClear());

describe("a child Space row on the phone", () => {
  it.each(["creating", "failed"] as const)("is not a link while it is %s, because its Space does not exist", async (state) => {
    await render(row(state));
    const r = screen.getByTestId("child-space-local:mac");
    await fireEvent.press(r);
    expect(mockPush).not.toHaveBeenCalled();
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("opens the Space's page once it is ready", async () => {
    await render(row("ready"));
    await fireEvent.press(screen.getByRole("link"));
    expect(mockPush).toHaveBeenCalledWith({ pathname: "/space/[id]", params: { id: "local:mac" } });
  });
});
