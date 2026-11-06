from cua_sandbox.builder.build import SETUP_SPACESD_PS1


def test_the_local_account_name_can_be_chosen(monkeypatch):
    from cua_sandbox.builder.windows_unattend import generate_autounattend_xml

    monkeypatch.delenv("CUA_WINDOWS_USER", raising=False)
    assert "<Name>User</Name>" in generate_autounattend_xml()
    monkeypatch.setenv("CUA_WINDOWS_USER", "Docker")
    xml = generate_autounattend_xml()
    assert "<Name>Docker</Name>" in xml and "<Username>Docker</Username>" in xml
    assert "<Name>Alice</Name>" in generate_autounattend_xml(username="Alice")


def test_the_daemon_gets_a_program_firewall_rule_so_no_prompt_blocks_it():
    # A port rule alone still raises "allow public and private networks"; unattended, nobody clicks it.
    assert 'add rule name="cua-spacesd program"' in SETUP_SPACESD_PS1
    assert 'program="$Exe"' in SETUP_SPACESD_PS1
    assert "profile=any" in SETUP_SPACESD_PS1
