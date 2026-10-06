# Run Cua Driver with a local model (Ollama + opencode + Qwen 3.6)

End-to-end setup for driving Cua Driver from a model running entirely on your
own machine — no API keys, no network round-trip. The stack is:

```
opencode  ──MCP──▶  cua-driver  ──▶  macOS apps
   │
   └──OpenAI-compatible HTTP──▶  Ollama  ──▶  qwen3.6:35b-a3b-mtp-q8_0
```

Every number in this document was measured on an Apple M5 Max / 128 GB running
macOS 26.5.2, Ollama 0.32.5, opencode 1.18.14, cua-driver 0.19.0. Your figures
will differ; the shape of the tradeoffs will not.

---

## 1. Hardware

The model is a 35B mixture-of-experts with ~3B parameters active per token. Only
3B are *computed* per token, but all 35B must be *resident*, so memory is the
binding constraint, not compute.

| Unified memory | Verdict |
|---|---|
| < 32 GB | Not enough for the 8-bit build. Use `qwen3.6:35b` (4-bit, ~23 GB) and expect it to be tight. |
| 32 GB | Works at 8-bit, little headroom. Close memory-heavy apps. |
| 48 GB+ | Comfortable, including long contexts. |
| 128 GB | Plenty. You can keep several models resident at once. |

Decode speed is memory-bandwidth bound. This is why higher precision costs
throughput, and why an M-series *Max* chip meaningfully outruns the base tier.

---

## 2. Install and start Ollama

```bash
brew install ollama          # or: brew upgrade ollama
brew services start ollama
ollama --version             # must be >= 0.32.x
```

**The version matters.** The MLX runner (`mlx_metal_v3/libmlxc.dylib`) only
ships in recent builds. On an older Ollama, MLX-format model tags silently fall
back to the llama.cpp path. Confirm the runner is present:

```bash
ls "$(brew --prefix)/Cellar/ollama/$(ollama --version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)/libexec/lib/ollama/"
# expect: llama-quantize  llama-server  mlx_metal_v3
```

---

## 3. Pick the model

This is the decision that matters most, and the obvious choice is wrong. Three
builds of the same model, all measured on the same machine:

| Tag | Size | Backend | Precision | Warm tok/s | Vision |
|---|---|---|---|---|---|
| `qwen3.6:35b` | 23 GB | llama.cpp | 4-bit Q4_K_M | 43 | ✅ |
| `qwen3.6:35b-a3b-mxfp8` | 37 GB | **MLX** | 8-bit MXFP8 | 30–34 | ❌ **silently broken** |
| **`qwen3.6:35b-a3b-mtp-q8_0`** | **38 GB** | **llama.cpp** | **8-bit Q8_0** | **37–39** | ✅ |

**Use `qwen3.6:35b-a3b-mtp-q8_0`.** It is the only build that gets you 8-bit
precision *and* working vision *and* competitive speed.

Two traps worth understanding:

- **MLX is not automatically faster.** It is Apple-native and genuinely quick at
  4-bit, but MXFP8 doubles the bytes read per token, and that costs more than the
  backend saves. The MLX 8-bit build is the *slowest* of the three.
- **`mtp` is the tag that matters.** Multi-token prediction lets the model emit
  several tokens per forward pass, which claws back most of what 8-bit weights
  cost. Without it, `35b-a3b-q8_0` lands near 30 tok/s.

### Vision is not optional for computer use, and one build lies about it

`ollama show` reports `vision` for the MXFP8 build. It does not work. The
capability flag comes from the model card, not the runtime, and images are
dropped with **no error** — you get a confident, wrong answer.

Verify any build before trusting it with screenshots:

```bash
# Generate a solid red 64x64 PNG, ask what colour it is.
python3 -c "
import struct,zlib
raw=b''.join(b'\x00'+bytes((255,0,0))*64 for _ in range(64))
def chunk(t,d):
    c=t+d; return struct.pack('>I',len(d))+c+struct.pack('>I',zlib.crc32(c)&0xffffffff)
open('/tmp/red.png','wb').write(b'\x89PNG\r\n\x1a\n'
  +chunk(b'IHDR',struct.pack('>IIBBBBB',64,64,8,2,0,0,0))
  +chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b''))"

B64=$(base64 -i /tmp/red.png | tr -d '\n')
curl -s http://localhost:11434/api/generate -d "{
  \"model\":\"qwen3.6:35b-a3b-mtp-q8_0\",
  \"prompt\":\"What single color is this image? One word.\",
  \"images\":[\"$B64\"],\"stream\":false,\"think\":false}" | python3 -m json.tool
```

`"Red"` means vision works. Anything else means the image never reached the
model — do not use that build for screenshot-driven automation.

You can also check the manifest before downloading 38 GB. Builds with working
vision ship a separate projector layer:

```bash
curl -s "https://registry.ollama.ai/v2/library/qwen3.6/manifests/35b-a3b-mtp-q8_0" \
  | python3 -c "import json,sys; print([(l['mediaType'].split('.')[-1], round(l['size']/1e9,1)) for l in json.load(sys.stdin)['layers'] if l['size']>1e8])"
# [('model', 37.8), ('projector', 0.9)]   ← the projector is the vision tower
```

### Pull it

```bash
ollama pull qwen3.6:35b-a3b-mtp-q8_0     # ~38 GB
ollama show qwen3.6:35b-a3b-mtp-q8_0     # expect Projector: clip, 446.57M
```

---

## 4. Configure Ollama for agent workloads

Ollama's defaults are tuned for chat, not for long agent loops. Edit the launchd
plist — **not** your shell profile:

```bash
$EDITOR ~/Library/LaunchAgents/homebrew.mxcl.ollama.plist
```

```xml
<key>EnvironmentVariables</key>
<dict>
    <key>OLLAMA_FLASH_ATTENTION</key><string>1</string>
    <key>OLLAMA_KV_CACHE_TYPE</key><string>q8_0</string>
    <key>OLLAMA_KEEP_ALIVE</key><string>-1</string>
</dict>
```

Reload with `launchctl`, **not** `brew services restart`:

```bash
P=~/Library/LaunchAgents/homebrew.mxcl.ollama.plist
launchctl bootout gui/$(id -u)/homebrew.mxcl.ollama
launchctl bootstrap gui/$(id -u) "$P"
```

**Two traps here, both of which will silently give you the default.**

*First*, `brew services restart` **regenerates this plist from the formula and
discards your edits**. Use `launchctl` as above. Any later `brew services
start|restart|stop`, or a `brew upgrade ollama`, will wipe the setting again —
re-apply and re-verify after upgrades.

*Second*, the server runs under launchd, which does **not** inherit your
interactive shell environment. `export OLLAMA_KEEP_ALIVE=-1` in `~/.zshrc` is
invisible to the daemon.

Because both failure modes are silent, verify what the daemon actually sees
rather than what you think you set:

```bash
grep '"server config"' "$(brew --prefix)/var/log/ollama.log" | tail -1 | tr ' ' '\n' | grep OLLAMA_KEEP_ALIVE
# want: OLLAMA_KEEP_ALIVE:2562047h47m16.854775807s   (max int64 — this is how -1 renders)
# wrong: OLLAMA_KEEP_ALIVE:5m0s                      (the default; your edit did not take)
```

Then confirm a loaded model actually sticks:

```bash
ollama ps     # UNTIL column should read "Forever"
```

| Setting | Value | Why |
|---|---|---|
| `OLLAMA_KEEP_ALIVE` | `-1` | Default is 5 min. Agent loops idle longer than that between runs, and reloading 38 GB costs ~5–22 s *and* discards the prompt cache. |
| `OLLAMA_FLASH_ATTENTION` | `1` | Needed to make the 262 144-token context affordable. |
| `OLLAMA_KV_CACHE_TYPE` | `q8_0` | Halves KV cache memory at full context. |
| `OLLAMA_NUM_PARALLEL` | leave at `1` | Raising it splits the context window N ways — wrong when single requests carry screenshots. |

---

## 5. Install Cua Driver and grant permissions

```bash
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
cua-driver permissions grant     # Accessibility + Screen Recording
cua-driver permissions status
```

On macOS, grant via `cua-driver permissions grant`. It launches the app through
LaunchServices so the TCC dialogs attribute to `com.trycua.driver` rather than to
your terminal. Granting from a plain shell invocation attributes the permission
to the wrong process and the driver will still report *not granted*.

Start the daemon:

```bash
cua-driver serve        # or let the installed CuaDriver.app run it
```

---

## 6. Configure opencode

Install opencode per <https://opencode.ai> (the installer drops the binary in
`~/.opencode/bin`). Then edit `~/.config/opencode/config.json`:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "provider": {
    "ollama": {
      "npm": "@ai-sdk/openai-compatible",
      "options": { "baseURL": "http://localhost:11434/v1" },
      "models": {
        "qwen3.6:35b-a3b-mtp-q8_0": {
          "modalities": { "input": ["text", "image"], "output": ["text"] }
        }
      }
    }
  },
  "mcp": {
    "cua-driver": {
      "type": "local",
      "command": ["/Users/YOU/.local/bin/cua-driver", "mcp"],
      "enabled": true
    }
  }
}
```

Declare `"image"` in `input` **only** for a build whose vision you verified in
step 3. Declaring it for a build that silently drops images is worse than not
declaring it — the agent will send screenshots and reason confidently over
nothing.

Confirm registration:

```bash
opencode models | grep ollama
# ollama/qwen3.6:35b-a3b-mtp-q8_0
```

---

## 7. Run it

Interactive:

```bash
opencode --model ollama/qwen3.6:35b-a3b-mtp-q8_0
```

One-shot / scripted — `--auto` is required, or the run blocks forever waiting for
tool-permission approval that a non-interactive session can never deliver:

```bash
opencode run --auto --model ollama/qwen3.6:35b-a3b-mtp-q8_0 \
  "Using the cua-driver MCP tools, launch Calculator and tell me its window id."
```

Add `--print-logs --log-level INFO` to see agent loop steps. Without it a slow
first inference is indistinguishable from a hang.

---

## 8. What to expect

**The first inference takes 2–3 minutes.** cua-driver exposes ~54 MCP tools, and
their combined JSON schema is a large prompt prefix that must be evaluated before
the model emits a single token. This is a one-time cost per session: once the
prefix is cached, subsequent steps run 3–80 s each.

Do not mistake this for a hang. Confirm work is actually in flight:

```bash
grep "v1/chat/completions" "$(brew --prefix)/var/log/ollama.log" | tail -5
ollama ps     # a loaded model with a *resetting* timer means active generation
```

**Known limitations at this model size:**

- **Opaque handles get hallucinated.** `snapshot_id` follows `^s[0-9a-f]{8}$`
  (e.g. `s0000000c`) and `element_token` looks like `s0000000c:9`. A local model
  will sometimes invent plausible base64 blobs instead of using the handle it was
  just given. Re-snapshotting via `get_window_state` and retrying usually
  recovers.
- **Refusals need a recent driver.** Before `dc6f32cd4`, refusal payloads failed
  the tool's own advertised `outputSchema`, so strict MCP clients replaced the
  actionable message with an opaque `-32602` schema error. Symptom: the agent
  abandons the accessibility route and falls back to blind pixel clicking. Fixed
  in `main`; make sure your build includes it.
- **Pixel clicking is the last resort, not the fallback.** Prefer
  `element_token`. Pixel input is driver-unverifiable — only the calling agent
  can confirm it from a screenshot.

**Reducing the tool-schema tax.** If local computer use feels sluggish or the
model picks the wrong tool, narrowing the exposed tool set is a bigger lever than
any quantization change. See `restrict-tool-access` in the how-to guides.

---

## 9. Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Run appears hung, no output for minutes | First inference evaluating the tool schema | Wait. Confirm with the `v1/chat/completions` log grep above. |
| Run blocks forever, no inference request at all | Missing `--auto` in non-interactive mode | Add `--auto`. |
| Model describes a screenshot wrongly / generically | Vision silently dropped by the build | Run the red-PNG test in step 3; switch to an `mtp` build. |
| 20 s pause between agent runs | Model evicted after `KEEP_ALIVE` | Set `OLLAMA_KEEP_ALIVE=-1` in the **plist**, reload with `launchctl` (not `brew services`). |
| `-32602 Structured content does not match the tool's output schema` | Driver predates `dc6f32cd4` | Update Cua Driver. |
| Permissions report *not granted* despite Settings showing granted | TCC grant is cdhash-pinned to a previous build | `tccutil reset` the service, then `cua-driver permissions grant` again. |
| `bind … path must be shorter than SUN_LEN` | Unix socket path over ~104 chars | Use a short `--socket` path such as `/tmp/cua.sock`. |
| Slower than the table above after switching to an "8-bit MLX" tag | MXFP8 is bandwidth-bound | Use `35b-a3b-mtp-q8_0`, or `35b-mlx` if you want raw 4-bit speed. |

---

## Appendix: verifying which backend is live

Ollama does not announce its choice. Read the runner process:

```bash
pgrep -fl "ollama runner"
# ... --mlx-engine --model qwen3.6:35b-a3b-mxfp8     ← MLX
# ... llama-server --model … --mmproj …              ← llama.cpp (--mmproj = vision)
```

The presence of `--mmproj` on the llama.cpp path is the strongest available
signal that the vision projector actually loaded.
