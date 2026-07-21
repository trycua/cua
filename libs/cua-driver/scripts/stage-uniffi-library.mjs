#!/usr/bin/env node

import { copyFileSync, existsSync, mkdirSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const driverRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const file =
  process.platform === "darwin"
    ? "libcua_driver_sdk.dylib"
    : process.platform === "win32"
      ? "cua_driver_sdk.dll"
      : "libcua_driver_sdk.so"
const source = join(driverRoot, "rust", "target", "release", file)
if (!existsSync(source)) {
  throw new Error(`missing ${source}; build cua-driver-sdk --release first`)
}

const destinations = [
  join(driverRoot, "python", "src", "cua_driver", file),
  join(driverRoot, "typescript", "dist", "native", file),
]
for (const destination of destinations) {
  mkdirSync(dirname(destination), { recursive: true })
  copyFileSync(source, destination)
  console.log(`staged ${destination}`)
}
