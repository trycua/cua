# Third-party notices: Cua Driver Bench task pack

No third-party code, data, fonts, images or icons are vendored in this
directory. The task apps' `package-lock.json` files pin one development
dependency, Electron, which `npm ci` downloads from the npm registry together
with its own dependencies. They are used under their own licences:

| Package | Version | Licence |
| --- | --- | --- |
| `electron` | 43.4.0 | MIT |
| `@electron/get` | 5.1.0 | MIT |
| `@electron-internal/extract-zip` | 1.0.5 | BSD-2-Clause |
| `@types/node` | 24.13.3 | MIT |
| `debug` | 4.4.3 | MIT |
| `env-paths` | 3.0.0 | MIT |
| `graceful-fs` | 4.2.11 | ISC |
| `ms` | 2.1.3 | MIT |
| `progress` | 2.0.3 | MIT |
| `semver` | 7.8.5 | ISC |
| `sumchecker` | 3.0.1 | Apache-2.0 |
| `undici` | 7.29.0 | MIT |
| `undici-types` | 7.18.2 | MIT |

The Electron runtime that `electron` downloads bundles Chromium and Node.js;
their licence texts ship inside the Electron distribution (`LICENSES.chromium.html`).

LibreOffice, GnuCash, Google Chrome and Node.js are runtime prerequisites that
you install yourself; they are not distributed here. The `cdb-s03` GnuCash book
and its parser were written from the public GnuCash file-format documentation.
