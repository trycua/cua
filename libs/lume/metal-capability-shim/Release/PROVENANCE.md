# Release artifact provenance

This manifest describes the locally verified release candidate. It does not record or authorize a
published release.

## Source

- Frozen source revision: `1629eb71e78dd5682cf39a241ff68517438ea629`
- Source file SHA-256: `e1371b1e579bca895e6b3a2b581b9f328203d9786bf809912a4a5d1672010cce`
- Evidence build source revision: `5336ee9b61f35ffa058c746af237cb334e58bab9`
- Evidence build source file SHA-256: `a0d055caa64afe60d1139ded329d4062f271ab6f1a7f24e6f5738a6f3da4fd40`
- Build script SHA-256 at the frozen revision: `f60b741bca2005d22f4af5f1398614e4f5aa1d49171cc73cd5d1d20bf60cb264`
- Probe source SHA-256: `481dd635efcbee2b614b5ae47defa277a792cb91e3f99e5d0954400a5a787edc`
- Source archive: `git archive --format=tar.gz --prefix=cua-1629eb71/ 1629eb71e78dd5682cf39a241ff68517438ea629 libs/lume/metal-capability-shim`

The clean source revision carries the same executable statements as the evidence build used by the
M1 Ultra/Tahoe TinyLlama, MLX-LM, and Gemma 4 runs. The packaged binaries retain the exact
evidence-matched hashes recorded below.

## Toolchain and build

- Command Line Tools package: `com.apple.pkg.CLTools_Executables` version `26.4.0.0.1774242506`
- Compiler: Apple clang `21.0.0` (`clang-2100.0.123.102`)
- SDK: macOS `26.4`
- Deployment target: macOS `13.0`
- Linker recorded by `LC_BUILD_VERSION`: `1266.8`
- Build command: `DEVELOPER_DIR=/Library/Developer/CommandLineTools ./Scripts/build.sh <empty-output-directory>`

On 2026-08-10, two independently retained local artifact sets from the original 2026-08-09
certification matched the dylib hashes. The release package script creates the clean source archive
from the frozen Git object and rejects any binary or archive input that does not match
[SHA256SUMS](SHA256SUMS).

## Binary inspection

- `LumeMetalCapabilities-arm64.dylib`: thin `arm64`, 69,456 bytes, ad-hoc signed
- `LumeMetalCapabilities-arm64e.dylib`: thin `arm64e`, 69,456 bytes, ad-hoc signed
- Install name: `@rpath/LumeMetalCapabilities.dylib`
- Linkage: Foundation, Metal, Objective-C runtime, CoreFoundation, and libSystem
- Both dylibs record macOS SDK `26.4` and minimum macOS `13.0`
- The verification script rejects the research timing, mesh, broad-family, argument-layout, and
  pipeline-compatibility strings and verifies architecture, signatures, and checksums.

The dylibs are ad-hoc signed. They are not Developer ID signed or notarized, and this provenance
record must not be used to imply either property.
