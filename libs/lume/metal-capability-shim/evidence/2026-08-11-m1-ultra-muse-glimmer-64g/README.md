# M1 Ultra Muse Glimmer 30B validation, 2026-08-11

This evidence set measures Meta's official Muse Glimmer 30B Q4_K-M GGUF in a 64 GiB macOS Tahoe
guest on an M1 Ultra host. It compares stock and process-scoped unlocked Metal capability profiles
through llama.cpp b10359.

## Results

| Workload                       | Stock guest median | Unlocked guest median | Speedup |     Stock range |  Unlocked range |
| ------------------------------ | -----------------: | --------------------: | ------: | --------------: | --------------: |
| Muse Glimmer 30B Q4_K-M, pp512 |      25.8328 tok/s |         194.971 tok/s |   7.55x | 25.7641-26.0987 | 194.565-195.331 |
| Muse Glimmer 30B Q4_K-M, tg128 |      2.37551 tok/s |         21.0823 tok/s |   8.87x | 2.14729-2.41391 | 21.0729-21.0954 |

Each value is the median of three `samples_ts` measurements. llama-bench's default built-in warmup
ran in each process and is excluded from the samples.

## Method

Prompt processing and generation ran in separate fresh processes. Both profiles used eight threads,
full GPU offload, and identical executable arguments:

```sh
./llama-bench -m ./muse-glimmer-30B-kquant-17gb.gguf \
  -p 512 -n 0 -r 3 -t 8 -ngl -1 -o json

./llama-bench -m ./muse-glimmer-30B-kquant-17gb.gguf \
  -p 0 -n 128 -r 3 -t 8 -ngl -1 -o json
```

The unlocked processes alone received:

```sh
DYLD_INSERT_LIBRARIES=./LumeMetalCapabilities-arm64.dylib
LUME_METAL_APPLE_FAMILY_MAX=1009
```

All four arms exited successfully. Guest memory telemetry reported 98% free memory, zero swap, and
zero compressor use before and after each arm. Stock stderr records Apple family 5 with SIMD-group
reduction, SIMD-group matrix multiplication, and bfloat disabled. Unlocked stderr records shim
activation and Apple family 9 with those paths enabled.

A separate VM had intermittent CPU activity during the run. We retained this disclosure because
the host was shared. Stock pp512 host GPU telemetry stayed between 0% and 5%, and its three samples
spanned 1.30% of the median. Stock tg128 samples had a wider 11.22% span and agreed within 5.6% of an
earlier independent run. The unlocked sample spans were 0.39% for pp512 and 0.11% for tg128.

## Files

- `clean-*.json`: final llama-bench JSON. Public sanitization changed only `model_filename` from an
  absolute guest path to its basename; numeric output is unchanged.
- `clean-*.stderr`: Metal initialization and capability markers. Public sanitization removed guest
  timestamps and process IDs from the two shim activation lines.
- `results.csv`: samples, medians, ranges, and ratios.
- `telemetry.csv`: wall time, exit status, guest memory summary, and aggregate host GPU range.
- `metadata.md`: environment, hashes, commands, scope, and sanitization record.
- `SHA256SUMS`: checksums for every file except the manifest itself.

## Scope

These measurements cover one text-only Muse Glimmer quantization through llama.cpp on the listed
host and guest. No multimodal projector or drafter was loaded. The results are not Ollama throughput
and do not establish performance for other models, quantizations, runtimes, hosts, or guest releases.
