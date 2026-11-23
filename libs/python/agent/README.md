# Cua Agent

> [!WARNING]
> **Deprecated:** `cua-agent` is no longer maintained by Cua and will not receive updates or fixes. For desktop control, connect your coding agent to [Cua Driver](https://cua.ai/docs/cua-driver/guides/connect-your-agent) over MCP. For sandboxes, use the [`cua` SDK and CLI](https://cua.ai/docs/cua-sdk/quickstart) (`pip install cua`) and [run a coding agent in a sandbox](https://cua.ai/docs/cua-sdk/guides/run-a-coding-agent). See [Migrate from deprecated packages](https://cua.ai/docs/cua-sdk/guides/cua-cloud-ended#deprecated-packages). Its tests, builds and publishing are switched off. Existing releases stay on PyPI, so current installs keep working.

Computer-Use framework with liteLLM integration for running agentic workflows on macOS, Windows, and Linux sandboxes.

**[Documentation](https://cua.ai/docs/cua/reference/agent-sdk)** - Installation, guides, and configuration.

> [!WARNING]
> **Removed:** the `omni` extra (`cua-agent[omni]`) is gone. It pulled in `cua-som`, which is deprecated, no longer maintained and licensed under AGPL-3.0. The omniparser loop still works if you install `cua-som` yourself (`pip install cua-som`); it will not receive updates or fixes.

## Spending limits in existing installations

`max_trajectory_budget` requires a known response price. Missing pricing is
unknown, not zero: the first unpriced response reports that the limit cannot
be enforced and prevents another model request. With `raise_error=True`, it
raises `BudgetExceededError` immediately instead.

This includes local/self-hosted models without configured pricing and the direct
Gemini SDK loop, which reports token counts but no response price. Budgeted runs
with that Gemini loop therefore stop after the first response. Runs without a
spending limit can continue; their total cost remains unknown. This change does
not estimate Gemini charges from token counts or reactivate this deprecated
package's release pipeline.
