# M1 Ultra Gemma 4 validation, 2026-08-10

This evidence series tests the Apple-family-only shim with a current, larger model: Google's official
Gemma 4 12B instruction-tuned QAT Q4_0 GGUF. It uses the same M1 Ultra host, Tahoe guest, llama.cpp
release, benchmark shape, and safe shim as the TinyLlama launch evidence.

## Result

| Workload                    | Bare-metal host | Stock guest | Safe-shim guest | Guest speedup | Shim / host |
| --------------------------- | --------------: | ----------: | --------------: | ------------: | ----------: |
| Gemma 4 12B QAT Q4_0, pp512 |    517.88 tok/s | 71.66 tok/s |    515.76 tok/s |         7.20× |      99.59% |
| Gemma 4 12B QAT Q4_0, tg128 |     52.38 tok/s |  3.41 tok/s |     49.67 tok/s |        14.54× |      94.82% |

Values are medians of the ten `samples_ts` values emitted by `llama-bench` for each row.

The final sample ranges were:

| Workload          | Bare-metal host | Stock guest | Safe-shim guest |
| ----------------- | --------------: | ----------: | --------------: |
| Prompt processing |   516.33–518.29 | 69.94–72.77 |   514.14–516.54 |
| Token generation  |     51.92–52.52 |   3.37–3.50 |     49.52–49.79 |

## Measurement hygiene

A preliminary stock run overlapped with a separate host compute workload. Its prompt results were
lower and its generation samples ranged from 0.55 to 3.45 tok/s. We rejected that run before using
or copying it into the public evidence set. After the competing process exited, we reran the stock,
safe-shim, and bare-metal legs in the same uncontended window. The final files in this directory are
only from that window. macOS reported no thermal or performance warning during the final runs.

## Important scope

- The result validates one official Gemma 4 quantization through llama.cpp's Metal backend. It does
  not establish the same speedup for other Gemma variants, runtimes, context sizes, or modalities.
- Speculative decoding and the multimodal projector were deliberately excluded so the comparison
  isolates the same Metal capability path as the TinyLlama benchmark.
- The final guest runs use the evidence-matched Apple-family-only dylib at revision
  `5336ee9b61f35ffa058c746af237cb334e58bab9`.
- The safe-shim stderr records Apple family 9, SIMD-group matrix and reduction support, and bfloat16.
  The stock stderr records Apple family 5 with those fast paths disabled.

See [metadata.md](metadata.md) for exact environment, command, revision, and artifact hashes.
