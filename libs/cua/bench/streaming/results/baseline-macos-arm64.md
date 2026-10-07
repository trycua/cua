# Streaming benchmark

- git: `3301ee317eac`
- host: arch=aarch64, cpus=18, docker_containers_running_at_start=2, driver_override=~/projects/wt-bench/libs/images/cua-desktop-linux/dist/arm64/cua-spacesd, note=runsc sidecar lanes (WS+QUIC) re-run after fixing sidecar addressing under gVisor; same image, driver and seconds, os=macos
- image: `space.sh default (cua-e2e-local/cua-desktop-linux:docker-local-<arch>)`
- 20 s per run

| run | enc | TTFF ms | fps | int p50/p95 ms | g2g p50/p95 ms | in→photon p50 ms | video KB/s | cli CPU % | srv CPU % | notes |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| `runc/ws/videotoolbox/rust/static` | cua-media-codec | 161 | 0.1 | 1454.2/1454.2 | –/– | 13.6 | 0.1 | 0.2 | 1.3 | decode p50 62.6 ms; container CPU 3% |
| `runc/ws/videotoolbox/rust/desktop-static` | cua-media-codec | 28 | 0.1 | –/– | –/– | – | 0.4 | 0.1 | 1.7 | decode p50 5.7 ms; container CPU 4% |
| `runc/ws/videotoolbox/rust/timecode` | cua-media-codec | 94 | 27.6 | 36.0/39.3 | 14.0/25.0 | 36.0 | 6.8 | 2.4 | 7.8 | decode p50 0.7 ms; container CPU 17% |
| `runc/ws/videotoolbox/rust/scroll` | cua-media-codec | 105 | 27.6 | 36.3/39.3 | 18.0/27.0 | – | 20.5 | 2.6 | 15.5 | decode p50 0.8 ms; container CPU 27% |
| `runc/ws/videotoolbox/rust/video` | cua-media-codec | 100 | 27.4 | 36.0/44.7 | 22.0/32.0 | – | 435.9 | 2.5 | 29.6 | decode p50 0.7 ms; container CPU 49% |
| `runc/ws/videotoolbox/rust/drag` | cua-media-codec | 22 | 27.7 | 36.3/40.0 | 23.0/32.0 | – | 31.3 | 4.0 | 27.7 | decode p50 0.7 ms; container CPU 37% |
| `runc/ws/videotoolbox/rust/avsync` | cua-media-codec | 89 | 2.0 | 189.6/902.2 | –/– | – | 0.4 | 0.4 | 2.7 | A/V skew p50 -9.9 ms (max 11.9); audio lat p50 33 ms; decode p50 1.4 ms; container CPU 9% |
| `runc/ws/videotoolbox/rust/stall` | cua-media-codec | 100 | 28.1 | 35.1/38.2 | 17.0/678.0 | – | 6.9 | 3.4 | 13.2 | recovery p50 26 ms (max 68); decode p50 0.7 ms; container CPU 25% |
| `runc/ws/openh264/rust/timecode` | cua-media-codec | 96 | 27.5 | 36.1/40.8 | 18.0/29.0 | 33.5 | 6.8 | 5.3 | 21.7 | decode p50 0.6 ms; container CPU 42% |
| `runc/ws/openh264/rust/video` | cua-media-codec | 96 | 28.1 | 35.0/43.0 | 20.0/29.0 | – | 451.4 | 4.3 | 28.3 | decode p50 0.9 ms; container CPU 46% |
| `runc/ws/openh264/rust-sidecar/static` | cua-media-codec | 105 | 0.1 | 1530.4/1530.4 | –/– | 26.0 | 0.1 | 0.3 | 3.4 | decode p50 2.8 ms |
| `runc/ws/openh264/rust-sidecar/desktop-static` | cua-media-codec | 32 | 0.1 | –/– | –/– | – | 0.4 | 0.3 | 3.4 | decode p50 3.4 ms |
| `runc/ws/openh264/rust-sidecar/timecode` | cua-media-codec | 101 | 28.0 | 35.7/38.4 | 17.0/26.0 | 35.4 | 6.9 | 6.6 | 21.8 | decode p50 0.9 ms |
| `runc/ws/openh264/rust-sidecar/scroll` | cua-media-codec | 107 | 28.1 | 35.3/38.3 | 18.0/28.0 | – | 20.5 | 5.2 | 25.7 | decode p50 0.6 ms |
| `runc/ws/openh264/rust-sidecar/video` | cua-media-codec | 106 | 28.1 | 35.1/42.6 | 20.0/29.0 | – | 448.7 | 4.5 | 28.0 | decode p50 0.9 ms |
| `runc/ws/openh264/rust-sidecar/drag` | cua-media-codec | 31 | 28.3 | 35.1/38.5 | 21.0/30.0 | – | 33.0 | 5.5 | 27.7 | decode p50 0.5 ms |
| `runc/ws/openh264/rust-sidecar/avsync` | cua-media-codec | 86 | 2.0 | 628.6/902.1 | –/– | – | 0.4 | 0.6 | 3.3 | A/V skew p50 -8.9 ms (max 12.2); audio lat p50 28 ms; decode p50 0.3 ms |
| `runc/ws/openh264/rust-sidecar/stall` | cua-media-codec | 100 | 28.5 | 34.4/37.2 | 14.0/661.0 | – | 6.9 | 3.5 | 9.9 | recovery p50 20 ms (max 20); decode p50 0.3 ms |
| `runc/quic/openh264/rust-sidecar/static` | cua-media-codec | 91 | 0.1 | 1522.2/1522.2 | –/– | 8.6 | 0.1 | 0.2 | 1.5 | decode p50 1.1 ms |
| `runc/quic/openh264/rust-sidecar/desktop-static` | cua-media-codec | 21 | 0.1 | 3234.6/3234.6 | –/– | – | 0.4 | 0.2 | 1.5 | decode p50 2.2 ms |
| `runc/quic/openh264/rust-sidecar/timecode` | cua-media-codec | 84 | 28.1 | 35.3/38.0 | 11.0/19.0 | 36.3 | 6.9 | 2.9 | 6.9 | decode p50 0.3 ms |
| `runc/quic/openh264/rust-sidecar/scroll` | cua-media-codec | 90 | 28.2 | 35.2/38.4 | 16.0/26.0 | – | 20.9 | 4.6 | 21.5 | decode p50 0.4 ms |
| `runc/quic/openh264/rust-sidecar/video` | cua-media-codec | 104 | 27.8 | 35.5/43.2 | 20.0/29.0 | – | 445.6 | 5.1 | 31.0 | decode p50 1.0 ms |
| `runc/quic/openh264/rust-sidecar/drag` | cua-media-codec | 22 | 27.9 | 35.8/37.9 | 20.0/27.0 | – | 32.1 | 5.1 | 25.0 | decode p50 0.5 ms |
| `runc/quic/openh264/rust-sidecar/avsync` | cua-media-codec | 81 | 2.0 | 762.0/902.2 | –/– | – | 0.4 | 0.5 | 2.3 | A/V skew p50 -9.3 ms (max 12.1); audio lat p50 23 ms; decode p50 0.1 ms |
| `runc/quic/openh264/rust-sidecar/loss` | cua-media-codec | 85 | 18.2 | 35.1/41.1 | 11.0/19.0 | – | 8.4 | 2.3 | 8.2 | recovery p50 400 ms (max 861); dropped 29 dgrams; decode p50 0.3 ms |
| `runc/ws/sdk/rust-sdk/timecode` | – | 169 | 27.7 | 36.1/38.7 | 13.0/21.0 | – | 0.0 | 4.6 | – |  |
| `runc/ws/sdk/node/timecode` | – | 158 | 27.7 | 36.1/38.6 | 15.0/23.0 | – | 0.0 | 10.3 | – |  |
| `runc/ws/sdk/python/timecode` | – | 160 | 27.7 | 36.2/38.4 | 14.0/22.0 | – | 0.0 | 7.6 | – |  |
| `runc/ws/sdk/browser/timecode` | – | 110 | 28.2 | 35.3/37.9 | 14.0/22.0 | – | 6.9 | 15.4 | – |  |
| `runsc/ws/videotoolbox/rust/static` | cua-media-codec | 149 | 0.7 | 1521.6/1526.6 | –/– | 13.6 | 0.2 | 0.2 | 21.7 | decode p50 1.6 ms; container CPU 27% |
| `runsc/ws/videotoolbox/rust/desktop-static` | cua-media-codec | 44 | 0.1 | –/– | –/– | – | 0.4 | 0.1 | 16.6 | decode p50 4.2 ms; container CPU 24% |
| `runsc/ws/videotoolbox/rust/timecode` | cua-media-codec | 149 | 27.6 | 36.0/38.9 | 12.0/20.0 | 34.5 | 6.8 | 2.6 | 30.3 | decode p50 0.8 ms; container CPU 61% |
| `runsc/ws/videotoolbox/rust/scroll` | cua-media-codec | 142 | 28.0 | 35.4/38.8 | 14.0/23.0 | – | 20.7 | 2.8 | 36.5 | decode p50 0.9 ms; container CPU 69% |
| `runsc/ws/videotoolbox/rust/video` | cua-media-codec | 141 | 27.9 | 35.2/43.7 | 20.0/28.0 | – | 448.6 | 2.7 | 48.6 | decode p50 0.8 ms; container CPU 92% |
| `runsc/ws/videotoolbox/rust/drag` | cua-media-codec | 47 | 28.1 | 35.7/39.8 | 23.0/32.0 | – | 32.8 | 3.9 | 47.1 | decode p50 0.9 ms; container CPU 90% |
| `runsc/ws/videotoolbox/rust/avsync` | cua-media-codec | 165 | 2.0 | 824.1/901.9 | –/– | – | 0.4 | 0.2 | 20.9 | A/V skew p50 -46.9 ms (max 74.5); audio lat p50 56 ms; decode p50 1.3 ms; container CPU 37% |
| `runsc/ws/videotoolbox/rust/stall` | cua-media-codec | 142 | 27.8 | 35.1/39.3 | 13.0/672.0 | – | 6.8 | 2.6 | 32.8 | recovery p50 39 ms (max 64); decode p50 0.6 ms; container CPU 49% |
| `runsc/ws/openh264/rust/timecode` | cua-media-codec | 116 | 28.1 | 35.1/38.2 | 10.0/19.0 | 34.6 | 6.9 | 2.6 | 29.4 | decode p50 0.2 ms; container CPU 42% |
| `runsc/ws/openh264/rust/video` | cua-media-codec | 120 | 28.1 | 35.1/41.7 | 18.0/27.0 | – | 449.2 | 4.4 | 45.4 | decode p50 0.9 ms; container CPU 77% |
| `runsc/ws/openh264/rust-sidecar/static` | cua-media-codec | 123 | 0.7 | 1522.4/1541.7 | –/– | 20.9 | 0.2 | 0.5 | 25.5 | decode p50 0.8 ms |
| `runsc/ws/openh264/rust-sidecar/desktop-static` | cua-media-codec | 40 | 0.1 | 18654.4/18654.4 | –/– | – | 0.4 | 0.3 | 26.8 | decode p50 2.6 ms |
| `runsc/ws/openh264/rust-sidecar/timecode` | cua-media-codec | 117 | 27.8 | 35.8/38.9 | 14.0/24.0 | 35.1 | 6.8 | 5.5 | 37.9 | decode p50 0.7 ms |
| `runsc/ws/openh264/rust-sidecar/scroll` | cua-media-codec | 119 | 28.0 | 35.4/38.5 | 16.0/23.0 | – | 21.5 | 3.9 | 43.5 | decode p50 0.4 ms |
| `runsc/ws/openh264/rust-sidecar/video` | cua-media-codec | 114 | 28.0 | 35.0/42.4 | 18.0/26.0 | – | 446.7 | 4.3 | 42.9 | decode p50 0.9 ms |
| `runsc/ws/openh264/rust-sidecar/drag` | cua-media-codec | 37 | 28.2 | 35.4/38.1 | 20.0/29.0 | – | 31.6 | 4.8 | 39.7 | decode p50 0.5 ms |
| `runsc/ws/openh264/rust-sidecar/avsync` | cua-media-codec | 110 | 2.0 | 476.7/901.6 | –/– | – | 0.4 | 0.4 | 18.6 | A/V skew p50 -67.3 ms (max 70.1); audio lat p50 80 ms; decode p50 0.2 ms |
| `runsc/ws/openh264/rust-sidecar/stall` | cua-media-codec | 139 | 27.8 | 35.4/37.9 | 12.0/673.0 | – | 6.8 | 3.0 | 28.3 | recovery p50 20 ms (max 20); decode p50 0.3 ms |
| `runsc/quic/openh264/rust-sidecar/static` | – | – | – | –/– | –/– | – | – | – | – | error: no frames decoded |
| `runsc/quic/openh264/rust-sidecar/desktop-static` | cua-media-codec | 39 | 0.1 | –/– | –/– | – | 0.4 | 0.2 | 23.3 | decode p50 2.7 ms |
| `runsc/quic/openh264/rust-sidecar/timecode` | cua-media-codec | 122 | 27.9 | 35.6/38.1 | 11.0/19.0 | 35.5 | 6.9 | 3.6 | 31.0 | decode p50 0.3 ms |
| `runsc/quic/openh264/rust-sidecar/scroll` | cua-media-codec | 159 | 28.0 | 35.3/38.2 | 14.0/23.0 | – | 20.6 | 3.7 | 41.2 | decode p50 0.3 ms |
| `runsc/quic/openh264/rust-sidecar/video` | cua-media-codec | 163 | 27.9 | 35.2/43.1 | 20.0/29.0 | – | 447.2 | 5.9 | 50.5 | decode p50 1.0 ms |
| `runsc/quic/openh264/rust-sidecar/drag` | cua-media-codec | 48 | 28.1 | 35.6/38.0 | 21.0/29.0 | – | 32.2 | 5.3 | 46.6 | decode p50 0.6 ms |
| `runsc/quic/openh264/rust-sidecar/avsync` | cua-media-codec | 134 | 2.0 | 240.0/901.4 | –/– | – | 0.4 | 0.4 | 16.2 | A/V skew p50 -22.1 ms (max 24.4); audio lat p50 43 ms; decode p50 0.1 ms |
| `runsc/quic/openh264/rust-sidecar/loss` | cua-media-codec | 105 | 18.5 | 35.4/39.3 | 12.0/21.0 | – | 8.4 | 2.9 | 33.0 | recovery p50 391 ms (max 850); dropped 29 dgrams; decode p50 0.3 ms |

## Not run

- `encoder/nvenc`: deferred until GPU access (plan §8.6): probe-only (DetectedOnly), skipped by selection; tests gated behind CUA_CODEC_TEST_*
- `encoder/vaapi`: deferred until GPU access (plan §8.6): probe-only (DetectedOnly), skipped by selection; tests gated behind CUA_CODEC_TEST_*
- `encoder/qsv`: deferred until GPU access (plan §8.6): probe-only (DetectedOnly), skipped by selection; tests gated behind CUA_CODEC_TEST_*
- `encoder/amf`: deferred until GPU access (plan §8.6): probe-only (DetectedOnly), skipped by selection; tests gated behind CUA_CODEC_TEST_*
- `encoder/mediafoundation`: deferred until GPU access (plan §8.6): probe-only (DetectedOnly), skipped by selection; tests gated behind CUA_CODEC_TEST_*
- `encoder/videotoolbox`: unavailable in this matrix: the server runs in a Linux container (OpenH264); VideoToolbox encode applies to macOS guests
- `runtime/qemu`: optional arm64 VM lane not run by this harness yet (docker runc/runsc only)
