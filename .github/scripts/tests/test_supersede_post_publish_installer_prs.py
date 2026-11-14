import pytest

from supersede_post_publish_installer_prs import parse_version, select_superseded


def pr(number, version, branch=None):
    return {
        "number": number,
        "title": f"chore(cua-driver): advance published installer version to {version}",
        "headRefName": branch or f"post-publish-installer-{number}-1",
    }


def numbers(prs):
    return [p["number"] for p in prs]


def test_parse_version_is_numeric_not_lexical():
    assert parse_version("0.49.10") > parse_version("0.49.9")
    assert parse_version("1.0") is None
    assert parse_version("0.49.7-rc1") is None


def test_older_and_equal_versions_are_superseded():
    open_prs = [pr(1, "0.38.0"), pr(2, "0.49.6"), pr(3, "0.49.7"), pr(9, "0.49.7")]
    assert numbers(select_superseded(open_prs, 9, "0.49.7")) == [1, 2, 3]


def test_semver_compare_not_string_compare():
    open_prs = [pr(1, "0.49.9"), pr(2, "0.49.10")]
    assert numbers(select_superseded(open_prs, 9, "0.49.10")) == [1, 2]
    assert numbers(select_superseded(open_prs, 9, "0.49.9")) == [1]


def test_newer_pr_is_kept():
    assert select_superseded([pr(1, "0.50.0")], 9, "0.49.7") == []


def test_new_pr_never_closes_itself():
    assert select_superseded([pr(9, "0.49.7")], 9, "0.49.7") == []


def test_unrelated_prs_are_left_alone():
    other_branch = pr(1, "0.40.0", branch="feature/x")
    other_title = {**pr(2, "0.40.0"), "title": "fix: something"}
    assert select_superseded([other_branch, other_title], 9, "0.49.7") == []


def test_invalid_new_version_is_rejected():
    with pytest.raises(ValueError):
        select_superseded([], 9, "latest")
