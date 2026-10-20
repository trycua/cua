# OSWorld 2 + Cua Driver Fleet template plan

## Goal

Run a paired OSWorld 2 browser-use ablation on one reusable Fleet VM contract
without leaking credentials, gated task contents, or evaluator data.

## Definition of done

- Cua Cloud builds a release-pinned OSWorld 2 container disk containing the
  pinned Cua Driver release and provisions it only by immutable digest.
- The VM preflight proves the native screenshot, non-empty accessibility tree,
  pinned Driver executable, and guest-local Chrome CDP listener.
- Harbor runs one control trial and one Driver treatment trial serially against
  one Fleet slot, with identical task, model, prompt, and turn budget.
- The official evaluator scores both trials and the result records exact image,
  Driver, task, model-route, and runner revisions.

## Repository ownership

- **Cua:** observation/action policy, release manifest, pilot tooling, and
  Driver-side evidence contract in this directory.
- **Cua Cloud:** private ECR repository, image builder, immutable template
  renderer, claim preflight, and Fleet lifecycle.
- **Harbor:** private-source OSWorld 2 adapter and the paired
  screenshot-control/Cua-Driver-treatment runner.

Gated task modules, assets, evaluator credentials, and generated private Harbor
datasets stay outside all three source trees.

## Execution order

1. Merge and apply the private Cloud ECR declaration.
2. Manually dispatch the Cloud image build. It verifies the official OSWorld 2
   qcow2 and Cua Driver archives, installs Driver into the guest, pushes the
   container disk, and reports its manifest digest.
3. Render the Cloud template with that exact digest and run its one-VM
   preflight. Chrome CDP remains guest-local on `127.0.0.1:1337`; only the
   OSWorld control service is exposed.
4. Generate one private Harbor task from the pinned gated OSWorld checkout.
5. Run Harbor with `n_concurrent_trials: 1`: screenshot-only control first,
   screenshot plus Cua Driver treatment second.
6. Verify cleanup and record the paired evaluator outcomes and provenance.

## First result and expansion

The first paired task is an integration proof, not a publishable benchmark
claim. After it passes, freeze a browser-heavy subset before looking at
treatment outcomes and run multiple attempts per task. Report success rate,
steps, wall time, tokens, retries, tool refusals, and categorized failures.

## Confirmed pilot evidence

The recovered disposable pilot already proved one OSWorld 2 VM with native
screenshots, a non-empty Linux accessibility tree, exact Chrome binding, CDP
semantics/actions, and an official task score of `1.0`. That run used the
combined mode interactively; it does not establish a control-versus-treatment
benchmark result.

## Remaining proof

The derived image build, immutable template preflight, and paired Harbor run
remain unproven on the exact reconciled commits. Do not publish benchmark
claims until those three gates pass and their artifacts are retained.
