# M1 Ultra validation, 2026-08-09

This is a fresh stock-versus-safe-shim validation. It is independent of the historical M5 Max benchmark set and must not be presented as a reproduction of the M5 absolute numbers.

## Result

| Workload                     | Bare-metal host |  Stock guest | Safe-shim guest | Guest speedup | Shim/host |
| ---------------------------- | --------------: | -----------: | --------------: | ------------: | --------: |
| TinyLlama 1.1B Q4_K_M, pp512 |  4,871.99 tok/s | 431.86 tok/s |  4,786.70 tok/s |        11.08× |    98.25% |
| TinyLlama 1.1B Q4_K_M, tg128 |    286.71 tok/s |  12.63 tok/s |    206.60 tok/s |        16.36× |    72.06% |

Values are medians of the ten `samples_ts` values emitted by `llama-bench` for each row.

The guest capability probe also changed as expected:

| Capability                 |        Stock |    Safe shim |
| -------------------------- | -----------: | -----------: |
| `supportsFamily:1009`      |      `false` |       `true` |
| Maximum threadgroup memory | 32,768 bytes | 65,536 bytes |

A malformed `LUME_METAL_APPLE_FAMILY_MAX` produced the stock result, confirming fail-closed activation.

## MLX-LM compatibility result

We also ran ten repetitions of MLX-LM 0.31.3 with MLX 0.32.0 and
`mlx-community/Llama-3.2-3B-Instruct-4bit` at the same 512-token prompt and 128-token generation
sizes:

| Workload          |    Stock guest | Safe-shim guest |  Ratio |
| ----------------- | -------------: | --------------: | -----: |
| Prompt processing | 1,656.55 tok/s |  1,665.47 tok/s | 1.005× |
| Token generation  |   172.09 tok/s |    170.86 tok/s | 0.993× |

The safe profile produced no material MLX-LM speed change, while confirming that MLX-LM remained
operational. A broader research profile that advertised `MTLGPUFamilyMetal3` failed during MLX
device initialization because the paravirtualized device could not create the residency set that
MLX requests on that path. That failure is why the release shim changes Apple-family answers only.

## Important scope

- This run proves the reduced shim activates the intended llama.cpp fast paths without a private feature-profile hook or the research hook’s timing, mesh, ray-tracing, argument-layout, or pipeline fallbacks.
- It does not validate every Metal feature or workload.
- The official `llama.cpp b10167` release binary used here has a different SHA-256 from the historical handoff binary. Keep this evidence series separate from the M5 tables.
- The final ten-repeat llama.cpp and MLX-LM runs use the Apple-family-only candidate at `5336ee9b61f35ffa058c746af237cb334e58bab9`. Earlier `tiny-hooked-minimal` and `candidate-hooked-smoke` files used the superseded broader profile and are retained only as development history; they are not headline evidence.

See [metadata.md](metadata.md) for the exact environment and hashes. Raw JSON and stderr are retained beside this file.
