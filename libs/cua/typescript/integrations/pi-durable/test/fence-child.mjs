// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// One writer process of the concurrent fence test: opens the fenced log at
// argv[2] with epoch argv[3] and appends rows (epoch, n) until the log refuses
// it as stale (exit 75) or argv[4] rows are written (exit 0).

import { FencedSqliteDatabase, StaleEpochError } from "../dist/fence.js"
import { openNodeSqliteDatabase } from "@earendil-works/pi-durable/storage/sqlite/node"

const [path, epochText, countText] = process.argv.slice(2)
const epoch = Number(epochText)
const inner = await openNodeSqliteDatabase(path)
let db
try {
  db = await FencedSqliteDatabase.open(inner, epoch)
  await db.transaction((tx) => tx.exec("CREATE TABLE IF NOT EXISTS rows (id INTEGER PRIMARY KEY AUTOINCREMENT, epoch INTEGER NOT NULL)"))
} catch (error) {
  if (error instanceof StaleEpochError) {
    process.stdout.write(`REFUSED open\n`)
    process.exit(75)
  }
  throw error
}
process.stdout.write("READY\n")
let written = 0
try {
  for (; written < Number(countText); written++) {
    await db.transaction((tx) => tx.run("INSERT INTO rows (epoch) VALUES (?)", epoch))
    if (written % 5 === 0) process.stdout.write(`ROW ${written}\n`)
    await new Promise((resolve) => setTimeout(resolve, 1))
  }
} catch (error) {
  if (error instanceof StaleEpochError) {
    process.stdout.write(`REFUSED ${written}\n`)
    process.exit(75)
  }
  throw error
}
process.stdout.write(`DONE ${written}\n`)
await db.close()
