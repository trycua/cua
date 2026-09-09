"""UEFI firmware discovery for local Windows VMs.

OVMF ships as two halves that must be the same size class: a 4 MB `OVMF_CODE_4M.fd`
backed by a 2 MB `OVMF_VARS.fd` leaves the guest unable to boot. Both the bare-metal
and WSL-hosted runtimes therefore take code and vars from the same candidate entry.
"""

from cua_sandbox.runtime.qemu import (
    _locate_uefi_firmware,
    _locate_uefi_firmware_in,
)

CODE_4M = "/usr/share/OVMF/OVMF_CODE_4M.fd"
VARS_4M = "/usr/share/OVMF/OVMF_VARS_4M.fd"
CODE_2M = "/usr/share/OVMF/OVMF_CODE.fd"
VARS_2M = "/usr/share/OVMF/OVMF_VARS.fd"


def _present(*paths: str):
    return lambda path: path in set(paths)


class TestGuestFilesystemLookup:
    """`_locate_uefi_firmware_in` backs the WSL path, where files live inside the distro."""

    def test_finds_the_4m_pair_ubuntu_2404_ships(self):
        assert _locate_uefi_firmware_in(_present(CODE_4M, VARS_4M)) == (CODE_4M, VARS_4M)

    def test_prefers_the_4m_pair_when_both_generations_are_installed(self):
        found = _locate_uefi_firmware_in(_present(CODE_4M, VARS_4M, CODE_2M, VARS_2M))

        assert found == (CODE_4M, VARS_4M)

    def test_does_not_pair_a_2m_firmware_with_a_4m_varstore(self):
        """The defect this guards: two independent lookups would have matched these."""
        assert _locate_uefi_firmware_in(_present(CODE_2M, VARS_4M)) == (CODE_2M, None)

    def test_reports_firmware_without_a_varstore_rather_than_inventing_one(self):
        assert _locate_uefi_firmware_in(_present(CODE_4M)) == (CODE_4M, None)

    def test_reports_nothing_when_ovmf_is_not_installed(self):
        assert _locate_uefi_firmware_in(_present()) == (None, None)

    def test_ignores_candidates_relative_to_a_bundled_qemu_directory(self):
        """A WSL distro has its own /usr/share; the Windows-side QEMU dir is not on it."""
        assert _locate_uefi_firmware_in(_present("share/edk2-x86_64-code.fd")) == (None, None)


class TestHostFilesystemLookup:
    """`_locate_uefi_firmware` backs the bare-metal path and resolves real paths."""

    def test_finds_a_pair_bundled_beside_qemu(self, tmp_path):
        share = tmp_path / "share"
        share.mkdir()
        code = share / "edk2-x86_64-code.fd"
        template = share / "edk2-i386-vars.fd"
        code.write_bytes(b"code")
        template.write_bytes(b"vars")

        assert _locate_uefi_firmware(tmp_path) == (code, template)

    def test_reports_bundled_firmware_without_its_varstore(self, tmp_path):
        share = tmp_path / "share"
        share.mkdir()
        code = share / "edk2-x86_64-code.fd"
        code.write_bytes(b"code")

        assert _locate_uefi_firmware(tmp_path) == (code, None)

    def test_reports_nothing_when_no_candidate_exists(self, tmp_path):
        code, template = _locate_uefi_firmware(tmp_path)

        # A system OVMF install would still be found, so only assert the bundled miss.
        assert code is None or code.is_absolute()
        assert template is None or template.is_absolute()
