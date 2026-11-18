"""LoRA fine-tune with mlx_lm.lora on every computer you give it.

    cua run train.py::finetune                                   # one computer
    STORE=s3://bucket/runs cua run train.py::finetune --nodes 2  # a gang: data_dir follows the run
"""
import os
import sys
import subprocess

import cua

app = cua.App("lora-demo")
image = cua.Image.python("3.12").uv_pip_install("mlx==0.32.4", "mlx-lm==0.32.1")
MODEL = "mlx-community/Qwen2.5-0.5B-Instruct-bf16"
ITERS = int(os.environ.get("ITERS", "400"))  # a multiple of --save-every
STORE = os.environ.get("STORE")  # read where you run `cua run`
# mlx_lm.lora saves on rank 0 only, so a gang needs the shared store: then every
# rank starts from the same data_dir.  Without one, data_dir stays on each host.


@app.function(image=image, nodes=(1, 2) if STORE else 1, accelerator="gpu", retries=3, timeout="2h", store=STORE)
def finetune(cluster: cua.Cluster):
    runs = cluster.data_dir / "adapters"
    # mlx_lm.lora counts iterations from 1 again when it resumes, so each leg
    # writes to its own dir named by the iterations done before it.
    saves = [(int(p.parent.name) + int(p.name[:7]), p) for p in runs.glob("*/[0-9]*_adapters.safetensors")]
    done, resume = max(saves, default=(0, None))
    leg = runs / f"{done:07d}"
    if done < ITERS:
        print(f"training iterations {done + 1}..{ITERS}" + (f", resuming {resume.name}" if resume else ""))
        # mlx_lm.lora (here with the ranks meeting on the CPU before each gradient exchange, so a
        # Mac's GPU never waits 5 s for the other rank: lora_synced.py) splits each batch across
        # the ranks and averages gradients
        cluster.mlx_launch([
            os.path.join(os.path.dirname(__file__), "lora_synced.py"), "--model", MODEL, "--train", "--data", os.path.dirname(__file__),
            # Adam's state is not saved; a resumed leg steps gently so a fresh Adam keeps the adapter on course.
            "--learning-rate", "5e-5" if resume else "2e-4", "--mask-prompt", "--num-layers", "12",
            "--batch-size", "32", "--grad-accumulation-steps", "4",  # ranks sync once per 128 examples
            "--iters", str(ITERS - done), "--save-every", "50", "--seed", str(done),
            "--adapter-path", str(leg), *(["--resume-adapter-file", str(resume)] if resume else [])])
    else:
        leg = max(p for p in runs.iterdir() if p.name.isdigit())  # already trained: the last leg
    if cluster.rank != 0:
        return None  # mlx_lm.lora saves on rank 0; its result comes back to you
    for name, adapter in (("BEFORE", []), ("AFTER", ["--adapter-path", str(leg)])):
        out = subprocess.run([sys.executable, "-m", "mlx_lm", "generate", "--model", MODEL, "--max-tokens", "40",
                              "--verbose", "False", "--prompt", "Who wrote Hamlet?", *adapter],
                             capture_output=True, text=True)
        print(name, out.stdout.strip() or out.stderr.strip()[-500:])
    final = cluster.data_dir / "final"  # the adapter and its config: what `cua run` saves for you
    final.mkdir(exist_ok=True)
    for f in ("adapters.safetensors", "adapter_config.json"):
        (final / f).write_bytes((leg / f).read_bytes())
    return final
