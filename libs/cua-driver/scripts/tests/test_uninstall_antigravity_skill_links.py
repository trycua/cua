"""Release uninstall: Antigravity skill links are removed only when the driver owns them."""

import pytest


ANTIGRAVITY_SKILL_DIRS = [
    ".gemini/config/skills",
    ".gemini/antigravity/skills",
    ".gemini/antigravity-cli/skills",
]


@pytest.mark.parametrize("skills_dir", ANTIGRAVITY_SKILL_DIRS)
def test_driver_owned_antigravity_link_is_removed(release_install, skills_dir):
    f = release_install
    pack = f.home / ".cua-driver/skills/cua-driver"
    pack.mkdir(parents=True)
    link = f.home / skills_dir / "cua-driver"
    link.parent.mkdir(parents=True)
    link.symlink_to(pack)

    result = f.run()

    assert result.returncode == 0, result.stdout + result.stderr
    assert not link.is_symlink(), result.stdout
    assert pack.is_dir()


@pytest.mark.parametrize("skills_dir", ANTIGRAVITY_SKILL_DIRS)
def test_hand_managed_antigravity_link_is_preserved(release_install, tmp_path, skills_dir):
    f = release_install
    own_copy = tmp_path / "work/cua-driver-skill"
    own_copy.mkdir(parents=True)
    link = f.home / skills_dir / "cua-driver"
    link.parent.mkdir(parents=True)
    link.symlink_to(own_copy)

    result = f.run()

    assert result.returncode == 0, result.stdout + result.stderr
    assert link.is_symlink(), result.stdout
    assert link.resolve() == own_copy.resolve()
