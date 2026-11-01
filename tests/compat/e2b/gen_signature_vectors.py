"""Regenerates fixtures/signature-vectors.json from the installed e2b SDK's
get_signature (the Rust side checks its signer against these).

    python gen_signature_vectors.py > fixtures/signature-vectors.json
"""

import json
import random
import time

import e2b

rng = random.Random(1729)
alpha = "abcdefghijklmnopqrstuvwxyz0123456789/._- :%&?#é日"
out = []
for i in range(60):
    path = "/" + "".join(rng.choice(alpha) for _ in range(rng.randint(0, 40)))
    user = rng.choice(["", "user", "root", "ünï"])
    token = "".join(rng.choice("abcdef0123456789") for _ in range(rng.randint(8, 64)))
    op = rng.choice(["read", "write"])
    exp = None if i % 3 == 0 else rng.randint(1, 100000)
    s = e2b.get_signature(path, op, user or None, token, exp)
    expiration = s["expiration"]
    out.append(
        {
            "path": path,
            "operation": op,
            "user": user,
            "token": token,
            "expiration": expiration,
            "signature": s["signature"],
        }
    )
print(json.dumps({"sdk": e2b.__name__, "vectors": out}, indent=1, ensure_ascii=False))
