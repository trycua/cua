import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const packageRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const fixture = JSON.parse(
  fs.readFileSync(
    path.join(packageRoot, "..", "compat-fixtures", "typescript-package.json"),
    "utf8",
  ),
)

const normalize = (value) => value.replace(/\s+/g, " ").trim()

test("released package exports and declarations remain available", () => {
  const packageJson = JSON.parse(
    fs.readFileSync(path.join(packageRoot, "package.json"), "utf8"),
  )
  for (const [subpath, conditions] of Object.entries(fixture.package_exports)) {
    assert.deepEqual(packageJson.exports[subpath], conditions)
  }

  const nativeDeclarations = [
    "cua_driver_contract.d.ts",
    "cua_driver_sdk.d.ts",
  ].map((name) =>
    fs.readFileSync(path.join(packageRoot, "dist", "native", name), "utf8"),
  ).join("\n")
  const declarationPattern =
    /^export declare (?:abstract )?(?:class|enum|function|const)\s+([A-Za-z0-9_]+)|^export (?:type|interface)\s+([A-Za-z0-9_]+)/gm
  const actualExports = new Set(
    [...nativeDeclarations.matchAll(declarationPattern)].map(
      (match) => match[1] ?? match[2],
    ),
  )
  for (const expected of fixture.root_declaration_exports) {
    assert.ok(actualExports.has(expected), `missing declaration export ${expected}`)
  }

  const normalizedSdk = normalize(
    fs.readFileSync(
      path.join(packageRoot, "dist", "native", "cua_driver_sdk.d.ts"),
      "utf8",
    ),
  )
  for (const declaration of fixture.cua_driver_declarations) {
    assert.ok(
      normalizedSdk.includes(normalize(declaration)),
      `missing CuaDriver declaration: ${declaration}`,
    )
  }

  const nativeEntrypoint = fs.readFileSync(
    path.join(packageRoot, "src", "native", "index.ts"),
    "utf8",
  )
  for (const fragment of fixture.native_entrypoint_fragments) {
    assert.ok(
      normalize(nativeEntrypoint).includes(normalize(fragment)),
      `native entrypoint no longer exposes ${fragment}`,
    )
  }

  for (const [entrypoint, fragments] of Object.entries(
    fixture.entrypoint_declarations,
  )) {
    const declarations = fs.readFileSync(
      path.join(packageRoot, "dist", entrypoint),
      "utf8",
    )
    for (const fragment of fragments) {
      assert.ok(
        normalize(declarations).includes(normalize(fragment)),
        `${entrypoint} no longer exposes ${fragment}`,
      )
    }
  }
})
