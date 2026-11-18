"""A tiny DDP job: fit y = sin(x) with an MLP.  Plain PyTorch, run by torchrun.

Rank 0 writes ckpt.pt (model, optimizer, step) into --out every --save-every
steps, atomically; on start it loads the newest one and sends it to every
rank, so a restarted job continues where it stopped, on any number of ranks.
"""
import argparse
import os
import time

import torch
import torch.distributed as dist
import torch.nn as nn

p = argparse.ArgumentParser()
p.add_argument("--out", required=True)
p.add_argument("--steps", type=int, default=300)
p.add_argument("--save-every", type=int, default=50)
p.add_argument("--batch", type=int, default=256, help="global batch, split across ranks")
args = p.parse_args()

cuda = torch.cuda.is_available()
dist.init_process_group("nccl" if cuda else "gloo")
rank, world = dist.get_rank(), dist.get_world_size()
device = torch.device(f"cuda:{int(os.environ['LOCAL_RANK'])}" if cuda else "cpu")
if cuda:
    torch.cuda.set_device(device)
torch.manual_seed(0)

model = nn.Sequential(nn.Linear(1, 64), nn.Tanh(), nn.Linear(64, 64), nn.Tanh(), nn.Linear(64, 1)).to(device)
opt = torch.optim.Adam(model.parameters(), lr=3e-3)

# Resume: rank 0 reads the checkpoint, every rank gets the same state.
ckpt_path = os.path.join(args.out, "ckpt.pt")
state = [torch.load(ckpt_path, map_location="cpu") if rank == 0 and os.path.exists(ckpt_path) else None]
dist.broadcast_object_list(state, src=0)
start = 0
if state[0] is not None:
    model.load_state_dict(state[0]["model"])
    opt.load_state_dict(state[0]["opt"])
    start = state[0]["step"]
    if rank == 0:
        print(f"resumed from step {start} (world size {world})", flush=True)
ddp = nn.parallel.DistributedDataParallel(model)  # averages gradients across ranks

t0 = time.time()
for step in range(start + 1, args.steps + 1):
    g = torch.Generator().manual_seed(step * 1000 + rank)  # this rank's share of the global batch
    x = (torch.rand(args.batch // world, 1, generator=g) * 6 - 3).to(device)
    loss = nn.functional.mse_loss(ddp(x), torch.sin(x))
    opt.zero_grad()
    loss.backward()
    opt.step()
    if step % 25 == 0:
        dist.all_reduce(loss)
        if rank == 0:
            print(f"step {step} loss {loss.item() / world:.5f} ({time.time() - t0:.1f}s)", flush=True)
    if rank == 0 and (step % args.save_every == 0 or step == args.steps):
        tmp = ckpt_path + ".tmp"
        torch.save({"model": model.state_dict(), "opt": opt.state_dict(), "step": step}, tmp)
        os.replace(tmp, ckpt_path)  # a crash never leaves half a checkpoint
if rank == 0:
    torch.save(model.state_dict(), os.path.join(args.out, "model.pt"))
    print(f"done: {args.steps} steps on {world} rank(s), {'cuda' if cuda else 'cpu'}", flush=True)
dist.destroy_process_group()
