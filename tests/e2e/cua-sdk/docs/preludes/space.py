# `spaces` and a registered direct `space` on the fixtures' confined cua-spacesd
# core; teleport reads the fixtures' synthetic Firefox profile.
import os

import cua

spaces = cua.embedded(teleport_home=os.environ["CUA_SPACES_TELEPORT_HOME"]).spaces()
# Top-level await: the page continues this prelude inside its event loop.
_docs_space = await spaces.add(  # noqa: F704
    os.environ["CUA_DOCS_SPACE_URL"], os.environ["CUA_DOCS_SPACE_TOKEN"], "docs"
)
space = await spaces.space(_docs_space.id)  # noqa: F704
