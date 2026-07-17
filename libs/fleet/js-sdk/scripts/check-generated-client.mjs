import { execFileSync } from "node:child_process"
import { readFile, writeFile } from "node:fs/promises"

const generatedClientPath = new URL("../src/generated/cyclops-cs-backend.ts", import.meta.url)
const before = await readFile(generatedClientPath)

try {
  execFileSync("npm", ["run", "generate"], { stdio: "inherit" })
  const after = await readFile(generatedClientPath)
  if (!before.equals(after)) {
    throw new Error("Generated client drift detected; run npm run generate and commit the result")
  }
} finally {
  await writeFile(generatedClientPath, before)
}
