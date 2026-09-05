# Documentation indexing and MCP serving

The docs indexer captures the public site from `https://cua.ai/docs` and builds
SQLite FTS5 and LanceDB from the same completed crawl. It discovers pages through
links from that root; it does not establish coverage of unlinked repository pages.

## Build locally

Use the server's locked environment so the writer and reader use compatible
LanceDB, PyArrow, and embedding packages. Run these commands from the repository root:

```bash
uv run --project docs/scripts/docs-mcp-server --locked --with playwright playwright install chromium
uv run --project docs/scripts/docs-mcp-server --locked --with playwright docs/scripts/crawl_docs.py
uv run --project docs/scripts/docs-mcp-server --locked docs/scripts/generate_db.py
```

The crawler writes `docs/crawled_data/_corpus.json` only after every discovered
URL succeeds. The snapshot records the source, capture time, content hash, pages,
and expected URL set. Individual crawl JSON files and `_all_pages.json` are
diagnostic outputs, not indexing inputs. A failed crawl retains the last completed
snapshot and stops the scheduled workflow before generation.

Both `generate_db.py` and the compatibility entry point `generate_sqlite.py`
build the complete pair. Run either one, once. Empty text is excluded from both
indexes with a reason in the manifest. Short pages and short paragraphs remain
searchable in both indexes.

Each successful build writes this layout under `docs/docs_db`:

```text
current.json                         # build ID and manifest hash
generations/<build-id>/
  manifest.json                      # corpus ID, expected/included URLs, exclusions,
                                     # capture time, embedding model, file hashes
  docs.sqlite                        # pages, pages_fts, index_metadata
  docs.lance/                        # chunks carrying the same build ID
```

The builder verifies the URL sets, build IDs, chunk count, and file hashes before
activation. A failed build leaves the last active generation in place.
Generations are immutable after activation and are retained for rollback.
Garbage collection is a separate operator action.

Run focused offline verification with synthetic embeddings:

```bash
uv run --no-project --with pytest --with pytest-asyncio --with markdown-it-py --with lancedb==0.37.1 --with pyarrow==25.0.1 python -m pytest -q docs/scripts/tests/test_docs_index_corpus.py docs/scripts/tests/test_docs_index_publication.py
uv lock --project docs/scripts/docs-mcp-server --check --offline
```

## Scheduled generation and publication

`modal_app.py` schedules a docs crawl at 6 AM UTC. It calls
`generate_docs_indexes` once to create and validate both indexes on local
temporary storage, then copies the immutable generation to the volume and commits
it before committing `current.json`. The old generation is retained.

The docs publisher uploads files beneath
`docs_db/generations/<build-id>/`, uploads the manifest, and writes
`docs_db/current.json` last. An interrupted upload cannot change the serving
pointer. The docs schedule passes `publish_code=False` so it does not publish or
delete code-index objects. The code index still uses its legacy publication path
with `publish_docs=False`; the docs generation contract does not certify code-index
freshness.

A daily schedule is an attempt, not freshness evidence. A successful source build,
object-store publication, consumer copy, and reader activation are separate steps.

## Consumer contract and rollout

The container reads generations beneath `DOCS_DB_PATH`. It checks `current.json`
on each docs request, validates the manifest and every file, and opens both
databases before switching handles. A bad candidate retains the last good pair
and logs the refresh rejection. If no verified pair has been loaded, docs queries
fail explicitly. Legacy flat `docs.sqlite` and `docs.lance/` files are not treated
as a verified generation.

The storage consumer must implement this contract:

1. Read `docs_db/current.json` once and retain that build ID and manifest hash.
2. Download its manifest and every listed file to a staging directory outside the
   serving generation directory. Verify the manifest hash and every file hash.
3. Install the complete directory at `DOCS_DB_PATH/generations/<build-id>/`
   without changing any existing generation.
4. Atomically replace the local `current.json` only after the complete directory
   is visible. Keep the prior generation while readers may still hold its handles.
5. Confirm `SELECT * FROM index_metadata` reports the intended build and corpus.
   Compare every `pages.url` with an exact-URL vector query, including short pages,
   and check a deliberately excluded empty page against the manifest.

For a local filesystem mirror, `docs_index.copy_generation` implements validated
copy and pointer activation. Object-store download and deployment configuration
are outside this directory. Copying the bucket wholesale with an unordered sync
is not an activation protocol.

Roll out the consumer's staged-copy support first, generate and publish one
validated pair, and verify that its files are installed. Then deploy the reader
image with `docs_index.py` and matching locked dependencies. Deploying the new
reader against only legacy files makes docs queries unavailable. Keep the previous
image and generation until the end-to-end checks pass.

A reader release does not migrate the storage consumer or create a paired index.
Complete those prerequisites before merging or publishing a reader change that
an automated deployment might pick up. Operators must explicitly perform and
verify the migration. Production generation, publication, and deployment commands
mutate remote state and require the appropriate operational authorization.

## Code-index coverage

Code indexing groups parseable repository tags by their prefixes:
`<component>-v<major>.<minor>.<patch>...`, plus `v...` tags grouped as `cua`.
For each tag it scans matching files across the repository tree, not just a
component-specific directory. A component label therefore identifies a tag
family, not proof of coverage for every product carrying that name.

The implemented file filter covers `.py`, `.ts`, `.tsx`, and `.js`.
Rust and Swift files are not indexed by this filter. SQLite accepts content
up to 1,000,000 characters; vector indexing accepts up to 100,000 characters.
Those limits use decoded string length. Version, component, and language coverage
must be enumerated from the serving database:

```sql
SELECT DISTINCT component FROM code_files;
SELECT DISTINCT language FROM code_files;
SELECT DISTINCT version FROM code_files WHERE component = 'agent';
```

The scheduled code index runs at 5 AM UTC. Per-component SQLite and LanceDB shards
are aggregated into `code_index.sqlite` and `code_index.lancedb`. This schedule
does not prove that a tag, component, or source language is present in the
serving index.
