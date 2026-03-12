"""Unit tests for the Image builder (no runtime needed)."""

import json
import pytest
from cua_sandbox.image import Image


class TestImageBuilder:
    def test_linux_defaults(self):
        img = Image.linux()
        assert img.os_type == "linux"
        assert img.distro == "ubuntu"
        assert img.version == "24.04"

    def test_macos(self):
        img = Image.macos("15")
        assert img.os_type == "macos"

    def test_windows(self):
        img = Image.windows("11")
        assert img.os_type == "windows"

    def test_chaining_is_immutable(self):
        base = Image.linux()
        with_curl = base.apt_install("curl")
        with_git = base.apt_install("git")
        assert len(base._layers) == 0
        assert len(with_curl._layers) == 1
        assert len(with_git._layers) == 1
        assert with_curl._layers[0]["packages"] == ["curl"]
        assert with_git._layers[0]["packages"] == ["git"]

    def test_full_chain(self):
        img = (
            Image.linux("debian", "12")
            .apt_install("curl", "git")
            .pip_install("numpy")
            .env(FOO="bar", BAZ="qux")
            .run("echo hello")
            .copy("/local/file", "/remote/file")
            .expose(8080)
            .expose(3000)
        )
        d = img.to_dict()
        assert d["os_type"] == "linux"
        assert d["distro"] == "debian"
        assert len(d["layers"]) == 3  # apt, pip, run
        assert d["env"] == {"FOO": "bar", "BAZ": "qux"}
        assert d["ports"] == [8080, 3000]
        assert d["files"] == [["/local/file", "/remote/file"]]

    def test_roundtrip(self):
        img = Image.linux().apt_install("curl").env(X="1").expose(80)
        d = img.to_dict()
        img2 = Image.from_dict(d)
        assert img2.to_dict() == d

    def test_cloud_init(self):
        img = Image.linux().apt_install("curl", "git").pip_install("flask").run("systemctl start app")
        script = img.to_cloud_init()
        assert "#!/bin/bash" in script
        assert "apt-get" in script
        assert "pip install flask" in script
        assert "systemctl start app" in script

    def test_from_registry(self):
        img = Image.from_registry("cua/ubuntu-desktop:24.04")
        assert img._registry == "cua/ubuntu-desktop:24.04"
        d = img.to_dict()
        assert d["registry"] == "cua/ubuntu-desktop:24.04"

    def test_repr(self):
        img = Image.linux().apt_install("curl")
        r = repr(img)
        assert "linux" in r
        assert "1 layers" in r

    def test_brew_and_choco(self):
        mac = Image.macos().brew_install("wget")
        win = Image.windows().choco_install("firefox")
        assert mac._layers[0]["type"] == "brew_install"
        assert win._layers[0]["type"] == "choco_install"
