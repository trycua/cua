# Changelog

## Week of March 10-14, 2026

This week focused on expanding native agent loop support, broadening inference API integrations, and improving sandbox usability with interactive and non-interactive shell access.

### Highlights

- Added the Yutori N1 browser-use agent loop as a native agent loop, including adapter and loop registration work to integrate it cleanly with the agent stack.
- Added GPT-5.4 support for the Computer tool, with follow-up compatibility and correctness coverage for computer handling.
- Expanded inference API compatibility with support for Sonnet 4.6 and OpenAI provider inference API integrations.
- Added interactive and non-interactive shell support for sandboxes through new `cua sb shell` and `cua sb exec` commands, plus corresponding documentation updates.
- Continued investment in sandbox infrastructure with cloud transport and ephemeral VM support, alongside the new sandbox SDK and runtime support across QEMU WSL2/KVM, Hyper-V, and Docker.

### Features

- Added Yutori N1 browser-use agent loop support as a native loop, including adapter exports, provider routing, and loop package registration.
- Added GPT-5.4 support for the Computer tool.
- Added Sonnet 4.6 inference API support.
- Added OpenAI provider inference API support.
- Added `cua sb shell` and `cua sb exec` for interactive and non-interactive sandbox access.
- Added sandbox SDK support for QEMU WSL2/KVM, Hyper-V, and Docker runtimes.
- Added cloud transport with ephemeral VM support for sandboxes.
- Added a PR template and CI enforcement for customer impact notes.

### Fixes

- Fixed agent click behavior by reading viewport width and height directly.
- Fixed agent computer action selection to return the correct action for the received function.
- Fixed broken documentation links and related link-checker ignore patterns.

### Infrastructure

- Bumped internal package versions including `cua-agent` and `cua-cli`.
- Updated dependencies such as `orjson`, `black`, `tornado`, `authlib`, and `@hono/node-server`.
- Added test coverage for GPT-5.4 computer handling backwards compatibility and correctness.

### Documentation

- Added shell and exec command coverage to the cloud CLI reference.
- Added sandbox usage examples to the sandbox README.
- Updated custom agent guides to match the actual `BaseAgent` interface.

### Notable merged PRs

- `#1169` `feat(cli): Add shell and exec commands to cua sb`
- `#1158` `docs: update custom agent guides to match actual BaseAgent interface`
- `#1155` `feat(agent): Add Yutori n1 browser-use agent loop`
- `#1152` `fix broken hud liks`
- `#1150` `feat(agent): Add GPT 5.4 support for Computer tool`
