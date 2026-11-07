// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

const mockRequests: Array<Record<string, unknown>> = [];
const mockExchange = jest.fn();
jest.mock("expo-auth-session", () => ({
  fetchDiscoveryAsync: async () => ({ authorizationEndpoint: "https://auth.test/auth", tokenEndpoint: "https://auth.test/token" }),
  // What expo-auth-session builds for a custom scheme: two slashes, which the provider refuses.
  makeRedirectUri: ({ scheme, path }: { scheme: string; path: string }) => `${scheme}://${path}`,
  AuthRequest: class {
    codeVerifier = "verifier-1";
    constructor(c: Record<string, unknown>) {
      mockRequests.push(c);
    }
    async promptAsync() {
      return { type: "success", params: { code: "code-1" } };
    }
  },
  exchangeCodeAsync: (...a: unknown[]) => mockExchange(...a),
  ResponseType: { Code: "code" },
}));

import { session } from "../src/auth/session";

describe("signing in", () => {
  it("sends the redirect the cua.ai provider has registered for the phone, one slash, in both steps", async () => {
    mockExchange.mockResolvedValue({ accessToken: "a", refreshToken: "r", expiresIn: 3600 });
    await session.signIn();
    expect(mockRequests[0]).toMatchObject({ clientId: "cua-spaces-mobile", redirectUri: "ai.cua.spaces:/oauth/callback", usePKCE: true });
    expect(mockExchange.mock.calls[0]![0]).toMatchObject({ code: "code-1", redirectUri: "ai.cua.spaces:/oauth/callback" });
    expect(await session.load()).toBe(true);
  });
});
