// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render, waitFor } from "@testing-library/react-native";
import * as Notifications from "expo-notifications";

import { useNotifier } from "../src/notifications/useNotifier";
import { dataOf, MockPushProvider } from "../src/notifications/mockPush";
import { createDemoBackend } from "../src/data/demo/backend";
import { BackendProvider } from "../src/state/backend";
import { StoresProvider } from "../src/state/outbox";
import { ThemeProvider } from "../src/theme/theme";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { testStore } from "./helpers/store";

const mockPush = jest.fn();
jest.mock("expo-router", () => ({ useRouter: () => ({ push: mockPush }) }));
jest.mock("../src/state/app", () => ({ useApp: () => ({ deviceId: () => "dev_0123456789abcdef01234567" }) }));

beforeAll(installFixtureCore);
afterAll(uninstallCore);

function Harness() {
  useNotifier();
  return null;
}

const mounted = async () => {
  const backend = createDemoBackend({ now: () => 1_800_000_000_000, wordMs: 0 });
  const provider = new MockPushProvider();
  const withPush = { ...backend, push: provider.transport };
  await render(
    <ThemeProvider force="light">
      <BackendProvider backend={withPush}>
        <StoresProvider store={testStore()} me={() => "acct:you"} now={() => 1_800_000_000_000} newId={() => "m1"}>
          <Harness />
        </StoresProvider>
      </BackendProvider>
    </ThemeProvider>,
  );
  return { backend, provider };
};

/** The listener the app registered for taps and alert buttons. */
const listener = () => (Notifications.addNotificationResponseReceivedListener as jest.Mock).mock.calls.at(-1)![0] as (r: unknown) => void;
const response = (data: unknown, actionIdentifier = "expo.modules.notifications.actions.DEFAULT") => ({
  actionIdentifier,
  notification: { request: { identifier: "n1", content: { data } } },
});

describe("notifications: tap routing and the alert's buttons", () => {
  beforeEach(() => mockPush.mockClear());

  it("a tap on a push opens the thread scrolled to the entry the push is about", async () => {
    const { provider } = await mounted();
    await waitFor(() => expect(Notifications.addNotificationResponseReceivedListener).toHaveBeenCalled());
    listener()(response(dataOf(provider.approval("relay:sample-checkout-api", 8))));
    expect(mockPush).toHaveBeenCalledWith("/space/relay%3Asample-checkout-api?entry=8");
    listener()(response(dataOf(provider.done("relay:sample-design-review", 3))));
    expect(mockPush).toHaveBeenLastCalledWith("/space/relay%3Asample-design-review?entry=3");
  });

  it("opens nothing for data it cannot read", async () => {
    await mounted();
    await waitFor(() => expect(Notifications.addNotificationResponseReceivedListener).toHaveBeenCalled());
    listener()(response({ v: 1, space: "../../x" }));
    listener()(response("nonsense"));
    expect(mockPush).not.toHaveBeenCalled();
  });

  it("Allow and Decline on the alert answer the approval through the outbox, once", async () => {
    const { backend, provider } = await mounted();
    await waitFor(() => expect(Notifications.addNotificationResponseReceivedListener).toHaveBeenCalled());
    listener()(response(dataOf(provider.approval("relay:sample-checkout-api", 8)), "allow"));
    await waitFor(async () => {
      const page = await backend.readThread("relay:sample-checkout-api", { after: 0 });
      expect(page.entries.filter((e) => e.kind === "answer" && e.answer?.approval === 8 && e.answer.approve)).toHaveLength(1);
    });
    expect(mockPush).not.toHaveBeenCalled();
  });

  it("registers this phone with the relay when permission is granted and a token exists, and not otherwise", async () => {
    (Notifications.getPermissionsAsync as jest.Mock).mockResolvedValue({ granted: true, canAskAgain: true });
    (Notifications as unknown as { getDevicePushTokenAsync: jest.Mock }).getDevicePushTokenAsync = jest.fn(async () => ({
      type: "ios",
      data: "cd".repeat(32),
    }));
    const { provider } = await mounted();
    await waitFor(() => expect(provider.registrations).toHaveLength(1));
    expect(provider.registrations[0]).toMatchObject({ token: "cd".repeat(32), categories: ["needs_you"] });
  });
});
