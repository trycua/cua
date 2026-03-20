"""Tinker checkpoint management and vLLM weight synchronisation.

Two responsibilities — kept together because both are about writing model
weights out of Tinker and delivering them somewhere:

``save``         — persist full training state (weights + optimiser) to a
                   named Tinker checkpoint so training can be resumed later.

``sync_to_vllm`` — push the latest LoRA adapter weights to a running vLLM
                   server so the next rollout round uses the updated policy.

Weight sync flow
----------------
1. Call ``training_client.save_weights_for_sampler`` to create a lightweight
   weights-only checkpoint on the Tinker side (faster than ``save_state``).
2. Obtain a signed download URL via ``RestClient``.
3. Stream-download the tar archive and extract the LoRA adapter directory.
4. POST the adapter path to vLLM's ``/load_lora_adapter`` endpoint.
   vLLM must be started with ``--enable-lora`` for this to work.

Note: vLLM's dynamic LoRA API does not require a server restart; the adapter
is hot-loaded into the running process and takes effect on the next request.
"""

from __future__ import annotations

import tarfile
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING

import requests

if TYPE_CHECKING:
    from tinker import TrainingClient


# ---------------------------------------------------------------------------
# Checkpoint persistence
# ---------------------------------------------------------------------------


def save(training_client: "TrainingClient", name: str) -> str:
    """Save full training state (weights + optimiser) to a named checkpoint.

    Use this at the end of every N epochs so training can be resumed with
    ``service_client.create_training_client_from_state_with_optimizer``.

    Parameters
    ----------
    training_client:
        Active Tinker ``TrainingClient`` with the weights to persist.
    name:
        Human-readable checkpoint label (e.g. ``"epoch-5"``).  The returned
        tinker:// URI embeds this name and can be passed to ``load_state``
        or shared with colleagues.

    Returns
    -------
    A ``tinker://`` path string identifying the checkpoint.
    """
    path: str = training_client.save_state(name=name).result()
    print(f"[checkpoint] State saved: {path}")
    return path

