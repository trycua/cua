| Package | Install | Import |
| --- | --- | --- |
| `cua-sandbox` {{sandboxVersion}} (Python 3.11 to 3.13) | `pip install cua-sandbox` | `from cua_sandbox import Sandbox, Image` |
| `cua` {{version}} with the extra | `pip install "cua[sandbox]"` | `from cua import Sandbox, Image` (the same classes) |
| MCP client support | `pip install "cua-sandbox[mcp]"` | `sb.mcp()` |
| Typed Cua Driver access | `pip install "cua-sandbox[driver]"` | `sb.driver` |

`configure()` sets process-wide options; environment variables override it:

| Variable | Purpose | Default |
| --- | --- | --- |
| `FLEETS_TOKEN` | Cloud bearer token; wins over client credentials | None |
| `CUA_CLIENT_ID`, `CUA_CLIENT_SECRET` | Cloud OAuth client credentials | None; then the `cua auth login` session |
| `CUA_FLEET_BASE_URL`, `CUA_TOKEN_URL` | Cloud API and token endpoints | `https://run.cua.ai`, the cua.ai token endpoint |
| `CUA_FLEET_MAX_POOL_SIZE`, `CUA_FLEET_CLAIM_TTL`, `CUA_FLEET_WARM`, `CUA_FLEET_POOL_IDLE_GC` | Cloud capacity defaults (`CUA_FLEET_POOL_IDLE_GC=off` disables idle cleanup) | 10, 15 min, warm for canonical images, 30 min |
| `CUA_IMAGE_LINUX`, `CUA_IMAGE_WINDOWS`, `CUA_IMAGE_MACOS` | Override `Image.linux()`, `Image.windows()`, `Image.macos()` | The canonical images |
