import cua

app = cua.App("nca")


@app.function(gpu="apple", kind="machine", timeout="3m",
              image=cua.Image.python().uv_pip_install("mlx", "numpy", "pillow", "resvg_py"))
def train():
    import io, time, pathlib
    import numpy as np
    import mlx.core as mx
    import mlx.optimizers as optim
    import resvg_py
    from mlx.utils import tree_map
    from PIL import Image

    N, C, H, STEPS, BATCH, POOL, BUDGET = 32, 16, 96, 40, 16, 1024, 85

    png = bytes(resvg_py.svg_to_bytes(svg_path="logo.svg", width=28, height=28))
    im = np.array(Image.open(io.BytesIO(png)).convert("RGBA")).astype(np.float32) / 255
    tgt = np.zeros((N, N, 4), np.float32)
    tgt[2:30, 2:30] = im
    tgt[..., :3] *= tgt[..., 3:]
    target = mx.array(tgt)
    ys, xs = np.nonzero(tgt[..., 3] > 0.5)
    k = np.argmin((ys - N / 2) ** 2 + (xs - N / 2) ** 2)
    sy, sx = int(ys[k]), int(xs[k])
    seed_np = np.zeros((N, N, C), np.float32)
    seed_np[sy, sx, 3:] = 1.0
    seed = mx.array(seed_np)

    def sobel(x):
        p = mx.pad(x, [(0, 0), (1, 1), (1, 1), (0, 0)], mode="edge")
        n = x.shape[1]
        r = lambda dy, dx: p[:, dy:dy + n, dx:dx + n]
        gx = (r(0, 2) + 2 * r(1, 2) + r(2, 2) - r(0, 0) - 2 * r(1, 0) - r(2, 0)) / 8
        gy = (r(2, 0) + 2 * r(2, 1) + r(2, 2) - r(0, 0) - 2 * r(0, 1) - r(0, 2)) / 8
        return mx.concatenate([x, gx, gy], axis=-1)


    def alive(x):
        a = mx.pad(x[..., 3:4], [(0, 0), (1, 1), (1, 1), (0, 0)], mode="edge")
        n = x.shape[1]
        a = mx.maximum(mx.maximum(a[:, :, 0:n], a[:, :, 1:n + 1]), a[:, :, 2:n + 2])
        a = mx.maximum(mx.maximum(a[:, 0:n], a[:, 1:n + 1]), a[:, 2:n + 2])
        return a > 0.1


    mx.random.seed(0)
    params = {"w1": mx.random.normal((3 * C, H)) * (2 / (3 * C)) ** 0.5, "b1": mx.zeros((H,)), "w2": mx.zeros((H, C))}

    def step(p, x, m):
        x = x + (mx.maximum(sobel(x) @ p["w1"] + p["b1"], 0) @ p["w2"]) * m
        return x * alive(x)


    def loss_fn(p, x0, masks):
        x = x0
        for t in range(STEPS):
            x = step(p, x, masks[t])
        return ((x[..., :4] - target) ** 2).mean(), mx.stop_gradient(x)

    vg = mx.value_and_grad(loss_fn)

    def train_step(p, x0, masks):
        (loss, x), g = vg(p, x0, masks)
        return loss, x, tree_map(lambda v: v / (mx.sqrt((v * v).sum()) + 1e-8), g)

    train_step = mx.compile(train_step)
    opt = optim.Adam(learning_rate=2e-3)
    model = None

    pool = np.tile(seed_np[None], (POOL, 1, 1, 1))
    t0, it = time.time(), 0
    while time.time() - t0 < BUDGET:
        idx = np.random.choice(POOL, BATCH, replace=False)
        x0 = pool[idx]
        err = ((x0[..., :4] - tgt) ** 2).mean((1, 2, 3))
        x0[err.argmax()] = seed_np
        if it == 1500:
            opt.learning_rate = 6e-4
        masks = (mx.random.uniform(shape=(STEPS, BATCH, N, N, 1)) < 0.5).astype(mx.float32)
        loss, x, g = train_step(params, mx.array(x0), masks)
        params = opt.apply_gradients(g, params)
        mx.eval(params, opt.state, loss, x)
        pool[idx] = np.array(x)
        it += 1
        if it % 25 == 0:
            print(it, round(time.time() - t0), round(loss.item(), 5), flush=True)

    x = seed[None]
    for _ in range(96):
        m = (mx.random.uniform(shape=(1, N, N, 1)) < 0.5).astype(mx.float32)
        x = step(params, x, m)
    rgba = np.clip(np.array(x[0, ..., :4]), 0, 1)
    a = rgba[..., 3:]
    comp = rgba[..., :3] + (1 - a)
    out = pathlib.Path("out")
    out.mkdir(exist_ok=True)
    Image.fromarray((np.clip(comp, 0, 1) * 255).astype(np.uint8)).resize((256, 256), Image.BICUBIC).save(out / "grown.png")
    np.savez(out / "weights.npz", **{k: np.array(v) for k, v in params.items()})
    print("iters", it, "final", loss.item(), flush=True)
    return str(out)
