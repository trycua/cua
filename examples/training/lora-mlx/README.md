# LoRA fine-tune that survives losing a computer

`train.py` fine-tunes Qwen2.5-0.5B-Instruct (bf16) with LoRA on a small
dataset that teaches a distinctive answer style (short factual answers in a
pirate voice), so the before/after sample is obvious after a minute of
training. The training itself is stock `mlx_lm.lora` (run through
`lora_synced.py`, which adds a CPU meeting point before each gradient
exchange); the job file only picks the flags. The same script runs on Apple silicon (Metal) and on NVIDIA GPUs
(`mlx[cuda]` is picked on Linux hosts with an NVIDIA GPU).

```
cua run train.py::finetune                                    # one computer
STORE=s3://bucket/runs cua run train.py::finetune --nodes 2   # two computers
```

## What it shows

- **Data parallel with no code of your own.** `cluster.mlx_launch` runs
  `mlx_lm.lora` with `MLX_HOSTFILE`/`MLX_RANK` set, so `mx.distributed.init()`
  joins the gang's ring. mlx-lm 0.32.1 splits each batch across the ranks
  (`iterate_batches(comm_group=...)`) and averages gradients with
  `nn.utils.average_gradients` before each optimizer update.
- **Gradient accumulation.** `--batch-size 32 --grad-accumulation-steps 4`:
  each rank works on its half of 32 examples per step and the ranks exchange
  gradients once per 128 examples. Measured on two Macs over Wi-Fi, each rank
  in a macOS VM Space (mlx_lm.lora's own tokens/s, the gang's total): one Mac
  550-660, both 880 (about 1.35x). Syncing more often (batch 16, every 64
  examples) left two Macs at 680-745, barely ahead of one.
- **Ranks meet on the CPU before each update.** `lora_synced.py` runs
  `mlx_lm.lora` unchanged except for one tiny CPU all_sum before each
  gradient exchange. mlx_lm averages gradients inside a GPU step, so the rank
  that gets there first waits for the other inside a Metal command buffer,
  and macOS aborts any command buffer that waits 5 s
  (`kIOGPUCommandBufferCallbackErrorTimeout`), in a VM or not. Two fresh
  computers can reach the first update 5 s or more apart. Measured in a macOS
  VM Space: a rank held back 7 s before the first update made stock
  mlx_lm.lora fail exactly that way; with the CPU meeting it waited 6-29 s on
  the CPU and finished, at the same tokens/s
  ([CUA-1826](https://linear.app/trycua/issue/CUA-1826)). Checked end to end:
  the full two-Mac gang (each rank in a fresh macOS VM Space, 100 iterations,
  `retries=0`) finished on its first attempt three times in a row, at 730-800
  tokens/s against 800 for stock `mlx_lm.lora` on the same day.
- **Resume from data_dir.** `--adapter-path` lives under `cluster.data_dir`
  and `--save-every 50` writes `0000050_adapters.safetensors`, ... When the
  pool restarts the gang (a computer was lost, a rank failed), the function
  finds the newest saved adapter and passes `--resume-adapter-file`. Each leg
  writes to its own directory named by the iterations done before it, because
  mlx_lm.lora counts from 1 again when it resumes.
- **The store (`store=`).** mlx_lm.lora saves on rank 0 only. With
  `STORE=s3://bucket/prefix` (or `file:///shared/path`) the runtime restores
  every rank's data_dir from the run's newest snapshot before the function
  runs and syncs rank 0's changes in the background, so a gang placed on new
  computers starts from the same adapter on every rank. Without a store
  data_dir stays on each host, so the demo runs on one computer then. AWS
  credentials come from your shell (`AWS_ACCESS_KEY_ID`,
  `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`) or the host's instance role.
- **Output.** mlx_lm's own progress (rank 0), `cua: data_dir restored ...` /
  `cua: data_dir synced ...`, and at the end the base model's and the tuned
  model's answer to a prompt that is not in the training data. The returned
  directory (`adapters.safetensors` + `adapter_config.json`) lands in
  `./cua-runs/<run>/final/`.

## Files

| File | Purpose |
|---|---|
| `train.py` | the job (`finetune`) |
| `lora_synced.py` | `mlx_lm.lora` with the ranks meeting on the CPU before each gradient exchange |
| `train.jsonl`, `valid.jsonl` | 1600 training / 32 held-out chat examples (about 270 KB) |
| `gen_data.py` | regenerates the two JSONL files (deterministic) |

`ITERS` (400, a multiple of 50) sets the number of iterations: `cua run
train.py::finetune -e ITERS=800`.

## Using the adapter

```
mlx_lm.generate --model mlx-community/Qwen2.5-0.5B-Instruct-bf16 \
  --adapter-path cua-runs/<run>/final --prompt "What is the capital of Sweden?"
```

## Running without Cua

The job only needs the pure-Python runtime in
`libs/cua/crates/cua-cli/jobs-runtime`:

```
PYTHONPATH=libs/cua/crates/cua-cli/jobs-runtime \
CUA_JOB_DATA=$PWD/lora-data CUA_RESULT_PATH=$PWD/lora-result.tar.gz \
uv run --no-project --python 3.12 --with mlx==0.32.4 --with mlx-lm==0.32.1 \
  python -m _cua_entry train.py finetune
```

For two ranks start a second process with `CUA_RANK=1`, and give both
`CUA_WORLD_SIZE=2`, `CUA_PEERS=127.0.0.1:47100,127.0.0.1:47102`, the same
`CUA_RUN_ID` and `CUA_CHECKPOINT_URL=file:///abs/store`.

Keep module-level code in job files light: the CLI imports the file to read the
decorator spec, so heavy libraries are imported inside the function.

## Notes from testing

- NVIDIA hosts: the AWS Deep Learning AMI exports its own CUDA and cuDNN in
  `LD_LIBRARY_PATH`, which shadow the CUDA libraries inside the `mlx[cuda]`
  wheels (`CUDNN_STATUS_SUBLIBRARY_LOADING_FAILED`). `_cua_entry` drops
  `/usr/local/cuda*` entries from `LD_LIBRARY_PATH` and re-executes itself
  (`CUA_KEEP_LD_LIBRARY_PATH=1` opts out).
- Apple silicon: when the GPU is shared with other heavy work, macOS can abort a
  long Metal command buffer (`kIOGPUCommandBufferCallbackErrorTimeout`). That
  fails the attempt; the pool retries it and the job resumes.
