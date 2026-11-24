// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, screen, waitFor } from "@testing-library/react-native";

import { ThreadScreen } from "../src/features/thread/ThreadScreen";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { renderWithApp } from "./helpers/render";

jest.mock("expo-router", () => ({ useRouter: () => ({ push: jest.fn() }) }));

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const thread = (id = "relay:sample-checkout-api") => renderWithApp(<ThreadScreen spaceId={id} onBack={() => {}} onFullStream={() => {}} />);

describe("a Space as a chat", () => {
  it("shows the room's messages, the Cua author with its APP tag, and the status pill under the header", async () => {
    await thread();
    expect(await screen.findByText(/nightly run has 1 failing test/)).toBeTruthy();
    expect(screen.getAllByText("Cua").length).toBeGreaterThan(0);
    expect(screen.getAllByText("APP").length).toBeGreaterThan(0);
    expect(screen.getByLabelText(/^checkout-api\. Needs you, approval/)).toBeTruthy();
    expect(screen.getByText("Sample data")).toBeTruthy();
  });

  it("draws the push approval as a small card, opens the changes with colored additions and deletions, then collapses when answered", async () => {
    const { backend } = await thread();
    expect(await screen.findByText(/Claude Code wants to push a branch and open a pull request on acme\/checkout-api/)).toBeTruthy();
    expect(screen.getByText("fix/retry-backoff into main")).toBeTruthy();
    expect(screen.getByText("+32")).toBeTruthy();
    expect(screen.getByText("-19")).toBeTruthy();
    expect(screen.queryByText("+9")).toBeNull();
    await fireEvent.press(screen.getByLabelText("Show changes"));
    expect(await screen.findAllByText("+9")).toBeTruthy();
    expect(screen.getAllByText("-14").length).toBeGreaterThan(0);
    await fireEvent.press(screen.getByLabelText("Push and open PR"));
    await waitFor(() => expect(screen.queryByText("+32")).toBeNull());
    expect(screen.getByText(/Approved the push and pull request of fix\/retry-backoff/)).toBeTruthy();
    const page = await backend.readThread("relay:sample-checkout-api", { after: 0 });
    expect(page.entries.at(-1)).toMatchObject({ kind: "answer", answer: { approve: true } });
  });

  it("groups tool calls into one muted phrase and opens it to short rows", async () => {
    await thread();
    const phrase = await screen.findByText("Read 2 files, edited 2 files, ran 1 command");
    expect(screen.queryByText("Ran npm test")).toBeNull();
    await fireEvent.press(phrase);
    expect(await screen.findByText("Ran npm test")).toBeTruthy();
  });

  it("sends a message with Send, and an @mention wakes an asleep agent", async () => {
    const { backend } = await thread("relay:sample-design-review");
    await screen.findByText(/Reviewed 3 PRs/);
    expect(screen.getByLabelText(/Send, type a message first/)).toBeTruthy();
    const box = screen.getByTestId("composer-input");
    await fireEvent.changeText(box, "@reviewer one more: PR 412");
    await fireEvent.press(screen.getByTestId("composer-send"));
    expect(await screen.findByText("@reviewer")).toBeTruthy();
    await waitFor(async () => expect((await backend.members("relay:sample-design-review"))[0]!.status).toBe("working"));
  });

  it("shows the live agent's words as they stream, then the reply", async () => {
    await thread("relay:sample-ledger");
    await screen.findByText(/Matched 41 of 44/);
    await fireEvent.changeText(screen.getByTestId("composer-input"), "@bookkeeper status?");
    await fireEvent.press(screen.getByTestId("composer-send"));
    expect(await screen.findByText(/Got it: "status\?"/, undefined, { timeout: 3000 })).toBeTruthy();
  });
});
