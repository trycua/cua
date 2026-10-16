# Licence notices for the Cua Spaces UI

`notices.mjs` regenerates the npm dependency section of the repository's
[`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md) for
`apps/cua-spaces-web` and `apps/cua-spaces-desktop`, and checks every
licence against an allow-list.

```sh
pnpm install                                  # in each app, so node_modules exists
node scripts/licenses/notices.mjs             # rewrite the section, print a summary
node scripts/licenses/notices.mjs --check     # fail if the section is out of date
```

It follows each app's `dependencies` and `optionalDependencies` through the
installed `node_modules` (npm, pnpm or yarn layouts) and skips packages inside
this repository. An app that is missing or not installed is skipped with a
note. Exit status is 1 when a licence is outside the allow-list or, with
`--check`, when the section is stale.

Allowed: MIT, ISC, BSD-2-Clause, BSD-3-Clause, Apache-2.0, 0BSD, Unlicense,
CC0-1.0, BlueOak-1.0.0 and OFL-1.1 (fonts). MPL-2.0 passes but is printed as
`REVIEW` so a person checks it. For an `OR` expression, one allowed branch is
enough; for `AND`, every part must be allowed. Anything else, including no
declared licence or `SEE LICENSE IN`, fails.

## Shipping the notices

Each app bundle ships `THIRD_PARTY_NOTICES.md` and links to it from its About
panel. Run `notices.mjs` before packaging so the bundled copy is current.

**Electron (`apps/cua-spaces-desktop`).** Copy the file into the app's
resources with electron-builder's `extraResources`:

```yaml
extraResources:
  - from: ../../THIRD_PARTY_NOTICES.md
    to: THIRD_PARTY_NOTICES.md
```

At runtime it is at `path.join(process.resourcesPath, "THIRD_PARTY_NOTICES.md")`.
Add a "Third-party notices" entry to the About panel (`app.setAboutPanelOptions`
credits, or the app's own About view) that opens it with
`shell.openPath`, or renders it in a window.

**SwiftUI (`apps/cua-spaces-macos`).** Add the file to the app target's bundle
resources (in `project.yml`, a `resources` entry pointing at
`../../THIRD_PARTY_NOTICES.md`), next to the app's own
`apps/cua-spaces-macos/THIRD_PARTY_NOTICES.md`. In the About panel, add a
"Third-party notices" link that opens
`Bundle.main.url(forResource: "THIRD_PARTY_NOTICES", withExtension: "md")`
with `NSWorkspace.shared.open`, or pass it in the `.credits` option of
`NSApplication.orderFrontStandardAboutPanel(options:)`.

**Web (`apps/cua-spaces-web`).** When the web UI is served on its own, copy
the file into the build output and link it from the About or settings page.
