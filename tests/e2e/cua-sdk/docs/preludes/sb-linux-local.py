# Hidden docs prelude `sb-linux-local`: a local Linux desktop sandbox `sb`
# (the canonical image, which runs cua-spacesd), for fragments that act on
# "your sandbox". Combine with `local-cleanup`.
from cua_sandbox import Image as _Image
from cua_sandbox import Sandbox as _Sandbox

sb = await _Sandbox.create(_Image.linux(), local=True)  # noqa: F704
