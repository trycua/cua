# Environment and provenance

## Host and guest

- Host: `Mac13,2`, Apple M1 Ultra, 48-core GPU, 20 CPU cores, 128 GiB memory
- Host macOS: 26.6.1 (`25G76`), Darwin 25.6.0
- Guest: `VirtualMac2,1`, 8 vCPU, 64 GiB memory, 150 GiB virtual disk
- Guest macOS: 26.5.2 (`25F84`), Darwin 25.5.0
- Guest image manifest:
  `sha256:ed1783e80e08e888889b54a8c0387105a37fbcfc995079f76db48e2280081487`
- Lume CLI used for final inspection: 0.5.3
- Host thermal status: no thermal or performance warning during the final arms

## Model

- Official repository: `meta-models/Muse-Glimmer-30B-GGUF`
- Pinned revision: `a0532f7263ee67f1e0a5f5c5fdcd50dd62fc9aa4`
- File: `muse-glimmer-30B-kquant-17gb.gguf`
- Download size: 16,756,681,056 bytes
- SHA-256: `7e9b74b7c8875e9e265695df9613bf6290f2392e479ce740495a129019c488d8`
- Parsed architecture: `muse-glimmer`
- Parsed parameter count: 27,854,794,240
- Parsed quantization: GGUF V3, Q4_K Medium
- Text-only run: no multimodal projector or drafter was loaded

## Runtime and shim

- Official llama.cpp release: `b10359`
- Build commit: `84f712946729f8517c972da4eb80db810ffe3210`
- Release archive SHA-256: `8eb447e8e972fff6818ea9ac7db9af270ee1d4ced46d2a7eaf918a5a0c0c8ffe`
- `llama-bench` SHA-256: `8a0ea51a40658436b4ec34a3f3ada1085f5feaa1d049e87ee7205f725a689f69`
- Safe shim SHA-256: `515b9d84ab67e86d959282e95c194a5ad8a947261514b947d6f7483c57797b31`
- Stock capability probe: Apple family 1009 unsupported, 32,768-byte maximum threadgroup memory
- Unlocked capability probe: Apple family 1009 supported, 65,536-byte maximum threadgroup memory

## Benchmark protocol

The public path-normalized commands preserve the executable arguments used in the guest:

```sh
./llama-bench -m ./muse-glimmer-30B-kquant-17gb.gguf \
  -p 512 -n 0 -r 3 -t 8 -ngl -1 -o json

./llama-bench -m ./muse-glimmer-30B-kquant-17gb.gguf \
  -p 0 -n 128 -r 3 -t 8 -ngl -1 -o json
```

The unlocked environment added:

```sh
DYLD_INSERT_LIBRARIES=./LumeMetalCapabilities-arm64.dylib
LUME_METAL_APPLE_FAMILY_MAX=1009
```

- Prompt processing and token generation ran in separate fresh processes.
- llama-bench's default built-in same-process warmup remained enabled and is excluded from the
  three recorded samples.
- Every arm used eight threads and full GPU offload.
- Stock and unlocked executable arguments were identical for each workload.
- All arms exited with status 0.
- Before and after every arm, the guest reported 98% memory free, zero swap, and zero compressor use.

## Measurement boundary

A separate VM on the same host showed intermittent CPU activity during the final series. It was not
stopped or modified. Aggregate host GPU telemetry stayed at 0-5% during stock pp512. It measured
22-27% during the steady stock tg128 interval, which includes the target Metal workload and cannot be
assigned per process by that counter. The pp512 samples were tight. The stock tg128 median remained
within 5.6% of an earlier independent run, while both unlocked rows had sub-0.4% sample spans.

## Public sanitization

This directory is a minimal public subset of a larger private capture. It excludes VM names, host
process lists, absolute machine paths, hardware identifiers, and unrelated diagnostic runs. In the
four llama-bench JSON files, only `model_filename` changed from the absolute guest path to the model
basename. The numeric output is unchanged. In unlocked stderr, the timestamp and guest process ID
were removed from the shim activation line. No result, capability marker, or timing value changed.
