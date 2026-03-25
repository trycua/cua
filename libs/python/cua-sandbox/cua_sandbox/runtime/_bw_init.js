#!/usr/bin/env node
/**
 * Non-interactive bubblewrap project initialiser.
 *
 * Usage:
 *   node _bw_init.js <manifest_url> <output_dir> <package_id> \
 *                    [keystore_path [keystore_alias [keystore_password [fetch_url]]]]
 *
 * manifest_url: The TWA origin URL (used in twa-manifest.json host field).
 * fetch_url:    Optional URL to fetch the Web App Manifest from (defaults to
 *               manifest_url).  Use this when manifest_url is an
 *               emulator-internal address like http://10.0.2.2:3000 that is
 *               not reachable from the host — pass the host-accessible URL
 *               (e.g. http://127.0.0.1:3000) as fetch_url while keeping
 *               manifest_url as the in-emulator origin for assetlinks.json.
 */

const path = require('path');
const fs = require('fs');
const { execSync } = require('child_process');

const npmPrefix = execSync('npm root -g').toString().trim();
const BW_CLI = path.join(npmPrefix, '@bubblewrap', 'cli');
const corePath = path.join(BW_CLI, 'node_modules', '@bubblewrap', 'core');
const { TwaManifest } = require(path.join(corePath, 'dist/lib/TwaManifest'));

const [
  ,
  ,
  manifestUrl,
  outputDir,
  packageId,
  keystorePath,
  keystoreAlias,
  keystorePassword,
  fetchUrl,
] = process.argv;

if (!manifestUrl || !outputDir || !packageId) {
  console.error(
    'Usage: node _bw_init.js <manifest_url> <output_dir> <package_id> [keystore_path [alias [password [fetch_url]]]]'
  );
  process.exit(1);
}

const KEYSTORE = keystorePath || path.join(outputDir, 'android.keystore');
const KEYSTORE_ALIAS = keystoreAlias || 'android';
const KEYSTORE_PASS = keystorePassword || 'android';

// If the manifest URL uses an emulator-internal host (10.0.2.2), replace it
// with 127.0.0.1 for fetching from the host machine.
const resolvedFetchUrl =
  fetchUrl || manifestUrl.replace('10.0.2.2', '127.0.0.1').replace('10.0.3.2', '127.0.0.1');

(async () => {
  try {
    console.log(`Fetching web manifest from: ${resolvedFetchUrl}`);
    const twa = await TwaManifest.fromWebManifest(resolvedFetchUrl);

    // Override host with the in-emulator origin so assetlinks.json is verified
    // against the address Chrome will actually use inside the emulator.
    const originUrl = new URL(manifestUrl);
    twa.packageId = packageId;
    twa.host = originUrl.host; // e.g. "cuaai--todo-gym-web.modal.run"
    // startUrl comes from Web App Manifest start_url (already parsed by fromWebManifest).
    // Do NOT override it with the manifest file path — that would open manifest.json.
    twa.manifestUrl = manifestUrl;
    twa.signingKey = { path: KEYSTORE, alias: KEYSTORE_ALIAS };

    const outFile = path.join(outputDir, 'twa-manifest.json');
    const json = twa.toJson();
    json.signingKey = {
      path: KEYSTORE,
      alias: KEYSTORE_ALIAS,
      keypassword: KEYSTORE_PASS,
      password: KEYSTORE_PASS,
    };
    fs.writeFileSync(outFile, JSON.stringify(json, null, 2));
    console.log(`twa-manifest.json written to ${outFile}`);
    console.log(`  host: ${json.host}  package: ${json.packageId}`);
  } catch (e) {
    console.error('Error:', e.message || e);
    process.exit(1);
  }
})();
