// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, screen, waitFor } from "@testing-library/react-native";

import { DiffScreen } from "../src/features/diff/DiffScreen";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { renderWithApp } from "./helpers/render";

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const diff = (props: Partial<React.ComponentProps<typeof DiffScreen>> = {}) =>
  renderWithApp(<DiffScreen spaceId="relay:sample-checkout-api" onBack={() => {}} {...props} />);

describe("reviewing what an agent changed", () => {
  it("lists the files with the same counts and bars as the push card", async () => {
    await diff();
    expect(await screen.findByText("5 files")).toBeTruthy();
    expect(screen.getAllByText("+32").length).toBeGreaterThan(0);
    expect(screen.getAllByText("-19").length).toBeGreaterThan(0);
    expect(screen.getByText("fix/retry-backoff into main", { exact: false })).toBeTruthy();
    // The renamed, added, deleted and binary files say so.
    expect(screen.getByLabelText(/^src\/net\/transport\.py, Renamed/)).toBeTruthy();
    expect(screen.getByLabelText(/^tests\/test_ws_reconnect\.py, Added, \+21/)).toBeTruthy();
    expect(screen.getByLabelText(/^docs\/retry-notes\.md, Deleted, -14/)).toBeTruthy();
    expect(screen.getByLabelText(/^assets\/status\.png, Modified/)).toBeTruthy();
  });

  it("opens a file's diff as numbered hunks and offers the changed words", async () => {
    await diff();
    await fireEvent.press(await screen.findByTestId("diff-file-ws_reconnect.py"));
    expect(await screen.findByText(/^@@ -18,10 \+18,17 @@/)).toBeTruthy();
    expect(screen.getByLabelText(/^Added, line 21: self\.base = base/)).toBeTruthy();
    expect(screen.getByLabelText(/^Removed, line 18: /)).toBeTruthy();
    // Unified shows no highlights; Words marks what changed inside a changed line.
    expect(screen.queryAllByTestId("changed-segment")).toHaveLength(0);
    await fireEvent.press(screen.getByTestId("mode-words"));
    const marks = screen.getAllByTestId("changed-segment");
    expect(marks.length).toBeGreaterThan(0);
    expect(marks.map((m) => m.props.children).join("")).toContain("jitter=0.1");
  });

  it("previews an image, which has no text diff, and text files", async () => {
    await diff();
    await fireEvent.press(await screen.findByTestId("diff-file-status.png"));
    expect(await screen.findByTestId("preview-image")).toBeTruthy();
    expect(screen.getByText(/status\.png, 283 bytes/)).toBeTruthy();
    // The diff tab says there is none.
    await fireEvent.press(screen.getByTestId("tab-diff"));
    expect(await screen.findByText("Binary file. There is no text diff to show.")).toBeTruthy();
  });

  it("goes back to the list, and straight to a file when opened from one", async () => {
    const back = jest.fn();
    await diff({ onBack: back });
    await fireEvent.press(await screen.findByTestId("diff-file-retry-notes.md"));
    await screen.findByText(/^@@ -1,4 \+0,0 @@/);
    await fireEvent.press(screen.getByTestId("diff-back"));
    expect(await screen.findByText("5 files")).toBeTruthy();
    await fireEvent.press(screen.getByTestId("diff-back"));
    expect(back).toHaveBeenCalled();
  });

  it("says why when a Space cannot list its changes, and tries again", async () => {
    let n = 0;
    await renderWithApp(<DiffScreen spaceId="relay:sample-checkout-api" onBack={() => {}} />, {
      wrap: (b) => ({
        ...b,
        changes: async (...a: Parameters<NonNullable<typeof b.changes>>) => {
          if (n++ === 0) throw new Error("This machine has no changes tool.");
          return b.changes!(...a);
        },
      }),
    });
    expect(await screen.findByText("This Space cannot list its changes from the phone yet.")).toBeTruthy();
    expect(screen.getByText("This machine has no changes tool.")).toBeTruthy();
    await fireEvent.press(screen.getByText("Try again"));
    await waitFor(() => expect(screen.getByText("5 files")).toBeTruthy());
  });
});
