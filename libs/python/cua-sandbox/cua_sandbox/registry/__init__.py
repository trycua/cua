"""Legacy QEMU image format (build/push/pull of ``vnd.trycua.qemu`` artifacts).

Resolving what an image is (kind, variant, OS, digest) is the native
resolver's job: :func:`cua_sandbox.image.resolve_image_kind` (``cua_image``).
"""

from cua_sandbox.registry.cache import ImageCache
from cua_sandbox.registry.qemu_builder import QEMUImageConfig
from cua_sandbox.registry.qemu_builder import build_image as build_qemu_image
from cua_sandbox.registry.qemu_builder import pull_qemu_image
from cua_sandbox.registry.qemu_builder import push_image as push_qemu_image

__all__ = [
    "ImageCache",
    "QEMUImageConfig",
    "push_qemu_image",
    "pull_qemu_image",
    "build_qemu_image",
]
