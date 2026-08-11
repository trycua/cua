# Apple Silicon and macOS VMs: 11–16× Faster LLM Inference with llama.cpp

_Published on August 11, 2026 by Francesco Bonacci and Johnny Franks_

If you've been following Cua from the start, you may remember that it began with a [Show HN](https://news.ycombinator.com/item?id=42908061) launch for Lume, our macOS virtualization stack.

Today, we're sharing the first result from a broader effort to connect that `Virtualization.framework` foundation to the local computer-use environments behind [Cua Driver](https://cua.ai/docs/tutorials/drive-your-first-app) and the infrastructure behind [Cua Cloud and Fleets](https://cua.ai/signup?redirect_url=%2Fwaitlist): a small, process-scoped compatibility layer that unlocks newer Metal fast paths inside a macOS guest.

We're releasing this work today as a research release under the same permissive license as Lume and Cua, so others can reproduce the results and help map which Apple Silicon chips, macOS releases, and Metal workloads benefit.

![Apple Silicon macOS VM LLM inference benchmark showing 7.2× faster prompt processing and 14.5× faster token generation.](https://github.com/user-attachments/assets/6e3aa770-d274-4c77-b99b-ae74668b5f5e)

Apple Vz users have been running into these limitations elsewhere too. Tart, another notable CLI built on Apple's `Virtualization.framework`, has an open [“No GPU passthrough in macOS guest?”](https://github.com/openai/tart/issues/1032) issue asking whether the framework can provide usable graphics and decent LLM performance in a macOS VM guest. The VM continues to use the virtual GPU that Apple provides. Our work exposes newer Metal paths on that device and closes part of the practical gap.

On an M1 Ultra, TinyLlama 1.1B running through llama.cpp processed prompts **11.08× faster** and generated tokens **16.36× faster** than the same workload in the same stock VM. Prompt processing reached 98% of our bare-metal result. The source, build scripts, capability probe, and raw benchmark logs are included so you can inspect and reproduce the result.

We repeated the experiment with Google's [Gemma 4 12B QAT Q4_0](https://huggingface.co/google/gemma-4-12B-it-qat-q4_0-gguf), a 6.98 GB model released this year. The same layer improved prompt processing **7.20×** and token generation **14.54×**. The unlocked VM reached 99.59% of bare-metal prompt speed and 94.82% of bare-metal generation speed.

## The cap inside a macOS VM

Apple's `Virtualization.framework` presents a macOS guest with a [virtual graphics device](https://developer.apple.com/documentation/virtualization/vzmacgraphicsdeviceconfiguration). The guest submits Metal work through a purpose-built GPU driver, and Apple's host stack executes it on the physical GPU. This arrangement is paravirtualization, where the host keeps control of the hardware and the guest uses a virtualization-aware device.

This differs from other virtualization stacks built on QEMU and KVM, which can use a different architecture. On x86 Linux hosts, [VFIO](https://www.kernel.org/doc/html/latest/driver-api/vfio.html) can assign a compatible physical PCI device or hardware function to a VM through an IOMMU, giving the guest direct access to that device. This is the model usually meant by GPU passthrough.

In our stock Tahoe VM, the paravirtualized device reported roughly an Apple 5-era family, 32 KB of maximum threadgroup memory, and SIMD-group matrix support as unavailable. Modern Metal software uses those answers to select kernels, so llama.cpp took a slower path even though the device could execute newer kernels.

Apple documents GPU capability through [GPU families and feature tables](https://developer.apple.com/metal/capabilities/) and recommends [querying the device at runtime](https://developer.apple.com/documentation/metal/detecting-gpu-features-and-metal-software-versions). That makes the reported capability boundary consequential: applications are doing exactly what the platform tells them to do.

![An illustrative Apple GPU capability ladder: the stock macOS guest reports an older capability band, while the tested profile exposes newer Metal paths including SIMD-group matrix operations, bfloat16, and 64 KB threadgroup memory.](https://github.com/user-attachments/assets/55b9d614-f94b-4840-b97c-ea5d51e595b5)

## The solution: a process-scoped Metal capability shim

We built a small Metal capability shim (a compatibility layer inserted between an application and an API) that runs inside one guest process. It intercepts selected Metal capability queries and changes the answers returned to that process. Metal applications use those answers to select kernels, so returning the tested Apple-family and threadgroup-memory values lets llama.cpp choose its newer GPU paths. For our tested profile, the shim:

- answers `supportsFamily:` through Apple family 9 (`1009`); and
- raises the reported maximum threadgroup memory from 32 KB to 64 KB.

That was enough for the tested llama.cpp build to select newer SIMD-group reduction, SIMD-group matrix, and bfloat16 paths:

| Capability                 | Stock guest | Tested profile |
| -------------------------- | ----------: | -------------: |
| `supportsFamily:1009`      |       false |       **true** |
| SIMD-group matrix          |         off |         **on** |
| SIMD-group reduction       |         off |         **on** |
| bfloat16                   |         off |         **on** |
| Maximum threadgroup memory |       32 KB |      **64 KB** |

The tested profile changes two reported values: Apple-family answers and the threadgroup-memory limit. Common, Mac, Metal, and working-set-size values keep their stock settings during the benchmark. We removed the original research hook's private feature-profile hook, clock and timing interposition, mesh substitution, ray-tracing override, argument-layout guard, and pipeline-compilation fallback. Its source is small enough to audit, and malformed or missing configuration keeps the process on its stock capability path.

<picture>
  <source media="(max-width: 600px)" srcset="https://github.com/user-attachments/assets/6c7e1a9e-bb01-4eba-887b-ff362bac340b">
  <img src="https://github.com/user-attachments/assets/34f15072-f18b-4747-9a98-6397601cbffc" alt="From conservative capability answers to faster Metal kernels: the host Apple GPU, Virtualization.framework bridge, and guest paravirtualized GPU stay unchanged while a process-scoped capability query selects either the stock Apple 5 and 32 KB path or the tested Apple 9 and 64 KB path.">
</picture>

The workload stays on Apple's `Virtualization.framework` graphics path and executes on the host's Apple GPU. The capability changes are scoped to the injected guest process.

Physical GPU assignment, raw PCI or VFIO passthrough, and kernel changes sit outside this mechanism. A reported family describes the paths covered by our tests; each additional Metal API requires separate validation.

The shim unlocks Metal capabilities on Apple's existing virtual GPU path. VM users often encounter the broader limitation under the name “GPU passthrough.”

## Fresh result from the minimal artifact

We tested on one Apple M1 Ultra with a 48-core GPU and macOS 26.6.1. The guest was the current public Tahoe Cua image (macOS 26.5.2, 8 vCPU, and 16 GiB) running in Lume 0.5.1. All three runs used the official llama.cpp `b10167` release and the same TinyLlama 1.1B Chat Q4_K_M model.

The command was:

```bash
llama-bench -m tinyllama-1.1b-chat-v1.0.Q4_K_M.gguf \
  -p 512 -n 128 -r 10 -t 8 -ngl -1 -o json
```

Values below are medians of the ten samples emitted for each benchmark row:

| Workload                      | Bare-metal host |  Stock guest | **Unlocked guest** | Guest speedup | Unlocked / host |
| ----------------------------- | --------------: | -----------: | -----------------: | ------------: | --------------: |
| Prompt processing, 512 tokens |  4,871.99 tok/s | 431.86 tok/s | **4,786.70 tok/s** |    **11.08×** |      **98.25%** |
| Token generation, 128 tokens  |    286.71 tok/s |  12.63 tok/s |   **206.60 tok/s** |    **16.36×** |      **72.06%** |

Prompt processing nearly reached the host result. Generation reached 72.06% of host speed, leaving a measurable VM gap. The gain depends on the host GPU, guest version, application, and workload shape.

The [TinyLlama raw results and environment record](https://github.com/trycua/cua/tree/main/evidence/lume-metal-capability-shim/2026-08-09-m1-ultra) include the exact image digest, model and binary hashes, commands, JSON output, stderr, and checksums. These release-candidate results certify the reduced shim used in this post.

### A current 12B model

TinyLlama makes a useful controlled benchmark because it runs quickly and exposes the Metal path clearly. We also wanted a larger model that developers might choose today, so we ran Google's official Gemma 4 12B instruction-tuned QAT Q4_0 GGUF through the same llama.cpp binary.

The host, VM, shim, benchmark shape, and ten-sample method stayed the same. We disabled speculative decoding and left the multimodal projector unloaded, keeping the comparison on the same Metal inference path:

| Workload                      | Bare-metal host | Stock guest | **Unlocked guest** | Guest speedup | Unlocked / host |
| ----------------------------- | --------------: | ----------: | -----------------: | ------------: | --------------: |
| Prompt processing, 512 tokens |    517.88 tok/s | 71.66 tok/s |   **515.76 tok/s** |     **7.20×** |      **99.59%** |
| Token generation, 128 tokens  |     52.38 tok/s |  3.41 tok/s |    **49.67 tok/s** |    **14.54×** |      **94.82%** |

The [Gemma 4 evidence](https://github.com/trycua/cua/tree/main/evidence/lume-metal-capability-shim/2026-08-10-m1-ultra-gemma4) pins Google's model revision and SHA-256 alongside the final raw samples. We discarded and reran a preliminary stock series after detecting another host compute workload. The retained stock, unlocked, and bare-metal files come from the same uncontended window and show tight sample ranges.

We also tested [MLX-LM](https://github.com/ml-explore/mlx-lm) 0.31.3 with `mlx-community/Llama-3.2-3B-Instruct-4bit` on MLX 0.32.0. Performance stayed flat because MLX-LM was already fast in the stock VM:

| Workload                      |    Stock guest | Unlocked guest |  Ratio |
| ----------------------------- | -------------: | -------------: | -----: |
| Prompt processing, 512 tokens | 1,656.55 tok/s | 1,665.47 tok/s | 1.005× |
| Token generation, 128 tokens  |   172.09 tok/s |   170.86 tok/s | 0.993× |

That flat result helped define the release profile. During ablation, advertising `MTLGPUFamilyMetal3` made MLX request a residency set unavailable through the paravirtualized device. The release shim limits changed answers to Apple-family enums and keeps Metal 3 at its stock value. The relevant MLX branch is visible in its [Metal residency implementation](https://github.com/ml-explore/mlx/blob/v0.32.0/mlx/backend/metal/resident.cpp).

## Where this sits with Apple's platform

This runs entirely on Apple hardware through the paravirtualized GPU path that Apple ships with `Virtualization.framework`. The shim affects selected values read by one guest process. The host, guest kernel, other guest processes, content-protection state, and licensing state keep their existing configuration.

The technique relies on private, version-sensitive behavior in the guest's Metal implementation. Apple may change it between macOS releases, so we test each host and guest combination independently. Unsupported methods keep the process on its stock path, and each additional API needs its own virtualization test.

We would welcome clarification from Apple on the intended behavior and supportability of the unrestricted feature level for paravirtualized graphics. Apple engineers working on Metal or `Virtualization.framework` can reach us at [vz@trycua.com](mailto:vz@trycua.com).

## Try it in a Lume VM

The source lives in [`libs/lume/metal-capability-shim`](https://github.com/trycua/cua/tree/main/libs/lume/metal-capability-shim). Build and verify both architecture-specific dylibs:

```bash
cd libs/lume/metal-capability-shim
./Scripts/build.sh
./Scripts/verify.sh
```

Stop the VM, enable the unrestricted feature level for VMs launched by your macOS user, and restart it:

```bash
lume stop my-vm
defaults write com.apple.gpusw.ParavirtualizedGraphics \
  ForceUnrestrictedDeviceFeatureLevel -bool true
lume run my-vm
```

Copy the matching dylib and the probe or workload into the guest, then scope activation to that process:

```bash
lume ssh my-vm \
  "DYLD_INSERT_LIBRARIES=/path/to/LumeMetalCapabilities-arm64.dylib \
   LUME_METAL_APPLE_FAMILY_MAX=1009 \
   /path/to/metal-capabilities 1009"
```

For a long-running inference server, renderer, or worker, use a per-workload LaunchAgent. Set `DYLD_INSERT_LIBRARIES` in that workload's environment so the login session remains stock. The [Lume guide](https://cua.ai/docs/how-to-guides/lume/gpu-passthrough) has a complete template, checksum and verification steps, and rollback instructions.

Removing the environment variables and restarting the workload returns it to stock behavior. To restore the host preference, stop the VM, delete `ForceUnrestrictedDeviceFeatureLevel`, and start the VM again.

## Limitations

- **Experimental and version-sensitive.** The shim uses private guest Metal implementation details that can change in any macOS release.
- **Per-process.** It affects only the injected workload and its children; hardened or platform-protected executables may reject library injection.
- **Configured capability profile.** It reports the Apple-family values covered by our tests. Physical-GPU capability discovery remains outside its scope.
- **Narrow validation.** The current evidence covers the capability probe, two llama.cpp workloads, and one MLX-LM compatibility run on the listed M1 Ultra host and Tahoe guest. Additional chips, guest releases, models, and Metal APIs need separate tests.
- **Still a VM.** Existing `Virtualization.framework` rendering and virtualization limits remain.

## Wrapping up

The guest's conservative answers hid a surprisingly capable GPU path. On our test machine, two narrowly scoped capability changes moved TinyLlama prompt processing from 432 to 4,787 tokens per second. With Gemma 4 12B, prompt processing moved from 71.66 to 515.76 tokens per second and generation from 3.41 to 49.67 while the workload stayed on Apple's existing GPU bridge.

Lume started as a way to make macOS VMs practical for developers. This result gives us a foundation to test across more Apple Silicon generations, guest releases, and Metal workloads.

Want to help? [Star Cua on GitHub](https://github.com/trycua/cua) and test the shim on your setup. [Open an issue](https://github.com/trycua/cua/issues/new/choose) with your host chip, host and guest versions, exact workload, and both stock and unlocked results. If you validate a new combination or improve the shim, [send a pull request](https://github.com/trycua/cua/pulls).
