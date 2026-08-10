from __future__ import annotations

import json
from pathlib import Path

import pytest
from release_backfill import (
    BackfillError,
    agent_environment,
    agent_request,
    apply_releases,
    author_batches,
    author_candidates,
    canonical_bytes,
    digest_text,
    digest_value,
    final_body,
    parse_agent_output,
    render_managed_block,
    reusable_candidates,
    rollback_releases,
    snapshot_payload,
    validate_agent_content,
    validate_documents,
    validate_json_schema,
    verify_execution_source,
)
from release_backfill_claude import claude_command


REPO_ROOT = Path(__file__).resolve().parents[3]


def fixture_documents():
    config = {"managedBlockVersion": 1}
    release = {
        "key": "cua-driver-rs:0.8.2",
        "releaseId": 7,
        "product": "cua-driver-rs",
        "displayName": "Cua Driver",
        "version": "0.8.2",
        "tag": "cua-driver-rs-v0.8.2",
        "tagSha": "a" * 40,
        "previousTag": "cua-driver-rs-v0.8.1",
        "previousTagSha": "b" * 40,
        "rangeKind": "continuous",
        "pathEraIds": ["driver-rs-current"],
        "paths": ["libs/cua-driver"],
        "excludePaths": ["libs/cua-driver/python"],
        "compareUrl": (
            "https://github.com/trycua/cua/compare/cua-driver-rs-v0.8.1...cua-driver-rs-v0.8.2"
        ),
        "publishedAt": "2026-07-15T00:00:00Z",
        "name": "cua-driver-rs-v0.8.2",
        "body": "Original notes.\n",
        "bodySha256": digest_text("Original notes.\n"),
        "draft": False,
        "prerelease": True,
        "assets": [
            {
                "id": 9,
                "name": "driver.tar.gz",
                "size": 10,
                "state": "uploaded",
                "digest": "sha256:abc",
                "updatedAt": "2026-07-15T00:00:00Z",
            }
        ],
    }
    release["snapshotSha256"] = digest_value(release)
    catalog = {
        "schemaVersion": 1,
        "kind": "release-backfill-catalog",
        "repository": "trycua/cua",
        "generatedAt": "2026-07-17T00:00:00Z",
        "releases": [release],
    }
    packet = {
        "key": release["key"],
        "catalogSnapshotSha256": release["snapshotSha256"],
        "sources": [
            {
                "id": "pr:12",
                "kind": "pull_request",
                "title": "fix(driver): preserve focus while reconnecting",
                "body": "Closes #7",
                "url": "https://github.com/trycua/cua/pull/12",
                "commitShas": ["c" * 40],
                "files": ["libs/cua-driver/rust/src/main.rs"],
                "contributors": [
                    {"login": "outside-author", "roles": ["author"], "external": True},
                    {"login": "maintainer", "roles": ["coauthor"], "external": False},
                ],
                "issues": [7],
            }
        ],
    }
    packet["evidenceSha256"] = digest_value(packet)
    evidence = {
        "schemaVersion": 1,
        "kind": "release-backfill-evidence",
        "repository": "trycua/cua",
        "generatedAt": "2026-07-17T00:00:00Z",
        "releases": [packet],
    }
    request = {
        "system": (
            "Write accurate historical release notes using only the supplied evidence. "
            "Return one JSON object with keys summary and changes. Each change must have "
            "type (feat, fix, perf, or revert), summary, and a non-empty evidence array. "
            "Use plain text without Markdown links, GitHub handles, or facts absent from the "
            "evidence. Prefer user-visible behavior. Omit maintenance-only sources unless "
            "they materially changed installation, compatibility, security, or reliability."
        ),
        "release": {
            "key": release["key"],
            "product": release["displayName"],
            "version": release["version"],
            "tag": release["tag"],
            "existingReleaseNotes": release["body"],
        },
        "sources": packet["sources"],
        "outputExample": {
            "summary": "This patch improves VM startup reliability.",
            "changes": [
                {
                    "type": "fix",
                    "summary": "Wait for the guest agent before reporting readiness",
                    "evidence": ["pr:1234"],
                }
            ],
        },
    }
    request = agent_request(release, packet)
    candidate = {
        "key": release["key"],
        "summary": "This patch preserves input focus during reconnects.",
        "summaryEvidence": ["pr:12"],
        "changes": [
            {
                "type": "fix",
                "summary": "Preserve input focus while reconnecting",
                "evidence": ["pr:12"],
            }
        ],
        "provenance": {
            "modelId": "fixture-agent",
            "promptVersion": 1,
            "promptSha256": digest_value(request),
            "evidenceSha256": packet["evidenceSha256"],
            "generatedAt": "2026-07-17T00:00:00Z",
        },
    }
    candidate["candidateSha256"] = digest_value(candidate)
    candidates = {
        "schemaVersion": 1,
        "kind": "release-backfill-candidates",
        "generatedAt": "2026-07-17T00:00:00Z",
        "promptVersion": 1,
        "candidates": [candidate],
    }
    skips = {
        "schemaVersion": 1,
        "kind": "release-backfill-skip-ledger",
        "generatedAt": "2026-07-17T00:00:00Z",
        "skips": [],
    }
    return config, catalog, evidence, candidates, skips


def test_candidate_validation_and_rendering_are_evidence_bound():
    config, catalog, evidence, candidates, skips = fixture_documents()
    rendered = validate_documents(config, catalog, evidence, candidates, skips)
    assert len(rendered) == 1
    block = rendered[0]["managedBlock"]
    assert "<!-- cua-release-backfill:v1:start" in block
    assert "## Summary" in block
    assert "## Fixes" in block
    assert "[#12](https://github.com/trycua/cua/pull/12)" in block
    assert "Thanks @outside-author." in block
    assert "@maintainer" not in block
    assert rendered[0]["finalBody"].startswith("Original notes.\n\n")

    invalid = json.loads(json.dumps(candidates))
    invalid["candidates"][0]["changes"][0]["evidence"] = ["pr:999"]
    invalid["candidates"][0]["candidateSha256"] = digest_value(
        {key: value for key, value in invalid["candidates"][0].items() if key != "candidateSha256"}
    )
    with pytest.raises(BackfillError, match="outside the release packet"):
        validate_documents(config, catalog, evidence, invalid, skips)


def test_managed_block_collision_fails_closed():
    with pytest.raises(BackfillError, match="already contains"):
        final_body(
            "<!-- cua-release-backfill:v1:start evidence-sha256=abc -->\nold",
            "new",
        )


def test_original_body_bytes_are_an_exact_prefix():
    original = "Original notes with trailing spaces.  \n"
    combined = final_body(original, "managed\n")
    assert combined.encode().startswith(original.encode())
    assert combined == original + "\nmanaged\n"


def test_fenced_agent_json_is_accepted_but_multiple_objects_are_rejected():
    value = parse_agent_output(
        '```json\n{"summary":"A","changes":[]}\n```\nA note outside the object.'
    )
    assert value["summary"] == "A"
    with pytest.raises(BackfillError, match="more than one"):
        parse_agent_output("```json\n{}\n```\n```json\n{}\n```")


def test_batched_agent_outputs_stay_keyed_to_each_evidence_packet(tmp_path: Path):
    config, catalog, evidence, _candidates, skips = fixture_documents()
    command = tmp_path / "agent.py"
    command.write_text(
        "import json,sys\n"
        "request=json.load(sys.stdin)\n"
        "key=request['release']['key']\n"
        "json.dump({'summary':'A verified fix.','summaryEvidence':['pr:12'],"
        "'changes':[{'type':'fix','summary':'Preserve focus',"
        "'evidence':['pr:12']}]},sys.stdout)\n"
    )
    generated, generated_skips = author_candidates(
        catalog, evidence, skips, ["python3", str(command)], "fixture-agent", 1
    )
    assert generated_skips["skips"] == []
    assert generated["candidates"][0]["key"] == "cua-driver-rs:0.8.2"


def test_evidence_backed_empty_changes_become_a_durable_skip(tmp_path: Path):
    _config, catalog, evidence, _candidates, skips = fixture_documents()
    command = tmp_path / "agent.py"
    command.write_text(
        "import json,sys\n"
        "json.load(sys.stdin)\n"
        "json.dump({'summary':'Internal maintenance only.',"
        "'summaryEvidence':['pr:12'],'changes':[]},sys.stdout)\n"
    )
    generated, generated_skips = author_candidates(
        catalog, evidence, skips, ["python3", str(command)], "fixture-agent", 1
    )
    assert generated["candidates"] == []
    assert generated_skips["skips"] == [
        {
            "key": "cua-driver-rs:0.8.2",
            "stage": "authoring",
            "reason": "no-product-changes",
            "details": "AI review found no user-visible change to publish; evidence: pr:12",
        }
    ]


def test_agent_content_allows_evidence_backed_no_product_changes():
    _config, _catalog, evidence, _candidates, _skips = fixture_documents()
    summary, references, changes = validate_agent_content(
        {
            "summary": "This release contains internal maintenance only.",
            "summaryEvidence": ["pr:12"],
            "changes": [],
        },
        evidence["releases"][0],
    )
    assert summary == "This release contains internal maintenance only."
    assert references == ["pr:12"]
    assert changes == []


def test_agent_environment_scrubs_github_credentials(monkeypatch):
    monkeypatch.setenv("GH_TOKEN", "secret-gh-token")
    monkeypatch.setenv("GITHUB_TOKEN", "secret-actions-token")
    monkeypatch.setenv("BACKFILL_CLAUDE_MODEL", "sonnet")
    environment = agent_environment()
    assert "GH_TOKEN" not in environment
    assert "GITHUB_TOKEN" not in environment
    assert environment["BACKFILL_CLAUDE_MODEL"] == "sonnet"


@pytest.mark.parametrize("forbidden", ["www.example.com", "[label]", "<tag>", "`code`"])
def test_agent_content_rejects_link_and_markup_shapes(forbidden):
    _config, _catalog, evidence, _candidates, _skips = fixture_documents()
    with pytest.raises(BackfillError, match="empty, too long, or contains"):
        validate_agent_content(
            {
                "summary": f"A release with {forbidden}",
                "summaryEvidence": ["pr:12"],
                "changes": [{"type": "fix", "summary": "Preserve focus", "evidence": ["pr:12"]}],
            },
            evidence["releases"][0],
        )


def test_author_batches_bound_release_count_sources_and_bytes():
    _config, catalog, evidence, _candidates, _skips = fixture_documents()
    release = catalog["releases"][0]
    packet = evidence["releases"][0]
    releases = {release["key"]: release}
    packets = [packet, packet, packet]
    batches = author_batches(
        packets,
        releases,
        max_releases=2,
        max_sources=2,
        max_bytes=1_000_000,
    )
    assert [len(batch) for batch in batches] == [2, 1]
    batches = author_batches(
        packets,
        releases,
        max_releases=10,
        max_sources=10,
        max_bytes=1,
    )
    assert [len(batch) for batch in batches] == [1, 1, 1]


def test_claude_author_uses_no_tools_without_plan_mode(tmp_path):
    command = claude_command("sonnet", tmp_path / "debug.log")
    assert command[command.index("--permission-mode") + 1] == "default"
    assert command[command.index("--tools") + 1] == ""
    assert command[command.index("--output-format") + 1] == "stream-json"


def test_resume_reuses_only_current_prompt_and_evidence_candidates():
    _config, catalog, evidence, candidates, _skips = fixture_documents()
    reusable, stale = reusable_candidates(catalog, evidence, candidates["candidates"])
    assert len(reusable) == 1
    assert stale == []

    changed = json.loads(json.dumps(candidates["candidates"]))
    changed[0]["provenance"]["promptVersion"] = 999
    changed[0]["candidateSha256"] = digest_value(
        {key: value for key, value in changed[0].items() if key != "candidateSha256"}
    )
    reusable, stale = reusable_candidates(catalog, evidence, changed)
    assert reusable == []
    assert stale == ["cua-driver-rs:0.8.2"]


class FakeMutationApi:
    def __init__(self, catalog_release):
        self.release_data = {
            "id": catalog_release["releaseId"],
            "tag_name": catalog_release["tag"],
            "name": catalog_release["name"],
            "body": catalog_release["body"],
            "draft": catalog_release["draft"],
            "prerelease": catalog_release["prerelease"],
        }
        self.tag_sha = catalog_release["tagSha"]
        self.assets = [
            {
                "id": item["id"],
                "name": item["name"],
                "size": item["size"],
                "state": item["state"],
                "digest": item["digest"],
                "updated_at": item["updatedAt"],
            }
            for item in catalog_release["assets"]
        ]
        self.patches = []

    def release(self, repository, release_id):
        assert repository == "trycua/cua"
        assert release_id == self.release_data["id"]
        return dict(self.release_data)

    def release_assets(self, repository, release_id):
        return list(self.assets)

    def tag_commit_sha(self, repository, tag):
        return self.tag_sha

    def patch_release_body(self, repository, release_id, body):
        self.patches.append({"body": body})
        self.release_data["body"] = body
        return dict(self.release_data)


def test_apply_is_body_only_idempotent_and_rollback_is_hash_guarded(tmp_path: Path):
    config, catalog, evidence, candidates, skips = fixture_documents()
    rendered = validate_documents(config, catalog, evidence, candidates, skips)
    release = catalog["releases"][0]
    api = FakeMutationApi(release)
    journal = tmp_path / "journal.jsonl"

    apply_releases(
        api=api,
        catalog=catalog,
        rendered=rendered,
        keys=[release["key"]],
        journal=journal,
        execute=False,
        pace_seconds=0,
    )
    assert api.patches == []
    assert not journal.exists()

    apply_releases(
        api=api,
        catalog=catalog,
        rendered=rendered,
        keys=[release["key"]],
        journal=journal,
        execute=True,
        pace_seconds=0,
    )
    assert api.patches == [{"body": rendered[0]["finalBody"]}]
    assert len(journal.read_text().splitlines()) == 2

    apply_releases(
        api=api,
        catalog=catalog,
        rendered=rendered,
        keys=[release["key"]],
        journal=journal,
        execute=True,
        pace_seconds=0,
    )
    assert len(api.patches) == 1

    rollback_releases(
        api=api,
        catalog=catalog,
        journal=journal,
        keys=[release["key"]],
        execute=True,
        pace_seconds=0,
    )
    assert api.release_data["body"] == release["body"]
    assert set(api.patches[0]) == {"body"}
    assert set(api.patches[1]) == {"body"}


def test_apply_recovers_verified_journal_after_post_patch_crash(tmp_path: Path):
    config, catalog, evidence, candidates, skips = fixture_documents()
    rendered = validate_documents(config, catalog, evidence, candidates, skips)
    release = catalog["releases"][0]
    payload = rendered[0]
    api = FakeMutationApi(release)
    api.release_data["body"] = payload["finalBody"]
    journal = tmp_path / "journal.jsonl"
    prepared = {
        "schemaVersion": 1,
        "operation": "apply",
        "timestamp": "2026-07-17T00:00:00Z",
        "repository": "trycua/cua",
        "key": release["key"],
        "releaseId": release["releaseId"],
        "tag": release["tag"],
        "preBody": release["body"],
        "preBodySha256": release["bodySha256"],
        "postBodySha256": payload["finalBodySha256"],
        "status": "prepared",
    }
    journal.write_text(json.dumps(prepared) + "\n")
    apply_releases(
        api=api,
        catalog=catalog,
        rendered=rendered,
        keys=[release["key"]],
        journal=journal,
        execute=True,
        pace_seconds=0,
    )
    entries = [json.loads(line) for line in journal.read_text().splitlines()]
    assert [entry["status"] for entry in entries] == ["prepared", "verified"]
    assert api.patches == []


def test_asset_or_flag_drift_prevents_apply(tmp_path: Path):
    config, catalog, evidence, candidates, skips = fixture_documents()
    rendered = validate_documents(config, catalog, evidence, candidates, skips)
    release = catalog["releases"][0]
    api = FakeMutationApi(release)
    api.assets[0]["size"] += 1
    with pytest.raises(BackfillError, match="state drift"):
        apply_releases(
            api=api,
            catalog=catalog,
            rendered=rendered,
            keys=[release["key"]],
            journal=tmp_path / "journal.jsonl",
            execute=True,
            pace_seconds=0,
        )
    assert api.patches == []


def test_execute_requires_exact_clean_approved_commit(tmp_path: Path):
    import subprocess

    subprocess.run(["git", "init"], cwd=tmp_path, check=True, capture_output=True)
    subprocess.run(["git", "config", "user.email", "release@example.com"], cwd=tmp_path, check=True)
    subprocess.run(["git", "config", "user.name", "Release Test"], cwd=tmp_path, check=True)
    (tmp_path / "tracked.txt").write_text("clean\n")
    subprocess.run(["git", "add", "tracked.txt"], cwd=tmp_path, check=True)
    subprocess.run(["git", "commit", "-m", "seed"], cwd=tmp_path, check=True, capture_output=True)
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=tmp_path, check=True, text=True, capture_output=True
    ).stdout.strip()
    verify_execution_source(tmp_path, head, True)
    with pytest.raises(BackfillError, match="approved-commit"):
        verify_execution_source(tmp_path, None, True)
    (tmp_path / "tracked.txt").write_text("dirty\n")
    with pytest.raises(BackfillError, match="clean approved worktree"):
        verify_execution_source(tmp_path, head, True)


def test_checked_in_backfill_artifacts_match_their_schemas():
    root = REPO_ROOT / ".github/release-backfill"
    for name in ("catalog", "evidence", "candidates", "skip-ledger", "rendered"):
        validate_json_schema(
            json.loads((root / f"{name}.json").read_text()),
            root / f"{name}.schema.json",
            name,
        )


def test_schema_validation_rejects_an_unexpected_artifact_field():
    _config, catalog, _evidence, _candidates, _skips = fixture_documents()
    catalog["unexpected"] = True
    with pytest.raises(BackfillError, match="catalog schema validation failed"):
        validate_json_schema(
            catalog,
            REPO_ROOT / ".github/release-backfill/catalog.schema.json",
            "catalog",
        )


def test_catalog_snapshot_hash_excludes_only_its_hash_field():
    _config, catalog, _evidence, _candidates, _skips = fixture_documents()
    release = catalog["releases"][0]
    assert digest_value(snapshot_payload(release)) == release["snapshotSha256"]
    assert canonical_bytes({"b": 1, "a": 2}) == b'{"a":2,"b":1}'
