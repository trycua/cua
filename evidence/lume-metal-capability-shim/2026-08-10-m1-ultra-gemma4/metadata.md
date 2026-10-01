# Environment and provenance

## Host

- Model: `Mac13,2`
- Chip: Apple M1 Ultra, 48-core GPU
- CPU cores: 20
- Memory: 128 GiB
- macOS: 26.6.1 (`25G76`)
- Metal: Metal 4
- Thermal state during final runs: no thermal or performance warning reported

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
- Model repository: `google/gemma-4-12B-it-qat-q4_0-gguf`
- Model revision: `29d097773436b69ff9feafd636ab4cf873786537`
- Model file: `gemma-4-12b-it-qat-q4_0.gguf`
- Model file size: 6,975,879,296 bytes
- Model SHA-256: `93567e57a8fe10b23569b9d9ec38cd005deedf71e29477c421a4b83f418a538b`
- Command: `llama-bench -p 512 -n 128 -r 10 -t 8 -ngl -1 -o json`
- Speculative decoding: disabled
- Multimodal projector: not loaded

## Shim

- Safe-profile artifact revision: `5336ee9b61f35ffa058c746af237cb334e58bab9`
- Source SHA-256: `a0d055caa64afe60d1139ded329d4062f271ab6f1a7f24e6f5738a6f3da4fd40`
- Build script SHA-256: `f60b741bca2005d22f4af5f1398614e4f5aa1d49171cc73cd5d1d20bf60cb264`
- Verification script SHA-256: `806896603e01be50a9c3e74ac3d87cc1f8ac59699a853f5949b188b40daa0b7a`
- Capability probe source SHA-256: `481dd635efcbee2b614b5ae47defa277a792cb91e3f99e5d0954400a5a787edc`
- Capability probe binary SHA-256: `6e0f26c6c138b4c31379fb659cb5748d98db993d4d19c8db0f0e4a44ed714396`
- `arm64` dylib SHA-256: `515b9d84ab67e86d959282e95c194a5ad8a947261514b947d6f7483c57797b31`
- `arm64e` dylib SHA-256: `62504be341ed74191eab22f19e21d99b2afbe6cc029902c760578ffbbd0588cb`
- Benchmarked `arm64` binary load-command metadata: deployment target `13.0`, macOS SDK `26.4`
- Byte-identical reproduction toolchain: Command Line Tools `26.4.0.0.1774242506`, Apple clang `21.0.0` (`clang-2100.0.123.102`), linker `1266.8`
- A clean 2026-08-10 rebuild from the frozen source using the recovered toolchain reproduced both dylibs and the capability probe byte-for-byte
- A current SDK `26.5` rebuild remains a different, uncertified artifact
- Activation: `DYLD_INSERT_LIBRARIES=<arm64 dylib> LUME_METAL_APPLE_FAMILY_MAX=1009`

The final evidence uses the exact same safe-profile dylib hashes as the 2026-08-09 TinyLlama and
MLX-LM evidence series.
