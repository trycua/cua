# Hidden docs prelude `image`: fragments that start from "an image" (`image`)
# the page defined in an earlier section get a small public one.
from cua_sandbox import Image, Sandbox  # noqa: F401

image = Image.from_registry("python:3.12-slim")
