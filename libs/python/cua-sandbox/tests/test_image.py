"""Unit tests for the Image builder (no runtime needed)."""

import pytest
from cua_sandbox.image import Image, cloud_registry_image

LINUX = "ghcr.io/trycua/linux:24.04"
WINDOWS = "ghcr.io/trycua/windows:2022"


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
        assert img.distro == "windows"
        assert img.version == "11"

    def test_windows_defaults_to_the_version_with_a_pinned_disk(self):
        assert Image.windows().version == "2022"


class TestBuiltinRegistryImages:
    """Built-in descriptors are the canonical ghcr.io/trycua images; the native
    resolver picks the variant (rootfs, -disk containerDisk, Lume) per backend."""

    @pytest.mark.parametrize(
        "image, expected",
        [
            (Image.linux(), LINUX),
            (Image.linux("ubuntu", "24.04"), LINUX),
            (Image.linux(kind="vm"), LINUX),
            (Image.linux(kind="container"), LINUX),
            (Image.linux("ubuntu", "22.04"), "ghcr.io/trycua/linux:22.04"),
            (Image.windows(), WINDOWS),
            (Image.windows("2022"), WINDOWS),
            (Image.macos(), "ghcr.io/trycua/macos:26"),
            (Image.macos("15"), "ghcr.io/trycua/macos:15"),
            (Image.macos("tahoe"), "ghcr.io/trycua/macos:26"),
        ],
    )
    def test_builtin_descriptors_are_canonical(self, image, expected):
        assert cloud_registry_image(image) == expected

    @pytest.mark.parametrize(
        "image",
        [
            Image.linux("debian", "12"),
            # Client Windows has no canonical image; it installs from an ISO locally.
            Image.windows("11"),
            Image.windows("10"),
            Image.android("14"),
        ],
    )
    def test_other_descriptors_have_no_canonical_image(self, image):
        assert cloud_registry_image(image) is None

    @pytest.mark.parametrize("image", [Image.linux(), Image.windows()])
    def test_explicit_registry_overrides_the_canonical_image(self, image):
        override = image._with(_registry="registry.example/custom:latest")

        assert cloud_registry_image(override) == "registry.example/custom:latest"

    @pytest.mark.parametrize(
        "variable",
        ["CUA_IMAGE_LINUX", "CUA_DEFAULT_LINUX_IMAGE", "CUA_SANDBOX_LINUX_CONTAINER_IMAGE"],
    )
    def test_one_override_per_os_and_the_old_names(self, monkeypatch, variable):
        monkeypatch.setenv(variable, "registry.example/linux:1")
        assert cloud_registry_image(Image.linux()) == "registry.example/linux:1"
        # Only the default version is overridden.
        assert cloud_registry_image(Image.linux("ubuntu", "22.04")) == (
            "ghcr.io/trycua/linux:22.04"
        )

    def test_linux_kind_defaults_to_the_image(self):
        assert Image.linux().kind is None
        assert Image.windows().kind == "vm"

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
        img = (
            Image.linux().apt_install("curl", "git").pip_install("flask").run("systemctl start app")
        )
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


class TestApkInstall:
    def test_android_apk_install(self):
        img = Image.android().apk_install("/path/to/app.apk")
        assert len(img._layers) == 1
        assert img._layers[0]["type"] == "apk_install"
        assert img._layers[0]["packages"] == ["/path/to/app.apk"]

    def test_android_apk_install_multiple(self):
        img = Image.android().apk_install("app1.apk", "app2.apk", "app3.apk")
        assert img._layers[0]["packages"] == ["app1.apk", "app2.apk", "app3.apk"]

    def test_android_apk_install_cloud_init(self):
        img = Image.android().apk_install("app.apk").run("echo done")
        script = img.to_cloud_init()
        assert "adb install app.apk" in script
        assert "echo done" in script

    def test_android_chaining(self):
        img = (
            Image.android("14")
            .apk_install("browser.apk")
            .env(ADB_HOST="localhost")
            .run("adb shell settings put global window_animation_scale 0.0")
        )
        assert len(img._layers) == 2  # apk_install + run
        assert img._env == (("ADB_HOST", "localhost"),)


class TestFromFileWithLayers:
    def test_osworld_apt_install(self):
        img = Image.from_file("/tmp/fake.qcow2", os_type="linux", agent_type="osworld").apt_install(
            "curl", "git"
        )
        assert img._disk_path == "/tmp/fake.qcow2"
        assert img._agent_type == "osworld"
        assert len(img._layers) == 1
        assert img._layers[0]["type"] == "apt_install"

    def test_osworld_full_chain(self):
        img = (
            Image.from_file("/tmp/fake.qcow2", os_type="linux", agent_type="osworld")
            .apt_install("curl", "git")
            .pip_install("numpy")
            .env(DISPLAY=":0")
            .run("systemctl restart app")
        )
        assert len(img._layers) == 3  # apt, pip, run
        assert img._disk_path == "/tmp/fake.qcow2"
        assert img._agent_type == "osworld"
        d = img.to_dict()
        assert d["os_type"] == "linux"

    def test_from_file_android_apk(self):
        img = Image.from_file("/tmp/android.qcow2", os_type="android").apk_install("myapp.apk")
        assert img._layers[0]["type"] == "apk_install"
        assert img._disk_path == "/tmp/android.qcow2"

    def test_from_file_preserves_agent_type_through_chain(self):
        img = (
            Image.from_file("/tmp/fake.qcow2", os_type="linux", agent_type="osworld")
            .apt_install("curl")
            .pip_install("flask")
            .run("echo hi")
        )
        assert img._agent_type == "osworld"
        assert img._disk_path == "/tmp/fake.qcow2"


class TestOsCompatibilityErrors:
    """Verify that install methods raise ValueError for wrong OS types."""

    def test_apt_install_on_windows(self):
        with pytest.raises(ValueError, match="apt_install is not supported on 'windows'"):
            Image.windows().apt_install("curl")

    def test_apt_install_on_macos(self):
        with pytest.raises(ValueError, match="apt_install is not supported on 'macos'"):
            Image.macos().apt_install("curl")

    def test_apt_install_on_android(self):
        with pytest.raises(ValueError, match="apt_install is not supported on 'android'"):
            Image.android().apt_install("curl")

    def test_winget_install_on_linux(self):
        with pytest.raises(ValueError, match="winget_install is not supported on 'linux'"):
            Image.linux().winget_install("Firefox")

    def test_winget_install_on_macos(self):
        with pytest.raises(ValueError, match="winget_install is not supported on 'macos'"):
            Image.macos().winget_install("Firefox")

    def test_choco_install_on_linux(self):
        with pytest.raises(ValueError, match="choco_install is not supported on 'linux'"):
            Image.linux().choco_install("firefox")

    def test_brew_install_on_windows(self):
        with pytest.raises(ValueError, match="brew_install is not supported on 'windows'"):
            Image.windows().brew_install("wget")

    def test_brew_install_on_linux(self):
        with pytest.raises(ValueError, match="brew_install is not supported on 'linux'"):
            Image.linux().brew_install("wget")

    def test_apk_install_on_linux(self):
        with pytest.raises(ValueError, match="apk_install is not supported on 'linux'"):
            Image.linux().apk_install("app.apk")

    def test_apk_install_on_windows(self):
        with pytest.raises(ValueError, match="apk_install is not supported on 'windows'"):
            Image.windows().apk_install("app.apk")

    def test_apk_install_on_macos(self):
        with pytest.raises(ValueError, match="apk_install is not supported on 'macos'"):
            Image.macos().apk_install("app.apk")

    def test_from_file_wrong_os(self):
        """from_file with os_type='windows' should reject apt_install."""
        with pytest.raises(ValueError, match="apt_install is not supported on 'windows'"):
            Image.from_file("/tmp/win.qcow2", os_type="windows").apt_install("curl")

    def test_pip_and_uv_allowed_on_all(self):
        """pip_install and uv_install are cross-platform — no OS restriction."""
        Image.linux().pip_install("flask")
        Image.windows().pip_install("flask")
        Image.macos().pip_install("flask")
        Image.android().pip_install("flask")
        Image.linux().uv_install("flask")

    def test_run_and_env_allowed_on_all(self):
        """run() and env() are cross-platform."""
        Image.linux().run("echo hi").env(X="1")
        Image.windows().run("echo hi").env(X="1")
        Image.android().run("echo hi").env(X="1")


def _published(ref):
    import json
    from pathlib import Path

    root = Path(__file__).resolve().parents[3]
    catalog = json.loads((root / "images/sandbox-images.json").read_text())
    return next((i["published"] for i in catalog["images"] if i["ref"] == ref), True)


class TestTiersAndOmarchy:
    """Tiers (``<os-version>[-<tier>]``) and Omarchy come from the native
    resolver, which refuses catalog entries CI has not published."""

    def test_full_is_the_default(self):
        assert Image.linux(tier="full")._registry is None
        assert cloud_registry_image(Image.linux(tier="full")) == LINUX

    @pytest.mark.parametrize(
        "make, ref",
        [
            (lambda: Image.linux(tier="slim"), "ghcr.io/trycua/linux:24.04-slim"),
            (lambda: Image.macos("26", tier="xcode"), "ghcr.io/trycua/macos:26-xcode"),
            (lambda: Image.omarchy(), "ghcr.io/trycua/omarchy:edge"),
        ],
    )
    def test_tier_and_omarchy_refs(self, make, ref):
        from cua_sandbox._sdk import native

        if _published(ref):
            img = make()
            assert cloud_registry_image(img) == ref
            assert img.to_dict()["registry"] == ref
        else:
            with pytest.raises(native().CuaError.ImageNotPublished, match="not published yet"):
                make()

    def test_omarchy_is_an_amd64_linux_vm_by_explicit_ref(self):
        img = Image.from_registry("ghcr.io/trycua/omarchy:edge", kind="vm")
        assert (img.os_type, img.kind) == ("linux", "vm")

    def test_xcode_is_macos_only(self):
        from cua_sandbox._sdk import native

        with pytest.raises(native().CuaError.InvalidArgument):
            Image.linux(tier="xcode")
