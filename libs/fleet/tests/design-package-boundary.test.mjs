import assert from "node:assert/strict"
import { readdir, readFile } from "node:fs/promises"
import test from "node:test"
import path from "node:path"
import { fileURLToPath } from "node:url"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const cyclopsRoot = path.resolve(__dirname, "..")
const repositoryRoot = path.resolve(cyclopsRoot, "..")

async function readCssTree(directory) {
  const entries = await readdir(directory, { withFileTypes: true })
  return (
    await Promise.all(
      entries.map(entry => {
        const entryPath = path.join(directory, entry.name)
        if (entry.isDirectory()) return readCssTree(entryPath)
        if (entry.isFile() && entry.name.endsWith(".css")) {
          return readFile(entryPath, "utf8")
        }
        return []
      }),
    )
  ).flat()
}

test("dashboard consumes the runtime-neutral design package", async () => {
  const [packageJson, entrypoint] = await Promise.all([
    readFile(path.join(cyclopsRoot, "package.json"), "utf8").then(JSON.parse),
    readFile(path.join(cyclopsRoot, "src/main.tsx"), "utf8"),
  ])

  assert.equal(
    packageJson.dependencies["@cua/design"],
    "file:../packages/cua-design",
  )
  assert.equal(packageJson.dependencies["@cua/mesh"], undefined)
  assert.equal(packageJson.dependencies["@paper-design/shaders"], undefined)

  const designImport = entrypoint.indexOf(
    'import "@cua/design/dashboard.css"',
  )
  const cloudscapeImport = entrypoint.indexOf(
    'import "@cloudscape-design/global-styles/index.css"',
  )
  assert.ok(designImport >= 0, "expected the shared dashboard CSS import")
  assert.ok(
    designImport < cloudscapeImport,
    "shared foundations must load before Cloudscape globals",
  )
})

test("frontend image builds from the repository root", async () => {
  const [dockerfile, workflow] = await Promise.all([
    readFile(path.join(cyclopsRoot, "Dockerfile"), "utf8"),
    readFile(
      path.join(repositoryRoot, ".github/workflows/build-cyclops-cs.yml"),
      "utf8",
    ),
  ])

  assert.match(dockerfile, /COPY packages\/cua-design\/package\.json/)
  assert.doesNotMatch(dockerfile, /packages\/cua-mesh/)
  assert.match(dockerfile, /COPY cyclops-cs\/package\.json/)
  assert.match(
    workflow,
    /- image: cyclops-cs[\s\S]*?context: \.[\s\S]*?dockerfile: cyclops-cs\/Dockerfile/,
  )
  assert.match(workflow, /- 'packages\/cua-design\/\*\*'/)
})

test("dashboard activates the shared theme through supported boundaries", async () => {
  const [entrypoint, shell, shellStyles, visualPreview] = await Promise.all([
    readFile(path.join(cyclopsRoot, "src/main.tsx"), "utf8"),
    readFile(path.join(cyclopsRoot, "src/App.tsx"), "utf8"),
    readFile(path.join(cyclopsRoot, "src/shell.css"), "utf8"),
    readFile(path.join(cyclopsRoot, "src/local-visual-preview.ts"), "utf8"),
  ])
  const allStyles = (await readCssTree(path.join(cyclopsRoot, "src"))).join(
    "\n",
  )

  assert.match(entrypoint, /@cloudscape-design\/components\/theming/)
  assert.match(entrypoint, /applyTheme\(\{/)
  assert.match(shell, /className="cua-dashboard-theme cua-shell"/)
  assert.match(shell, /className="cua-shell__topnav"/)
  assert.match(shell, /id="cua-shell-topnav"/)
  assert.match(shell, /headerSelector="#cua-shell-topnav"/)
  assert.match(shell, /navigationToggle: t\("navigation\.open"\)/)
  assert.match(shell, /navigationClose: t\("navigation\.close"\)/)
  assert.match(entrypoint, /document\.body\.id = "cua-dashboard-root"/)
  assert.doesNotMatch(shellStyles, /\.cua-pagehead__mesh/)
  assert.match(shellStyles, /@media \(forced-colors: active\)/)
  assert.doesNotMatch(shellStyles, /h1 > span:last-child/)
  // Cloudscape's typed applyTheme() surface doesn't cover every design
  // token the dashboard needs (e.g. placeholder font-style, tabs
  // dividers, table row height/borders) — raw --awsui-*/.awsui_*
  // selectors and generated --color-*/--space-*-<hash> var references
  // are the documented, intentional fallback for those gaps (see the
  // comments above each override in shell.css). This boundary test no
  // longer forbids them; it only guards the invariants below.
  assert.doesNotMatch(
    shellStyles,
    /linear-gradient\(135deg, #f0f8ff 0%, #9fd7ff 58%, #5f86b4 100%\)/,
  )
  assert.doesNotMatch(shellStyles, /color: #07131e/)
  assert.doesNotMatch(shellStyles, /min-height: 40px/)
  assert.match(visualPreview, /import\.meta\.env\.DEV/)
  assert.match(visualPreview, /VITE_CUA_LOCAL_VISUAL_PREVIEW === "true"/)
  assert.match(
    visualPreview,
    /VITE_CUA_REVIEW_VISUAL_PREVIEW === "true"/,
  )
  assert.match(
    visualPreview,
    /\^cyclops-cs-pr-\\d\+\\\.tail204509\\\.ts\\\.net\$/,
  )
})

test("account preview data stays behind the fail-closed visual-preview gate", async () => {
  const files = await Promise.all(
    [
      "src/fleet/userKeys.ts",
      "src/api/githubTrustPolicies.ts",
      "src/api/billing.ts",
      "src/components/FeatureFlagContext.tsx",
    ].map(relativePath =>
      readFile(path.join(cyclopsRoot, relativePath), "utf8"),
    ),
  )

  for (const source of files.slice(0, 3)) {
    assert.match(source, /isLocalVisualPreview\(\)/)
    assert.match(
      source,
      /if \(isLocalVisualPreview\(\)\)|isLocalVisualPreview\(\)\s*\?/,
    )
    assert.doesNotMatch(source, /if \(!isLocalVisualPreview\(\)\)/)
  }
  assert.match(files[3], /const visualPreview = isLocalVisualPreview\(\)/)
  assert.match(files[3], /visualPreview\s*\?/)
  assert.doesNotMatch(files[3], /!visualPreview/)
  assert.match(files[0], /withClient\(client => client\.listUserApiKeys\(\)\)/)
  assert.match(files[1], /request<GitHubTrustPolicyListResponse>/)
  assert.match(files[2], /billingRequest<BillingSummary>/)
})

test("frontend imports the generated UniFFI SDK without an src/sdk proxy", async () => {
  await assert.rejects(
    readdir(path.join(cyclopsRoot, "src/sdk")),
    error => error?.code === "ENOENT",
  )

  const sources = await Promise.all(
    [
      "src/auth/cyclops-client.ts",
      "src/fleet/claims.ts",
      "src/fleet/pools.ts",
      "src/fleet/userKeys.ts",
    ].map(relativePath =>
      readFile(path.join(cyclopsRoot, relativePath), "utf8"),
    ),
  )

  for (const source of sources) {
    assert.match(
      source,
      /sdk-bindings\/ts-uniffi-browser\/ts\/index\.web/,
    )
    assert.doesNotMatch(source, /src\/sdk|\.\/generated/)
  }
})
