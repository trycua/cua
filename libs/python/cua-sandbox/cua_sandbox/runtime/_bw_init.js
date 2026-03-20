#!/usr/bin/env node
/**
 * Non-interactive bubblewrap project initialiser.
 *
 * Usage:
 *   node _bw_init.js <manifest_url> <output_dir> <package_id>
 *
 * Generates twa-manifest.json in <output_dir> using @bubblewrap/core directly,
 * bypassing the interactive CLI.  A debug signing key (android.keystore) is
 * created in <output_dir> if one doesn't already exist.
 */

const path = require('path');
const fs = require('fs');

// Locate @bubblewrap/cli in the global npm prefix, then find @bubblewrap/core
// bundled inside it (bubblewrap vendors its own copy of core).
const { execSync } = require('child_process');
const npmPrefix = execSync('npm root -g').toString().trim();
const BW_CLI = path.join(npmPrefix, '@bubblewrap', 'cli');
const corePath = path.join(BW_CLI, 'node_modules', '@bubblewrap', 'core');
const { TwaManifest } = require(path.join(corePath, 'dist/lib/TwaManifest'));

const [, , manifestUrl, outputDir, packageId] = process.argv;
if (!manifestUrl || !outputDir || !packageId) {
  console.error('Usage: node _bw_init.js <manifest_url> <output_dir> <package_id>');
  process.exit(1);
}

const KEYSTORE = path.join(outputDir, 'android.keystore');
const KEYSTORE_ALIAS = 'android';
const KEYSTORE_PASS = 'android';

(async () => {
  try {
    console.log(`Fetching web manifest: ${manifestUrl}`);
    const twa = await TwaManifest.fromWebManifest(manifestUrl);

    // Override package ID and signing key with debug defaults.
    twa.packageId = packageId;
    twa.signingKey = {
      path: KEYSTORE,
      alias: KEYSTORE_ALIAS,
    };

    const outFile = path.join(outputDir, 'twa-manifest.json');
    const json = twa.toJson();
    // Embed keystore passwords so bubblewrap build doesn't prompt.
    json.signingKey = {
      path: KEYSTORE,
      alias: KEYSTORE_ALIAS,
      keypassword: KEYSTORE_PASS,
      password: KEYSTORE_PASS,
    };
    fs.writeFileSync(outFile, JSON.stringify(json, null, 2));
    console.log(`twa-manifest.json written to ${outFile}`);
  } catch (e) {
    console.error('Error:', e.message || e);
    process.exit(1);
  }
})();
