# docs: test="docs"
import cua

print([cua.image_alias("macos:sequoia"), cua.image_alias("ubuntu:24.04")])
# ['ghcr.io/trycua/macos:15', None]
