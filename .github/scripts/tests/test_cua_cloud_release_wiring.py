

def test_docs_pin_dispatch_uses_the_release_app_not_aws():
    """The Copybara App behind AWS secret copybara/github-app has no installation,
    so the docs pin dispatch must mint its trycua/cloud token from the release App."""
    from pathlib import Path
    wf = (Path(__file__).resolve().parents[3] / ".github/workflows/cd-docs-pin-dispatch.yml").read_text()
    assert "secrets.RELEASE_APP_ID" in wf and "secrets.RELEASE_APP_PRIVATE_KEY" in wf
    assert "role-to-assume" not in wf and "id-token" not in wf
    assert "repositories: cloud" in wf and "permission-contents: write" in wf
