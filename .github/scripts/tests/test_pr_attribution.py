from __future__ import annotations

import pytest

from release_attribution import ReleaseError, source_pull_numbers, validate_pr_attribution


def config(**overrides):
    value = {
        "bots": ["github-actions[bot]"],
        "coauthorOverrides": {},
        "ignoredCoauthorEmails": ["noreply@anthropic.com"],
        "identityOverrides": {},
        "internalHandles": ["maintainer"],
        "optOutHandles": [],
    }
    value.update(overrides)
    return value


def pull(*, body: str = "", login: str = "landing-author", number: int = 50):
    return {"number": number, "body": body, "user": {"login": login}}


def commit(
    *,
    sha: str = "abc123",
    name: str = "Contributor",
    email: str = "contributor@example.com",
    login: str | None = None,
    committer_email: str | None = None,
    committer_login: str | None = None,
    message: str = "fix: preserve attribution",
):
    return {
        "sha": sha,
        "author": {"login": login} if login else None,
        "committer": {"login": committer_login} if committer_login else None,
        "commit": {
            "author": {"name": name, "email": email},
            "committer": {
                "name": "Landing Author",
                "email": committer_email or email,
            },
            "message": message,
        },
    }


class SourceGitHub:
    def __init__(self, authors, source_emails):
        self.authors = authors
        self.source_emails = source_emails

    def pull(self, repository: str, number: int):
        assert repository == "trycua/cua"
        return {"number": number, "user": {"login": self.authors[number]}}

    def pull_commits(self, repository: str, number: int):
        assert repository == "trycua/cua"
        return [
            {"commit": {"author": {"email": email}}} for email in self.source_emails.get(number, [])
        ]


def validate(
    *,
    pull_value=None,
    commits=None,
    base=None,
    head=None,
    authors=None,
    source_emails=None,
):
    base = base or config()
    validate_pr_attribution(
        repository="trycua/cua",
        pull=pull_value or pull(),
        commits=commits or [],
        base_config=base,
        head_config=head or base,
        github=SourceGitHub(authors or {}, source_emails or {}),
    )


def test_resolvable_github_and_noreply_identities_pass():
    validate(
        commits=[
            commit(
                login="landing-author",
                email="work@example.edu",
                message=(
                    "fix: pair safely\n\n"
                    "Co-authored-by: Pair User "
                    "<456+second-user@users.noreply.github.com>"
                ),
            ),
        ]
    )


def test_distinct_resolvable_commit_author_requires_preserved_source():
    external = commit(login="source-author", email="source@institution.example")
    with pytest.raises(ReleaseError, match="distinct from landing PR author"):
        validate(commits=[external])

    validate(
        pull_value=pull(body="Salvaged from #12"),
        commits=[external],
        authors={12: "source-author"},
        source_emails={12: ["source@institution.example"]},
    )


def test_explicit_cherry_pick_source_reference_is_parsed_but_supersedes_is_not():
    assert source_pull_numbers(
        "Cert discovery (from #2280, thanks — cherry-picked to preserve authorship)",
        "trycua/cua",
    ) == [2280]
    assert source_pull_numbers(
        "The identity is verified by [PR #2280](https://github.com/trycua/cua/pull/2280)",
        "trycua/cua",
    ) == [2280]
    assert source_pull_numbers("Supersedes #2280", "trycua/cua") == []


def test_direct_contributor_with_unlinked_email_does_not_fail():
    validate(
        pull_value=pull(login="direct-contributor"),
        commits=[commit(email="private@institution.example")],
    )

    validate(
        pull_value=pull(
            login="direct-contributor",
            body="Based on #99 for the API shape",
        ),
        commits=[commit(email="private@institution.example")],
    )


def test_unlinked_author_committed_by_landing_author_requires_source_evidence():
    with pytest.raises(ReleaseError, match="has no explicit same-repository source PR"):
        validate(
            commits=[
                commit(
                    email="source@university.example",
                    committer_email="landing@example.com",
                    committer_login="landing-author",
                )
            ]
        )


def test_unresolved_institutional_coauthor_requires_source_reference():
    with pytest.raises(ReleaseError, match="has no explicit same-repository source PR"):
        validate(
            commits=[
                commit(
                    message=(
                        "fix: preserve work\n\n"
                        "Co-authored-by: External Person <person@university.example>"
                    )
                )
            ]
        )


def test_verified_source_pr_produces_exact_override_and_accepts_it():
    landing = pull(
        body="Cert discovery (from #2280, thanks — cherry-picked to preserve authorship)",
        number=2360,
    )
    commits = [
        commit(
            sha="a27753e",
            name="Source Contributor",
            email="source@university.example",
            committer_email="landing@example.com",
            committer_login="landing-author",
        )
    ]
    with pytest.raises(ReleaseError) as error:
        validate(
            pull_value=landing,
            commits=commits,
            authors={2280: "source-login"},
            source_emails={2280: ["source@university.example"]},
        )
    assert '"identityOverrides": {\n    "source@university.example": "source-login"\n  }' in str(
        error.value
    )

    head = config(identityOverrides={"source@university.example": "source-login"})
    validate(
        pull_value=landing,
        commits=commits,
        head=head,
        authors={2280: "source-login"},
        source_emails={2280: ["source@university.example"]},
    )


def test_ambiguous_source_pr_authors_fail_closed():
    with pytest.raises(ReleaseError, match="ambiguous source-PR authors"):
        validate(
            pull_value=pull(body="Salvaged from #10; based on #11"),
            commits=[
                commit(
                    email="source@university.example",
                    committer_email="landing@example.com",
                    committer_login="landing-author",
                )
            ],
            authors={10: "first-author", 11: "second-author"},
            source_emails={
                10: ["source@university.example"],
                11: ["source@university.example"],
            },
        )


def test_source_pr_author_without_exact_email_is_not_mapping_evidence():
    with pytest.raises(ReleaseError, match="not an exact commit-author email"):
        validate(
            pull_value=pull(body="Salvaged from #10"),
            commits=[
                commit(
                    email="fabricated@institution.example",
                    committer_email="landing@example.com",
                    committer_login="landing-author",
                )
            ],
            authors={10: "source-author"},
            source_emails={10: ["actual@institution.example"]},
        )


def test_mapping_only_override_requires_and_accepts_exact_source_evidence():
    head = config(identityOverrides={"historic@institution.example": "historic-author"})
    with pytest.raises(ReleaseError, match="has no explicit same-repository source PR"):
        validate(head=head)

    validate(
        pull_value=pull(
            body=("The identity is verified by [PR #20](https://github.com/trycua/cua/pull/20)")
        ),
        head=head,
        authors={20: "historic-author"},
        source_emails={20: ["historic@institution.example"]},
    )


def test_existing_identity_override_cannot_be_removed_or_changed():
    base = config(identityOverrides={"known@institution.example": "known-author"})
    with pytest.raises(ReleaseError, match="removes or changes trusted identityOverrides"):
        validate(base=base, head=config())
    with pytest.raises(ReleaseError, match="removes or changes trusted identityOverrides"):
        validate(
            base=base,
            head=config(identityOverrides={"known@institution.example": "other-author"}),
        )


def test_internal_bot_and_ignored_coauthors_are_excluded():
    validate(
        commits=[
            commit(
                login="maintainer",
                message=(
                    "fix: generated and maintained\n\n"
                    "Co-authored-by: Maintainer <123+maintainer@users.noreply.github.com>\n"
                    "Co-authored-by: Actions "
                    "<github-actions[bot]@users.noreply.github.com>\n"
                    "Co-authored-by: Claude <noreply@anthropic.com>"
                ),
            ),
            commit(
                sha="bot123",
                name="Actions",
                email="github-actions[bot]@users.noreply.github.com",
                login="github-actions[bot]",
            ),
        ]
    )


def test_coauthor_trailer_parsing_is_case_insensitive_and_multiline():
    with pytest.raises(ReleaseError, match="pair@institution.example"):
        validate(
            commits=[
                commit(
                    message=(
                        "fix: parse trailers\n\n"
                        "co-AUTHORED-by: Pair Person <pair@institution.example>\n"
                        "Signed-off-by: Landing Author <landing@example.com>"
                    )
                )
            ]
        )
