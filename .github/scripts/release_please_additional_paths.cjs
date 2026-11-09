// Give the pinned Release Please engine the `additional-paths` package option.
//
// Release Please 17.3.0 (and every later 17.x) has no `additional-paths`: the
// key is dropped while the config is parsed, so each commit counts only for
// the package whose path holds its files. Cua Spaces ships libs/cua, the web
// UI and the Electron app, yet only apps/cua-spaces-macos commits opened a
// release PR (#7056, #7062). Upstream proposes the same option in
// googleapis/release-please#2534; delete this file once the pinned engine has it.
//
// release_please.sh preloads it (`node --require`). A commit that touches a
// package's additional path is then also counted for that package, in history
// order, so the package's last release commit still bounds its list.
const fs = require('node:fs');
const path = require('node:path');
const { VERSION } = require('release-please');
const { CommitSplit } = require('release-please/build/src/util/commit-split');
const { Manifest, DEFAULT_RELEASE_PLEASE_CONFIG } = require('release-please/build/src/manifest');

const ENGINE = '17.3.0';
if (VERSION !== ENGINE) {
  throw new Error(`release_please_additional_paths.cjs patches release-please ${ENGINE}, found ${VERSION}`);
}

const strip = p => p.replace(/^\/+|\/+$/g, '');
function additionalPaths(config) {
  return Object.entries(config.packages)
    .filter(([, pkg]) => (pkg['additional-paths'] || []).length)
    .map(([pkgPath, pkg]) => {
      // The engine's exclude-paths filter keeps only files under the package path,
      // so it would drop every commit that only touches an additional path.
      if (pkg['exclude-paths']) throw new Error(`${pkgPath}: additional-paths with exclude-paths is unsupported`);
      return [strip(pkgPath), pkg['additional-paths'].map(strip)];
    });
}

// Offline previews split with the checked-out config. A real run reads the
// config the engine reads: the target branch's copy, which a queued run can
// see newer than its own checkout.
let additional = additionalPaths(
  JSON.parse(fs.readFileSync(path.resolve(__dirname, '../../release-please-config.json'), 'utf8')),
);
const fromManifest = Manifest.fromManifest;
Manifest.fromManifest = async function fromManifestWithAdditionalPaths(github, targetBranch, configFile, ...rest) {
  additional = additionalPaths(await github.getFileJson(configFile || DEFAULT_RELEASE_PLEASE_CONFIG, targetBranch));
  return fromManifest.call(this, github, targetBranch, configFile, ...rest);
};

const split = CommitSplit.prototype.split;
CommitSplit.prototype.split = function splitWithAdditionalPaths(commits) {
  const result = split.call(this, commits);
  for (const [pkgPath, paths] of additional) {
    const own = new Set(result[pkgPath] || []);
    const touches = commit => (commit.files || []).some(file => paths.some(p => file.startsWith(`${p}/`)));
    const counted = commits.filter(commit => own.has(commit) || touches(commit));
    if (counted.length) result[pkgPath] = counted;
  }
  return result;
};
