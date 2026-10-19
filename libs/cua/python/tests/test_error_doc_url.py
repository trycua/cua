"""`CuaError.doc_url` links each variant to its errors-reference entry."""

import cua


def test_every_variant_links_to_its_entry():
    base = "https://cua.ai/docs/cua-sdk/reference/errors#"
    assert cua.CuaError.NotFound("x").doc_url == base + "notfound"
    assert cua.CuaError.SpacesdNotAvailable("y").doc_url == base + "spacesdnotavailable"
    assert cua.error_doc_url("InsufficientDisk") == base + "insufficientdisk"
