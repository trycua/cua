// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor } from "@testing-library/react-native";
import * as Notifications from "expo-notifications";

import { SettingsScreen } from "../src/features/settings/SettingsScreen";
import { usePushState } from "../src/notifications/push";
import { usePrefs } from "../src/state/prefs";
import { ThemeProvider } from "../src/theme/theme";
import { installFixtureCore, uninstallCore } from "./helpers/core";

const mockSample = { capabilities: { sample: true }, sample: { offline: false, setOffline: jest.fn() } };
jest.mock("../src/state/app", () => ({
  useApp: () => ({ account: null, signOut: jest.fn(), backend: mockSample, deviceId: () => "dev_0123456789abcdef01234567" }),
}));

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const settings = () =>
  render(
    <ThemeProvider force="light">
      <SettingsScreen onClose={() => {}} />
    </ThemeProvider>,
  );

describe("Settings: notifications and the sample switch", () => {
  beforeEach(() => {
    usePushState.setState({ registered: false, unavailable: false, configured: true });
    usePrefs.setState({ notifyNeedsYou: true, notifyDone: false, sampleOffline: false });
  });

  it("asks to allow notifications until they are allowed, in the core's words", async () => {
    await settings();
    expect(await screen.findByText("Allow notifications so your agents can reach you.")).toBeTruthy();
    expect(screen.getByText("Needs you")).toBeTruthy();
    expect(screen.getAllByText("Done").length).toBeGreaterThan(1);
    expect(screen.getByText("Allow and Decline on an alert ask you to unlock your phone first.")).toBeTruthy();
  });

  it("says push to a closed app is not set up when it is allowed but the relay has no push for the account", async () => {
    (Notifications.getPermissionsAsync as jest.Mock).mockResolvedValue({ granted: true, canAskAgain: true });
    usePushState.setState({ unavailable: true });
    await settings();
    expect(await screen.findByText(/Push to a closed app is not set up for this account yet\./)).toBeTruthy();
    (Notifications.getPermissionsAsync as jest.Mock).mockResolvedValue({ granted: false, canAskAgain: true });
  });

  it("does not say push is on when the relay took the token but cannot send", async () => {
    (Notifications.getPermissionsAsync as jest.Mock).mockResolvedValue({ granted: true, canAskAgain: true });
    usePushState.setState({ registered: true, configured: false });
    await settings();
    expect(await screen.findByText(/Push to a closed app is not set up for this account yet\./)).toBeTruthy();
    expect(screen.queryByText(/Push is on/)).toBeNull();
    (Notifications.getPermissionsAsync as jest.Mock).mockResolvedValue({ granted: false, canAskAgain: true });
  });

  it("says push is on once the relay has this phone", async () => {
    (Notifications.getPermissionsAsync as jest.Mock).mockResolvedValue({ granted: true, canAskAgain: true });
    usePushState.setState({ registered: true });
    await settings();
    expect(await screen.findByText("Push is on. Your agents can reach you when Cua Spaces is closed.")).toBeTruthy();
    (Notifications.getPermissionsAsync as jest.Mock).mockResolvedValue({ granted: false, canAskAgain: true });
  });

  it("the switches change what is asked of the relay, and a sample build can pretend to be offline", async () => {
    await settings();
    await fireEvent(await screen.findByTestId("switch-done"), "valueChange", true);
    expect(usePrefs.getState().notifyDone).toBe(true);
    await fireEvent(screen.getByTestId("switch-offline"), "valueChange", true);
    await waitFor(() => expect(usePrefs.getState().sampleOffline).toBe(true));
    expect(screen.getAllByText("Sample data").length).toBeGreaterThan(1);
  });
});

describe("Settings: the phone's lock", () => {
  afterEach(() => {
    mockSample.capabilities.sample = true;
    usePrefs.setState({ requireUnlock: true });
  });

  it("has a switch that is on by default and turns off, and says so when the phone has no passcode", async () => {
    mockSample.capabilities.sample = false;
    await settings();
    const sw = await screen.findByTestId("switch-lock");
    expect(sw.props.value).toBe(true);
    expect(screen.getByText("Asked when you open Cua Spaces and before you open the desktop of a computer.")).toBeTruthy();
    fireEvent(sw, "valueChange", false);
    await waitFor(() => expect(usePrefs.getState().requireUnlock).toBe(false));
  });

  it("is not in a sample build", async () => {
    await settings();
    expect(screen.queryByTestId("switch-lock")).toBeNull();
  });
});
