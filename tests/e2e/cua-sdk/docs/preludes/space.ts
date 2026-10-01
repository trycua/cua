// `spaces` and a registered direct `space` on the fixtures' confined cua-spacesd
// core; teleport reads the fixtures' synthetic Firefox profile. `askUser`
// stands in for the reader's approval UI: the default selection, with the
// sensitive items acknowledged.
import { embedded } from "@trycua/cua"

const spaces = embedded({ teleportHome: process.env.CUA_SPACES_TELEPORT_HOME }).spaces()
const space = await spaces.space(
  (await spaces.add(process.env.CUA_DOCS_SPACE_URL!, process.env.CUA_DOCS_SPACE_TOKEN!, "docs")).id,
)
const askUser = (_manifest: unknown) => ({ include: undefined, acknowledgeSensitive: true })
