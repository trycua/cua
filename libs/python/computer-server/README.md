<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

[![Python](https://img.shields.io/badge/Python-333333?logo=python&logoColor=white&labelColor=333333)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
[![PyPI](https://img.shields.io/pypi/v/cua-computer-server?color=333333)](https://pypi.org/project/cua-computer-server/)

</h1>
</div>

**Computer Server** is the server component for the Computer-Use Interface (CUI) framework powering Cua for interacting with local macOS and Linux sandboxes, PyAutoGUI-compatible, and pluggable with any AI agent systems (Cua, Langchain, CrewAI, AutoGen).

## Features

- WebSocket API for computer-use
- Cross-platform support (macOS, Linux)
- Integration with CUA computer library for screen control, keyboard/mouse automation, and accessibility

## Install

To install the Computer-Use Interface (CUI):

```bash
pip install cua-computer-server
```

## Run

Refer to this notebook for a step-by-step guide on how to use the Computer-Use Server on the host system or VM:

- [Computer-Use Server](../../notebooks/computer_server_nb.ipynb)

## Environment variables

Authentication behavior is controlled by the following environment variables:

| Variable | Default | Description |
| --- | --- | --- |
| `CONTAINER_NAME` | _unset_ | When set, the server runs in cloud auth mode: every `/ws`, `/cmd`, and `/responses` request must present a matching container name and a valid Cua API key. When unset, the server runs in local development mode and accepts all requests. |
| `CUA_AUTH_TTL_SECONDS` | `60` | TTL for cached authentication sessions. |
| `CUA_ENABLE_PUBLIC_PROXY` | _unset_ | When truthy (`1`/`true`/`yes`/`y`/`on`), the `/responses` agent proxy skips API-key validation even if `CONTAINER_NAME` is set. |
| `UNAVAILABLE_WITHOUT_CONTAINER_NAME` | _unset_ | When truthy (`1`/`true`/`yes`/`y`/`on`), requests are rejected if `CONTAINER_NAME` is not set instead of falling through to local development mode. Use this to ensure a cloud deployment cannot accidentally serve unauthenticated traffic. |
| `UNAVAILABLE_WITHOUT_CONTAINER_NAME_RESPONSE_STATUS_CODE` | `503` | HTTP status code returned by `/cmd` and `/responses` when `UNAVAILABLE_WITHOUT_CONTAINER_NAME` rejects a request. WebSocket clients receive the same code in the error JSON payload before the connection is closed. |

## Docs

- [Commands](https://trycua.com/docs/libraries/computer-server/Commands)
- [REST-API](https://trycua.com/docs/libraries/computer-server/REST-API)
- [WebSocket-API](https://trycua.com/docs/libraries/computer-server/WebSocket-API)
- [Index](https://trycua.com/docs/libraries/computer-server/index)
