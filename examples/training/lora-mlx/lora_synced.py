"""`mlx_lm.lora` (same flags), with the ranks meeting on the CPU before each gradient all_sum.

mlx_lm averages gradients inside the step it runs on the GPU, so a rank that gets there first
waits for the others inside a Metal command buffer, and macOS aborts a command buffer after
5 s (`kIOGPUCommandBufferCallbackErrorTimeout`). The first update of a run often comes 5 s or
more apart on two computers (fresh VMs compile their GPU kernels at different speeds), so that
rank failed and the gang restarted. Meeting first on the CPU stream, which may wait as long as
it takes, leaves only one step's difference for the GPU wait. It costs one tiny all_sum per
update (CUA-1826).
"""
import mlx.core as mx
import mlx_lm.lora as lora
from mlx_lm.tuner import trainer

_train = lora.train


def _meet_before_updates(accum, base):
    def batches(*args, **kw):
        gen = base(*args, **kw)
        world = mx.distributed.init()
        if not kw.get("loop") or world.size() == 1:  # evaluation, or a single rank
            yield from gen
            return
        for i, batch in enumerate(gen, 1):
            if i % accum == 0:  # this iteration's step averages the gradients
                mx.distributed.all_sum(mx.array(1.0), stream=mx.cpu).item()
            yield batch
    return batches


def _synced_train(*args, **kw):
    kw["iterate_batches"] = _meet_before_updates(
        kw["args"].grad_accumulation_steps, kw.get("iterate_batches", trainer.iterate_batches))
    return _train(*args, **kw)


lora.train = _synced_train

if __name__ == "__main__":
    lora.main()
