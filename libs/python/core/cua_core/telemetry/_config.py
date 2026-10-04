"""Shared telemetry enablement rules for every Cua Python library.

Telemetry is OFF when any of these hold:

* ``DO_NOT_TRACK`` is set to a non-empty value other than ``0``
* ``CUA_TELEMETRY`` is ``0``/``false``/``no``/``off``
* legacy ``CUA_TELEMETRY_ENABLED`` is ``0``/``false``/``no``/``off``
* legacy ``CUA_TELEMETRY_DISABLED`` is truthy
* the machine setting is off: ``[telemetry] enabled = "off"`` in
  ``$CUA_HOME/config.toml`` (default ``~/.cua/config.toml``), which
  ``cua telemetry off`` and the Cua Spaces app write; ``CUA_TELEMETRY=1``
  still wins over it, as in every other Cua program

In CI (``CI``, ``GITHUB_ACTIONS``, ...) telemetry defaults to OFF unless
``CUA_TELEMETRY`` is explicitly ``1``/``true``/``yes``/``on``. Otherwise it
defaults to ON.
"""

from __future__ import annotations

import os
import re
from pathlib import Path
from typing import Mapping, Optional

try:
    import tomllib as _tomllib
except ImportError:  # pragma: no cover - Python < 3.11
    _tomllib = None  # type: ignore[assignment]

_FALSY = {"0", "false", "no", "off"}
_TRUTHY = {"1", "true", "yes", "on"}

CI_ENV_VARS = (
    "CI",
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "BUILDKITE",
    "CIRCLECI",
    "JENKINS_URL",
    "TF_BUILD",
    "CONTINUOUS_INTEGRATION",
)


def _norm(value: Optional[str]) -> str:
    return (value or "").strip().lower()


def is_ci(env: Optional[Mapping[str, str]] = None) -> bool:
    """True when the process looks like it runs in a CI system."""
    env = os.environ if env is None else env
    for name in CI_ENV_VARS:
        value = _norm(env.get(name))
        if value and value not in _FALSY:
            return True
    return False


def _config_home(env: Mapping[str, str]) -> Optional[Path]:
    home = (env.get("CUA_HOME") or "").strip()
    if home:
        return Path(home)
    user_home = (env.get("HOME") or env.get("USERPROFILE") or "").strip()
    return Path(user_home) / ".cua" if user_home else None


def machine_setting(env: Optional[Mapping[str, str]] = None) -> Optional[bool]:
    """The ``[telemetry] enabled`` value in ``$CUA_HOME/config.toml``, if set.

    This is the machine-wide switch the ``cua`` CLI and the Cua Spaces app
    write. ``$CUA_HOME`` and the home directory are read from ``env``.
    """
    env = os.environ if env is None else env
    home = _config_home(env)
    if home is None or _tomllib is None:
        return None
    try:
        with open(home / "config.toml", "rb") as f:
            doc = _tomllib.load(f)
    except (OSError, ValueError):
        return None
    table = doc.get("telemetry")
    if not isinstance(table, dict) or "enabled" not in table:
        return None
    value = table["enabled"]
    if isinstance(value, bool):
        return value
    if isinstance(value, int):
        return value != 0
    if isinstance(value, str):
        v = _norm(value)
        if v in _FALSY or v in {"disable", "disabled"}:
            return False
        if v in _TRUTHY or v in {"enable", "enabled"}:
            return True
    return None


def telemetry_enabled_from_env(env: Optional[Mapping[str, str]] = None) -> bool:
    """Apply the Cua telemetry enablement rules to ``env`` (default ``os.environ``)."""
    env = os.environ if env is None else env

    dnt = _norm(env.get("DO_NOT_TRACK"))
    if dnt and dnt != "0":
        return False

    cua = _norm(env.get("CUA_TELEMETRY"))
    if cua in _FALSY:
        return False
    if _norm(env.get("CUA_TELEMETRY_ENABLED")) in _FALSY:
        return False
    if _norm(env.get("CUA_TELEMETRY_DISABLED")) in _TRUTHY:
        return False

    if cua in _TRUTHY:
        return True
    machine = machine_setting(env)
    if machine is not None:
        return machine
    if is_ci(env):
        return False
    return True


_SAFE_SEGMENT = re.compile(r"^[A-Za-z0-9._:@\-]+$")

# Provider prefixes that may appear before a "/" in a model id (LiteLLM and
# Cua routing prefixes). Anything else before a "/" may be a user or org name.
KNOWN_MODEL_PROVIDERS = frozenset(
    {
        "anthropic",
        "openai",
        "azure",
        "azure_ai",
        "gemini",
        "google",
        "vertex_ai",
        "bedrock",
        "openrouter",
        "ollama",
        "ollama_chat",
        "huggingface",
        "huggingface-local",
        "mlx",
        "together_ai",
        "groq",
        "mistral",
        "deepseek",
        "xai",
        "fireworks_ai",
        "cohere",
        "cua",
        "omniparser",
        "moondream3",
        "human",
        "hosted_vllm",
        "vllm",
    }
)


def _sanitize_model_part(part: str) -> str:
    segments = part.split("/")
    if not all(seg and _SAFE_SEGMENT.match(seg) for seg in segments):
        return "custom"
    if len(segments) == 1:
        return part
    provider = segments[0].lower()
    if provider not in KNOWN_MODEL_PROVIDERS:
        return "custom"
    if len(segments) == 2:
        return part
    # provider/org/model: org may be a private user or organisation name.
    return f"{segments[0]}/custom"


def sanitize_model_name(model: Optional[str], max_len: int = 64) -> Optional[str]:
    """Return a telemetry-safe model label.

    Plain ids (``gpt-4o``) and ``<known-provider>/<model>`` ids
    (``anthropic/claude-sonnet-4-5``) are kept. Composed ids
    (``omniparser+openai/gpt-4o``) are sanitized per part. Anything path-like
    (leading ``/``, ``~`` or ``.``, a backslash, ``://``), with an unknown
    prefix before ``/``, with unusual characters, or longer than ``max_len``
    becomes ``"custom"``. ``provider/org/model`` becomes ``provider/custom``.
    """
    if model is None:
        return None
    text = str(model).strip()
    if not text:
        return None
    if len(text) > max_len or "\\" in text or "://" in text:
        return "custom"
    if text.startswith(("/", "~", ".")):
        return "custom"
    return "+".join(_sanitize_model_part(p) for p in text.split("+"))
