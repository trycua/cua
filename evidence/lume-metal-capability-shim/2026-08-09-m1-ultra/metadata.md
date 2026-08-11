# Environment and provenance

## Host

- Model: `Mac13,2`
- Chip: Apple M1 Ultra, 48-core GPU
- CPU cores: 20
- Memory: 128 GiB
- macOS: 26.6.1 (`25G76`)
- Metal: Metal 4

## Guest

- Image reference: `ghcr.io/trycua/macos-tahoe-cua:latest`
- Pulled manifest: `sha256:ed1783e80e08e888889b54a8c0387105a37fbcfc995079f76db48e2280081487`
- macOS: 26.5.2 (`25F84`)
- Virtual hardware: `VirtualMac2,1`
- CPU: 8 vCPU
- Memory: 16 GiB
- SIP: disabled in the published test image
- Lume: 0.5.1, official Developer ID-signed and notarized release

## Workload

- `llama.cpp`: official release `b10167`, build commit `ee3d1b54c`
- `llama-bench` SHA-256: `28faa552714d0d8150b1daa4e991181d62084c88e25afe9d49d08351794f0da7`
- Model: `tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf`
- Model SHA-256: `9fecc3b3cd76bba89d504f29b616eedf7da85b96540e490ca5824d3f7d2776a0`
- Command: `llama-bench -p 512 -n 128 -r 10 -t 8 -ngl -1 -o json`

### MLX-LM compatibility run

- Python: 3.12.13
- MLX: 0.32.0
- MLX-LM: 0.31.3
- Model: `mlx-community/Llama-3.2-3B-Instruct-4bit`
- Model revision: `7f0dc925e0d0afb0322d96f9255cfddf2ba5636e`
- Configuration: 512 prompt tokens, 128 generated tokens, 10 repetitions, seed 42, EOS stopping disabled
- Harness SHA-256: `1daee0e796a62744c1288ea77b29de07d6f2a92434a35c5cf1b261c725aacee4`

## Shim

- Safe-profile artifact revision: `5336ee9b61f35ffa058c746af237cb334e58bab9`
- Source SHA-256: `a0d055caa64afe60d1139ded329d4062f271ab6f1a7f24e6f5738a6f3da4fd40`
- Build script SHA-256: `f60b741bca2005d22f4af5f1398614e4f5aa1d49171cc73cd5d1d20bf60cb264`
- Verification script SHA-256: `806896603e01be50a9c3e74ac3d87cc1f8ac59699a853f5949b188b40daa0b7a`
- Capability probe source SHA-256: `481dd635efcbee2b614b5ae47defa277a792cb91e3f99e5d0954400a5a787edc`
- Capability probe binary SHA-256: `6e0f26c6c138b4c31379fb659cb5748d98db993d4d19c8db0f0e4a44ed714396`
- `arm64` dylib SHA-256: `515b9d84ab67e86d959282e95c194a5ad8a947261514b947d6f7483c57797b31`
- `arm64e` dylib SHA-256: `62504be341ed74191eab22f19e21d99b2afbe6cc029902c760578ffbbd0588cb`
- Build toolchain: Command Line Tools `26.4.0.0.1774242506`, Apple clang `21.0.0` (`clang-2100.0.123.102`), macOS SDK `26.4`
- Binary load-command metadata: deployment target `13.0`, macOS SDK `26.4`, linker `1266.8`
- Activation: `DYLD_INSERT_LIBRARIES=<arm64 dylib> LUME_METAL_APPLE_FAMILY_MAX=1009`

The exact artifact pair was recovered from two independently retained local build sets. A clean
rebuild from the frozen source revision using the pinned Command Line Tools 26.4 installation also
reproduced both dylibs and the capability probe byte-for-byte on 2026-08-10. The committed release
manifest and provenance record define the local release-asset set.

The safe-profile capability probe reported `supports_family=true` for family `1009` and 65,536 bytes
of maximum threadgroup memory. `tiny-safe-shim-10x-final.json`, `mlx-stock-10x.json`, and
`mlx-safe-shim-10x.json` are the sources for the published medians. The broad-profile MLX failure is
retained in `mlx-broad-profile-failure.stderr` as the negative control that motivated the narrower
family policy.
