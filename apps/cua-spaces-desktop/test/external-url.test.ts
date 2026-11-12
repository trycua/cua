// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { opensOutside } from "../src/external-url";

describe("what the app opens outside itself", () => {
  it("opens web pages and the diagnostic report's email to Cua support", () => {
    for (const url of [
      "https://cua.ai/pricing",
      "http://localhost:8080/x",
      "mailto:support@trycua.com",
      "mailto:support@trycua.com?subject=Spaces%3A%20create%20failed&body=a%0D%0Ab",
    ]) {
      expect(opensOutside(url), url).toBe(true);
    }
  });

  it("keeps everything else closed, including mail to anyone else", () => {
    for (const url of [
      "",
      "file:///etc/passwd",
      "javascript:alert(1)",
      "mailto:other@example.com?subject=x",
      "mailto:support@trycua.com,other@example.com",
      "mailto:support@trycua.com.evil.example",
      "mailto:support@trycua.com?cc=other@example.com",
      "mailto:support@trycua.com?subject=x&bcc=other@example.com",
      "mailto:support@trycua.com?subject=a b",
    ]) {
      expect(opensOutside(url), url).toBe(false);
    }
  });
});
