# Your first verified runtime trial

In this tutorial, we will install the runtime, run the synthetic lifecycle
task, and verify its recorded evidence.

## What you will build

You will create one local trial directory containing frozen inputs, a
hash-chained event log, evaluator output, cleanup state, and a final result.
The local adapter tests the runtime lifecycle. It does not certify desktop or
driver behavior.

## Before you start

- Use a POSIX shell from the repository root.
- Install Python 3.11 or newer.
- Start from a clean repository checkout.

## Step 1: Install the runtime

Create a virtual environment and install the runtime with its pinned schema
dependencies:

```console
python3 -m venv .venv
.venv/bin/python -m pip install -e libs/cua-bench-runtime
```

Confirm that the command is available:

```console
.venv/bin/cdb --help
```

The output should list `validate`, `run`, `explain`, `export-trial`, and
`report`.

## Step 2: Validate the synthetic task

```console
.venv/bin/cdb validate --kind task \
  libs/cua-bench-runtime/conformance/tasks/synthetic-echo-v1/task.cuabench.json
```

The command should exit successfully without changing the repository.

## Step 3: Run the trial

Create a fresh output root, then run the reference agent:

```console
tutorial_root="$(mktemp -d)"
.venv/bin/cdb run \
  --task libs/cua-bench-runtime/conformance/tasks/synthetic-echo-v1/task.cuabench.json \
  --agent libs/cua-bench-runtime/conformance/agents/reference_ok.py \
  --out "$tutorial_root" \
  --trial-id first-trial
```

The final line should report a completed trial and the path
`$tutorial_root/first-trial`.

## Step 4: Verify the evidence

```console
.venv/bin/cdb explain --verify-only "$tutorial_root/first-trial"
```

The verifier checks the immutable config, input-manifest binding, event chain,
participation receipt, evaluation, and cleanup result. It should exit
successfully.

Now print the human-readable explanation:

```console
.venv/bin/cdb explain "$tutorial_root/first-trial"
```

Notice that task outcome, driver participation, and certification appear as
separate decisions. The headless task can complete without a GUI participation
requirement.

## What you built

You ran one complete runtime lifecycle and verified its evidence before reading
the result. You can repeat the tutorial because each run starts in a new output
directory.

## Next steps

- [Run and verify a task trial](../how-to/run-and-verify-a-trial.md)
- [Read the trial directory reference](../reference/trial-artifacts.md)
- [Understand the four benchmark decisions](../explanation/outcome-participation-certification-and-comparison.md)
