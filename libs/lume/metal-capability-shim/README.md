# Lume Metal capability shim

This experimental, process-scoped shim changes selected Metal capability answers inside a macOS virtual machine running on Apple Silicon. GPU commands still use Apple’s paravirtualized graphics path. The shim does not pass a physical device to the guest, patch the host, or change the guest kernel.

The launch artifact is intentionally narrow. It can raise reported support for Apple GPU families through a configured ceiling and raise the threadgroup-memory limit. It does not alter the Common, Mac, or Metal family ranges, and it contains no private feature-profile hook, clock interposition, mesh-draw substitution, ray-tracing override, or pipeline-compilation fallback.

## Build

Build on Apple Silicon with Xcode Command Line Tools installed:

```bash
./Scripts/build.sh
./Scripts/verify.sh
```

The scripts produce `arm64` and `arm64e` dylibs, their `SHA256SUMS`, and a small capability probe under `dist/`. Record the Xcode, SDK, source revision, checkout path, and output hashes because toolchain and build-environment details can change the binary bytes.

The evidence-matched M1 Ultra/Tahoe release binaries use Command Line Tools 26.4. The clean source
revision and exact binary provenance are documented in
[Release/PROVENANCE.md](Release/PROVENANCE.md). To build and verify the source when the matching
toolchain is installed:

```bash
DEVELOPER_DIR=/Library/Developer/CommandLineTools ./Scripts/build.sh
./Scripts/verify.sh --no-build
```

`Scripts/package-release.sh ARTIFACT_DIR RELEASE_DIR` packages a verified binary set, the frozen
source archive, and the committed checksum manifest without overwriting an existing release output.
The release assets are intentionally not committed under `dist/`.

## Run one process

Choose the dylib matching the target process architecture. The tested profile uses Apple-family ceiling `1009` (Apple 9) and 64 KB of reported threadgroup memory:

```bash
DYLD_INSERT_LIBRARIES=/path/to/LumeMetalCapabilities-arm64.dylib \
LUME_METAL_APPLE_FAMILY_MAX=1009 \
./metal-capabilities 1009
```

`LUME_METAL_APPLE_FAMILY_MAX` is required. If it is absent, zero, outside the Apple-family range, or malformed, the library leaves the process unchanged. The other controls are optional:

| Variable                                  |   Default | Behavior                                                               |
| ----------------------------------------- | --------: | ---------------------------------------------------------------------- |
| `LUME_METAL_MAX_THREADGROUP_MEMORY`       |   `65536` | Raise reported maximum threadgroup memory to at least this many bytes. |
| `LUME_METAL_RECOMMENDED_WORKING_SET_SIZE` | unchanged | Raise the recommended working-set size only when explicitly set.       |

The shim only changes `supportsFamily:` answers in Apple's family range (`1001` through the configured ceiling). It preserves the device's original answers for Common, Mac, Metal, and unknown families. Use these controls only with a tested workload and host/guest combination. A reported capability does not prove that every Metal API using that capability works correctly.

## Remove

Remove `DYLD_INSERT_LIBRARIES` and the `LUME_METAL_*` variables from the workload environment, then restart that workload. The shim makes no persistent system change.

## Compatibility

This code relies on private, version-sensitive Metal implementation details in the macOS guest. Apple may change them in any macOS release. Keep activation scoped to one process, test each host/guest version independently, and treat a missing private class or method as unsupported. Do not broaden the profile by advertising `MTLGPUFamilyMetal3`: MLX-LM uses that answer to select residency sets, which the tested paravirtual device could not create.

## Evidence

- [TinyLlama and MLX-LM validation](../../../evidence/lume-metal-capability-shim/2026-08-09-m1-ultra)
- [Gemma 4 12B validation](../../../evidence/lume-metal-capability-shim/2026-08-10-m1-ultra-gemma4)

## License

MIT. See [LICENSE](LICENSE).
