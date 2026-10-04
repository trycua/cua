# Hidden docs prelude `images-dev`: the `dev` image the "Chain and fork"
# section of the images guide built (same layers).
from cua_sandbox import Image as _Image

base = _Image.linux().apt_install("curl", "git", "python3-pip")
dev = base.pip_install("ipython", "rich").env(DEBUG="1")
