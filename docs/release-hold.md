# How to hold and release a release PR

Release Please PRs merge themselves once ci-gate is green (`release-pr-auto-merge.yml`).
To make one wait, for example for a real-Mac test gate, label it.

## Hold

Add the `release-hold` label to the release PR.

- The auto-merge workflow switches GitHub auto-merge off and never turns it on while the label is present.
- `ci-gate`, a required check, fails while the label is present, so nothing can merge the PR, including a person or a bot that enables auto-merge by hand.

Turning auto-merge off by hand is not a hold. The workflow turns it back on at the next PR update or sweep (every 30 minutes). That is how #6749 (cua-spaces 0.22.0) merged while it was meant to wait.

## Release

Remove the label. ci-gate reruns and goes green, and auto-merge is enabled at the next run. To merge now, run the `Release: Auto-merge release PRs` workflow from the Actions tab, or merge the PR after ci-gate passes.

`hold` is the older name of the label and still works.
