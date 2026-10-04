# docs: test="docs"
from cua_sandbox import Image

base = Image.linux().apt_install("git")
dev = base.pip_install("rich")
same = Image.from_dict(dev.to_dict()).to_dict() == dev.to_dict()
print([same, len(base.to_dict()["layers"]), len(dev.to_dict()["layers"])])
# [True, 1, 2]
