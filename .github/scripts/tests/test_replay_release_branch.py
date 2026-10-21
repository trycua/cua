import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parents[1]
SB = "libs/python/cua-sandbox"


def git(cwd, *args, check=True):
    env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@t")
    return subprocess.run(["git", *args], cwd=cwd, env=env, check=check, capture_output=True, text=True)


def write(repo, rel, text):
    path = repo / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


class ReplayReleaseBranch(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q", "-b", "main")
        for name in ("replay_release_branch.sh", "merge_release_please_manifests.py"):
            dest = self.repo / ".github/scripts" / name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(SCRIPTS / name, dest)
        write(self.repo, ".release-please-manifest.json", json.dumps({SB: "0.9.1", "libs/cua": "0.5.0"}, indent=2) + "\n")
        write(self.repo, "docs/gen.mdx", "version 0.9.1\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "base")
        git(self.repo, "checkout", "-qb", "release")
        write(self.repo, ".release-please-manifest.json", json.dumps({SB: "0.9.2", "libs/cua": "0.5.0"}, indent=2) + "\n")
        git(self.repo, "commit", "-qam", "chore(main): release sandbox 0.9.2")
        write(self.repo, "docs/gen.mdx", "version 0.9.2\n")
        git(self.repo, "commit", "-qam", "chore(sandbox): synchronize release metadata")
        git(self.repo, "checkout", "-q", "main")

    def advance_main(self, manifest, doc):
        write(self.repo, ".release-please-manifest.json", json.dumps(manifest, indent=2) + "\n")
        write(self.repo, "docs/gen.mdx", doc)
        git(self.repo, "commit", "-qam", "main moves")
        # the workflow's remote refs
        git(self.repo, "update-ref", "refs/remotes/origin/main", "main")
        git(self.repo, "update-ref", "refs/remotes/origin/release", "release")
        git(self.repo, "checkout", "-q", "release")

    def run_script(self):
        return subprocess.run(
            [str(self.repo / ".github/scripts/replay_release_branch.sh"), "origin/release", "--component", "sandbox"],
            cwd=self.repo, capture_output=True, text=True,
            env=dict(os.environ, RUNNER_TEMP=str(self.tmp), GIT_EDITOR="true",
                     GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@t", GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@t"),
        )

    def test_manifest_and_generated_commit_both_conflict(self):
        # main bumped another component AND regenerated the same doc.
        self.advance_main({SB: "0.9.1", "libs/cua": "0.6.0"}, "version 0.9.1 and more\n")
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        manifest = json.loads((self.repo / ".release-please-manifest.json").read_text())
        self.assertEqual(manifest, {SB: "0.9.2", "libs/cua": "0.6.0"})
        # the generated commit was dropped; main's doc stays for the caller to regenerate
        self.assertEqual((self.repo / "docs/gen.mdx").read_text(), "version 0.9.1 and more\n")
        self.assertEqual(git(self.repo, "log", "--format=%s", "origin/main..HEAD").stdout.strip(),
                         "chore(main): release sandbox 0.9.2")

    def test_real_conflict_fails(self):
        self.advance_main({SB: "0.9.1", "libs/cua": "0.6.0"}, "x\n")
        # a hand edit in the release commit that conflicts with main outside the manifest
        git(self.repo, "checkout", "-q", "release")
        git(self.repo, "reset", "-q", "--hard", "HEAD~1")
        write(self.repo, "README.md", "release edit\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "chore(main): release sandbox 0.9.2 edit")
        git(self.repo, "checkout", "-q", "main")
        write(self.repo, "README.md", "main edit\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "main readme")
        git(self.repo, "update-ref", "refs/remotes/origin/main", "main")
        git(self.repo, "update-ref", "refs/remotes/origin/release", "release")
        git(self.repo, "checkout", "-q", "release")
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("conflicts beyond", result.stdout + result.stderr)
        self.assertFalse((self.repo / ".git/rebase-merge").exists())


if __name__ == "__main__":
    unittest.main()
