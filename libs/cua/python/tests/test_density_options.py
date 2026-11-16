"""VM density surface: the `profile` create option and the `HostCapacity` error."""

import cua


def test_profile_is_a_create_option():
    o = cua.SandboxCreateOptions(on="local", image="ghcr.io/trycua/linux:24.04", profile="rollout")
    assert o.profile == "rollout"
    assert cua.SandboxCreateOptions(on="local", image="x").profile is None
    assert cua.SpaceCreateOptions(profile="rollout").profile == "rollout"
    assert cua.SpaceCreateOptions().profile is None


def test_named_volumes_are_a_create_option_and_a_record():
    m = cua.VolumeMount(name="shared", path="/mnt/shared")
    assert (m.read_only, m.subpath) == (False, "")
    ro = cua.VolumeMount(name="data", path="/data", read_only=True, subpath="in")
    o = cua.SpaceCreateOptions(volumes=[m, ro])
    assert [v.path for v in o.volumes] == ["/mnt/shared", "/data"]
    assert o.volumes[1].read_only and o.volumes[1].subpath == "in"
    assert cua.SpaceCreateOptions().volumes == []
    v = cua.NamedVolume(name="shared", mounted_in=["local:a"])
    assert v.mounted_in == ["local:a"]


def test_host_capacity_is_an_error_with_its_entry():
    e = cua.CuaError.HostCapacity("at its VM memory limit")
    assert isinstance(e, cua.CuaError)
    assert "VM memory limit" in str(e)
    assert e.doc_url == "https://cua.ai/docs/cua-sdk/reference/errors#hostcapacity"
