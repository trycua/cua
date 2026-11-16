// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, screen, waitFor } from "@testing-library/react-native";

import { HomeScreen } from "../src/features/home/HomeScreen";
import type { Backend } from "../src/data/types";
import { setClockForTests } from "../src/ui/clock";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { renderWithApp } from "./helpers/render";

jest.mock("expo-router", () => ({ useRouter: () => ({ push: jest.fn() }) }));

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const open = jest.fn();
const home = async (wrap?: (b: Backend) => Backend) => renderWithApp(<HomeScreen onOpen={open} onSettings={() => {}} />, { wrap });

/** Every string on screen: text and accessibility labels. */
function textOf(n: unknown): string {
  if (n == null) return "";
  if (typeof n === "string") return n;
  if (Array.isArray(n)) return n.map(textOf).join(" ");
  const o = n as { props?: Record<string, unknown>; children?: unknown };
  return `${typeof o.props?.accessibilityLabel === "string" ? o.props.accessibilityLabel : ""} ${textOf(o.children)}`;
}

describe("Home", () => {
  it("labels sample data, draws the stories bar and one status per row, Needs you first", async () => {
    await home();
    expect(await screen.findByText("Sample data. Not your account.")).toBeTruthy();
    expect(screen.getByLabelText("Spaces")).toBeTruthy();
    expect(screen.getAllByLabelText("Live desktop").length).toBeGreaterThan(0);
    expect((await screen.findAllByText("Needs you")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("Running").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Asleep").length).toBeGreaterThan(0);
  });

  it("opens a Space when a row is pressed", async () => {
    open.mockClear();
    await home();
    fireEvent.press((await screen.findAllByLabelText(/^Claude Code, in checkout-api/))[0]!);
    expect(open).toHaveBeenCalledWith("relay:sample-checkout-api");
  });

  it("lists a Space that has no agent yet, so every Space can be opened", async () => {
    await home((b) => ({
      ...b,
      listSpaces: async () => {
        const l = await b.listSpaces();
        return { ...l, agents: l.agents.filter((r) => !r.places.some((p) => p.spaceId === "relay:sample-ledger")) };
      },
    }));
    expect((await screen.findAllByLabelText(/^Ledger, on Desktop PC/)).length).toBeGreaterThan(0);
  });

  it("shows a quiet offline line, in the core's words, over what was last loaded and says when", async () => {
    // A fixed clock: the core words the line from what it is told, so the same times give the same answer.
    setClockForTests(() => 1_800_000_000_000);
    let fail = false;
    await home((b) => ({
      ...b,
      listSpaces: async () => {
        if (fail) throw new Error("no route to host");
        return b.listSpaces();
      },
    }));
    await screen.findByText("Sample data. Not your account.");
    await screen.findAllByText("Needs you");
    fail = true;
    screen.getByTestId("home-list").props.refreshControl.props.onRefresh();
    expect(await screen.findByText("You're offline. Showing what was saved just now.")).toBeTruthy();
    expect(screen.getAllByText("Needs you").length).toBeGreaterThan(0);
    setClockForTests(null);
  });

  it("starts a task from the compose button", async () => {
    const push = jest.fn();
    open.mockClear();
    await renderWithApp(<HomeScreen onOpen={open} onSettings={() => {}} onNewTask={push} />);
    await fireEvent.press(await screen.findByLabelText("Start a task"));
    expect(push).toHaveBeenCalled();
  });

  it("says nothing about devices or machines, and uses no middle dot", async () => {
    await home();
    await screen.findByText("Sample data. Not your account.");
    const tree = textOf(screen.toJSON());
    expect(tree).not.toMatch(/devices?|machines?/i);
    expect(tree).not.toMatch(/·/);
  });

  it("filters by search and by machine, and says when nothing matches", async () => {
    await home();
    await screen.findByText("Sample data. Not your account.");
    fireEvent.changeText(screen.getByLabelText("Search Spaces, apps or agents"), "zzz-none");
    expect(await screen.findByText("Nothing matches")).toBeTruthy();
    fireEvent.changeText(screen.getByLabelText("Search Spaces, apps or agents"), "");
    fireEvent.press(screen.getByLabelText(/^Desktop PC, online/));
    await waitFor(() => expect(screen.queryAllByLabelText(/in checkout-api/).length).toBe(0));
    expect(screen.getAllByLabelText(/Ledger|Desktop PC/).length).toBeGreaterThan(0);
  });
});
