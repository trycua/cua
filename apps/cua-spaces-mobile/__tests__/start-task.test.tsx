// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, fireEvent, screen, waitFor } from "@testing-library/react-native";

import { StartTaskSheet } from "../src/features/task/StartTaskSheet";
import { installFixtureCore, uninstallCore } from "./helpers/core";
import { renderWithApp } from "./helpers/render";

beforeAll(installFixtureCore);
afterAll(uninstallCore);

const sheet = (props: { onClose?: () => void; onStarted?: (id: string) => void } = {}, opts: Parameters<typeof renderWithApp>[1] = {}) =>
  renderWithApp(<StartTaskSheet onClose={props.onClose ?? (() => {})} onStarted={props.onStarted ?? (() => {})} newId={() => "task-0001"} />, opts);

describe("starting a task from the phone", () => {
  it("shows the core's question, the agent and where it runs (Let Cua decide first), and what cannot start says why", async () => {
    await sheet();
    expect(await screen.findByText("What do you want to do?")).toBeTruthy();
    expect(screen.getByLabelText("Describe the job")).toBeTruthy();
    expect(screen.getByTestId("task-agent").props.accessibilityLabel).toBe("Claude Code");
    expect(screen.getByTestId("task-where").props.accessibilityLabel).toBe("Let Cua decide");
    expect(screen.getByTestId("task-repo").props.accessibilityLabel).toBe("Add a repo");
    await fireEvent.press(screen.getByTestId("task-agent"));
    expect(screen.getByLabelText("Codex, Sign in to Codex in Settings, Connections").props.accessibilityState.disabled).toBe(true);
    await fireEvent.press(screen.getByTestId("task-where"));
    expect(screen.getByLabelText("Old MacBook, Offline").props.accessibilityState.disabled).toBe(true);
    expect(screen.getByLabelText("Linked cloud, Budget reached this month").props.accessibilityState.disabled).toBe(true);
    expect(screen.getByLabelText("Studio Mac, New Space").props.accessibilityState.disabled).toBe(false);
    expect(screen.getByLabelText("In checkout-api, Message the agent there")).toBeTruthy();
  });

  it("starts a task on a machine with a repo: the job is the first message, and the daemon is asked once with the core's request", async () => {
    const started = jest.fn();
    const { demo } = await sheet({ onStarted: started });
    await screen.findByText("What do you want to do?");
    expect(screen.getByTestId("task-start").props.accessibilityState.disabled).toBe(true);
    await fireEvent.changeText(screen.getByTestId("task-input"), "Fix the flaky websocket test");
    await fireEvent.press(screen.getByTestId("task-where"));
    await fireEvent.press(screen.getByLabelText("Desktop PC, New Space"));
    await fireEvent.press(screen.getByTestId("task-repo"));
    await fireEvent.changeText(screen.getByTestId("task-repo-input"), "https://github.com/acme/checkout-api.git");
    expect(screen.getByTestId("task-repo").props.accessibilityLabel).toBe("acme/checkout-api");
    await fireEvent.press(screen.getByTestId("task-start"));
    await waitFor(() => expect(started).toHaveBeenCalledWith("relay:sample-task-1"));
    expect(demo.record.started).toHaveLength(1);
    expect(demo.record.started[0]).toMatchObject({
      agent: "claude-code",
      prompt: "Fix the flaky websocket test",
      message: "@claude Fix the flaky websocket test",
      where: { kind: "machine", machine: "desktop-pc" },
      os: "linux",
      repo: { url: "https://github.com/acme/checkout-api" },
    });
    // The new Space is in the list and the job is its first message.
    const list = await demo.listSpaces();
    expect(list.spaces.some((s) => s.id === "relay:sample-task-1")).toBe(true);
    const page = await demo.readThread("relay:sample-task-1", { after: 0 });
    // The machine adds the Repo line to the job it posts.
    expect(page.entries[0]).toMatchObject({ text: "@claude Fix the flaky websocket test\n\nRepo: https://github.com/acme/checkout-api" });
  });

  it("messages the agent in a Space that exists, with the handle it already has there", async () => {
    const started = jest.fn();
    const { demo } = await sheet({ onStarted: started });
    await screen.findByText("What do you want to do?");
    await fireEvent.changeText(screen.getByTestId("task-input"), "Status?");
    await fireEvent.press(screen.getByTestId("task-where"));
    await fireEvent.press(screen.getByLabelText("In checkout-api, Message the agent there"));
    await fireEvent.press(screen.getByTestId("task-start"));
    await waitFor(() => expect(started).toHaveBeenCalledWith("relay:sample-checkout-api"));
    expect(demo.record.started[0]).toMatchObject({ message: "@claude Status?", where: { kind: "space", space: "relay:sample-checkout-api" } });
  });

  it("says what is wrong with a repository that cannot be read, and does not start", async () => {
    const { demo } = await sheet();
    await screen.findByText("What do you want to do?");
    await fireEvent.changeText(screen.getByTestId("task-input"), "Do it");
    await fireEvent.press(screen.getByTestId("task-repo"));
    await fireEvent.changeText(screen.getByTestId("task-repo-input"), "not a repo");
    expect(screen.getByText("That doesn't look like a repository. Use owner/name or a GitHub link.")).toBeTruthy();
    await fireEvent.press(screen.getByTestId("task-start"));
    expect(await screen.findByTestId("task-note")).toBeTruthy();
    expect(demo.record.started).toHaveLength(0);
  });

  it("cannot be started twice: a second press is one task", async () => {
    const { demo } = await sheet();
    await screen.findByText("What do you want to do?");
    await fireEvent.changeText(screen.getByTestId("task-input"), "Once only");
    await fireEvent.press(screen.getByTestId("task-start"));
    await fireEvent.press(screen.getByTestId("task-start"));
    await waitFor(() => expect(demo.record.started).toHaveLength(1));
  });

  it("offline, the job is kept: the sheet closes with it waiting, and it starts once when the machines are back", async () => {
    const close = jest.fn();
    const started = jest.fn();
    const { demo } = await sheet({ onClose: close, onStarted: started });
    await screen.findByText("What do you want to do?");
    await fireEvent.changeText(screen.getByTestId("task-input"), "While offline");
    await act(async () => demo.sample!.setOffline(true));
    await fireEvent.press(screen.getByTestId("task-start"));
    await waitFor(() => expect(close).toHaveBeenCalled(), { timeout: 8_000 });
    expect(started).not.toHaveBeenCalled();
    expect(demo.record.started).toHaveLength(0);
    await act(async () => demo.sample!.setOffline(false));
  }, 15_000);

  it("follows a Space that is being made, and opens it when it is ready", async () => {
    const started = jest.fn();
    const { demo } = await sheet({ onStarted: started }, { createMs: 700 });
    await screen.findByText("What do you want to do?");
    await fireEvent.changeText(screen.getByTestId("task-input"), "Takes a moment");
    await fireEvent.press(screen.getByTestId("task-start"));
    expect(await screen.findByText("Starting your task")).toBeTruthy();
    expect(screen.getByText("A computer is making a Space and handing it the job. You can leave: it keeps going.")).toBeTruthy();
    expect(screen.getByLabelText("Close")).toBeTruthy();
    await waitFor(() => expect(started).toHaveBeenCalledWith("relay:sample-task-1"), { timeout: 8_000 });
    expect(demo.record.started).toHaveLength(1);
  }, 15_000);

  it("says why when the machines cannot be asked, and tries again", async () => {
    const { demo } = await sheet({}, { offline: true });
    expect(await screen.findByText("Network request failed")).toBeTruthy();
    await act(async () => demo.sample!.setOffline(false));
    await fireEvent.press(screen.getByTestId("task-options-retry"));
    expect(await screen.findByText("What do you want to do?")).toBeTruthy();
  });
});
