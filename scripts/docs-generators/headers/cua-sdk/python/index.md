| Package | Install | Import |
| --- | --- | --- |
| `cua-sandbox` {{sandboxVersion}} (Python 3.11 to 3.13) | `pip install cua-sandbox` | `from cua_sandbox import Sandbox, Image` |
| `cua` {{version}} with the extra | `pip install "cua[sandbox]"` | `from cua import Sandbox, Image` (the same classes) |
| MCP client support | `pip install "cua-sandbox[mcp]"` | `sb.mcp()` |
| Typed Cua Driver access | `pip install "cua-sandbox[driver]"` | `sb.driver` |

`configure()` sets process-wide options; environment variables override it:

| Variable | Purpose | Default |
| --- | --- | --- |
| `CUA_CLIENT_ID`, `CUA_CLIENT_SECRET` | API key credentials | None; then the `cua auth login` session |
| `CUA_TOKEN_URL` | Token endpoint | The cua.ai token endpoint |
| `CUA_IMAGE_LINUX`, `CUA_IMAGE_WINDOWS`, `CUA_IMAGE_MACOS` | Override `Image.linux()`, `Image.windows()`, `Image.macos()` | The canonical images |
