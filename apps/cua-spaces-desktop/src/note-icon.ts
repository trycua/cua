// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A notification's image (`SystemNote.icon`): a plain file name inside the
// directory of the web build the app loads, such as the Cua key
// (`public/cua-key-128.png`). Anything that is not a plain name is refused,
// so a note can only name a file of the build.
import { existsSync } from "node:fs";
import * as path from "node:path";

export function noteIconPath(webRoot: string, name: string): string | null {
  if (name === "" || name !== path.basename(name) || name.startsWith(".")) return null;
  const file = path.join(webRoot, name);
  return existsSync(file) ? file : null;
}
