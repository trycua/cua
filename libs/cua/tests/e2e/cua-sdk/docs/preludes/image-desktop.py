# Hidden docs prelude `image-desktop`: "an image" that keeps running on its
# own (the canonical Linux desktop), for fragments such as sidecars that need
# a live sandbox container.
from cua_sandbox import Image, Sandbox  # noqa: F401

image = Image.linux()
