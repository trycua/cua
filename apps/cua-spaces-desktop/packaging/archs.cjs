// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Which architectures a package is made for. A release packs every arch, and
// `afterPack` refuses one without its native layer (`pnpm native -- --target
// <triple>`). A local build that has only some native folders sets
// CUA_SPACES_ARCHS: `native` packs each arch whose `native/<platform>-<arch>`
// exists (a mac universal build needs both darwin folders), a comma list
// (`x64`, `arm64,x64`) packs those.
const fs = require("node:fs");
const path = require("node:path");

/**
 * `all` narrowed by `setting` (CUA_SPACES_ARCHS) for `platform`. When nothing
 * is left, `all` stays, so the build stops at the missing native layer and
 * says which one.
 */
function packageArchs(platform, all, setting, exists = (dir) => fs.existsSync(path.join(__dirname, "..", "native", dir))) {
  const value = (setting ?? "").trim();
  if (!value) return all;
  const has = (arch) => exists(`${platform}-${arch}`);
  const wanted =
    value === "native"
      ? all.filter((arch) => (arch === "universal" ? has("arm64") && has("x64") : has(arch)))
      : all.filter((arch) => value.split(",").map((a) => a.trim()).includes(arch));
  return wanted.length ? wanted : all;
}

module.exports = { packageArchs };
