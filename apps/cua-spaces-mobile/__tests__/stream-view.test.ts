// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { allowedNavigation, originOf } from "../src/features/stream/StreamView";

describe("what the live-desktop page may load", () => {
  const origin = originOf("https://relay.cua.ai/m/space-aaaa1111/viewer/#ticket=t");

  it("is the relay it came from, over https, under the viewer's path", () => {
    expect(origin).toBe("https://relay.cua.ai");
    expect(allowedNavigation(origin, "https://relay.cua.ai/m/space-aaaa1111/viewer/")).toBe(true);
    expect(allowedNavigation(origin, "https://relay.cua.ai/m/space-aaaa1111/viewer/app.js")).toBe(true);
    expect(allowedNavigation(origin, "about:blank")).toBe(true);
  });

  it("is nothing else: not another site, not plain http, not another path of the relay", () => {
    for (const url of [
      "https://evil.example/m/space-aaaa1111/viewer/",
      "http://relay.cua.ai/m/space-aaaa1111/viewer/",
      "https://relay.cua.ai/v1/devices",
      "https://relay.cua.ai/m/space-aaaa1111/files?path=/etc/passwd",
      "https://relay.cua.ai.evil.example/m/x/viewer/",
      "javascript:alert(1)",
      "data:text/html,hi",
      "file:///etc/passwd",
    ]) {
      expect(allowedNavigation(origin, url)).toBe(false);
    }
    expect(allowedNavigation(null, "https://relay.cua.ai/m/x/viewer/")).toBe(false);
  });
});
