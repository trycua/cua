// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, screen, waitFor } from "@testing-library/react-native";

import { ThreadScreen } from "../src/features/thread/ThreadScreen";
import { setClockForTests } from "../src/ui/clock";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { renderWithApp } from "./helpers/render";
import { memoryKv, testStore } from "./helpers/store";

// A restart is two launches of the thread, and the retry waits are real time.
jest.setTimeout(30_000);

jest.mock("expo-router", () => ({ useRouter: () => ({ push: jest.fn() }) }));

beforeAll(() => {
  installFixtureCore();
  setClockForTests(() => 1_800_000_000_000);
});
afterAll(() => {
  setClockForTests(null);
  uninstallCore();
});

const thread = (opts: Parameters<typeof renderWithApp>[1] = {}, id = "relay:sample-design-review") =>
  renderWithApp(<ThreadScreen spaceId={id} onBack={() => {}} onFullStream={() => {}} />, opts);

describe("sending while the connection is down", () => {
  it("keeps the message with an honest state, sends it once when the machine is back, and shows it as sent", async () => {
    const { demo, backend } = await thread();
    await screen.findByText(/Reviewed 3 PRs/);
    await act(async () => demo.sample!.setOffline(true));
    await fireEvent.changeText(screen.getByTestId("composer-input"), "@reviewer check PR 413 too");
    await fireEvent.press(screen.getByTestId("composer-send"));
    // The message shows at once as waiting, never as sent.
    expect(await screen.findByText("Waiting for a connection")).toBeTruthy();
    expect(screen.getByLabelText(/^@reviewer check PR 413 too\. Waiting for a connection/)).toBeTruthy();
    await act(async () => demo.sample!.setOffline(false));
    await waitFor(() => expect(screen.queryByText("Waiting for a connection")).toBeNull(), { timeout: 10_000 });
    const page = await backend.readThread("relay:sample-design-review", { after: 0 });
    expect(page.entries.filter((e) => e.text === "@reviewer check PR 413 too")).toHaveLength(1);
  });

  it("opening a thread that was never loaded while offline is the offline state, with a message that waits, not an error screen", async () => {
    const { demo } = await thread({ offline: true });
    expect(await screen.findByText("You're offline. Nothing was saved to show yet.")).toBeTruthy();
    expect(screen.queryByText("Could not open this thread")).toBeNull();
    await fireEvent.changeText(screen.getByTestId("composer-input"), "@reviewer hello later");
    await fireEvent.press(screen.getByTestId("composer-send"));
    expect(await screen.findByText("Waiting for a connection")).toBeTruthy();
    await act(async () => demo.sample!.setOffline(false));
    await waitFor(() => expect(screen.queryByText("Waiting for a connection")).toBeNull(), { timeout: 10_000 });
  });

  it("survives a restart: the message written down before the app quit is sent once by the next launch", async () => {
    const kv = memoryKv();
    const first = await thread({ store: testStore(kv), offline: false });
    await screen.findByText(/Reviewed 3 PRs/);
    await act(async () => first.demo.sample!.setOffline(true));
    await fireEvent.changeText(screen.getByTestId("composer-input"), "@reviewer one more");
    await fireEvent.press(screen.getByTestId("composer-send"));
    await screen.findByText("Waiting for a connection");
    // What is on disk is the sealed outbox (the fake sealer binds the slot like the real one).
    await waitFor(() => expect([...kv.data.keys()].some((k) => k.endsWith(":outbox"))).toBe(true));
    first.unmount();
    // A new launch: the same disk, a connected sample machine.
    const second = await thread({ store: testStore(kv) });
    await screen.findByText(/Reviewed 3 PRs/);
    await waitFor(
      async () => {
        const page = await second.backend.readThread("relay:sample-design-review", { after: 0 });
        expect(page.entries.filter((e) => e.text === "@reviewer one more")).toHaveLength(1);
      },
      { timeout: 10_000 },
    );
    expect(screen.queryByText("Waiting for a connection")).toBeNull();
  });
});
