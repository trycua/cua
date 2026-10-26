// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

import { join } from "node:path"

/**
 * A plan whose every step is visible if it runs twice or not at all: appends
 * add a line, writes carry the step number, removes drop an earlier file.
 */
export function crashPlan(cwd, count, sleep = 0.1) {
  const plan = []
  for (let i = 0; i < count; i++) {
    const pick = i % 4
    if (pick === 0) plan.push({ name: "bash", arguments: { command: `echo s${i} >> log.txt; sleep ${sleep}; echo appended ${i}` } })
    else if (pick === 1) plan.push({ name: "write", arguments: { path: join(cwd, `f${i}.txt`), content: `v${i}\n` } })
    else if (pick === 2) plan.push({ name: "bash", arguments: { command: `mkdir -p d; echo ${i} > d/n${i}; chmod 750 d/n${i}; ln -sf log.txt link${i}` } })
    else plan.push({ name: "bash", arguments: { command: `rm -f f${i - 2}.txt; echo r${i} >> log.txt` } })
  }
  return plan
}
