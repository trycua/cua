# Cua Agent

Computer-Use framework with liteLLM integration for running agentic workflows on macOS, Windows, and Linux sandboxes.

**[Documentation](https://cua.ai/docs/cua/reference/agent-sdk)** - Installation, guides, and configuration.

## OrcaRouter

[OrcaRouter](https://www.orcarouter.ai) is available as a named provider (`orcarouter/<vendor>/<model>`). It is picked up by the library, the CLI and the Gradio UI; the proxy inherits it through `ComputerAgent`.

Authenticate with either option - both end in the same `ORCA_KEY` credential:

```bash
# 1. Paste an existing key
cua-agent --orcarouter-api-key sk-orca-...

# 2. Connect with OrcaRouter (OAuth 2.0 + PKCE, S256)
cua-agent --connect-orcarouter            # loopback redirect
cua-agent --connect-orcarouter --no-browser   # out-of-band code, for containers/SSH
```

The key is stored through the project's existing dotenv file (gitignored) or the process environment. The PKCE exchange returns a durable API key, not a refresh token: it is reused until you revoke it at <https://www.orcarouter.ai/console/authorized-apps>.

Model choices come from the live catalog (`GET {ORCA_API_BASE_URL}/models`), filtered per entry point:

```bash
cua-agent --list-orcarouter-models --orcarouter-capability chat
cua-agent --list-orcarouter-models --orcarouter-capability chat --orcarouter-multimodal image
cua-agent --clear-orcarouter-key
```

Defaults are auth at `https://www.orcarouter.ai` and inference at `https://api.orcarouter.ai/v1`. `ORCA_AUTH_BASE_URL` and `ORCA_API_BASE_URL` override them explicitly; `ORCA_BASE_URL` covers a single-origin self-hosted deployment. Plain HTTP is accepted only for loopback origins.
