// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
//
// Wraps app.json for release builds. scripts/testflight.sh sets
// CUA_IOS_RELEASE=1 (production push entitlement) and CUA_BUILD_NUMBER
// (CFBundleVersion, increases with every upload).
module.exports = ({ config }) => {
  const release = process.env.CUA_IOS_RELEASE === "1";
  const buildNumber = process.env.CUA_BUILD_NUMBER;
  return {
    ...config,
    ios: {
      ...config.ios,
      ...(buildNumber ? { buildNumber } : {}),
      entitlements: {
        ...(config.ios && config.ios.entitlements),
        "aps-environment": release ? "production" : "development",
      },
    },
    plugins: (config.plugins || []).map((p) =>
      Array.isArray(p) && p[0] === "expo-notifications" ? ["expo-notifications", { ...p[1], mode: release ? "production" : "development" }] : p,
    ),
  };
};
