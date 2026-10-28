"""Data-parallel PyTorch with torchrun, one rank per computer.

    STORE=s3://bucket/runs cua run train.py::train --nodes 2   # e.g. on `cua cloud add aws --type g6.xlarge` hosts

NCCL and Gloo open ports of their own between ranks, so the ranks talk over the
hosts' own network (network="direct"), which needs dedicated computers in one
network: `cua cloud add aws` hosts in your VPC run each rank in the computer's own Space.
"""
import os

import cua

app = cua.App("torch-ddp")
image = cua.Image.python("3.12").uv_pip_install("torch==2.9.1", "numpy")


@app.function(image=image, nodes=(1, 2), accelerator="nvidia", kind="machine", network="direct",
              retries=3, timeout="1h", store=os.environ.get("STORE"))  # data_dir follows the run
def train(cluster: cua.Cluster):
    # torchrun gets --nnodes/--node_rank/--master_addr/--master_port from the cluster;
    # ddp.py checkpoints into data_dir and resumes from it.
    cluster.torchrun(["ddp.py", "--out", str(cluster.data_dir), "--steps", os.environ.get("STEPS", "300")])
    return cluster.data_dir / "model.pt" if cluster.rank == 0 else None
