// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { describeUpdateError } from "../src/update-error";

describe("describeUpdateError", () => {
  it("never returns an empty reason", () => {
    expect(describeUpdateError(new Error(""))).toBe("Error with no message");
    expect(describeUpdateError(undefined)).toBe("Error with no message");
  });

  it("carries the code, HTTP status and URL host, not the path or query", () => {
    const e = Object.assign(new Error(""), {
      code: "ERR_UPDATER_INVALID_RELEASE_FEED",
      statusCode: 404,
      url: "https://github.com/trycua/cua/releases/download/x/app.exe?token=secret",
    });
    expect(describeUpdateError(e)).toBe("Error with no message (code ERR_UPDATER_INVALID_RELEASE_FEED, HTTP 404, host github.com)");
  });

  it("finds the host in the message and keeps the message", () => {
    expect(describeUpdateError(new Error("net::ERR_FAILED https://objects.example.com/a/b?sig=1"))).toBe(
      "net::ERR_FAILED https://objects.example.com/a/b?sig=1 (host objects.example.com)",
    );
  });
});
