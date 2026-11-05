

def test_the_local_account_name_can_be_chosen(monkeypatch):
    from cua_sandbox.builder.windows_unattend import generate_autounattend_xml

    monkeypatch.delenv("CUA_WINDOWS_USER", raising=False)
    assert "<Name>User</Name>" in generate_autounattend_xml()
    monkeypatch.setenv("CUA_WINDOWS_USER", "Docker")
    xml = generate_autounattend_xml()
    assert "<Name>Docker</Name>" in xml and "<Username>Docker</Username>" in xml
    assert "<Name>Alice</Name>" in generate_autounattend_xml(username="Alice")
