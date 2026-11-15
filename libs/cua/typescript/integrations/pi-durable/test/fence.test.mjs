// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// The epoch fence on the log: SQLite floor row inside the commit transaction,
// and marker files for file logs.

import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { mkdtempSync, readdirSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"
import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"
import { NodeExecutionEnv } from "@earendil-works/pi-durable/env/node"
import { JsonlStorage } from "@earendil-works/pi-durable/storage/jsonl"
import { openNodeSqliteDatabase } from "@earendil-works/pi-durable/storage/sqlite/node"
import {
  FencedFileSystem,
  FencedSqliteDatabase,
  isStaleEpochError,
  openFencedSqlite,
  openFencedSqliteStorage,
  StaleEpochError,
} from "../dist/index.js"
import { open } from "./fixture.mjs"
import { LocalSpacesd } from "./local-space.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const temp = () => mkdtempSync(join(tmpdir(), "pdv2-fence-"))

async function rows(path) {
  const db = await openNodeSqliteDatabase(path)
  try {
    return (await db.all("SELECT epoch FROM rows ORDER BY id")).map((r) => r.epoch)
  } finally {
    await db.close()
  }
}

async function fenced(path, epoch) {
  const db = await FencedSqliteDatabase.open(await openNodeSqliteDatabase(path), epoch)
  await db.transaction((tx) => tx.exec("CREATE TABLE IF NOT EXISTS rows (id INTEGER PRIMARY KEY AUTOINCREMENT, epoch INTEGER NOT NULL)"))
  return db
}

const insert = (db, epoch) => db.transaction((tx) => tx.run("INSERT INTO rows (epoch) VALUES (?)", epoch))

test("epoch 1 is refused once epoch 2 has opened the same log, and nothing of its write lands", async () => {
  const path = join(temp(), "log.db")
  const one = await fenced(path, 1)
  await insert(one, 1)
  const two = await fenced(path, 2)
  await assert.rejects(
    insert(one, 1),
    (error) => error instanceof StaleEpochError && error.epoch === 1 && error.floor === 2 && error.resource === "log",
  )
  assert.equal(one.stale?.floor, 2)
  await insert(two, 2)
  assert.deepEqual(await rows(path), [1, 2])
  await one.close()
  await two.close()
})

test("a writer is refused mid-sequence: earlier commits stay, later ones are rolled back", async () => {
  const path = join(temp(), "log.db")
  const one = await fenced(path, 1)
  await insert(one, 1)
  await insert(one, 1)
  const two = await fenced(path, 2)
  // A multi-statement transaction of the stale writer is rolled back as a whole.
  await assert.rejects(
    one.transaction(async (tx) => {
      await tx.run("INSERT INTO rows (epoch) VALUES (1)")
      await tx.run("INSERT INTO rows (epoch) VALUES (1)")
    }),
    StaleEpochError,
  )
  assert.deepEqual(await rows(path), [1, 1])
  await one.close()
  await two.close()
})

test("the floor only rises: a lower epoch cannot open, an equal one can, a higher one raises", async () => {
  const path = join(temp(), "log.db")
  await (await fenced(path, 5)).close()
  await assert.rejects(fenced(path, 3), StaleEpochError)
  const again = await fenced(path, 5)
  assert.equal(await again.floor(), 5)
  const seven = await fenced(path, 7)
  assert.equal(await seven.floor(), 7)
  await assert.rejects(again.check(), StaleEpochError)
  await seven.check()
  await again.close()
  await seven.close()
})

test("check() latches a refusal that onStale listeners hear exactly once", async () => {
  const path = join(temp(), "log.db")
  const one = await fenced(path, 1)
  const heard = []
  one.onStale((error) => heard.push(error))
  const two = await fenced(path, 2)
  assert.equal(one.stale, undefined)
  await assert.rejects(one.check(), StaleEpochError)
  await assert.rejects(one.check(), StaleEpochError)
  await assert.rejects(insert(one, 1), StaleEpochError)
  assert.equal(heard.length, 1)
  assert.ok(one.stale)
  await one.close()
  await two.close()
})

test("openFencedSqliteStorage: a Pi storage whose commits are refused after a newer epoch opened", async () => {
  const dir = temp()
  const path = join(dir, "log.db")
  const old = await open({ dir, cwd: dir, spacesd: new LocalSpacesd(join(dir, "s")), plan: [], epoch: 1 })
  const submission = await old.root.submit({ type: "input", content: "hello", requestId: "r1" }, context)
  await submission.wait(context)
  const newer = await openFencedSqliteStorage(path, 2)
  // The old driver's next write goes through its own Harness; the commit is refused.
  await assert.rejects(
    old.root.submit({ type: "input", content: "again", requestId: "r2" }, context),
    isStaleEpochError,
  )
  await newer.close(context)
  await old.close()
})

test("concurrent processes: once a newer epoch is open, every older writer stops and no old row follows a newer one", async () => {
  const dir = temp()
  const path = join(dir, "log.db")
  const children = []
  const start = (epoch, count) => {
    const child = spawn(process.execPath, [join(here, "fence-child.mjs"), path, String(epoch), String(count)], {
      stdio: ["ignore", "pipe", "inherit"],
    })
    const record = { epoch, out: "", exit: new Promise((resolve) => child.on("exit", (code) => resolve(code))) }
    child.stdout.on("data", (chunk) => (record.out += chunk.toString()))
    children.push(record)
    return record
  }
  const waitFor = async (record, pattern) => {
    for (let i = 0; i < 2000; i++) {
      if (pattern.test(record.out)) return
      await new Promise((resolve) => setTimeout(resolve, 10))
    }
    throw new Error(`epoch ${record.epoch} never printed ${pattern}: ${record.out}`)
  }
  // The file exists (and is in WAL mode) before the writers race for it.
  await (await fenced(path, 0)).close()
  const olds = []
  for (const epoch of [1, 2, 3]) {
    const writer = start(epoch, 100000)
    olds.push(writer)
    await waitFor(writer, /ROW \d+/)
  }
  const newest = start(4, 40)
  assert.equal(await newest.exit, 0, newest.out)
  const codes = await Promise.all(olds.map((o) => o.exit))
  assert.deepEqual(codes, [75, 75, 75], olds.map((o) => o.out).join("|"))
  const all = await rows(path)
  assert.ok(all.length > 40)
  // Rows are in commit order: epochs never go down, so no stale write followed a newer epoch's.
  for (let i = 1; i < all.length; i++) assert.ok(all[i] >= all[i - 1], `row ${i}: ${all[i - 1]} then ${all[i]}`)
  assert.equal(all.filter((e) => e === 4).length, 40)
})

test("FencedFileSystem: markers fence a file log; stale writes are refused, floor only rises", async () => {
  const dir = temp()
  const env = new NodeExecutionEnv({ cwd: dir })
  const markers = join(dir, "fence")
  const one = new FencedFileSystem(env, markers, 1)
  await one.raise(context)
  const fs1 = one.fenced()
  assert.equal((await fs1.writeFile(join(dir, "a"), "1", context)).ok, true)
  const two = new FencedFileSystem(env, markers, 2)
  await two.raise(context)
  assert.deepEqual(readdirSync(markers), ["epoch-00000000000000000002"], "lower markers are pruned")
  await assert.rejects(fs1.writeFile(join(dir, "a"), "stale", context), StaleEpochError)
  await assert.rejects(fs1.appendFile(join(dir, "a"), "stale", context), StaleEpochError)
  await assert.rejects(new FencedFileSystem(env, markers, 1).raise(context), StaleEpochError)
  assert.equal((await env.readTextFile(join(dir, "a"), context)).value, "1")
  assert.equal((await two.fenced().appendFile(join(dir, "a"), "2", context)).ok, true)
  // Reads are not fenced.
  assert.equal((await fs1.readTextFile(join(dir, "a"), context)).value, "12")
  assert.equal(await two.floor(context), 2)
})

test("a JSONL log on a FencedFileSystem refuses the older driver's commit", async () => {
  const dir = temp()
  const env = new NodeExecutionEnv({ cwd: dir })
  const markers = join(dir, "fence")
  const one = new FencedFileSystem(env, markers, 1)
  await one.raise(context)
  const storage1 = await JsonlStorage.open(join(dir, "log"), one.fenced(), context)
  const id = await storage1.mintId()
  await storage1.commit([{ type: "conversation", value: conversation(id) }], context)
  const two = new FencedFileSystem(env, markers, 2)
  await two.raise(context)
  const id2 = await storage1.mintId()
  await assert.rejects(storage1.commit([{ type: "conversation", value: conversation(id2) }], context), isStaleEpochError)
  const storage2 = await JsonlStorage.open(join(dir, "log"), two.fenced(), context)
  assert.ok(await storage2.conversation(id, context))
})

function conversation(id) {
  return { id, createdAt: 0, ownership: { kind: "root" }, head: undefined, tail: undefined }
}

test("a stale writer that only reads is refused by its reads (a driver spinning on reads must halt)", async () => {
  const path = join(temp(), "log.db")
  const one = await fenced(path, 1)
  assert.equal((await one.get("SELECT 1 AS x")).x, 1)
  await fenced(path, 2)
  let heard
  one.onStale((error) => {
    heard = error
  })
  // The floor is re-read at most every 200 ms; the next read after that is refused.
  await new Promise((resolve) => setTimeout(resolve, 250))
  await assert.rejects(one.get("SELECT 1 AS x"), (error) => error instanceof StaleEpochError && error.epoch === 1 && error.floor === 2)
  assert.equal(heard?.floor, 2)
  await assert.rejects(one.all("SELECT 1 AS x"), StaleEpochError)
})
