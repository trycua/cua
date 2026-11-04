// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, screen, waitFor } from "@testing-library/react-native";

import { ThreadScreen } from "../src/features/thread/ThreadScreen";
import { setClockForTests } from "../src/ui/clock";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { renderWithApp } from "./helpers/render";

jest.mock("expo-router", () => ({ useRouter: () => ({ push: jest.fn() }) }));

beforeAll(() => {
  installFixtureCore();
  setClockForTests(() => 1_800_000_000_000);
});
afterAll(() => {
  setClockForTests(null);
  uninstallCore();
});

const thread = (id = "relay:sample-checkout-api") => renderWithApp(<ThreadScreen spaceId={id} onBack={() => {}} onFullStream={() => {}} />);

describe("approvals: scopes and n of m", () => {
  it("says 1 of 2 and 2 of 2 when two approvals wait", async () => {
    await thread();
    expect(await screen.findByText("1 of 2")).toBeTruthy();
    expect(screen.getByText("2 of 2")).toBeTruthy();
  });

  it("offers Allow for this Space only on the approval the daemon says can be remembered, and never from model text", async () => {
    await thread();
    await screen.findByText("1 of 2");
    // The push card allows once only; the network request offers the Space grant.
    expect(screen.getAllByLabelText("Allow for this Space")).toHaveLength(1);
    expect(screen.getAllByLabelText("Decline").length).toBe(1);
    expect(screen.getByLabelText("Not now")).toBeTruthy();
    expect(screen.getByLabelText("Allow once")).toBeTruthy();
  });

  it("allows for this Space: the answer carries the scope and the card collapses to what was decided", async () => {
    const { backend } = await thread();
    await fireEvent.press(await screen.findByLabelText("Allow for this Space"));
    expect(await screen.findByText(/^Allowed for this Space: Allow reaching registry\.npmjs\.org to install the test dependencies\?/)).toBeTruthy();
    await waitFor(async () => {
      const page = await backend.readThread("relay:sample-checkout-api", { after: 0 });
      expect(page.entries.at(-1)).toMatchObject({ kind: "answer", answer: { approval: 8, approve: true, scope: "space" } });
    });
    // The other approval is still waiting, and the counter is gone with only one left.
    expect(screen.queryByText("1 of 2")).toBeNull();
  });

  it("declines with the core's word", async () => {
    await thread();
    await fireEvent.press(await screen.findByLabelText("Decline"));
    expect(await screen.findByText(/^Declined: Allow reaching registry/)).toBeTruthy();
  });

  it("an answer made offline waits on its card, is not asked twice, and goes once when the machine is back", async () => {
    const { demo, backend } = await thread();
    await screen.findByText("1 of 2");
    await act(async () => demo.sample!.setOffline(true));
    await fireEvent.press(screen.getByLabelText("Allow once"));
    expect(await screen.findByText("Waiting for a connection")).toBeTruthy();
    // The card cannot be answered twice while the answer waits.
    expect(screen.getByLabelText("Allow once").props.accessibilityState.disabled).toBe(true);
    await act(async () => demo.sample!.setOffline(false));
    await waitFor(() => expect(screen.queryByText("Waiting for a connection")).toBeNull(), { timeout: 10_000 });
    const page = await backend.readThread("relay:sample-checkout-api", { after: 0 });
    expect(page.entries.filter((e) => e.kind === "answer" && e.answer?.approval === 8)).toHaveLength(1);
  });
});
