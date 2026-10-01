# Documentation Generators

Generators that keep reference pages in `docs/content/docs` in sync with source code.

## Layout

```text
scripts/docs-generators/
├── config.json                 # Libraries, source paths, output paths, generator scripts
├── runner.ts                   # Orchestrator (generate, --check, --list, --library)
├── cua-driver.ts               # cua-driver (Rust): CLI + MCP tools
├── lume.ts                     # Lume (Swift): CLI + HTTP API
├── sandbox.ts                  # Sandbox package and image facts (sandbox-facts.json)
├── prose-style.ts              # Prose style checks for generated pages
├── generate-changelog.ts       # Changelog pages
├── lib/mdx.ts                  # Shared helpers: escaping, banner, meta.json, drift sync
├── cua-sdk.ts                  # Cua SDK object pages (UniFFI metadata), index, Errors, Types
├── python-sdk.ts               # Python high-level API (griffe): cua, cua-sandbox
├── extract_python_docs.py      # Static griffe extractor (pinned in requirements.txt)
├── typescript-sdk.ts           # @trycua/cua hand-written layer (typedoc + plugin-markdown)
├── rust-crates.ts              # Rust crates (rustdoc JSON, pinned nightly in rustdoc-json/)
├── proto.ts                    # gRPC protocol (buf FileDescriptorSet)
├── headers/<product>/<page>.md # Curated prose a generator places under a page's intro
└── examples/<product>/...      # Tested example programs a generator embeds
```

## Usage

Run from `docs/`:

```bash
pnpm docs:generate                   # all enabled generators
pnpm docs:check                      # drift check (CI mode)
pnpm docs:list                       # configured generators
pnpm docs:generate:lume              # one library (also: sandbox, cua-driver, python, cua-sdk-ts)
```

## How it works

1. `config.json` lists each library: source paths to watch, output path, generator script,
   build command if needed, and `enabled`.
2. `runner.ts` reads it, detects changed files in CI, runs the matching generators and
   reports drift.
3. Each generator extracts metadata from source (for example `dump-docs` JSON from the
   Rust and Swift CLIs) and writes MDX.

CI: `.github/workflows/ci-check-docs.yml` runs the drift check;
`cd-cua-driver-docs.yml` regenerates the cua-driver pages.

## Status

| Generator | Output |
| --- | --- |
| cua-driver | `cua-driver/reference` |
| lume | `lume/reference` |
| cua-cli | `cua-cli/reference` |
| cua-sdk | `cua-sdk/reference`: `index`, one page per object, `errors`, `types`, `meta.json` |
| sandbox | `cua-sdk/reference/{os-image-catalog,runtime-support}.mdx` |
| cua-sdk-python | `cua-sdk/reference/python` |
| cua-sdk-ts | `cua-sdk/reference/typescript` |
| cua-rust | `cua-sdk/reference/rust` |
| cua-proto | `cua-sdk/reference/protocol` |

The Python generator runs griffe through `uv run --no-project` (Python 3.12); the
TypeScript generator needs `npm ci --ignore-scripts` in `libs/cua/typescript`.
Docs are generated for the latest source only; the version is stamped in each page banner.

## Curated headers and examples

Reference pages hold only generated content. Facts the source cannot carry
(install lines, topology tables, one-line contracts) live in
`headers/<product>/<page>.md`; the generator that owns the page inserts the
header under its intro and fills `{{version}}`. Keep headers to a few lines
or one table; explanations belong in guides.

Examples are programs under `examples/<product>/`, named after what they show
(`Sandboxes.create.py`, `SpacesdClient.run.ts`, `canonical_image.py`). The first
line declares how CI runs it, in the docs code-block policy's terms:

```python skip="pseudo"
# docs: test="docs" prelude="spacesd,spacesd-vars"
```

The generator embeds the file under that item as a `test=` block with a
stable id, so the docs-blocks suite (`tests/e2e/cua-sdk/run.py --lanes docs`)
runs it from the page. An example whose target does not exist fails the
generator.

## Adding a library

Add an entry to `config.json` (`name`, `language`, `sourcePath`, `docsOutputPath`,
`generatorScript`, `watchPaths`, `enabled`), then write the generator following `lume.ts`.
