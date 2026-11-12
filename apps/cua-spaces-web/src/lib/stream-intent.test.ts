// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { HOVER_MS, intentSpaceId, REPEAT_MS, watchStreamIntent } from "./stream-intent";

function link(href: string, inner = "tile"): HTMLElement {
  const a = document.createElement("a");
  a.setAttribute("href", href);
  a.innerHTML = `<span>${inner}</span>`;
  document.body.append(a);
  return a;
}

describe("intentSpaceId", () => {
  it("reads the Space of a link to its page, from anywhere inside the link", () => {
    const a = link("/spaces/local%3Asfa");
    expect(intentSpaceId(a)).toBe("local:sfa");
    expect(intentSpaceId(a.querySelector("span"))).toBe("local:sfa");
    expect(intentSpaceId(link("/spaces/cloud:abc/"))).toBe("cloud:abc");
  });

  it("ignores every other target", () => {
    expect(intentSpaceId(link("/spaces"))).toBeNull();
    expect(intentSpaceId(link("/spaces/x/agents"))).toBeNull();
    expect(intentSpaceId(link("/keyvault"))).toBeNull();
    expect(intentSpaceId(document.body)).toBeNull();
    expect(intentSpaceId(null)).toBeNull();
    expect(intentSpaceId(link("/spaces/%E0%A4%A"))).toBeNull();
  });
});

describe("watchStreamIntent", () => {
  const post = vi.fn();
  let stop: () => void;
  let t = 0;

  beforeEach(() => {
    vi.useFakeTimers();
    post.mockReset();
    t = 1_000;
    document.body.innerHTML = "";
  });
  afterEach(() => {
    stop?.();
    vi.useRealTimers();
  });

  const watch = (allowed?: () => boolean) => {
    stop = watchStreamIntent(document, { handler: { postMessage: post }, now: () => t, allowed });
  };
  const over = (el: Element) => el.dispatchEvent(new Event("pointerover", { bubbles: true }));
  const press = (el: Element) => el.dispatchEvent(new Event("pointerdown", { bubbles: true }));

  it("announces a Space once the pointer rests on its link", () => {
    watch();
    const a = link("/spaces/local%3Asfa");
    over(a);
    vi.advanceTimersByTime(HOVER_MS - 1);
    expect(post).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(post).toHaveBeenCalledWith({ type: "prewarm", spaceId: "local:sfa" });
  });

  it("does not announce a pointer passing over", () => {
    watch();
    const a = link("/spaces/a");
    const b = link("/spaces/b");
    over(a);
    vi.advanceTimersByTime(HOVER_MS - 10);
    over(b);
    vi.advanceTimersByTime(HOVER_MS - 10);
    over(document.body);
    vi.advanceTimersByTime(HOVER_MS * 2);
    expect(post).not.toHaveBeenCalled();
  });

  it("announces a press at once, and a Space again only after a while", () => {
    watch();
    const a = link("/spaces/a");
    press(a);
    expect(post).toHaveBeenCalledTimes(1);
    press(a);
    over(a);
    vi.advanceTimersByTime(HOVER_MS);
    expect(post).toHaveBeenCalledTimes(1);
    t += REPEAT_MS;
    press(a);
    expect(post).toHaveBeenCalledTimes(2);
  });

  it("says nothing while connecting by itself is off", () => {
    watch(() => false);
    press(link("/spaces/a"));
    expect(post).not.toHaveBeenCalled();
  });

  it("does nothing where the host draws no native video", () => {
    stop = watchStreamIntent(document, { handler: null });
    press(link("/spaces/a"));
    expect(post).not.toHaveBeenCalled();
  });

  it("stops listening", () => {
    watch();
    stop();
    press(link("/spaces/a"));
    expect(post).not.toHaveBeenCalled();
  });
});
