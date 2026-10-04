# Hidden docs preludes

A docs block tagged `prelude="<name>[,<name>]"` runs with setup the reader
does not see (`tests/e2e/cua-sdk/python/test_docs_blocks.py`):

| Name | Effect |
| --- | --- |
| `spacesd` | `CUA_DOCS_SPACESD_URL` and `CUA_DOCS_SPACESD_TOKEN` name the fixtures' MockServer cua-spacesd. |
| `space-url` | The direct-Space address and token a page shows (`space-url.subst.json`) become the fixtures' cua-spacesd server core, confined to temp directories. |
| `space` | `space.py` / `space.ts` define `spaces` and a registered direct `space` on that core, for excerpts that continue a page; teleport reads the fixtures' synthetic Firefox profile, never the host's. |

Blocks that name `fakefleet` (the fake Fleet API) are not run: Cua Cloud
has closed.

`<name>.subst.json` maps a literal the page shows to the environment variable
holding the fixture's value. A file `<name>.py` or `<name>.ts` here is prepended to the program of a
Python or TypeScript block that names it. A Python program may use top-level `await` (it then runs in one event loop,
like `python -m asyncio`). Keep preludes small: they are code the page does
not show.

Code preludes (each file says what it sets up): `local-cleanup` (deletes the
local sandboxes a block leaves; use it on every container-lane block that
does not delete its own), `image`, `image-desktop`, `images-dev`, `sb-web`,
`sb-web-local`, `sb-linux-local`, `sb-mcp-local`, `cua-embedded` and
`spacesd-3211`.
