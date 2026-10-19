// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { FileSendBridge, SentFile } from "../native/fileSend";
import type { WindowDragBridge, WindowDragEvent } from "../native/windowDrag";
import TeleportDropZone from "./TeleportDropZone";

/** Captured handlers for the two native drag sources the zone listens to. */
let dropHandler: ((event: { payload: DragDropPayload }) => void) | null = null;
type DragDropPayload =
  | { type: "over"; position: { x: number; y: number } }
  | { type: "drop"; position: { x: number; y: number }; paths: string[] }
  | { type: "leave" };

const unlistenDrop = vi.fn();
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async (handler: (event: { payload: DragDropPayload }) => void) => {
      dropHandler = handler;
      return unlistenDrop;
    },
  }),
}));

vi.mock("@tauri-apps/api/window", () => ({
  // The pop-out's top-left sits at (100, 50) logical points, so a screen point
  // maps into client space by subtracting that.
  getCurrentWindow: () => ({
    innerPosition: async () => ({ x: 200, y: 100 }),
    scaleFactor: async () => 2,
  }),
}));

const landed: SentFile = {
  name: "notes.pdf",
  dest: "/root/Downloads/notes.pdf",
  bytes: 2_200_000,
  sha256: "abc123def456",
};

function fakeFileSend(overrides: Partial<FileSendBridge> = {}): FileSendBridge {
  return {
    isNative: true,
    pickFiles: vi.fn(async () => ["/Users/me/notes.pdf"]),
    sendFiles: vi.fn(async () => [landed]),
    ...overrides,
  };
}

let dragHandler: ((event: WindowDragEvent) => void) | null = null;
function fakeWindowDrag(): WindowDragBridge {
  return {
    isNative: true,
    axTrusted: async () => true,
    requestAxTrust: async () => true,
    startWindowDrag: async () => true,
    setForeignWindowHidden: async () => false,
    dragDisplays: async () => [],
    onWindowDrag: async (handler) => {
      dragHandler = handler;
      return () => {};
    },
    onPermission: async () => () => {},
  };
}

/** jsdom gives every element a zero rect, so the zone's box is stubbed. */
function stubZoneBox(box = { left: 20, top: 200, right: 280, bottom: 320 }) {
  const zone = document.querySelector<HTMLElement>(".sl-dropzone");
  expect(zone).not.toBeNull();
  zone!.getBoundingClientRect = () =>
    ({
      ...box,
      width: box.right - box.left,
      height: box.bottom - box.top,
      x: box.left,
      y: box.top,
      toJSON: () => "",
    }) as DOMRect;
  return zone!;
}

function renderZone(props: Partial<Parameters<typeof TeleportDropZone>[0]> = {}) {
  const onTeleportApp = vi.fn();
  const fileSend = props.fileSend ?? fakeFileSend();
  const result = render(
    <TeleportDropZone
      spaceId="cloud:aurora"
      spaceName="Aurora"
      fileSend={fileSend}
      onTeleportApp={onTeleportApp}
      {...props}
    />,
  );
  return { ...result, onTeleportApp, fileSend };
}

/** Deliver a native drag event the way the webview would, inside act() so the
 * resulting state update is flushed before the assertion. */
function fireDrop(payload: DragDropPayload) {
  act(() => dropHandler!({ payload }));
}
function fireDrag(event: WindowDragEvent) {
  act(() => dragHandler!(event));
}

beforeEach(() => {
  dropHandler = null;
  dragHandler = null;
  vi.clearAllMocks();
});

describe("the Teleport drop zone", () => {
  it("offers the file and app actions the user asked for", () => {
    renderZone();
    expect(screen.getByRole("button", { name: "Send file…" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Teleport an app…" })).toBeInTheDocument();
    // The caption must name the target Space and both things that can be
    // dropped. It deliberately no longer spells out "Downloads": the second
    // hint line that did was removed as redundant clutter, and the outcome line
    // reports the real landing path once a send completes, which is the honest
    // place for it.
    expect(screen.getByText("Drop a file or window")).toBeInTheDocument();
    // The section names the target for assistive tech; the caption stays short.
    expect(screen.getByRole("region", { name: "Teleport to Aurora" })).toBeInTheDocument();
    // No label of its own: the page's section title names it once.
    expect(screen.queryByText("Teleport")).toBeNull();
  });

  it("sends a chosen file and reports where it landed, not just that it sent", async () => {
    const user = userEvent.setup();
    const { fileSend } = renderZone();
    await user.click(screen.getByRole("button", { name: "Send file…" }));
    await waitFor(() => expect(fileSend.sendFiles).toHaveBeenCalled());
    expect(fileSend.sendFiles).toHaveBeenCalledWith("cloud:aurora", ["/Users/me/notes.pdf"]);
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("/root/Downloads/notes.pdf");
    expect(status).toHaveTextContent("2.1 MB");
  });

  it("shows a failed transfer as failed and never claims it was sent", async () => {
    const user = userEvent.setup();
    const fileSend = fakeFileSend({
      sendFiles: vi.fn(async () => {
        throw new Error("notes.pdf did not land intact in the Space");
      }),
    });
    renderZone({ fileSend });
    await user.click(screen.getByRole("button", { name: "Send file…" }));
    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent("did not land intact");
    expect(status.dataset.kind).toBe("failed");
    expect(status.textContent).not.toMatch(/verified/);
  });

  it("uses the existing app-teleport path for Teleport an app…", async () => {
    const user = userEvent.setup();
    const { onTeleportApp, fileSend } = renderZone();
    await user.click(screen.getByRole("button", { name: "Teleport an app…" }));
    expect(onTeleportApp).toHaveBeenCalledTimes(1);
    // An app teleport is the teleport flow, never a file copy.
    expect(fileSend.sendFiles).not.toHaveBeenCalled();
  });

  it("sends a file dropped on the zone, and ignores one dropped elsewhere", async () => {
    const { fileSend } = renderZone();
    await waitFor(() => expect(dropHandler).not.toBeNull());
    stubZoneBox();

    // devicePixelRatio 1 in jsdom, so physical == logical here.
    fireDrop({ type: "drop", position: { x: 500, y: 500 }, paths: ["/tmp/away.txt"] });
    expect(fileSend.sendFiles).not.toHaveBeenCalled();

    fireDrop({ type: "drop", position: { x: 120, y: 260 }, paths: ["/tmp/here.txt"] });
    await waitFor(() =>
      expect(fileSend.sendFiles).toHaveBeenCalledWith("cloud:aurora", ["/tmp/here.txt"]),
    );
  });

  it("highlights itself as a drop target while a file is dragged over it", async () => {
    renderZone();
    await waitFor(() => expect(dropHandler).not.toBeNull());
    const zone = stubZoneBox();

    fireDrop({ type: "over", position: { x: 120, y: 260 } });
    await waitFor(() => expect(zone.dataset.dropTarget).toBe("true"));

    fireDrop({ type: "over", position: { x: 900, y: 900 } });
    await waitFor(() => expect(zone.dataset.dropTarget).toBeUndefined());

    fireDrop({ type: "over", position: { x: 120, y: 260 } });
    await waitFor(() => expect(zone.dataset.dropTarget).toBe("true"));
    fireDrop({ type: "leave" });
    await waitFor(() => expect(zone.dataset.dropTarget).toBeUndefined());
  });

  it("teleports the app of a WINDOW dragged into it, the way a notch tile does", async () => {
    const { onTeleportApp, fileSend } = renderZone({ windowDrag: fakeWindowDrag() });
    await waitFor(() => expect(dragHandler).not.toBeNull());
    const zone = stubZoneBox();

    // Window origin is (200,100) physical ÷ 2 = (100,50) logical, so a screen
    // point of (220, 310) is client (120, 260) — inside the zone.
    fireDrag({ phase: "start", x: 220, y: 310, windowId: 7, appId: "firefox", supported: true });
    await waitFor(() => expect(zone.dataset.dropTarget).toBe("true"));
    fireDrag({ phase: "end", x: 220, y: 310, windowId: 7, appId: "firefox", supported: true });

    await waitFor(() => expect(onTeleportApp).toHaveBeenCalledTimes(1));
    expect(zone.dataset.dropTarget).toBeUndefined();
    // A dropped window is a session teleport, not a file copy.
    expect(fileSend.sendFiles).not.toHaveBeenCalled();
  });

  it("ignores a window released outside the zone", async () => {
    const { onTeleportApp } = renderZone({ windowDrag: fakeWindowDrag() });
    await waitFor(() => expect(dragHandler).not.toBeNull());
    stubZoneBox();
    fireDrag({ phase: "start", x: 220, y: 310 });
    fireDrag({ phase: "end", x: 900, y: 900 });
    await waitFor(() => expect(onTeleportApp).not.toHaveBeenCalled());
  });
});
