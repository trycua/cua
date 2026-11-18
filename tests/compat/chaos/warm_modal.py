#!/usr/bin/env python3
"""Builds the sandbox image once so the first chaos seed does not time out."""

import modal

app = modal.App.lookup("warm", create_if_missing=True)
img = modal.Image.from_registry("ghcr.io/trycua/linux:24.04")
sb = modal.Sandbox.create("true", app=app, image=img, timeout=600)
sb.wait()
sb.terminate()
print("warm ok")
