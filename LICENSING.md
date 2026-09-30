# Licensing

## How licensing works here

The root [`LICENSE.md`](LICENSE.md) is the MIT License, and it covers everything
in this repository unless a subdirectory has its own LICENSE file. A
subdirectory with its own LICENSE file is licensed under that file instead. Each
package also states its licence in its metadata: `license` in `Cargo.toml` or
`package.json`, and `project.license` in `pyproject.toml`. Third-party material
keeps its original licence.

## LICENSE files in this repository

This table lists every licence file tracked in the repository. Each SPDX
identifier comes from the file's text. Where the text alone cannot tell
`-only` from `-or-later`, the table uses the variant the package metadata or
notices declare.

| Path                                                                                                                                                           | SPDX licence      | Scope                                                                                                                                    |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| [`LICENSE.md`](LICENSE.md)                                                                                                                                     | MIT               | Repository default                                                                                                                       |
| [`libs/cua-bench/LICENSE`](libs/cua-bench/LICENSE)                                                                                                             | MIT               | Package                                                                                                                                  |
| [`libs/cua-bench-s1/python/LICENSE`](libs/cua-bench-s1/python/LICENSE)                                                                                         | MIT               | Package                                                                                                                                  |
| [`libs/cua-s1/python/LICENSE`](libs/cua-s1/python/LICENSE)                                                                                                     | MIT               | Package                                                                                                                                  |
| [`libs/kasm/LICENSE`](libs/kasm/LICENSE)                                                                                                                       | MIT               | Package; includes portions copyright Kasm Technologies Inc.                                                                              |
| [`libs/lume/metal-capability-shim/LICENSE`](libs/lume/metal-capability-shim/LICENSE)                                                                           | MIT               | Package                                                                                                                                  |
| [`libs/python/cua/LICENSE`](libs/python/cua/LICENSE)                                                                                                           | MIT               | Package                                                                                                                                  |
| [`libs/python/cua-fleet/LICENSE`](libs/python/cua-fleet/LICENSE)                                                                                               | MIT               | Package                                                                                                                                  |
| [`libs/python/som/LICENSE`](libs/python/som/LICENSE)                                                                                                           | AGPL-3.0-or-later | Package; `-or-later` per `pyproject.toml`                                                                                                |
| [`libs/typescript/computer/LICENSE`](libs/typescript/computer/LICENSE)                                                                                         | MIT               | Package                                                                                                                                  |
| [`libs/typescript/core/LICENSE`](libs/typescript/core/LICENSE)                                                                                                 | MIT               | Package                                                                                                                                  |
| [`libs/typescript/fleet/LICENSE`](libs/typescript/fleet/LICENSE)                                                                                               | MIT               | Package                                                                                                                                  |
| [`libs/cua-driver/rust/crates/cua-perception/tests/quality-corpus/LICENSE`](libs/cua-driver/rust/crates/cua-perception/tests/quality-corpus/LICENSE)           | MIT               | Test corpus; Cua-authored synthetic data (see its `PROVENANCE.md`)                                                                       |
| [`libs/cua-driver/rust/crates/cua-perception/models/licenses/AGPL-3.0-only.txt`](libs/cua-driver/rust/crates/cua-perception/models/licenses/AGPL-3.0-only.txt) | AGPL-3.0-only     | Third-party model licence text; see [`THIRD_PARTY_NOTICES.md`](libs/cua-driver/rust/crates/cua-perception/models/THIRD_PARTY_NOTICES.md) |
| [`libs/cua-driver/rust/crates/cua-perception/models/licenses/Apache-2.0.txt`](libs/cua-driver/rust/crates/cua-perception/models/licenses/Apache-2.0.txt)       | Apache-2.0        | Third-party model licence text; see [`THIRD_PARTY_NOTICES.md`](libs/cua-driver/rust/crates/cua-perception/models/THIRD_PARTY_NOTICES.md) |

Packages without a LICENSE file of their own fall under the root MIT License.
When you add a LICENSE file to a subdirectory, add a row here and set the same
licence in the package metadata.
