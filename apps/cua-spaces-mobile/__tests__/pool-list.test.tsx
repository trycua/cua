// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { screen } from "@testing-library/react-native";

import { HomeScreen } from "../src/features/home/HomeScreen";
import { createLiveBackend } from "../src/data/live/backend";
import { RelayClient } from "../src/data/live/relay";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { fakeRelay } from "./helpers/fakeDaemon";
import { renderWithApp } from "./helpers/render";

jest.mock("expo-router", () => ({ useRouter: () => ({ push: jest.fn() }) }));

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const row = (id: string, name: string, extra: Record<string, unknown> = {}) => ({
  id,
  name,
  owner: { id: "acct:you" },
  role: "owner",
  online: true,
  sharing: true,
  version: "1",
  url: "",
  meta: {},
  ...extra,
});

/** A pool the way the relay reports it: a Mac that hosts Spaces, a plain computer that hosts none, one Space, and the phone. */
const machines = [
  row("studio", "Studio Mac", { meta: { "cua.host.spaces": "on", "cua.host.phone": "1", "cua.host.os": "macos" } }),
  row("plain-pc", "Plain PC", { meta: { "cua.host.os": "windows" } }),
  row("space-aaaa1111", "checkout-api", { host: "studio", meta: { "cua.host.os": "macos" } }),
];
const devices = [
  { id: "dev_0123456789abcdef01234567", name: "Dillon's iPhone", platform: "ios", phone: true, state: "enrolled", current: true, last_seen: 1_799_999_000 },
];

function live() {
  const fake = fakeRelay({
    machines,
    tools: (name) =>
      name === "thread_read"
        ? { entries: [], cursor: 0, unread: 0, me: { id: "acct:you" }, role: "owner", live: { key: "", agents: [] } }
        : name === "agent_members"
          ? { members: [] }
          : {},
    rest: (method, path) => (method === "GET" && path === "/v1/devices" ? { body: { devices } } : undefined),
  });
  return createLiveBackend({
    relay: new RelayClient("https://relay.test", fake.fetcher as never),
    token: async () => "acct-token",
    deviceSession: async () => "dev-session",
    me: () => "acct:you",
    deviceId: () => "dev_0123456789abcdef01234567",
    fetcher: fake.fetcher as never,
    now: () => 1_800_000_000_000,
  });
}

describe("the one list: Spaces and computers from the same pool the desktop reads", () => {
  it("never makes a Space of a computer, and shows the phone as a phone under its real name", async () => {
    const backend = live();
    const list = await backend.listSpaces();
    expect(list.spaces.map((s) => s.name)).toEqual(["checkout-api"]);
    expect(list.computers!.map((c) => c.name).sort()).toEqual(["Dillon's iPhone", "Plain PC", "Studio Mac"]);
    expect(list.computers!.some((c) => /^this (phone|device)$/i.test(c.name))).toBe(false);
    expect(list.computers!.find((c) => c.name === "Dillon's iPhone")).toMatchObject({ kind: "iphone", current: true });
  });

  it("renders the Home screen with the computers in their own section and only Spaces as Space rows", async () => {
    await renderWithApp(<HomeScreen onOpen={() => {}} onSettings={() => {}} />, { wrap: () => live() });
    const section = await screen.findByTestId("computers");
    expect(section).toBeTruthy();
    expect(screen.getByTestId("computer-plain-pc")).toBeTruthy();
    expect(screen.getByTestId("computer-studio")).toBeTruthy();
    expect(screen.getByLabelText(/^Dillon's iPhone, this phone/)).toBeTruthy();
    // A Space row is opened by its Space id; the plain computer has none and never gets a row.
    expect(await screen.findAllByLabelText(/^checkout-api, on Studio Mac/)).not.toHaveLength(0);
    expect(screen.queryAllByLabelText(/^Plain PC, on /)).toHaveLength(0);
    expect(screen.queryAllByLabelText(/^Studio Mac, on /)).toHaveLength(0);
  });
});
