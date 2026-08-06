import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import test from "node:test";

const execFileAsync = promisify(execFile);

test("patch guard rejects a malformed BillingSummary declaration", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cyclops-sdk-patch-"));
  const clientPath = join(directory, "cyclops-cs-backend.ts");
  await writeFile(clientPath, `  private customFetch = (...fetchParams: Parameters<typeof fetch>) =>\n    fetch(...fetchParams);\n\nexport interface BillingSummary {\n  card: BillingCardSummary | null;\n}\n`);

  await assert.rejects(
    execFileAsync(process.execPath, ["scripts/patch-generated-client.mjs", clientPath], {
      cwd: new URL("..", import.meta.url),
    }),
    /BillingSummary card declaration did not match/,
  );
});
