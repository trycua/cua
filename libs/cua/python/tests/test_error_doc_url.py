"""`CuaError.doc_url` links each variant to its errors-reference entry."""

import cua


def test_every_variant_links_to_its_entry():
    base = "https://cua.ai/docs/cua-sdk/reference/errors#"
    assert cua.CuaError.NotFound("x").doc_url == base + "notfound"
    assert cua.CuaError.SpacesdNotAvailable("y").doc_url == base + "spacesdnotavailable"
    assert cua.error_doc_url("InsufficientDisk") == base + "insufficientdisk"


def test_a_full_machine_is_host_capacity_not_not_found():
    err = cua.CuaError.HostCapacity("this machine has no room for a 2-CPU, 4 GiB Space now")
    assert isinstance(err, cua.CuaError)
    assert not isinstance(err, cua.CuaError.NotFound)
    assert err.doc_url.endswith("#hostcapacity")
