from cua_sandbox.runtime.images import SPACESD_PORT, internal_ports


def test_desktop_ports() -> None:
    # The canonical Linux image ships cua-spacesd (and its viewer) on 3211
    # and no VNC server.
    assert SPACESD_PORT == 3211
    assert internal_ports("ghcr.io/trycua/linux:24.04") == (3211, None)


def test_no_desktop_vnc_port() -> None:
    from cua_sandbox.runtime import images

    assert not hasattr(images, "DESKTOP_VNC_PORT")


def test_no_default_image_tables() -> None:
    """Default images come from the native resolver's canonical table only."""
    from cua_sandbox.runtime import images

    for name in ("CUA_DESKTOP_LINUX", "MACOS_SEQUOIA", "MACOS_VERSION_IMAGES", "resolve_image"):
        assert not hasattr(images, name), name
