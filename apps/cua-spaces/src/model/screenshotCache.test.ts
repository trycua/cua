// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, describe, expect, it, vi } from "vitest";

import {
  clearScreenshotCache,
  getRecentScreenshot,
  getScreenshot,
  rememberScreenshot,
  subscribeScreenshots,
} from "./screenshotCache";

afterEach(() => clearScreenshotCache());

const URL_A = "data:image/png;base64,AAAA";
const URL_B = "data:image/png;base64,BBBB";

describe("screenshot cache", () => {
  it("stores and reads the latest frame per Space", () => {
    rememberScreenshot("space-1", URL_A, 1000);
    expect(getScreenshot("space-1")).toEqual({ dataUrl: URL_A, ts: 1000 });
    expect(getScreenshot("space-2")).toBeNull();
  });

  it("keeps the newest frame and ignores out-of-order older writes", () => {
    rememberScreenshot("space-1", URL_A, 2000);
    rememberScreenshot("space-1", URL_B, 1000); // older: ignored
    expect(getScreenshot("space-1")?.dataUrl).toBe(URL_A);
    rememberScreenshot("space-1", URL_B, 3000); // newer: wins
    expect(getScreenshot("space-1")?.dataUrl).toBe(URL_B);
  });

  it("ignores empty ids and urls", () => {
    rememberScreenshot("", URL_A, 1000);
    rememberScreenshot("space-1", "", 1000);
    expect(getScreenshot("space-1")).toBeNull();
  });

  it("only returns a recent frame within the max age", () => {
    rememberScreenshot("space-1", URL_A, 1000);
    expect(getRecentScreenshot("space-1", 1500, 1000)).not.toBeNull();
    expect(getRecentScreenshot("space-1", 2500, 1000)).toBeNull();
  });

  it("notifies subscribers on write and clear", () => {
    const fn = vi.fn();
    const stop = subscribeScreenshots(fn);
    rememberScreenshot("space-1", URL_A, 1000);
    expect(fn).toHaveBeenCalledTimes(1);
    clearScreenshotCache();
    expect(fn).toHaveBeenCalledTimes(2);
    stop();
    rememberScreenshot("space-1", URL_A, 2000);
    expect(fn).toHaveBeenCalledTimes(2);
  });
});
