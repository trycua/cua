# docs: test="docs"
import cua

print([cua.canonical_image(os, None) for os in ("linux", "windows", "macos")])
# ['ghcr.io/trycua/linux:24.04', 'ghcr.io/trycua/windows:2022', 'ghcr.io/trycua/macos:26']
