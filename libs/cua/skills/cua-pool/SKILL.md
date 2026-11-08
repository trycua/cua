---
name: cua-pool
description: Run GPU and batch work on the user's own computers (their pool) with `cua run` and `cua jobs`, from inside a Space. Use for training, fine-tuning, evals and anything that needs a GPU or more than this Space has. Jobs are children of your Space: they end when you delete or stop it.
---

# GPU and batch jobs on the user's pool

Your Space is small and has no GPU. The user's computers do. `cua run` sends a
Python function (or a command) to the pool, runs it in a Space of its own on a
computer that has what it needs, streams its output here, and brings its result
back. Nothing to log in to or configure.

## See what is there

```bash
cua computers          # each computer: load, jobs, accelerators (e.g. nvidia:RTX PRO 6000:96 GB), the queue
```

## Run a function

```python
# train.py
import cua
app = cua.App("train")

@app.function(accelerator="nvidia", timeout="2h", retries=2)
def train():
    import torch
    print(torch.cuda.get_device_name(0), flush=True)
    ...                       # train; write files under the current directory
    return {"loss": 0.1}      # saved with the run
```

```bash
cua run train.py::train              # follows the output; the result lands in ./cua-runs/<run>/
cua run train.py::train --detach     # prints the run id and returns
cua run --accelerator gpu -- python script.py   # or any command; this directory goes with it
```

`accelerator` is `gpu` (any), `nvidia` or `apple`. `--nodes N` runs a gang of N.
A GPU is shared: ask for what you need, keep batch sizes modest, and expect to wait
in the queue when other work is using it (`cua computers` shows it).

## Follow and finish

```bash
cua jobs                       # your runs
cua jobs <run>                 # members, state, cost
cua jobs logs <run> -f         # live output
cua jobs result <run>          # rank 0's result
cua jobs cancel <run>
```

A run that is lost with its computer is placed again and resumes from
`cua.checkpoint.save(...)`; save one every few minutes for anything long.

## Rules

- Your runs belong to your Space's tree. You see only them. When this Space is
  deleted or stopped its runs are canceled, so cancel what you do not need and
  tell the user what is still running before you finish.
- Packages: a job starts from a plain Python image; `pip install` inside the
  function (or pass `image=cua.Image.python().pip_install(...)`).
- Say what you ran in one or two sentences: what it does, where it ran, how it
  went. Do not paste logs.
