# docs: test="docs"
import cua

print([cua.normalize_image("python:3.12-slim"), cua.normalize_image("ghcr.io/trycua/linux:24.04")])
# ['docker.io/library/python:3.12-slim', 'ghcr.io/trycua/linux:24.04']
