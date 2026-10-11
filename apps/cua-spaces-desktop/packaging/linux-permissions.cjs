// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Linux install tree's modes. The .deb installs `/opt/Cua Spaces` as fpm
// packs it: owned by root, with the modes the build machine left on the
// unpacked app, which a umask of 002 makes group-writable (775). The
// Keyvault trusts the bundled `cua` only when it and every folder above it
// are root's and writable by root alone (cua-keyvault `root_protected`), so
// such a tree turns the Keyvault off. `afterPack` gives every folder 755 and
// every file 755 or 644 (by its execute bit) before a package is made.
const fs = require("node:fs");
const path = require("node:path");

/** The mode a packed entry gets: owner-writable only, its execute bit kept. */
function packedMode(mode, isDirectory) {
  if (isDirectory) return 0o755;
  return mode & 0o111 ? 0o755 : 0o644;
}

/** Sets the packed modes under `root` (symlinks are left as they are). */
function normalizeModes(root) {
  const visit = (p) => {
    const st = fs.lstatSync(p);
    if (st.isSymbolicLink()) return;
    const mode = packedMode(st.mode, st.isDirectory());
    if ((st.mode & 0o7777) !== mode) fs.chmodSync(p, mode);
    if (st.isDirectory()) for (const name of fs.readdirSync(p)) visit(path.join(p, name));
  };
  visit(root);
}

module.exports = { normalizeModes, packedMode };
