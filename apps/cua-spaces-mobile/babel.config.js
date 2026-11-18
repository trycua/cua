// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

module.exports = function (api) {
  api.cache(true);
  return { presets: ["babel-preset-expo"] };
};
