// Builds the sticky comment for ci-pr-screenshots.yml: per screen and size,
// whether the rendering changed between the merge-base and the PR head, and
// where the images are. The images stay in workflow artifacts; they are never
// committed to a branch.
// Usage: node pr_screenshots_comment.mjs <head-dir> <base-dir> <out.md>
// Env: HEAD_SHA, BASE_SHA, RUN_URL, HEAD_ARTIFACT_URL, BASE_ARTIFACT_URL
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, existsSync } from "node:fs";

export const MARKER = "<!-- pr-ui-screenshots -->";

const sha = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");

function manifestOf(dir) {
  const f = `${dir}/manifest.json`;
  return existsSync(f) ? JSON.parse(readFileSync(f, "utf8")) : [];
}

/** "changed", "unchanged", "new" (no base image), "removed" (no head image) or "unavailable". */
export function status(headDir, baseDir, h, b) {
  const hf = h?.file && existsSync(`${headDir}/${h.file}`) ? `${headDir}/${h.file}` : null;
  const bf = b?.file && existsSync(`${baseDir}/${b.file}`) ? `${baseDir}/${b.file}` : null;
  if (hf && bf) return sha(hf) === sha(bf) ? "unchanged" : "changed";
  if (hf) return "new";
  if (bf) return "removed";
  return "unavailable";
}

export function render(headDir, baseDir, env) {
  const head = manifestOf(headDir);
  const base = manifestOf(baseDir);
  const find = (list, id, size) => list.find((m) => m.id === id && m.size === size);
  const ids = [...new Map([...head, ...base].map((m) => [m.id, m.label])).entries()];
  const rows = ids.map(([id, label]) => {
    const cells = ["desktop", "mobile"].map((size) => status(headDir, baseDir, find(head, id, size), find(base, id, size)));
    return { label, cells };
  });
  const changed = rows.filter((r) => r.cells.some((c) => c !== "unchanged" && c !== "unavailable")).length;
  const mark = { changed: "changed", unchanged: "no visible change", new: "new screen", removed: "screen removed", unavailable: "not available" };
  const lines = [
    MARKER,
    "## UI screenshots",
    "",
    `${changed} of ${rows.length} key screens changed, rendered from demo data at desktop (1440x900) and mobile (390x844).`,
    "",
    "| Screen | Desktop | Mobile |",
    "| --- | --- | --- |",
    ...rows.map((r) => `| ${r.label} | ${mark[r.cells[0]]} | ${mark[r.cells[1]]} |`),
    "",
    `Images: [after (PR head \`${env.HEAD_SHA.slice(0, 7)}\`)](${env.HEAD_ARTIFACT_URL}) and [before (merge-base \`${env.BASE_SHA.slice(0, 7)}\`)](${env.BASE_ARTIFACT_URL}), from [this run](${env.RUN_URL}). Artifacts expire after 30 days, and downloading one needs a GitHub sign-in with read access.`,
    "",
    "Embed the images that show your change in the PR description. Never commit them to the branch. Add `Screenshot routes: /path` to the description to render more routes.",
  ];
  return lines.join("\n") + "\n";
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [headDir, baseDir, out] = process.argv.slice(2);
  writeFileSync(out, render(headDir, baseDir, process.env));
}
