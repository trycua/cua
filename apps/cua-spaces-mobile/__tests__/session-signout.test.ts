// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

const mockRevoke = jest.fn();
const mockDiscovery = jest.fn();
jest.mock("expo-auth-session", () => ({
  fetchDiscoveryAsync: (...a: unknown[]) => mockDiscovery(...a),
  revokeAsync: (...a: unknown[]) => mockRevoke(...a),
  TokenTypeHint: { RefreshToken: "refresh_token" },
  makeRedirectUri: () => "app://cb",
  ResponseType: { Code: "code" },
}));

import { session } from "../src/auth/session";

type Saver = { save(t: { accessToken: string; refreshToken: string | null; expiresAt: number } | null): Promise<void> };

describe("signing out", () => {
  beforeEach(() => {
    mockRevoke.mockReset();
    mockDiscovery.mockReset();
    mockDiscovery.mockResolvedValue({ revocationEndpoint: "https://auth.test/revoke" });
  });

  it("tells the sign-in provider the refresh token is dead, and drops the tokens from the phone", async () => {
    await (session as unknown as Saver).save({ accessToken: "a", refreshToken: "refresh-1", expiresAt: 9_999_999_999 });
    mockRevoke.mockResolvedValue(true);
    await session.signOut();
    expect(mockRevoke).toHaveBeenCalledTimes(1);
    expect(mockRevoke.mock.calls[0]![0]).toMatchObject({ token: "refresh-1", tokenTypeHint: "refresh_token" });
    expect(await session.load()).toBe(false);
  });

  it("still signs out when the provider cannot be reached", async () => {
    await (session as unknown as Saver).save({ accessToken: "a", refreshToken: "refresh-2", expiresAt: 9_999_999_999 });
    mockDiscovery.mockRejectedValue(new Error("offline"));
    await expect(session.signOut()).resolves.toBeUndefined();
    expect(mockRevoke).not.toHaveBeenCalled();
    expect(session.account()).toBeNull();
  });
});
