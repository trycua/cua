// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** Where the phone signs in and which relay it uses. Build-time (`EXPO_PUBLIC_*`), defaulting to production. */

const staging = process.env.EXPO_PUBLIC_CUA_ENV === "staging";

export const config = {
  /**
   * A build with made up, labelled sample data and no sign-in (`EXPO_PUBLIC_CUA_SAMPLE=1` when bundling).
   * It is how the screenshots and the end-to-end flow run; a normal build cannot reach it.
   */
  sample: process.env.EXPO_PUBLIC_CUA_SAMPLE === "1",
  /**
   * In a sample build, keep the outbox and the cached threads on disk (sealed) like a real build does
   * (`EXPO_PUBLIC_CUA_SAMPLE_PERSIST=1`). It is how the restart-survives checks run on the Simulator; without it a
   * sample build keeps nothing between launches, so made up data is never mistaken for saved data.
   */
  samplePersist: process.env.EXPO_PUBLIC_CUA_SAMPLE_PERSIST === "1",
  /**
   * A local harness's token endpoint (`EXPO_PUBLIC_CUA_DEV_TOKEN_URL`, from `scripts/mobile-harness/harness.sh env`): in a
   * build made with it, "Sign in" asks that endpoint for a token for `devAccount` instead of opening the sign-in sheet,
   * so the phone can be driven against a local relay and daemon in an isolated home. A release build is made without
   * it and has no way into it (the sample flag works the same way).
   */
  devTokenUrl: process.env.EXPO_PUBLIC_CUA_DEV_TOKEN_URL ?? null,
  devAccount: process.env.EXPO_PUBLIC_CUA_DEV_ACCOUNT ?? "ada",
  /** The OIDC issuer (`cua-auth`'s DEFAULT_OIDC_ISSUER; staging has its own). */
  issuer: process.env.EXPO_PUBLIC_CUA_OIDC_ISSUER ?? (staging ? "https://auth.staging.cua.ai/realms/cyclops-cs" : "https://auth.cua.ai/realms/cyclops-cs"),
  /**
   * A public OAuth client for the phone. `cua-cli` only allows the loopback
   * redirect, so the phone needs its own client with the redirect
   * `ai.cua.spaces:/oauth/callback`; registering it is an account holder's step
   * (docs/SETUP.md). Until then sign-in is refused by the provider and
   * the app says so.
   */
  clientId: process.env.EXPO_PUBLIC_CUA_OIDC_CLIENT_ID ?? "cua-spaces-mobile",
  scope: "openid profile offline_access organization:*",
  relayUrl: process.env.EXPO_PUBLIC_CUA_RELAY_URL ?? (staging ? "https://relay.staging.cua.ai" : "https://relay.cua.ai"),
  redirectScheme: "ai.cua.spaces",
  redirectPath: "oauth/callback",
};
