---
name: cua-pool
description: Run GPU and batch work on the user's own computers (their pool) with `cua run` and `cua jobs`, from inside a Space, on any OS. Use for training, fine-tuning, evals and anything that needs a GPU or more than this Space has. Jobs belong to your Space: they end when you delete or stop it.
---

# GPU and batch jobs on the user's pool

Your Space is small and has no GPU. The user's computers do (NVIDIA on a PC or a
Linux box, Apple silicon on a Mac). `cua run` sends a Python function (or a
command) to the pool, runs it in a Space of its own on a computer that has what it
needs, streams its output here, and brings its result back. There is nothing to log
in to or configure, and it works the same whether your Space is Linux, Windows or
macOS.

## See what is there

```bash
cua computers     # each computer: OS, load, jobs, GPUs with free memory (nvidia:RTX PRO 6000:96 GB:80 GB free), the queue
```

## Run a function

```python
# train.py
import cua
app = cua.App("train")

@app.function(gpu="nvidia", gpu_memory="16GB", image=cua.Image.python().uv_pip_install("torch"),
              timeout="2h", retries=2)
def train():
    import torch
    print(torch.cuda.get_device_name(0), flush=True)
    ...
    return {"loss": 0.1}      # saved with the run as result.json
```

- `gpu`: `"nvidia"` (CUDA, PyTorch), `"apple"` (Metal, MLX; add `kind="machine"` so it runs on the Mac itself),
  `"any"`, or a model (`"L4"`, `"H100"`). `gpu_memory` is the video memory you need free: ask for what you need,
  not more; the pool places the job where it fits and queues it while the GPU is full.
- `image`: `cua.Image.python("3.12").uv_pip_install("torch", "numpy")`. The packages are cached on the computer
  between runs, so the second run starts fast.
- `on="local"` runs on the computer your Space is on; leave it out and the pool picks the best one.

```bash
cua run train.py::train               # follows the output; the result lands in ./cua-runs/<run>/
cua run train.py::train --detach      # prints the run id and returns
cua run --accelerator nvidia -- python script.py     # any command; this directory goes with it
```

## What comes back

Only what the function RETURNS comes back. Files it writes stay on the computer that ran it and are gone when the
run ends. To get files (weights, images), write them and return the path: `return "weights.npz"` brings that file
back, a directory path brings the directory, `bytes` come back as `result.bin`, and a dict as `result.json`.
Return the weights from the run that trains, so you never train twice. A function with one parameter is passed
the `cua.Cluster` (rank, world size); give it no parameters when you do not need that.

## Follow and finish

```bash
cua jobs                      # your runs
cua jobs <run>                # members, state, cost
cua jobs logs -f <run>        # live output
cua jobs result <run> -o out  # rank 0's return value, unpacked into out/
cua jobs cancel <run>
```

A run whose computer is lost is placed again and resumes from `cua.checkpoint.save(dir, step, files)`:
save one every few minutes for anything long and restore with `cua.checkpoint.latest(dir)`.

## Windows and macOS

Write paths with `pathlib`, not `/tmp` or `/`: a job's working directory is the current directory, and
`cua.Cluster.data_dir` is where files survive between attempts. A command after `--` runs in the job's
Space, which is Linux on a PC and a Mac VM or the Mac itself for `gpu="apple"`. In PowerShell write
`cua run train.py::train` exactly as above.

## Rules

- Your runs belong to your Space's tree and you see only them. When this Space is deleted or stopped its runs
  are canceled; cancel what you do not need and tell the user what is still running before you finish.
- To show the user a window, start the program in your Space (or the job's), then show it with the
  `stream_space_window` tool (or `show_space_pip` for the whole desktop): it opens on their screen.
- Say what you ran in one or two sentences: what it does, where it ran, how it went. Do not paste logs.
